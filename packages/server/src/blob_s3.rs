//! The S3-compatible blob backend (`object_store` crate, AWS feature).
//!
//! Works against AWS S3, MinIO, Cloudflare R2, and anything speaking
//! the S3 API. The endpoint is optional: absent = plain AWS; present =
//! path-style against the custom endpoint (the MinIO/R2 form).
//!
//! Key layout: `{slot_prefix}/{key}` — `media/{sha256}` and
//! `cache/{sha256}.webp` — mirroring the local store's directory
//! structure so the two are operationally symmetric.
//!
//! The `object_store` crate handles sigv4 signing internally; this
//! module is purely configuration + the [`BlobStore`] adapter.

use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as StorePath;
use object_store::{ObjectStore, PutPayload};

use crate::blob::{BlobSlot, BlobStore};

/// S3 configuration from the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Config {
    /// The bucket name (required).
    pub bucket: String,
    /// AWS region (required for AWS; `auto` works for MinIO/R2).
    pub region: String,
    /// Custom endpoint (path-style); `None` = plain AWS.
    pub endpoint: Option<String>,
    /// Access key (required).
    pub access_key: String,
    /// Secret key (required).
    pub secret_key: String,
}

/// Why an S3 configuration is incomplete.
#[derive(Debug, thiserror::Error)]
pub enum S3ConfigError {
    /// `I0721_S3_BUCKET` (or access/secret) missing while the backend
    /// selection says s3.
    #[error("S3 backend selected but {field} is not set (I0721_S3_{env})")]
    Missing {
        /// The human-facing field name.
        field: &'static str,
        /// The env var suffix.
        env: &'static str,
    },
}

impl S3Config {
    /// Read from `I0721_S3_*` environment variables.
    ///
    /// # Errors
    ///
    /// [`S3ConfigError::Missing`] when a required field is absent.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, S3ConfigError> {
        let get = |suffix: &str| lookup(&format!("I0721_S3_{suffix}"));
        Ok(Self {
            bucket: get("BUCKET").ok_or(S3ConfigError::Missing {
                field: "bucket name",
                env: "BUCKET",
            })?,
            region: get("REGION").ok_or(S3ConfigError::Missing {
                field: "region",
                env: "REGION",
            })?,
            endpoint: get("ENDPOINT"),
            access_key: get("ACCESS_KEY").ok_or(S3ConfigError::Missing {
                field: "access key",
                env: "ACCESS_KEY",
            })?,
            secret_key: get("SECRET_KEY").ok_or(S3ConfigError::Missing {
                field: "secret key",
                env: "SECRET_KEY",
            })?,
        })
    }
}

/// The slot→prefix mapping (mirrors the local directory layout).
fn slot_prefix(slot: BlobSlot) -> &'static str {
    match slot {
        BlobSlot::Media => "media",
        BlobSlot::Cache => "cache",
    }
}

/// Why an S3 backend operation failed.
#[derive(Debug)]
pub struct S3Error {
    /// The object_store error.
    pub source: object_store::Error,
    /// The full key that was being accessed.
    pub key: String,
}

impl fmt::Display for S3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "S3 operation on {:?}: {}", self.key, self.source)
    }
}

impl std::error::Error for S3Error {}

impl From<S3Error> for std::io::Error {
    fn from(err: S3Error) -> Self {
        std::io::Error::other(err)
    }
}

/// The S3 blob store.
pub struct S3BlobStore {
    store: Arc<dyn ObjectStore>,
}

impl S3BlobStore {
    /// Build from a complete [`S3Config`].
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when the underlying client cannot be
    /// constructed (bad endpoint URL, etc.).
    pub fn open(config: &S3Config) -> Result<Self, std::io::Error> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket)
            .with_region(&config.region)
            .with_access_key_id(&config.access_key)
            .with_secret_access_key(&config.secret_key);
        if let Some(endpoint) = config.endpoint.as_deref() {
            builder = builder.with_endpoint(endpoint).with_allow_http(true); // MinIO on localhost
        }
        let store = builder
            .build()
            .map_err(|err| std::io::Error::other(format!("S3 client construction: {err}")))?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    fn path(&self, slot: BlobSlot, key: &str) -> StorePath {
        StorePath::from(format!("{}/{}", slot_prefix(slot), key))
    }
}

#[async_trait::async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, slot: BlobSlot, key: &str, data: &[u8]) -> Result<(), std::io::Error> {
        let path = self.path(slot, key);
        self.store
            .put(&path, PutPayload::from(data.to_vec()))
            .await
            .map(|_| ())
            .map_err(|source| {
                S3Error {
                    source,
                    key: path.to_string(),
                }
                .into()
            })
    }

    async fn get(&self, slot: BlobSlot, key: &str) -> Result<Option<Bytes>, std::io::Error> {
        let path = self.path(slot, key);
        match self.store.get(&path).await {
            Ok(result) => {
                let bytes = result.bytes().await.map_err(|source| S3Error {
                    source,
                    key: path.to_string(),
                })?;
                Ok(Some(bytes))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(source) => Err(S3Error {
                source,
                key: path.to_string(),
            }
            .into()),
        }
    }

    async fn delete(&self, slot: BlobSlot, key: &str) -> Result<(), std::io::Error> {
        let path = self.path(slot, key);
        match self.store.delete(&path).await {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(source) => Err(S3Error {
                source,
                key: path.to_string(),
            }
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_bucket_is_an_explicit_error() {
        let err = S3Config::from_env(|_| None).unwrap_err();
        assert!(err.to_string().contains("bucket"), "{err}");
        assert!(err.to_string().contains("I0721_S3_BUCKET"), "{err}");
    }

    #[test]
    fn missing_secret_is_an_explicit_error() {
        let err = S3Config::from_env(|key| match key {
            "I0721_S3_BUCKET" => Some("b".to_owned()),
            "I0721_S3_REGION" => Some("auto".to_owned()),
            "I0721_S3_ACCESS_KEY" => Some("k".to_owned()),
            _ => None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("secret"), "{err}");
    }

    #[test]
    fn full_config_parses() {
        let config = S3Config::from_env(|key| match key {
            "I0721_S3_BUCKET" => Some("my-bucket".to_owned()),
            "I0721_S3_REGION" => Some("auto".to_owned()),
            "I0721_S3_ENDPOINT" => Some("http://127.0.0.1:9000".to_owned()),
            "I0721_S3_ACCESS_KEY" => Some("minioadmin".to_owned()),
            "I0721_S3_SECRET_KEY" => Some("minioadmin".to_owned()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.endpoint.as_deref(), Some("http://127.0.0.1:9000"));
    }

    #[test]
    fn slot_prefixes() {
        assert_eq!(slot_prefix(BlobSlot::Media), "media");
        assert_eq!(slot_prefix(BlobSlot::Cache), "cache");
    }

    #[test]
    fn endpoint_is_stored_for_path_style_access() {
        let config = S3Config {
            bucket: "b".to_owned(),
            region: "auto".to_owned(),
            endpoint: Some("http://127.0.0.1:9000".to_owned()),
            access_key: "k".to_owned(),
            secret_key: "s".to_owned(),
        };
        // object_store validates lazily — construction succeeds; the
        // endpoint's validity surfaces on the first request. The
        // construction test verifies the builder ACCEPTS the config.
        assert!(S3BlobStore::open(&config).is_ok());
    }
}
