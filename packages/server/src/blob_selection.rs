//! The blob backend selection: a pure, testable function that main.rs
//! calls. Each branch validates its own env completeness; an invalid
//! `I0721_STORAGE_BACKEND` or a missing required field answers a
//! human-readable error the binary prints before exiting.
//!
//! The bucket wave changed WHEN the variable is read (R-U): it is read
//! ONLY while migration 0007 is pending, it is REQUIRED then (no
//! default), and once 0007 is applied the variable is never read again
//! ([`bootstrap_backend_for_startup`] answers `Ok(None)` without
//! touching the env).

use std::sync::Arc;

use crate::blob::BlobStore;
use crate::blob_ipfs::{IpfsBlobStore, IpfsConfig, IpfsConfigError};
use crate::blob_s3::{S3BlobStore, S3Config, S3ConfigError};

/// The backends `select_blob_backend` accepts — the error message reads
/// this list, so the two can never drift apart.
pub const VALID_BACKENDS: [&str; 3] = ["local", "s3", "ipfs"];

/// Why the startup could not decide the bucket migration's facts.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// `I0721_STORAGE_BACKEND` is missing while 0007 is pending — the
    /// migration must not run half-built (fail-closed, no default).
    #[error("{var} is not set — it is required for the first start that runs the bucket migration (the bootstrap bucket's backend must be named explicitly)")]
    MissingBackend {
        /// The variable name.
        var: &'static str,
    },
    /// The variable is set but not one of the valid backends.
    #[error("{var}={value:?} is not a valid backend ({}) — refusing to run the bucket migration", VALID_BACKENDS.join(" | "))]
    InvalidBackend {
        /// The variable name.
        var: &'static str,
        /// The offending value.
        value: String,
    },
}

/// The backend label migration 0007 needs, decided BEFORE any pending
/// migration executes (R-U: an error here means `migrate()` never runs).
///
/// - 0007 pending → the env is REQUIRED (missing = [`StartupError::MissingBackend`],
///   invalid = [`StartupError::InvalidBackend`]).
/// - 0007 applied → `Ok(None)`: the variable is not read at all (set or
///   unset, it has no effect; setting it logs an info, never a behavior).
///
/// # Errors
///
/// [`StartupError`] when 0007 is pending and the env does not name a
/// valid backend.
pub fn bootstrap_backend_for_startup(
    env: Option<&str>,
    migration_0007_pending: bool,
) -> Result<Option<&'static str>, StartupError> {
    const VAR: &str = "I0721_STORAGE_BACKEND";
    if !migration_0007_pending {
        return Ok(None);
    }
    let Some(value) = env else {
        return Err(StartupError::MissingBackend { var: VAR });
    };
    match value {
        "local" | "s3" | "ipfs" => Ok(Some(
            VALID_BACKENDS
                .iter()
                .copied()
                .find(|b| *b == value)
                .unwrap(),
        )),
        other => Err(StartupError::InvalidBackend {
            var: VAR,
            value: other.to_owned(),
        }),
    }
}

/// Select and construct the blob backend for a process configuration.
///
/// STAGE-A NOTE: this remains for the transition and the `single`-store
/// fixtures; the runtime path goes through
/// [`crate::blob_registry::BlobRegistry`] now. It keeps the existing
/// semantics (invalid value = hard failure) and is no longer the
/// process-wide backend chooser.
///
/// # Errors
///
/// A human-readable error string when the selection is invalid or the
/// selected backend's environment is incomplete.
pub fn select_blob_backend(
    config: &crate::config::Config,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Arc<dyn BlobStore>, String> {
    match config.storage_backend.as_deref() {
        None => Err(
            "no storage backend is selected — the startup decision (bootstrap_backend_for_startup) was not consulted"
                .to_owned(),
        ),
        Some("local") => {
            let store = crate::blob::LocalBlobStore::open(&config.media_root)
                .map_err(|err| format!("local blob store open: {err}"))?;
            Ok(Arc::new(store))
        }
        Some("s3") => {
            let s3_config =
                S3Config::from_env(&env).map_err(|err: S3ConfigError| err.to_string())?;
            let store = S3BlobStore::open(&s3_config)
                .map_err(|err| format!("S3 blob store open: {err}"))?;
            Ok(Arc::new(store))
        }
        Some("ipfs") => {
            let ipfs_config =
                IpfsConfig::from_env(&env).map_err(|err: IpfsConfigError| err.to_string())?;
            Ok(Arc::new(IpfsBlobStore::open(&ipfs_config)))
        }
        Some(other) => Err(format!(
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
            storage_backend: Some(backend.to_owned()),
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
