//! Pure data structures and validation for the 0721 resource service.
//!
//! This crate is the contract layer between [`i0721-store`] (data access) and
//! [`i0721-server`] (HTTP surface): records, request/response shapes, the
//! site configuration model and every validation rule. It performs **zero
//! I/O** — no filesystem, no network, no clock — so the whole surface is
//! unit-testable without external services.
//!
//! Time is [`jiff::Timestamp`] throughout; the legacy time crate is
//! banned workspace-wide.
//!
//! ```text
//! i0721-server ──▶ i0721-store ──▶ i0721-proto
//!      └─────────────────────────────▲
//! ```

pub mod config;
pub mod file;
pub mod page;
pub mod size;
pub mod user;

pub use config::{
    default_site_config, FooterBanner, SiteConfig, SiteConfigStrict, CONFIG_SETTING_KEY,
};
pub use file::{classify_kind, validate_file_name, FileDto, FileKind, FileRecord, NewFile};
pub use page::{PageArgs, PageResult};
pub use size::parse_size_limit;
pub use user::{validate_display_name, validate_username, NewUser, Role, UserDto, UserRecord};

/// Well-known keys inside the `settings` table.
pub mod settings_keys {
    /// The whole [`crate::SiteConfig`] as one JSON row (atomic get/set).
    pub const SITE: &str = "site";

    /// Per-user JWT invalidation watermark: RFC 3339 timestamp of the most
    /// recent login. Tokens minted before it (minus a 5 s leeway, matching
    /// the legacy engine) are rejected.
    pub fn token_watermark(user_id: &uuid::Uuid) -> String {
        format!("internal/token-watermark/{user_id}")
    }
}
