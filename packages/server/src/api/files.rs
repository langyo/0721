//! The multipart upload lane (`POST /api/v1/files`) — the binary
//! REST-whitelist twin of the RPC file methods (`crate::rpc`).

use axum::extract::{Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use serde::Deserialize;

use crate::api::auth::require_auth;
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::pipeline::{process_upload, UploadRules};
use crate::state::AppState;
use i0721_proto::{validate_file_name, FileDto, NewFile, Role};

/// Optional query of the upload endpoint (the legacy `?name=`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UploadQuery {
    /// Force the display name of the uploaded file.
    pub name: Option<String>,
    /// The destination folder (validated: exists, live, folder, owner).
    pub folder_id: Option<uuid::Uuid>,
}

/// `POST /api/v1/files` (multipart, field `file`)
///
/// Pipeline: size limit → SHA-256 → WebP convert → thumbnail → dedupe
/// probe → blob write → metadata row. Identical content returns the
/// existing record with `200`; a fresh file answers `201`.
pub async fn upload(
    State(state): State<AppState>,
    Query(args): Query<UploadQuery>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    mut multipart: Multipart,
) -> Result<axum::response::Response, ApiError> {
    let (_, user) = require_auth(&state, &headers, &uri).await?;

    let field = multipart
        .next_field()
        .await
        .map_err(|err| ApiError::bad_request(format!("multipart read failed: {err}")))?
        .ok_or_else(|| ApiError::bad_request("no file field in the request"))?;
    let file_name = field.file_name().map(str::to_owned);
    let data = field
        .bytes()
        .await
        .map_err(|err| ApiError::bad_request(format!("multipart read failed: {err}")))?;

    let config = crate::api::settings::site_config(&state).await?;
    let rules = UploadRules {
        size_limit: i0721_proto::parse_size_limit(&config.upload.image_size_limit)
            .unwrap_or(8 * 1024 * 1024),
        webp_auto_convert: config.upload.webp_auto_convert,
    };

    // One upload pipeline at a time, up to the configured concurrency —
    // the legacy engine capped image processing at 4 in flight.
    let _slot = state
        .upload_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal("upload lane closed"))?;
    let processed =
        process_upload(&data, file_name.as_deref(), &rules).map_err(|err| match err {
            crate::pipeline::PipelineError::TooLarge { limit, actual } => {
                ApiError::too_large(format!("upload is {actual} bytes, the limit is {limit}"))
            }
            crate::pipeline::PipelineError::Decode(reason) => {
                ApiError::bad_request(format!("undecodable image: {reason}"))
            }
            crate::pipeline::PipelineError::Encode(reason) => {
                ApiError::internal(format!("WebP encoding failed: {reason}"))
            }
        })?;

    // Dedupe is per owner: re-uploading content THIS user already has
    // answers with their existing record; another user's identical file
    // is invisible here and becomes a fresh row below (sharing the same
    // content-addressed blob).
    if let Some(existing) = state
        .store
        .get_file_by_owner_and_sha256(user.id, &processed.sha256)
        .await?
    {
        return Ok((StatusCode::OK, Json(FileDto::from(existing))).into_response());
    }

    let storage_key = processed.sha256.clone();
    let thumbnail_key = processed
        .thumbnail
        .as_ref()
        .map(|_| format!("{storage_key}.webp"));

    // Same bytes ⇒ same key ⇒ idempotent write even when another owner
    // already placed the blob (write-then-rename underneath).
    state
        .blobs
        .put(BlobSlot::Media, &storage_key, &processed.data)
        .await?;
    if let (Some(thumbnail), Some(thumb_key)) =
        (processed.thumbnail.as_ref(), thumbnail_key.as_deref())
    {
        state
            .blobs
            .put(BlobSlot::Cache, thumb_key, thumbnail)
            .await?;
    }

    // Destination folder: must be the caller's live folder (admins may
    // target any).
    if let Some(folder_id) = args.folder_id {
        let folder = state
            .store
            .get_file(folder_id)
            .await?
            .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
            .ok_or_else(|| ApiError::bad_request("folder not found"))?;
        if user.role != Role::Admin && folder.owner_id != user.id {
            return Err(ApiError::forbidden("not your folder"));
        }
    }

    // Quota: the caller's live bytes + this upload against their limit
    // (user override ?? the global default; None = unlimited).
    if let Some(quota) = user.quota_bytes {
        let used = state.store.sum_live_sizes(user.id).await?;
        if used + data.len() as i64 > quota {
            return Err(ApiError::too_large(format!(
                "quota exceeded: {used} bytes used, the limit is {quota}"
            )));
        }
    }

    let name = args
        .name
        .clone()
        .or_else(|| {
            if config.upload.use_source_file_name {
                file_name.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| storage_key.clone());
    validate_file_name(&name).map_err(|err| ApiError::bad_request(err.to_string()))?;

    let new_file = NewFile {
        owner_id: user.id,
        parent_id: args.folder_id,
        name,
        kind: processed.kind,
        mime: processed.mime.clone(),
        size: processed.data.len() as i64,
        sha256: processed.sha256.clone(),
        storage_backend: "local".to_owned(),
        storage_key,
        thumbnail_key,
        original_name: file_name.unwrap_or_default(),
    };
    let record = match state.store.create_file(new_file).await {
        Ok(record) => record,
        // Lost the per-owner dedupe race: return the winner's record.
        Err(i0721_store::StoreError::Conflict(_)) => state
            .store
            .get_file_by_owner_and_sha256(user.id, &processed.sha256)
            .await?
            .ok_or_else(|| ApiError::conflict("content already uploaded"))?,
        Err(err) => return Err(err.into()),
    };
    Ok((StatusCode::CREATED, Json(FileDto::from(record))).into_response())
}
