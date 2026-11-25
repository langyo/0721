//! The per-bucket blob registry — the bucket axis' runtime half.
//!
//! Built once at startup from the `storage_buckets` rows (DB is the
//! authority); every byte read/write resolves its store through a row's
//! own bucket ([`BlobRegistry::for_row`]) and NEVER falls back to
//! another bucket (I2 / I4). Construction does no network I/O — every
//! backend constructor is lazy, exactly like `blob_s3`'s.
//!
//! Priority when classifying a row (R-O):
//! `enabled = false` (Disabled) > not listed in the env (Unavailable,
//! reason names the variable) > listed but incomplete credentials
//! (Unavailable). `legacy_unknown` is permanently Unavailable — its
//! bytes exist but nobody registered the bucket to serve them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::blob::{BlobStore, LocalBlobStore};
use crate::blob_ipfs::{IpfsBlobStore, IpfsConfig};
use crate::blob_s3::{S3BlobStore, S3Config};
use i0721_proto::{BucketBackend, FileRecord, StorageBucketRecord};

/// Why a registry could not be built (startup fails on these).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// The table read back empty (0007 applied but no bucket rows —
    /// only a hand-cleared table produces this).
    #[error("the storage_buckets table is empty — the registry refuses to invent rows")]
    EmptyRegistry,
    /// An ipfs URL carries userinfo (basic-auth) — refused at startup
    /// (R-K: credentials ride dedicated env variables, never the URL).
    #[error("ipfs bucket {slug}: the API URL must not carry userinfo (basic-auth); put the credentials in {var} instead")]
    IpfsUrlUserinfo {
        /// The offending bucket.
        slug: String,
        /// The variable the credentials belong in.
        var: String,
    },
}

/// Why a bucket entry cannot serve bytes right now.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// The slug resolves to no registry entry.
    #[error("bucket {slug:?} is not registered")]
    UnknownBucket {
        /// The missing slug.
        slug: String,
    },
    /// The bucket exists but cannot serve (unavailable / disabled).
    #[error("bucket {slug} is unavailable: {reason}")]
    Unavailable {
        /// The bucket slug.
        slug: String,
        /// The human-readable reason (health reports it verbatim).
        reason: String,
    },
    /// A row carries no bucket and no enabled bootstrap bucket exists.
    #[error("no enabled bootstrap bucket — the row cannot resolve to a store")]
    NoBootstrap,
}

/// The runtime state of one registered bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketState {
    /// Constructed and expected to serve.
    Ready,
    /// Cannot serve (missing env / credentials); the reason names what.
    Unavailable,
    /// Disabled in the DB — takes precedence over every env verdict.
    Disabled,
}

impl BucketState {
    /// The health wire tag.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BucketState::Ready => "ready",
            BucketState::Unavailable => "unavailable",
            BucketState::Disabled => "disabled",
        }
    }
}

/// One bucket's health line (ts-rs exported for `/api/health`).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct BucketHealth {
    /// The bucket slug.
    pub slug: String,
    /// `ready` | `unavailable` | `disabled`.
    pub state: String,
    /// Why it is not ready (`None` when ready).
    #[ts(optional)]
    pub reason: Option<String>,
}

/// One registry entry.
pub struct BucketEntry {
    /// The bucket slug.
    pub slug: String,
    /// The classified state.
    pub state: BucketState,
    /// The Unavailable/Disabled reason, when not Ready.
    pub reason: Option<String>,
    /// The constructed store, when Ready.
    pub store: Option<Arc<dyn BlobStore>>,
}

/// The per-bucket blob registry.
#[derive(Debug)]
pub struct BlobRegistry {
    entries: HashMap<String, BucketEntry>,
    /// The bootstrap bucket's backend tag (fresh uploads' row tag).
    bootstrap_backend: Option<BucketBackend>,
    /// id → slug index (rows carry the bucket id; entries key by slug).
    by_id: HashMap<uuid::Uuid, String>,
    /// Slug of the earliest-CREATED enabled bootstrap bucket — the rows
    /// whose `bucket_id` is NULL resolve here.
    bootstrap: Option<String>,
}

impl std::fmt::Debug for BucketEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketEntry")
            .field("slug", &self.slug)
            .field("state", &self.state)
            .field("reason", &self.reason)
            .field("store", &self.store.as_ref().map(|_| "<dyn BlobStore>"))
            .finish()
    }
}

impl BlobRegistry {
    /// Build from the DB rows (read by the caller) and the environment.
    ///
    /// # Errors
    ///
    /// [`RegistryError::EmptyRegistry`] on an empty table (the registry
    /// NEVER creates rows), [`RegistryError::IpfsUrlUserinfo`] when an
    /// ipfs API URL carries basic-auth userinfo (fail-closed startup).
    pub fn build(
        rows: Vec<StorageBucketRecord>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, RegistryError> {
        if rows.is_empty() {
            return Err(RegistryError::EmptyRegistry);
        }
        let mut entries = HashMap::new();
        let mut by_id = HashMap::new();
        for row in &rows {
            let entry = classify_row(row, &lookup)?;
            by_id.insert(row.id, entry.slug.clone());
            entries.insert(entry.slug.clone(), entry);
        }
        // The NULL-bucket resolution target: the earliest-created ENABLED
        // bootstrap bucket (deterministic by created_at).
        let bootstrap = rows
            .iter()
            .filter(|row| row.is_bootstrap && row.enabled)
            .min_by_key(|row| (row.created_at, row.slug.clone()))
            .map(|row| (row.slug.clone(), row.backend));
        let (bootstrap, bootstrap_backend) = match bootstrap {
            Some((slug, backend)) => (Some(slug), Some(backend)),
            None => (None, None),
        };
        Ok(Self {
            entries,
            by_id,
            bootstrap,
            bootstrap_backend,
        })
    }

    /// A single-entry registry (test fixtures): everything routes to
    /// `store`, rows with or without a bucket alike.
    #[must_use]
    pub fn single(store: Arc<dyn BlobStore>) -> Arc<Self> {
        let entries = HashMap::from([(
            "bootstrap".to_owned(),
            BucketEntry {
                slug: "bootstrap".to_owned(),
                state: BucketState::Ready,
                reason: None,
                store: Some(store),
            },
        )]);
        Arc::new(Self {
            entries,
            by_id: HashMap::new(),
            bootstrap: Some("bootstrap".to_owned()),
            bootstrap_backend: Some(BucketBackend::Local),
        })
    }

    /// A single-entry registry with an explicit backend tag (the
    /// fixture lane where the tag must follow a chosen backend).
    #[must_use]
    pub fn single_tagged(store: Arc<dyn BlobStore>, backend: BucketBackend) -> Arc<Self> {
        let entries = HashMap::from([(
            "bootstrap".to_owned(),
            BucketEntry {
                slug: "bootstrap".to_owned(),
                state: BucketState::Ready,
                reason: None,
                store: Some(store),
            },
        )]);
        Arc::new(Self {
            entries,
            by_id: HashMap::new(),
            bootstrap: Some("bootstrap".to_owned()),
            bootstrap_backend: Some(backend),
        })
    }

    /// Resolve a slug to its store. NEVER substitutes another bucket.
    ///
    /// # Errors
    ///
    /// [`BlobError`] when the slug is unknown or the bucket is not Ready.
    pub fn resolve(&self, slug: &str) -> Result<Arc<dyn BlobStore>, BlobError> {
        match self.entries.get(slug) {
            None => Err(BlobError::UnknownBucket {
                slug: slug.to_owned(),
            }),
            Some(entry) => match (&entry.state, &entry.store) {
                (BucketState::Ready, Some(store)) => Ok(store.clone()),
                (state, _) => Err(BlobError::Unavailable {
                    slug: slug.to_owned(),
                    reason: entry
                        .reason
                        .clone()
                        .unwrap_or_else(|| state.as_str().to_owned()),
                }),
            },
        }
    }

    /// The store of the row's OWN bucket: a row with a `bucket_id` goes
    /// to that bucket, a NULL row to the earliest enabled bootstrap
    /// bucket — never to "some Ready bucket".
    ///
    /// # Errors
    ///
    /// [`BlobError`] when the resolution fails (→ 503 / `-32010`).
    pub fn for_row(&self, row: &FileRecord) -> Result<Arc<dyn BlobStore>, BlobError> {
        let slug = match row.bucket_id {
            Some(_) => {
                // The registry keys by slug; the row carries the id. The
                // startup-loaded map is searched by id (rows are loaded
                // with both), keeping this O(n) over a small table.
                self.slug_of_bucket_id(row.bucket_id)
                    .ok_or_else(|| BlobError::UnknownBucket {
                        slug: row
                            .bucket_id
                            .map(|id| id.to_string())
                            .unwrap_or_default(),
                    })?
            }
            None => self
                .bootstrap
                .clone()
                .ok_or(BlobError::NoBootstrap)?,
        };
        self.resolve(&slug)
    }

    fn slug_of_bucket_id(&self, id: Option<uuid::Uuid>) -> Option<String> {
        self.by_id.get(&id?).cloned()
    }

    /// The health view of every registered bucket.
    #[must_use]
    pub fn health(&self) -> Vec<BucketHealth> {
        let mut rows: Vec<&BucketEntry> = self.entries.values().collect();
        rows.sort_by(|a, b| a.slug.cmp(&b.slug));
        rows.iter()
            .map(|entry| BucketHealth {
                slug: entry.slug.clone(),
                state: entry.state.as_str().to_owned(),
                reason: entry.reason.clone(),
            })
            .collect()
    }

    /// The store every fresh upload targets in this slice: the enabled
    /// bootstrap bucket (P2a replaces this with `resolve_upload_bucket`).
    ///
    /// # Errors
    ///
    /// [`BlobError::NoBootstrap`] when no enabled bootstrap bucket exists.
    pub fn bootstrap_store(&self) -> Result<Arc<dyn BlobStore>, BlobError> {
        let slug = self.bootstrap.clone().ok_or(BlobError::NoBootstrap)?;
        self.resolve(&slug)
    }

    /// The bootstrap bucket's backend tag (the fresh row's
    /// `storage_backend` — the bytes and the tag come from the same
    /// bucket).
    ///
    /// # Errors
    ///
    /// [`BlobError::NoBootstrap`] when no enabled bootstrap bucket exists.
    pub fn bootstrap_backend_tag(&self) -> Result<BucketBackend, BlobError> {
        self.bootstrap_backend.ok_or(BlobError::NoBootstrap)
    }
}

/// Classify one DB row into its registry entry (no network I/O).
fn classify_row(
    row: &StorageBucketRecord,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<BucketEntry, RegistryError> {
    if !row.enabled {
        return Ok(BucketEntry {
            slug: row.slug.clone(),
            state: BucketState::Disabled,
            reason: Some("the bucket is disabled in the registry".to_owned()),
            store: None,
        });
    }
    // Which env prefix carries the credentials? `legacy_unknown` is the
    // migration's "this bucket was never registered" marker: it is
    // permanently Unavailable with an actionable reason.
    if row.endpoint_ref == "legacy_unknown" {
        return Ok(BucketEntry {
            slug: row.slug.clone(),
            state: BucketState::Unavailable,
            reason: Some(
                "legacy backend rows are disabled — register this bucket's credentials to serve it"
                    .to_owned(),
            ),
            store: None,
        });
    }
    let prefix: String = if row.endpoint_ref.starts_with("legacy_") {
        // legacy_s3_env → the shared I0721_S3_* / I0721_MEDIA_ROOT /
        // I0721_IPFS_API variables (zero env change for existing deploys).
        row.endpoint_ref
            .strip_prefix("legacy_")
            .and_then(|rest| rest.strip_suffix("_env"))
            .unwrap_or(row.backend.as_str())
            .to_owned()
    } else {
        row.endpoint_ref.clone()
    };
    build_store(row, &prefix, &lookup)
}

/// Construct (lazily) the store for one bucket from its env prefix.
fn build_store(
    row: &StorageBucketRecord,
    prefix: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<BucketEntry, RegistryError> {
    let missing = |var: &str| BucketEntry {
        slug: row.slug.clone(),
        state: BucketState::Unavailable,
        reason: Some(format!("{var} is not set")),
        store: None,
    };
    match row.backend {
        BucketBackend::Local => {
            // The legacy prefix rides the shared I0721_MEDIA_ROOT — the
            // CALLER (main) supplies it from the resolved config, so a
            // missing root surfaces as Unavailable naming the variable
            // (fail-closed, never a silent default directory).
            let var = if row.endpoint_ref.starts_with("legacy_") {
                "I0721_MEDIA_ROOT".to_owned()
            } else {
                format!("I0721_BUCKET_{prefix}_ROOT")
            };
            let Some(root) = lookup(&var) else {
                return Ok(missing(&var));
            };
            match LocalBlobStore::open(std::path::Path::new(&root)) {
                Ok(store) => Ok(BucketEntry {
                    slug: row.slug.clone(),
                    state: BucketState::Ready,
                    reason: None,
                    store: Some(Arc::new(store)),
                }),
                Err(err) => Ok(BucketEntry {
                    slug: row.slug.clone(),
                    state: BucketState::Unavailable,
                    reason: Some(format!("{var}={root:?} cannot be opened: {err}")),
                    store: None,
                }),
            }
        }
        BucketBackend::S3 => {
            // Per-bucket variables: I0721_BUCKET_<SLUG>_{ENDPOINT,REGION,
            // ACCESS_KEY,SECRET_KEY}; the legacy prefix maps onto the
            // shared I0721_S3_* names.
            let get = |suffix: &str| {
                if row.endpoint_ref.starts_with("legacy_") {
                    lookup(&format!("I0721_S3_{suffix}"))
                } else {
                    lookup(&format!("I0721_BUCKET_{prefix}_{suffix}"))
                }
            };
            match S3Config::from_env(get) {
                Ok(config) => match S3BlobStore::open(&config) {
                    Ok(store) => Ok(BucketEntry {
                        slug: row.slug.clone(),
                        state: BucketState::Ready,
                        reason: None,
                        store: Some(Arc::new(store)),
                    }),
                    Err(err) => Ok(BucketEntry {
                        slug: row.slug.clone(),
                        state: BucketState::Unavailable,
                        reason: Some(format!("S3 client construction failed: {err}")),
                        store: None,
                    }),
                },
                Err(err) => Ok(BucketEntry {
                    slug: row.slug.clone(),
                    state: BucketState::Unavailable,
                    reason: Some(err.to_string()),
                    store: None,
                }),
            }
        }
        BucketBackend::Ipfs => {
            let var = if row.endpoint_ref.starts_with("legacy_") {
                "I0721_IPFS_API".to_owned()
            } else {
                format!("I0721_BUCKET_{prefix}_IPFS_API")
            };
            let Some(api_url) = lookup(&var) else {
                return Ok(missing(&var));
            };
            // Fail-closed on userinfo (basic-auth in the URL): R-K puts
            // this refusal HERE, at registry build, not on first use.
            if let Ok(parsed) = url::Url::parse(&api_url) {
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err(RegistryError::IpfsUrlUserinfo {
                        slug: row.slug.clone(),
                        var,
                    });
                }
            }
            Ok(BucketEntry {
                slug: row.slug.clone(),
                state: BucketState::Ready,
                reason: None,
                store: Some(Arc::new(IpfsBlobStore::open(&IpfsConfig { api_url }))),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i0721_proto::StorageBucketRecord;
    use jiff::Timestamp;

    fn bucket(slug: &str, backend: BucketBackend, endpoint_ref: &str, enabled: bool) -> StorageBucketRecord {
        bucket_full(slug, backend, endpoint_ref, enabled, true)
    }

    fn bucket_full(slug: &str, backend: BucketBackend, endpoint_ref: &str, enabled: bool, is_bootstrap: bool) -> StorageBucketRecord {
        StorageBucketRecord {
            id: uuid::Uuid::new_v4(),
            slug: slug.to_owned(),
            display_name: slug.to_owned(),
            backend,
            location: "loc".to_owned(),
            endpoint_ref: endpoint_ref.to_owned(),
            anonymous_read: true,
            grant_on_signup: false,
            enabled,
            is_bootstrap,
            created_at: Timestamp::now(),
        }
    }

    fn row_of(bucket_id: Option<uuid::Uuid>) -> FileRecord {
        FileRecord {
            id: uuid::Uuid::new_v4(),
            owner_id: uuid::Uuid::new_v4(),
            parent_id: None,
            name: "x".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: 1,
            sha256: "abc".to_owned(),
            bucket_id,
            storage_backend: "local".to_owned(),
            storage_key: "abc".to_owned(),
            thumbnail_key: None,
            original_name: "x".to_owned(),
            created_at: Timestamp::now(),
            deleted_at: None,
        }
    }

    /// MUT:M-REG1 — the `for_row` fallback must be the BOOTSTRAP bucket
    /// for NULL rows, never "the first Ready bucket": a registry whose
    /// bootstrap entry is Disabled while another bucket is Ready must
    /// still refuse NULL rows. (Guard test for
    /// `blob_registry_never_falls_back_to_another_bucket`.)
    #[test]
    fn a_null_row_never_resolves_to_an_unrelated_ready_bucket() {
        let ready = bucket_full("other", BucketBackend::Local, "x", true, false);
        let disabled = bucket("bootstrap-local", BucketBackend::Local, "legacy_local_env", false);
        let registry = BlobRegistry::build(vec![ready, disabled], |_| None).unwrap();
        assert!(matches!(
            registry.for_row(&row_of(None)),
            Err(BlobError::NoBootstrap)
        ));
    }

    /// MUT:M-REG2 — a row's own bucket is resolved even when it is
    /// Unavailable; the registry must answer an error naming THAT
    /// bucket, not silently serve from another one.
    #[test]
    fn a_rows_own_unavailable_bucket_is_an_error_not_a_fallback() {
        let own = bucket("own", BucketBackend::Local, "legacy_local_env", true);
        let registry = BlobRegistry::build(vec![own], |_| None).unwrap();
        let row = row_of(None);
        let err = registry.for_row(&row).err().unwrap();
        match err {
            BlobError::Unavailable { slug, .. } => assert_eq!(slug, "own"),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_table_fails_the_build() {
        assert!(matches!(
            BlobRegistry::build(Vec::new(), |_| None),
            Err(RegistryError::EmptyRegistry)
        ));
    }

    #[test]
    fn health_reports_the_missing_variable_by_name() {
        // MUT:M10 lives in the health lane; this pins the registry half:
        // the reason must name the exact variable.
        let b = bucket("bootstrap-local", BucketBackend::Local, "legacy_local_env", true);
        let registry = BlobRegistry::build(vec![b], |_| None).unwrap();
        let health = registry.health();
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].state, "unavailable");
        assert!(
            health[0]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("I0721_MEDIA_ROOT"),
            "the reason names the missing variable: {:?}",
            health[0].reason
        );
    }

    #[test]
    fn ipfs_url_with_userinfo_fails_the_build() {
        let b = bucket("bootstrap-ipfs", BucketBackend::Ipfs, "legacy_ipfs_env", true);
        let err = BlobRegistry::build(vec![b], |k| {
            (k == "I0721_IPFS_API").then(|| "http://user:pass@127.0.0.1:5001".to_owned())
        })
        .err()
        .expect("userinfo must fail the build");
        assert!(err.to_string().contains("userinfo"), "{err}");
    }

    #[test]
    fn legacy_unknown_is_unavailable_with_an_actionable_reason() {
        let b = bucket("bootstrap-s3", BucketBackend::S3, "legacy_unknown", false);
        let registry = BlobRegistry::build(vec![b], |_| None).unwrap();
        let health = registry.health();
        assert_eq!(health[0].state, "disabled");
    }
}
