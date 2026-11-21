//! Process configuration — the A bootstrap layer.
//!
//! Every knob is an environment variable so the container image stays
//! configuration-free: the deployment layer (compose / systemd) passes
//! values, the site configuration lives in the `settings` table. A set
//! variable that fails to parse is an explicit error — never a silent
//! fallback.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use ipnet::IpNet;

/// Default bind address. The port stays unprivileged on purpose: the runtime
/// image runs as a non-root user, which cannot bind ports below 1024.
pub const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 8080);

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

/// Default first-boot admin password (README says it loudly: change it).
pub const DEFAULT_SEED_ADMIN_PASSWORD: &str = "admin";

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
    /// `I0721_TRUSTED_PROXIES` contains a non-CIDR entry.
    #[error("I0721_TRUSTED_PROXIES entry `{value}` is not a CIDR network")]
    TrustedProxies {
        /// The raw value.
        value: String,
    },
}

/// Widen one address into its singleton network (`/32` or `/128`).
fn ip_to_singleton_net(ip: IpAddr) -> IpNet {
    match ip {
        IpAddr::V4(v4) => {
            IpNet::V4(ipnet::Ipv4Net::new(v4, 32).expect("a valid v4 address accepts /32"))
        }
        IpAddr::V6(v6) => {
            IpNet::V6(ipnet::Ipv6Net::new(v6, 128).expect("a valid v6 address accepts /128"))
        }
    }
}

/// Whether `net` covers `ip` (mixed families simply do not match).
#[must_use]
pub fn net_contains(net: &IpNet, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpNet::V4(net), IpAddr::V4(ip)) => net.contains(&ip),
        (IpNet::V6(net), IpAddr::V6(ip)) => net.contains(&ip),
        _ => false,
    }
}

/// Parse a comma-separated CIDR list (`I0721_TRUSTED_PROXIES`).
fn parse_trusted_proxies(raw: &str) -> Result<Vec<IpNet>, ConfigError> {
    let mut nets = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // CIDRs parse directly; a bare host address widens to /32 (or
        // /128) so "192.168.1.1" and "10.0.0.0/8" both work.
        let candidate = entry
            .parse::<IpNet>()
            .ok()
            .or_else(|| entry.parse::<IpAddr>().ok().map(ip_to_singleton_net));
        nets.push(candidate.ok_or_else(|| ConfigError::TrustedProxies {
            value: entry.to_owned(),
        })?);
    }
    Ok(nets)
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
    /// First-boot admin password (empty user table only).
    pub seed_admin_password: String,
    /// Networks whose `X-Forwarded-For` is honored for client-IP
    /// derivation (login throttling). Empty = never trust the header.
    pub trusted_proxies: Vec<IpNet>,
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
        let seed_admin_password = std::env::var("I0721_SEED_ADMIN_PASSWORD")
            .unwrap_or_else(|_| DEFAULT_SEED_ADMIN_PASSWORD.to_owned());
        let trusted_proxies = match std::env::var("I0721_TRUSTED_PROXIES") {
            Ok(raw) => parse_trusted_proxies(&raw)?,
            Err(_) => Vec::new(),
        };
        Ok(Self {
            database_url,
            listen,
            jwt_secret,
            media_root,
            static_dir,
            upload_concurrency,
            log_filter,
            seed_admin_password,
            trusted_proxies,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_proxies_parse_cidrs_and_bare_ips() {
        let nets = parse_trusted_proxies("10.0.0.0/8, 192.168.1.1, 2001:db8::/32").unwrap();
        assert_eq!(nets.len(), 3);
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(net_contains(&nets[0], ip("10.1.2.3")));
        assert!(net_contains(&nets[1], ip("192.168.1.1")));
        assert!(!net_contains(&nets[1], ip("192.168.1.2")));
        assert!(net_contains(&nets[2], ip("2001:db8::1")));
        assert!(!net_contains(&nets[0], ip("2001:db8::1")));
        assert!(parse_trusted_proxies(" , ").unwrap().is_empty());
        assert!(parse_trusted_proxies("not-a-net").is_err());
        let err = parse_trusted_proxies("10.0.0.0/8, bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

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

#[cfg(test)]
mod listen_tests {
    use super::DEFAULT_LISTEN;

    /// The runtime image runs as a non-root user: a default below 1024 would
    /// make every container boot fail on bind. Pin the unprivileged default.
    #[test]
    fn default_listen_port_is_unprivileged() {
        assert!(
            DEFAULT_LISTEN.port() >= 1024,
            "default listen must stay >= 1024"
        );
    }
}
