//! The site configuration model.
//!
//! The legacy engine kept this in a mutable `Config.toml` on disk and
//! restarted the process to apply changes. The rewrite stores the same
//! shape (kebab-case JSON, identical semantics) as one row of the
//! `settings` table and reads it per request — no restart, no file I/O.
//!
//! Seed defaults mirror `res/Config.default.toml`.

use serde::{Deserialize, Serialize};

/// `settings` key holding the whole [`SiteConfig`] as JSON.
pub const CONFIG_SETTING_KEY: &str = "site";

/// One footer banner entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FooterBanner {
    /// Visible text.
    pub text: String,
    /// Link target; `None` renders plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Portal-facing presentation settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PortalConfig {
    /// Suffix appended to the page title.
    #[serde(default = "default_title_suffix")]
    pub title_suffix: String,
    /// Footer banner entries.
    #[serde(default = "default_footer_banner")]
    pub footer_banner: Vec<FooterBanner>,
    /// Default language tag (legacy underscore form, e.g. `zh_hans`).
    #[serde(default = "default_language")]
    pub language: String,
    /// Timezone offset in hours applied on the timeline page.
    #[serde(default = "default_timezone")]
    pub timezone: i32,
}

fn default_title_suffix() -> String {
    "Ciallo～(∠·ω< )⌒★".to_owned()
}

fn default_footer_banner() -> Vec<FooterBanner> {
    vec![FooterBanner {
        text: "© 0721 project".to_owned(),
        url: Some("https://github.com/langyo/0721".to_owned()),
    }]
}

fn default_language() -> String {
    "zh_hans".to_owned()
}

fn default_timezone() -> i32 {
    8
}

impl Default for PortalConfig {
    fn default() -> Self {
        Self {
            title_suffix: default_title_suffix(),
            footer_banner: default_footer_banner(),
            language: default_language(),
            timezone: default_timezone(),
        }
    }
}

/// Router-facing settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RouterConfig {
    /// Public entry path of the media endpoint.
    /// Must stay `/media` — legacy image links point there.
    #[serde(default = "default_media_entry_path")]
    pub media_entry_path: String,
    /// Referrer host allow-list; empty = no limit.
    #[serde(default)]
    pub limit_referrer_host: Vec<String>,
}

fn default_media_entry_path() -> String {
    "/media".to_owned()
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            media_entry_path: default_media_entry_path(),
            limit_referrer_host: Vec::new(),
        }
    }
}

/// Upload pipeline settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UploadConfig {
    /// Upload size limit — `"8MiB"` style or a bare byte count string.
    #[serde(default = "default_image_size_limit")]
    pub image_size_limit: String,
    /// Auto-convert images (incl. animated GIF) to WebP.
    #[serde(default = "default_webp_auto_convert")]
    pub webp_auto_convert: bool,
    /// Keep the original file name as the display name.
    #[serde(default)]
    pub use_source_file_name: bool,
}

fn default_image_size_limit() -> String {
    "8MiB".to_owned()
}

fn default_webp_auto_convert() -> bool {
    true
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            image_size_limit: default_image_size_limit(),
            webp_auto_convert: default_webp_auto_convert(),
            use_source_file_name: false,
        }
    }
}

/// The whole site configuration (legacy `Config.toml` semantics).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SiteConfig {
    /// Portal presentation.
    #[serde(default)]
    pub portal: PortalConfig,
    /// Routing and hotlink protection.
    #[serde(default)]
    pub router: RouterConfig,
    /// Upload pipeline.
    #[serde(default)]
    pub upload: UploadConfig,
}

/// The seed configuration, mirroring `res/Config.default.toml`.
#[must_use]
pub fn default_site_config() -> SiteConfig {
    SiteConfig::default()
}

impl SiteConfig {
    /// Validate the configuration, returning a human-readable reason when
    /// a value cannot be honored.
    ///
    /// # Errors
    ///
    /// - `media_entry_path` is not `/media` (legacy image links must keep
    ///   working, so the public path is pinned).
    /// - `image_size_limit` does not parse (see [`crate::parse_size_limit`])
    ///   or exceeds the 16 MiB hard request-body cap.
    pub fn validate(&self) -> Result<(), String> {
        if self.router.media_entry_path != "/media" {
            return Err(format!(
                "router.media-entry-path must stay \"/media\" (got {:?}): \
                 legacy image links depend on it",
                self.router.media_entry_path
            ));
        }
        match crate::parse_size_limit(&self.upload.image_size_limit) {
            None => Err(format!(
                "upload.image-size-limit {:?} is not a byte size \
                 (\"8MiB\" style or a bare byte count)",
                self.upload.image_size_limit
            )),
            Some(n) if n > 16 * 1024 * 1024 => Err(format!(
                "upload.image-size-limit {:?} exceeds the 16 MiB request-body cap",
                self.upload.image_size_limit
            )),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_mirror_the_legacy_default_toml() {
        let cfg = default_site_config();
        assert_eq!(cfg.portal.title_suffix, "Ciallo～(∠·ω< )⌒★");
        assert_eq!(
            cfg.portal.footer_banner,
            vec![FooterBanner {
                text: "© 0721 project".to_owned(),
                url: Some("https://github.com/langyo/0721".to_owned()),
            }]
        );
        assert_eq!(cfg.portal.language, "zh_hans");
        assert_eq!(cfg.portal.timezone, 8);
        assert_eq!(cfg.router.media_entry_path, "/media");
        assert!(cfg.router.limit_referrer_host.is_empty());
        assert_eq!(cfg.upload.image_size_limit, "8MiB");
        assert!(cfg.upload.webp_auto_convert);
        assert!(!cfg.upload.use_source_file_name);
    }

    #[test]
    fn kebab_case_json_round_trip() {
        let cfg = default_site_config();
        let json = serde_json::to_value(&cfg).unwrap();
        assert!(json.get("portal").unwrap().get("title-suffix").is_some());
        assert!(json
            .get("router")
            .unwrap()
            .get("media-entry-path")
            .is_some());
        assert!(json
            .get("upload")
            .unwrap()
            .get("webp-auto-convert")
            .is_some());
        let back: SiteConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let cfg: SiteConfig =
            serde_json::from_str(r#"{"upload": {"use-source-file-name": true}}"#).unwrap();
        assert!(cfg.upload.use_source_file_name);
        assert!(cfg.upload.webp_auto_convert);
        assert_eq!(cfg.router.media_entry_path, "/media");
    }

    #[test]
    fn validation_rejects_moved_media_path() {
        let mut cfg = default_site_config();
        cfg.router.media_entry_path = "/img".to_owned();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validation_rejects_bad_size_limit() {
        let mut cfg = default_site_config();
        cfg.upload.image_size_limit = "8 potatoes".to_owned();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validation_rejects_limit_over_body_cap() {
        let mut cfg = default_site_config();
        cfg.upload.image_size_limit = "32MiB".to_owned();
        assert!(cfg.validate().is_err());
    }
}
