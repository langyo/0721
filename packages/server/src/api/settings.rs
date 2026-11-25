//! `/api/health` + the site-config read the RPC settings methods
//! (`settings.get` / `settings.put` in `crate::rpc`) share.
//!
//! The legacy engine read a mutable `Config.toml` and restarted the
//! process to apply it. The rewrite stores the same shape in the
//! `settings` table and reads it per request — no restart.

use axum::extract::State;
use axum::response::Json;
use serde::Serialize;

use i0721_proto::{default_site_config, SiteConfig, CONFIG_SETTING_KEY};

use crate::error::ApiError;
use crate::state::AppState;

/// One row of the health report's `settings` array (P-10): the key and
/// its R-F four-state tag (`absent` / `corrupt` / `unreadable` / `value`).
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct SettingHealth {
    /// The settings key.
    pub key: String,
    /// The four-state tag.
    pub state: String,
}

/// The settings keys health reports on (the site config and the upload
/// default bucket — the two keys the service executes).
pub const HEALTH_SETTINGS_KEYS: [&str; 2] = [CONFIG_SETTING_KEY, "storage_defaults"];

/// The health/config view of the settings source.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct HealthReport {
    /// Always `"ok"` — a failure fails the request instead.
    pub status: &'static str,
    /// Service version.
    pub version: &'static str,
    /// Where the site configuration came from in the last read:
    /// `settings` (a valid stored row), `default` (absent or corrupt —
    /// the documented fallback) or `unreadable` (the store itself
    /// failed; previously this was wrongly reported as `default`).
    pub config_source: &'static str,
    /// Every registered bucket's runtime state (empty when the registry
    /// could not be built, which fails the start instead).
    pub buckets: Vec<crate::blob_registry::BucketHealth>,
    /// The executed settings keys' four-state read (P-10). A read
    /// FAILURE is `unreadable`, never `absent` — the truth, not a
    /// fallback (R-P/HealthReport).
    pub settings: Vec<SettingHealth>,
}

/// Read the site configuration: the stored row when present and valid,
/// the seed defaults otherwise (with a log line — a missing or corrupt
/// row never takes the service down; `GET /api/health` exposes it).
///
/// # Errors
///
/// [`ApiError`] only when the store itself fails.
pub async fn site_config(state: &AppState) -> Result<SiteConfig, ApiError> {
    match state.store.get_setting(CONFIG_SETTING_KEY).await? {
        Some(raw) => match serde_json::from_value::<SiteConfig>(raw) {
            Ok(config) => Ok(config),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "stored site config is unreadable — serving defaults"
                );
                Ok(default_site_config())
            }
        },
        None => Ok(default_site_config()),
    }
}

/// `GET /api/health` (mounted at the app root, ayane-style).
pub async fn health(State(state): State<AppState>) -> Json<HealthReport> {
    use i0721_store::SettingsState;
    // The P-10 four-state array: one row per executed settings key.
    let mut settings = Vec::with_capacity(HEALTH_SETTINGS_KEYS.len());
    for key in HEALTH_SETTINGS_KEYS {
        let state_tag = match state.store.settings_state(key).await {
            Ok(SettingsState::Absent) => "absent",
            Ok(SettingsState::Corrupt { .. }) => "corrupt",
            Ok(SettingsState::Unreadable { .. }) => "unreadable",
            Ok(SettingsState::Value(_)) => "value",
            // A store failure must NOT read as "absent" — the truth is a
            // distinct fourth state (the same lesson config_source
            // learned: a failure is never a default).
            Err(_) => "unreadable",
        };
        settings.push(SettingHealth {
            key: key.to_owned(),
            state: state_tag.to_owned(),
        });
    }
    let config_source = match state.store.settings_state(CONFIG_SETTING_KEY).await {
        Ok(SettingsState::Value(value)) => {
            if serde_json::from_value::<SiteConfig>(value).is_ok() {
                "settings"
            } else {
                "default"
            }
        }
        Ok(_) => "default",
        // R-P/HealthReport: a store failure is NOT a "default" — the
        // truth ("unreadable") is a distinct fourth state.
        Err(err) => {
            tracing::warn!(error = %err, "settings read failed — health reports unreadable");
            "unreadable"
        }
    };
    Json(HealthReport {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        config_source,
        buckets: state.registry.health(),
        settings,
    })
}
