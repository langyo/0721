//! The public share lane: `GET /api/v1/share/{token}/meta` (metadata,
//! no credentials) and `GET /api/v1/share/{token}/download` (streaming
//! download / inline preview by `allow_preview` + type). Expired or
//! revoked tokens answer 404 — the same shape as "never existed".

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

/// The metadata a share page renders (no credentials anywhere).
#[derive(Debug, Serialize)]
pub struct ShareMeta {
    pub file_name: String,
    pub file_size: i64,
    pub kind: String,
    pub mime: String,
    pub allow_preview: bool,
    pub expires_at: Option<String>,
}

/// Resolve a live, unexpired share to its file record.
async fn resolve(
    state: &AppState,
    token: &str,
) -> Result<(i0721_proto::ShareRecord, i0721_proto::FileRecord), ApiError> {
    let share = state
        .store
        .get_share(token)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    if let Some(expiry) = share.expires_at.as_deref() {
        let exp: jiff::Timestamp = expiry
            .parse()
            .map_err(|_| ApiError::internal("bad share expiry"))?;
        if jiff::Timestamp::now() > exp {
            return Err(ApiError::not_found("share not found"));
        }
    }
    let file = state
        .store
        .get_file(share.file_id)
        .await
        .map_err(ApiError::from)?
        .filter(|r| r.deleted_at.is_none())
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    Ok((share, file))
}

/// `GET /api/v1/share/{token}/meta`
pub async fn meta(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    match resolve(&state, &token).await {
        Ok((share, file)) => Json(ShareMeta {
            file_name: file.name,
            file_size: file.size,
            kind: file.kind.as_str().to_owned(),
            mime: file.mime,
            allow_preview: share.allow_preview,
            expires_at: share.expires_at,
        })
        .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /api/v1/share/{token}/download` — inline when preview is
/// allowed AND the type previews (image/document text/media);
/// attachment otherwise.
pub async fn download(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (share, file) = resolve(&state, &token).await?;
    let referer_ok = crate::media::referer_allows(&state, &headers).await?;
    if !referer_ok {
        return Err(ApiError::forbidden("Referer not allowed"));
    }
    let bytes = state
        .blobs
        .get(crate::blob::BlobSlot::Media, &file.storage_key)
        .await
        .map_err(ApiError::from)?
        .unwrap_or_default();
    let previewable = matches!(
        file.kind,
        i0721_proto::FileKind::Image
            | i0721_proto::FileKind::Document
            | i0721_proto::FileKind::Media
    );
    let inline = share.allow_preview && previewable;
    let disposition = if inline {
        format!("inline; filename=\"{}\"", file.name.replace('"', ""))
    } else {
        format!("attachment; filename=\"{}\"", file.name.replace('"', ""))
    };
    let mut response = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, file.mime.clone())],
        bytes,
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}
