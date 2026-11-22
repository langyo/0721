//! `GET /media/{key}` — the public path legacy image links depend on.
//!
//! Behavior carried over from the legacy engine: the referrer allow-list
//! (empty list = no check), anonymous access, `?thumbnail=true` serving
//! the cached WebP thumbnail — regenerating it on demand when the cache
//! entry is missing.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;

use crate::api::settings::site_config;
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::pipeline::thumbnail_for_media;
use crate::state::AppState;

/// The query of the media endpoint.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct MediaQuery {
    /// Serve the thumbnail variant.
    pub thumbnail: Option<bool>,
}

/// The referrer allow-list check shared by the media and share lanes.
pub(crate) async fn referer_allows(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<bool, ApiError> {
    let config = site_config(state).await?;
    if config.router.limit_referrer_host.is_empty() {
        return Ok(true);
    }
    let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(true);
    };
    let referer = url::Url::parse(referer)
        .map_err(|err| ApiError::bad_request(format!("bad referer: {err}")))?;
    let host = referer.host_str().unwrap_or_default();
    Ok(config
        .router
        .limit_referrer_host
        .iter()
        .any(|allowed| allowed == host))
}

/// `GET /media/{key}`
pub async fn download(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(args): Query<MediaQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let config = site_config(&state).await?;

    // Hotlink protection — the legacy semantics: the allow-list gates
    // only requests that carry a Referer at all.
    if !config.router.limit_referrer_host.is_empty() {
        if let Some(referer) = headers
            .get(header::REFERER)
            .and_then(|value| value.to_str().ok())
        {
            let referer = url::Url::parse(referer)
                .map_err(|err| ApiError::bad_request(format!("bad referer: {err}")))?;
            let host = referer.host_str().unwrap_or_default();
            if !config
                .router
                .limit_referrer_host
                .iter()
                .any(|allowed| allowed == host)
            {
                return Err(ApiError::forbidden("Referer not allowed"));
            }
        }
    }

    let record = state
        .store
        .get_file_by_storage_key(&key)
        .await?
        .ok_or_else(|| ApiError::not_found("media not found"))?;

    let (mime, bytes) = if args.thumbnail.unwrap_or(false) {
        if let Some(thumb_key) = record.thumbnail_key.as_deref() {
            if let Some(cached) = state.blobs.get(BlobSlot::Cache, thumb_key).await? {
                ("image/webp".to_owned(), cached)
            } else {
                regenerate_thumbnail(&state, &record.storage_key, thumb_key).await?
            }
        } else {
            // No thumbnail recorded (non-image): serve the original.
            let bytes = read_blob(&state, &record.storage_key).await?;
            (record.mime.clone(), bytes)
        }
    } else {
        let bytes = read_blob(&state, &record.storage_key).await?;
        (record.mime.clone(), bytes)
    };

    let mut response = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(response)
}

async fn read_blob(state: &AppState, key: &str) -> Result<Bytes, ApiError> {
    state
        .blobs
        .get(BlobSlot::Media, key)
        .await?
        .ok_or_else(|| ApiError::not_found("media blob missing"))
}

/// The legacy lazy path: no cache entry → regenerate from the original
/// blob, store it, serve it.
async fn regenerate_thumbnail(
    state: &AppState,
    storage_key: &str,
    thumb_key: &str,
) -> Result<(String, Bytes), ApiError> {
    let original = read_blob(state, storage_key).await?;
    let thumbnail = thumbnail_for_media(&original)
        .map_err(|err| ApiError::internal(format!("thumbnail regeneration failed: {err}")))?;
    state
        .blobs
        .put(BlobSlot::Cache, thumb_key, &thumbnail)
        .await?;
    Ok(("image/webp".to_owned(), thumbnail))
}
