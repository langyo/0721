//! Process configuration — the A bootstrap layer.
//!
//! Every knob is an environment variable so the container image stays
//! configuration-free: the deployment layer (compose / systemd) passes
//! values, the site configuration lives in the `settings` table. A set
//! variable that fails to parse is an explicit error — never a silent
//! fallback.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Default bind address (the container publishes :80).
pub const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 80);

/// Default root for blobs (`media/` and `media-cache/` live below it),
/// matching the legacy `MEDIA_DIR` / `MEDIA_CACHE_DIR` semantics.
pub const DEFAULT_MEDIA_ROOT: &str = "data";

/// Default directory of the placeholder static site.
pub const DEFAULT_STATIC_DIR: &str = "res/website";

/// Default concurrent upload-pipeline slots (the legacy limit was 4).
pub const DEFAULT_UPLOAD_CONCURRENCY: usize = 4;

/// Hard request-body cap (the legacy router limit); the configurable
/// per-site `upload.image-size-limit` is enforced on top of it.
pub const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Why the environment could not be turned into a [`Config`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required variable is not set.
    #[error("{var} is not set — the service refuses to start without it")]
    Missing {
        /// The variable name.
        var: &'static str,
    },
    /// `I0721_LISTEN` is not a socket address.
    #[error("I0721_LISTEN `{value}` is not a socket address")]
    Listen {
        /// The raw value.
        value: String,
        /// The parser error.
        #[source]
        source: std::net::AddrParseError,
    },
    /// `I0721_UPLOAD_CONCURRENCY` is not a positive integer.
    #[error("I0721_UPLOAD_CONCURRENCY `{value}` is not a positive integer")]
    UploadConcurrency {
        /// The raw value.
        value: String,
    },
}

/// Environment-driven process configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// PostgreSQL URL handed to the store.
    pub database_url: String,
    /// HTTP bind address.
    pub listen: SocketAddr,
    /// HS256 secret for the transition-era JWTs.
    pub jwt_secret: String,
    /// Root of the blob directories (`media/`, `media-cache/`).
    pub media_root: PathBuf,
    /// Directory of the placeholder static site.
    pub static_dir: PathBuf,
    /// Concurrent upload-pipeline slots.
    pub upload_concurrency: usize,
    /// `tracing` filter directive.
    pub log_filter: String,
}

impl Config {
    /// Read the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a required variable is missing or a set
    /// variable cannot be parsed.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            std::env::var("I0721_DATABASE_URL").map_err(|_| ConfigError::Missing {
                var: "I0721_DATABASE_URL",
            })?;
        let jwt_secret =
            std::env::var("JWT_SECRET").map_err(|_| ConfigError::Missing { var: "JWT_SECRET" })?;
        let listen = match std::env::var("I0721_LISTEN") {
            Ok(raw) => raw
                .parse()
                .map_err(|source| ConfigError::Listen { value: raw, source })?,
            Err(_) => DEFAULT_LISTEN,
        };
        let media_root = PathBuf::from(
            std::env::var("I0721_MEDIA_ROOT").unwrap_or_else(|_| DEFAULT_MEDIA_ROOT.to_owned()),
        );
        let static_dir = PathBuf::from(
            std::env::var("I0721_STATIC_DIR").unwrap_or_else(|_| DEFAULT_STATIC_DIR.to_owned()),
        );
        let upload_concurrency = match std::env::var("I0721_UPLOAD_CONCURRENCY") {
            Ok(raw) => raw
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or(ConfigError::UploadConcurrency { value: raw })?,
            Err(_) => DEFAULT_UPLOAD_CONCURRENCY,
        };
        let log_filter = std::env::var("I0721_LOG")
            .unwrap_or_else(|_| "i0721_server=info,tower_http=warn".to_owned());
        Ok(Self {
            database_url,
            listen,
            jwt_secret,
            media_root,
            static_dir,
            upload_concurrency,
            log_filter,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_errors_carry_the_raw_value() {
        let err = "not-an-addr".parse::<SocketAddr>().unwrap_err();
        let err = ConfigError::Listen {
            value: "not-an-addr".to_owned(),
            source: err,
        };
        assert!(err.to_string().contains("I0721_LISTEN"));
    }
}
