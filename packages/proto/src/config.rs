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

/// The strict wire form of [`SiteConfig`] — what `PUT /api/v1/settings`
/// accepts. Unlike the storage form (whose serde defaults make partial
/// rows readable), every section and every leaf is **required** and
/// unknown fields are rejected: a PUT is a full replacement, so a
/// missing section would silently wipe live configuration back to the
/// seed defaults instead of failing the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SiteConfigStrict {
    /// Portal presentation.
    pub portal: PortalConfigStrict,
    /// Routing and hotlink protection.
    pub router: RouterConfigStrict,
    /// Upload pipeline.
    pub upload: UploadConfigStrict,
}

/// Strict [`PortalConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PortalConfigStrict {
    /// Suffix appended to the page title.
    pub title_suffix: String,
    /// Footer banner entries.
    pub footer_banner: Vec<FooterBanner>,
    /// Default language tag.
    pub language: String,
    /// Timezone offset in hours.
    pub timezone: i32,
}

/// Strict [`RouterConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RouterConfigStrict {
    /// Public entry path of the media endpoint (pinned to `/media` by
    /// [`SiteConfig::validate`]).
    pub media_entry_path: String,
    /// Referrer host allow-list; empty = no limit.
    pub limit_referrer_host: Vec<String>,
}

/// Strict [`UploadConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct UploadConfigStrict {
    /// Upload size limit — `"8MiB"` style or a bare byte count string.
    pub image_size_limit: String,
    /// Auto-convert images (incl. animated GIF) to WebP.
    pub webp_auto_convert: bool,
    /// Keep the original file name as the display name.
    pub use_source_file_name: bool,
}

impl From<SiteConfigStrict> for SiteConfig {
    fn from(strict: SiteConfigStrict) -> Self {
        SiteConfig {
            portal: PortalConfig {
                title_suffix: strict.portal.title_suffix,
                footer_banner: strict.portal.footer_banner,
                language: strict.portal.language,
                timezone: strict.portal.timezone,
            },
            router: RouterConfig {
                media_entry_path: strict.router.media_entry_path,
                limit_referrer_host: strict.router.limit_referrer_host,
            },
            upload: UploadConfig {
                image_size_limit: strict.upload.image_size_limit,
                webp_auto_convert: strict.upload.webp_auto_convert,
                use_source_file_name: strict.upload.use_source_file_name,
            },
        }
    }
}

impl From<&SiteConfig> for SiteConfigStrict {
    fn from(config: &SiteConfig) -> Self {
        SiteConfigStrict {
            portal: PortalConfigStrict {
                title_suffix: config.portal.title_suffix.clone(),
                footer_banner: config.portal.footer_banner.clone(),
                language: config.portal.language.clone(),
                timezone: config.portal.timezone,
            },
            router: RouterConfigStrict {
                media_entry_path: config.router.media_entry_path.clone(),
                limit_referrer_host: config.router.limit_referrer_host.clone(),
            },
            upload: UploadConfigStrict {
                image_size_limit: config.upload.image_size_limit.clone(),
                webp_auto_convert: config.upload.webp_auto_convert,
                use_source_file_name: config.upload.use_source_file_name,
            },
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
    fn stored_partial_json_falls_back_to_defaults_on_read() {
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

    fn full_strict_json() -> serde_json::Value {
        serde_json::to_value(SiteConfigStrict::from(&default_site_config())).unwrap()
    }

    #[test]
    fn strict_wire_accepts_the_complete_document() {
        let strict: SiteConfigStrict =
            serde_json::from_value(full_strict_json()).expect("full document parses");
        assert_eq!(SiteConfig::from(strict), default_site_config());
    }

    #[test]
    fn strict_wire_rejects_a_missing_section() {
        let mut json = full_strict_json();
        json.as_object_mut().unwrap().remove("upload");
        assert!(
            serde_json::from_value::<SiteConfigStrict>(json).is_err(),
            "a PUT without the upload section must fail, not reset it to defaults"
        );
    }

    #[test]
    fn strict_wire_rejects_an_unknown_section() {
        let mut json = full_strict_json();
        json.as_object_mut()
            .unwrap()
            .insert("extra".to_owned(), serde_json::json!({}));
        assert!(serde_json::from_value::<SiteConfigStrict>(json).is_err());
    }

    #[test]
    fn strict_wire_rejects_an_unknown_field_and_a_missing_leaf() {
        let mut unknown_leaf = full_strict_json();
        unknown_leaf["upload"]["max-pixels"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SiteConfigStrict>(unknown_leaf).is_err());

        let mut missing_leaf = full_strict_json();
        missing_leaf["router"]
            .as_object_mut()
            .unwrap()
            .remove("media-entry-path");
        assert!(serde_json::from_value::<SiteConfigStrict>(missing_leaf).is_err());
    }
}
