//! `/api/v1/files` — timeline list, multipart upload (the legacy
//! pipeline), rename, soft delete.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::auth::require_auth;
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::pipeline::{process_upload, UploadRules};
use crate::state::AppState;
use i0721_proto::{validate_file_name, FileDto, NewFile, PageArgs, PageResult, Role};

/// Optional query of the upload endpoint (the legacy `?name=`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UploadQuery {
    /// Force the display name of the uploaded file.
    pub name: Option<String>,
}

/// The rename request body.
#[derive(Debug, Clone, Deserialize)]
pub struct RenameRequest {
    /// The new display name.
    pub name: String,
}

/// `GET /api/v1/files?offset&limit`
pub async fn list(
    State(state): State<AppState>,
    Query(args): Query<PageArgs>,
    headers: HeaderMap,
) -> Result<Json<PageResult<FileDto>>, ApiError> {
    require_auth(&state, &headers).await?;
    let (offset, limit) = args.sanitize();
    let total = state.store.count_files().await?;
    let items = state
        .store
        .list_files(offset, limit)
        .await?
        .into_iter()
        .map(FileDto::from)
        .collect();
    Ok(Json(PageResult {
        total,
        offset,
        limit,
        items,
    }))
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
    mut multipart: Multipart,
) -> Result<axum::response::Response, ApiError> {
    let (_, user) = require_auth(&state, &headers).await?;

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

/// The owner-or-admin gate: members may only mutate their own files,
/// admins any file. (The legacy engine gated deletion admin-only; the
/// netdisk reading of this wave is that your own uploads are yours —
/// recorded as a deliberate behavior change.)
async fn require_file_owner_or_admin(
    state: &AppState,
    headers: &HeaderMap,
    id: Uuid,
) -> Result<(i0721_proto::UserRecord, i0721_proto::FileRecord), ApiError> {
    let (_, user) = require_auth(state, headers).await?;
    let record = state
        .store
        .get_file(id)
        .await?
        .ok_or_else(|| ApiError::not_found("file not found"))?;
    if user.role != Role::Admin && record.owner_id != user.id {
        return Err(ApiError::forbidden("not your file"));
    }
    Ok((user, record))
}

/// `PATCH /api/v1/files/{id}`
pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<RenameRequest>,
) -> Result<Json<FileDto>, ApiError> {
    require_file_owner_or_admin(&state, &headers, id).await?;
    validate_file_name(&request.name).map_err(|err| ApiError::bad_request(err.to_string()))?;
    let record = state
        .store
        .rename_file(id, &request.name)
        .await?
        .ok_or_else(|| ApiError::not_found("file not found"))?;
    Ok(Json(FileDto::from(record)))
}

/// `DELETE /api/v1/files/{id}` — soft delete (guarded UPDATE). The blob
/// and thumbnail are reclaimed only when no other live row still
/// references the content-addressed key; full blob GC belongs to the
/// netdisk waves.
pub async fn remove(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    // The gate's fetch doubles as the pre-delete snapshot: the guarded
    // UPDATE below may be the second of a racing pair (then 404), and
    // after it the row is no longer readable through `get_file`.
    let (_, record) = require_file_owner_or_admin(&state, &headers, id).await?;
    let removed = state.store.soft_delete_file(id).await?;
    if !removed {
        return Err(ApiError::not_found("file not found"));
    }
    if state
        .store
        .count_live_files_by_storage_key(&record.storage_key)
        .await?
        == 0
    {
        // Compare-and-restore: an upload of the same content can land a new
        // live row between the count above and the unlink below (uploads
        // write the blob before `create_file`). Snapshot the bytes, unlink,
        // recount, and put the bytes back when a live row appeared — the
        // blob may flicker for a moment, but a live row is never orphaned.
        let snapshot = state
            .blobs
            .get(BlobSlot::Media, &record.storage_key)
            .await?;
        state
            .blobs
            .delete(BlobSlot::Media, &record.storage_key)
            .await?;
        if state
            .store
            .count_live_files_by_storage_key(&record.storage_key)
            .await?
            > 0
        {
            if let Some(bytes) = snapshot {
                state
                    .blobs
                    .put(BlobSlot::Media, &record.storage_key, &bytes)
                    .await?;
            }
        }
        // Thumbnails regenerate on demand when the cache is missing, so a
        // racing consumer simply pays one regeneration.
        if let Some(thumb_key) = record.thumbnail_key.as_deref() {
            state.blobs.delete(BlobSlot::Cache, thumb_key).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
