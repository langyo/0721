//! The IPFS blob backend (kubo RPC, pure reqwest).
//!
//! put = `POST /api/v0/add?pin=true` (multipart) → record the CID;
//! get = `POST /api/v0/cat?arg={cid}`;
//! delete = best-effort `POST /api/v0/pin/rm` (content-addressed: unpin
//! leaves reclamation to kubo's GC; another row referencing the same
//! content keeps it alive).
//!
//! ## The CID mapping problem
//!
//! [`BlobStore`] addresses blobs by sha256 key; IPFS addresses them by
//! CID. This backend bridges through **kubo's MFS** (Mutable File
//! System): `files cp /ipfs/{cid} /i0721/{slot}/{key}` plants a
//! deterministic name for every stored blob, and `files read
//! /i0721/{slot}/{key}` resolves it back. The MFS is kubo's own
//! persistent state — no side database, no in-memory mapping that
//! would die with the process.
//!
//! Trade-off: MFS state is node-local (a different kubo peer does not
//! carry the mapping). For the single-node deployments this wave
//! targets, that is correct; a multi-node IPFS cluster would need the
//! mapping in the metadata database instead (recorded as a follow-up).

use bytes::Bytes;

use crate::blob::{BlobSlot, BlobStore};

/// IPFS configuration from the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpfsConfig {
    /// The kubo RPC API base URL (e.g. `http://127.0.0.1:5001`).
    pub api_url: String,
}

/// Why an IPFS configuration is incomplete.
#[derive(Debug, thiserror::Error)]
pub enum IpfsConfigError {
    /// `I0721_IPFS_API` missing while the backend selection says ipfs.
    #[error("IPFS backend selected but the API URL is not set (I0721_IPFS_API)")]
    MissingApiUrl,
}

impl IpfsConfig {
    /// Read from `I0721_IPFS_API`.
    ///
    /// # Errors
    ///
    /// [`IpfsConfigError::MissingApiUrl`] when the env is absent.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, IpfsConfigError> {
        Ok(Self {
            api_url: lookup("I0721_IPFS_API").ok_or(IpfsConfigError::MissingApiUrl)?,
        })
    }
}

/// The MFS root for this service's blobs.
const MFS_ROOT: &str = "/i0721";

fn mfs_path(slot: BlobSlot, key: &str) -> String {
    let slot_dir = match slot {
        BlobSlot::Media => "media",
        BlobSlot::Cache => "cache",
    };
    format!("{MFS_ROOT}/{slot_dir}/{key}")
}

/// The IPFS blob store (kubo RPC client).
pub struct IpfsBlobStore {
    api_url: String,
    client: reqwest::Client,
}

impl IpfsBlobStore {
    /// Build from a complete [`IpfsConfig`].
    #[must_use]
    pub fn open(config: &IpfsConfig) -> Self {
        Self {
            api_url: config.api_url.trim_end_matches('/').to_owned(),
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_url, path)
    }

    async fn ensure_mfs_dir(&self, slot: BlobSlot) -> Result<(), std::io::Error> {
        let slot_dir = match slot {
            BlobSlot::Media => "media",
            BlobSlot::Cache => "cache",
        };
        let dir = format!("{MFS_ROOT}/{slot_dir}");
        // mkdir -p semantics: existing is fine.
        let resp = self
            .client
            .post(self.url("/api/v0/files/mkdir"))
            .query(&[("arg", dir.as_str()), ("parents", "true")])
            .send()
            .await
            .map_err(io("files/mkdir"))?;
        // EEXIST arrives as a 500 whose body says so; every other
        // failure (permissions, disk, node fault) must surface.
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let already_there = status.as_u16() == 500
                && (body.contains("already exists") || body.contains("file exists"));
            if !already_there {
                return Err(std::io::Error::other(format!("files/mkdir {dir}: {body}")));
            }
        }
        Ok(())
    }
}

fn io(op: &'static str) -> impl Fn(reqwest::Error) -> std::io::Error {
    move |err| std::io::Error::other(format!("IPFS {op}: {err}"))
}

/// The `/api/v0/add` response.
#[derive(serde::Deserialize)]
struct AddResponse {
    #[serde(rename = "Hash")]
    hash: String,
}

#[async_trait::async_trait]
impl BlobStore for IpfsBlobStore {
    async fn put(&self, slot: BlobSlot, key: &str, data: &[u8]) -> Result<(), std::io::Error> {
        self.ensure_mfs_dir(slot).await?;
        // 1. add (pin) → CID.
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(data.to_vec()).file_name(key.to_owned()),
        );
        let resp = self
            .client
            .post(self.url("/api/v0/add"))
            .query(&[("pin", "true"), ("quiet", "true")])
            .multipart(form)
            .send()
            .await
            .map_err(io("add"))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(std::io::Error::other(format!("IPFS add: {body}")));
        }
        let added: AddResponse = resp.json().await.map_err(io("add decode"))?;
        let cid = added.hash;

        // 2. MFS link: /ipfs/{cid} → /i0721/{slot}/{key}.
        let target = mfs_path(slot, key);
        let source = format!("/ipfs/{cid}");
        let resp = self
            .client
            .post(self.url("/api/v0/files/cp"))
            .query(&[
                ("arg", source.as_str()),
                ("arg", target.as_str()),
                ("force", "true"),
            ])
            .send()
            .await
            .map_err(io("files/cp"))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(std::io::Error::other(format!(
                "IPFS files/cp {target}: {body}"
            )));
        }
        Ok(())
    }

    async fn get(&self, slot: BlobSlot, key: &str) -> Result<Option<Bytes>, std::io::Error> {
        // MFS read resolves the key → CID → bytes.
        let path = mfs_path(slot, key);
        let resp = self
            .client
            .post(self.url("/api/v0/files/read"))
            .query(&[("arg", path.as_str())])
            .send()
            .await
            .map_err(io("files/read"))?;
        match resp.status().as_u16() {
            200 => {
                let bytes = resp.bytes().await.map_err(io("files/read body"))?;
                Ok(Some(bytes))
            }
            // kubo answers 500 for BOTH "file does not exist" (MFS
            // miss = our None) and genuine errors. Differentiate by the
            // body text — the same conservative face the delete path
            // uses: existence-related 500s are None; anything else
            // surfaces as an IoError.
            500 => {
                let body = resp.text().await.unwrap_or_default();
                if body.contains("does not exist")
                    || body.contains("not exist")
                    || body.contains("no such")
                {
                    Ok(None)
                } else {
                    Err(std::io::Error::other(format!(
                        "IPFS files/read {path}: 500 {body}"
                    )))
                }
            }
            code => {
                let body = resp.text().await.unwrap_or_default();
                Err(std::io::Error::other(format!(
                    "IPFS files/read {path}: {code} {body}"
                )))
            }
        }
    }

    async fn list_media_keys(&self) -> Result<Option<Vec<String>>, std::io::Error> {
        // Documented refusal (draft §7.1): MFS `files/ls` pagination and
        // directory typing are unverified on this lane — the backend
        // reports UNSUPPORTED rather than pretending to an empty scan.
        // The RPC surface turns this into "this backend cannot be
        // scanned", never into an empty diff.
        Ok(None)
    }

    async fn delete(&self, slot: BlobSlot, key: &str) -> Result<(), std::io::Error> {
        // 1. Resolve the MFS entry to a CID (for the pin removal).
        let path = mfs_path(slot, key);
        let stat_resp = self
            .client
            .post(self.url("/api/v0/files/stat"))
            .query(&[("arg", path.as_str())])
            .send()
            .await
            .map_err(io("files/stat"))?;
        if !stat_resp.status().is_success() {
            // Absent key: fine (delete is idempotent).
            return Ok(());
        }
        let stat: serde_json::Value = stat_resp.json().await.map_err(io("files/stat decode"))?;
        let cid = stat["Hash"].as_str().unwrap_or_default().to_owned();

        // 2. Remove the MFS entry.
        let resp = self
            .client
            .post(self.url("/api/v0/files/rm"))
            .query(&[("arg", path.as_str())])
            .send()
            .await
            .map_err(io("files/rm"))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(std::io::Error::other(format!(
                "IPFS files/rm {path}: {body}"
            )));
        }

        // 3. Best-effort unpin (content-addressed: another row
        // referencing the same CID keeps it alive; kubo's GC reclaims
        // unpinned content on its schedule).
        if !cid.is_empty() {
            let _ = self
                .client
                .post(self.url("/api/v0/pin/rm"))
                .query(&[("arg", cid.as_str())])
                .send()
                .await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_url_is_an_explicit_error() {
        let err = IpfsConfig::from_env(|_| None).unwrap_err();
        assert!(err.to_string().contains("I0721_IPFS_API"), "{err}");
    }

    #[test]
    fn api_url_parses() {
        let config = IpfsConfig::from_env(|k| {
            (k == "I0721_IPFS_API").then(|| "http://127.0.0.1:5001".to_owned())
        })
        .unwrap();
        assert_eq!(config.api_url, "http://127.0.0.1:5001");
    }

    #[test]
    fn mfs_paths() {
        assert_eq!(mfs_path(BlobSlot::Media, "abc"), "/i0721/media/abc");
        assert_eq!(mfs_path(BlobSlot::Cache, "x.webp"), "/i0721/cache/x.webp");
    }
}
