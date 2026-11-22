//! The blob backend selection: a pure, testable function that main.rs
//! calls. Each branch validates its own env completeness; an invalid
//! `I0721_STORAGE_BACKEND` or a missing required field answers a
//! human-readable error the binary prints before exiting.

use std::sync::Arc;

use crate::blob::BlobStore;
use crate::blob_ipfs::{IpfsBlobStore, IpfsConfig, IpfsConfigError};
use crate::blob_s3::{S3BlobStore, S3Config, S3ConfigError};

/// The backends `select_blob_backend` accepts — the error message reads
/// this list, so the two can never drift apart.
pub const VALID_BACKENDS: [&str; 3] = ["local", "s3", "ipfs"];

/// Select and construct the blob backend for a process configuration.
///
/// # Errors
///
/// A human-readable error string when the selection is invalid or the
/// selected backend's environment is incomplete.
pub fn select_blob_backend(
    config: &crate::config::Config,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn BlobStore>, String> {
    match config.storage_backend.as_str() {
        "local" => {
            let store = crate::blob::LocalBlobStore::open(&config.media_root)
                .map_err(|err| format!("local blob store open: {err}"))?;
            Ok(Arc::new(store))
        }
        "s3" => {
            let s3_config =
                S3Config::from_env(&env).map_err(|err: S3ConfigError| err.to_string())?;
            let store = S3BlobStore::open(&s3_config)
                .map_err(|err| format!("S3 blob store open: {err}"))?;
            Ok(Arc::new(store))
        }
        "ipfs" => {
            let ipfs_config =
                IpfsConfig::from_env(&env).map_err(|err: IpfsConfigError| err.to_string())?;
            Ok(Arc::new(IpfsBlobStore::open(&ipfs_config)))
        }
        other => Err(format!(
            "I0721_STORAGE_BACKEND={other:?} is not a valid backend ({})",
            VALID_BACKENDS.join(" | ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(backend: &str) -> crate::config::Config {
        crate::config::Config {
            database_url: "unused".to_owned(),
            listen: "127.0.0.1:8080".parse().unwrap(),
            jwt_secret: "test".to_owned(),
            media_root: std::path::PathBuf::from("/tmp/i0721-test-media"),
            static_dir: std::path::PathBuf::from("/tmp/i0721-test-static"),
            upload_concurrency: 1,
            log_filter: "off".to_owned(),
            seed_admin_password: "unused".to_owned(),
            trusted_proxies: Vec::new(),
            public_origin: None,
            cookie_secure: "auto".to_owned(),
            storage_backend: backend.to_owned(),
        }
    }

    #[test]
    fn local_backend_selects() {
        assert!(select_blob_backend(&test_config("local"), |_| None).is_ok());
    }

    #[test]
    fn s3_backend_missing_bucket_answers_a_named_error() {
        let result = select_blob_backend(&test_config("s3"), |_| None);
        let err = result.err().unwrap();
        assert!(err.contains("bucket"), "{err}");
        assert!(err.contains("I0721_S3_BUCKET"), "{err}");
    }

    #[test]
    fn s3_backend_full_config_selects() {
        let result = select_blob_backend(&test_config("s3"), |key| match key {
            "I0721_S3_BUCKET" => Some("b".to_owned()),
            "I0721_S3_REGION" => Some("auto".to_owned()),
            "I0721_S3_ACCESS_KEY" => Some("k".to_owned()),
            "I0721_S3_SECRET_KEY" => Some("s".to_owned()),
            _ => None,
        });
        assert!(result.is_ok());
    }

    #[test]
    fn ipfs_backend_missing_api_answers_a_named_error() {
        let result = select_blob_backend(&test_config("ipfs"), |_| None);
        let err = result.err().unwrap();
        assert!(err.contains("I0721_IPFS_API"), "{err}");
    }

    #[test]
    fn ipfs_backend_full_config_selects() {
        let result = select_blob_backend(&test_config("ipfs"), |key| {
            (key == "I0721_IPFS_API").then(|| "http://127.0.0.1:5001".to_owned())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn invalid_backend_answers_the_valid_options() {
        let err = select_blob_backend(&test_config("gcs"), |_| None)
            .err()
            .unwrap();
        assert!(err.contains("gcs"), "{err}");
        assert!(err.contains("local | s3 | ipfs"), "{err}");
    }
}
