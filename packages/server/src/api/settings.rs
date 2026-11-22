//! `/api/v1/settings` — the site configuration, admin-gated.
//!
//! The legacy engine read a mutable `Config.toml` and restarted the
//! process to apply it. The rewrite stores the same shape in the
//! `settings` table and reads it per request — no restart.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use serde::Serialize;

use i0721_proto::{default_site_config, SiteConfig, SiteConfigStrict, CONFIG_SETTING_KEY};

use crate::api::auth::require_admin;
use crate::error::ApiError;
use crate::state::AppState;

/// The health/config view of the settings source.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct HealthReport {
    /// Always `"ok"` — a failure fails the request instead.
    pub status: &'static str,
    /// Service version.
    pub version: &'static str,
    /// Where the site configuration came from in the last read.
    pub config_source: &'static str,
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
    let config_source = match state.store.get_setting(CONFIG_SETTING_KEY).await {
        Ok(Some(raw)) => {
            if serde_json::from_value::<SiteConfig>(raw).is_ok() {
                "settings"
            } else {
                "default"
            }
        }
        Ok(_) => "default",
        Err(err) => {
            tracing::warn!(error = %err, "settings read failed — health reports default");
            "default"
        }
    };
    Json(HealthReport {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        config_source,
    })
}

/// `GET /api/v1/settings`
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SiteConfig>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(site_config(&state).await?))
}

/// `PUT /api/v1/settings` — a **full replacement**.
///
/// The wire form is [`SiteConfigStrict`]: every section and leaf is
/// required and unknown fields are rejected, so a partial body fails
/// with 400 instead of silently resetting the omitted sections to the
/// seed defaults.
pub async fn set(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<SiteConfigStrict>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers).await?;
    // A body the strict wire form rejects is a client error: map the
    // extractor's 422 to the API's uniform 400.
    let Json(strict) = payload.map_err(|rejection| {
        ApiError::bad_request(format!("invalid settings document: {rejection}"))
    })?;
    let config: SiteConfig = strict.into();
    config.validate().map_err(ApiError::bad_request)?;
    state
        .store
        .set_setting(
            CONFIG_SETTING_KEY,
            serde_json::to_value(&config)
                .map_err(|err| ApiError::internal(format!("config serialization failed: {err}")))?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
