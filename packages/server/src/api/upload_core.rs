//! The shared upload core — the ONE chain every byte lane runs.
//!
//! Both `POST /api/v1/files` (multipart, [`crate::api::files`]) and the
//! MCP `upload_file` tool ([`crate::mcp`]) funnel through
//! [`run_upload`], in this order:
//!
//! 1. site-config pipeline parameters (size limit, WebP switch);
//! 2. the upload concurrency slot;
//! 3. the pipeline (size limit → SHA-256 → WebP → thumbnail);
//! 4. the per-owner content dedupe;
//! 5. destination-folder ownership;
//! 6. the quota (user override ?? site default);
//! 7. display-name validation;
//! 8. the blob writes (media + cache thumbnail);
//! 9. the metadata row, with the `Conflict` disambiguation.
//!
//! The extraction exists because the MCP tool used to carry a hand-rolled
//! copy of steps 1-9 that silently skipped the quota, the folder
//! ownership check, the upload semaphore and the configured pipeline
//! parameters, and hardcoded `storage_backend: "local"`. Keeping exactly
//! one chain means the next lane cannot repeat that.

use bytes::Bytes;
use uuid::Uuid;

use crate::api::settings::site_config;
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::pipeline::{process_upload, PipelineError, UploadRules};
use crate::state::AppState;
use i0721_proto::{validate_file_name, FileRecord, NewFile, Role, UserRecord};

/// The fallback size limit when the configured value does not parse
/// (`SiteConfig::validate` reports it; the upload lane still needs a
/// number — the legacy 8 MiB).
const FALLBACK_SIZE_LIMIT: u64 = 8 * 1024 * 1024;

/// One upload request, lane-independent: the caller has authenticated
/// and turned its own wire format into bytes + names.
pub struct UploadInput {
    /// The explicit display-name override (the REST lane's `?name=`).
    pub name: Option<String>,
    /// The source name (the multipart `filename`, the MCP `name`
    /// argument): feeds the pipeline's MIME guess and `original_name`,
    /// and becomes the display name when `upload.use_source_file_name`
    /// is on (the REST lane's historical behavior, now shared).
    pub source_name: Option<String>,
    /// Destination folder (validated below: exists, live, folder, owner).
    pub folder_id: Option<Uuid>,
    /// The uploaded bytes, exactly as the client sent them.
    pub data: Bytes,
}

/// What [`run_upload`] produced.
pub struct Uploaded {
    /// The metadata row (fresh, or the per-owner dedupe winner).
    pub record: FileRecord,
    /// `true` when this call inserted the row; `false` when the content
    /// dedupe answered with an existing one — the REST lane's `201` vs
    /// `200`.
    pub created: bool,
}

/// Run the upload chain for one authenticated user.
///
/// # Errors
///
/// [`ApiError`] with: `413` over the size limit or over quota, `400` for
/// an undecodable image, an invalid display name or an unknown folder,
/// `403` for another user's folder, `409` for a name clash inside the
/// destination folder, `500` for an encoding or storage failure.
pub async fn run_upload(
    state: &AppState,
    user: &UserRecord,
    input: UploadInput,
) -> Result<Uploaded, ApiError> {
    let UploadInput {
        name,
        source_name,
        folder_id,
        data,
    } = input;

    // 1. The site config drives the pipeline (size limit + WebP), read
    // per request like every other settings consumer.
    let config = site_config(state).await?;
    let rules = UploadRules {
        size_limit: i0721_proto::parse_size_limit(&config.upload.image_size_limit)
            .unwrap_or(FALLBACK_SIZE_LIMIT),
        webp_auto_convert: config.upload.webp_auto_convert,
    };

    // 2. One upload pipeline at a time, up to the configured
    // concurrency — the legacy engine capped image processing at 4 in
    // flight. The permit is held for the whole chain (as before).
    let _slot = state
        .upload_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal("upload lane closed"))?;

    // 3. The pipeline.
    let processed =
        process_upload(&data, source_name.as_deref(), &rules).map_err(pipeline_error)?;

    // 5. Destination folder: must be the caller's live folder (admins
    // may target any). Checked BEFORE any blob write so a rejected
    // upload leaves nothing behind.
    if let Some(folder_id) = folder_id {
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

    // 6. Quota: the caller's live bytes + this upload against their limit
    // (user override ?? the global default; None = unlimited).
    // TOCTOU boundary: the check-then-write window is bounded by the
    // upload semaphore (≤ upload_concurrency concurrent slots × the
    // per-file limit); a burst at the exact boundary can overshoot by at
    // most that many files' worth — accepted as the transition-era
    // trade-off, documented here so the next reader knows.
    // The SAME resolver as files.quota_get: user row ?? site default.
    let quota = match user.quota_bytes {
        Some(q) => Some(q),
        None => config.upload.default_quota_bytes,
    };
    if let Some(quota) = quota {
        let used = state.store.sum_live_sizes(user.id).await?;
        // The STORED size counts: it is what `sum_live_sizes` reports on
        // the next upload, so a converting pipeline (PNG → WebP) cannot
        // drift the ledger against the check.
        if used + processed.data.len() as i64 > quota {
            return Err(ApiError::too_large(format!(
                "quota exceeded: {used} bytes used, the limit is {quota}"
            )));
        }
    }

    // 7. The display name: the explicit override, else the source name
    // when the site asks for it, else the content hash.
    let display_name = name
        .or_else(|| {
            if config.upload.use_source_file_name {
                source_name.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| processed.sha256.clone());
    validate_file_name(&display_name).map_err(|err| ApiError::bad_request(err.to_string()))?;

    // 8. Dedupe runs AFTER the gates on purpose: a re-upload of content
    // the caller already owns still has to satisfy the folder/quota/name
    // gates of THIS request — otherwise an over-quota or foreign-folder
    // re-upload would answer 200 without ever being checked (the hole
    // this ordering closes; pinned by the over-quota re-upload test).
    if let Some(existing) = state
        .store
        .get_file_by_owner_and_sha256(user.id, &processed.sha256)
        .await?
    {
        return Ok(Uploaded {
            record: existing,
            created: false,
        });
    }

    // 9. Same bytes ⇒ same key ⇒ idempotent write even when another owner
    // already placed the blob (write-then-rename underneath).
    //
    // The bucket axis (R-I): the upload target comes ONLY from
    // `resolve_upload_bucket` — in this slice the override is always
    // `None`, so it resolves to the enabled bootstrap bucket. A
    // resolution failure is an ERROR; there is no fallback to any other
    // bucket (I2). `run_upload` never reads `config.storage_backend`:
    // the row's tag is the resolved bucket's backend.
    let (bucket_id, backend_tag, bucket_store) = state
        .registry
        .resolve_upload_bucket(None)
        .map_err(|err| ApiError::internal(format!("storage backend unavailable: {err}")))?;
    let storage_backend = backend_tag.as_str().to_owned();
    let storage_key = processed.sha256.clone();
    let thumbnail_key = processed
        .thumbnail
        .as_ref()
        .map(|_| format!("{storage_key}.webp"));
    bucket_store
        .put(BlobSlot::Media, &storage_key, &processed.data)
        .await?;
    if let (Some(thumbnail), Some(thumb_key)) =
        (processed.thumbnail.as_ref(), thumbnail_key.as_deref())
    {
        bucket_store
            .put(BlobSlot::Cache, thumb_key, thumbnail)
            .await?;
    }

    // 10. The metadata row.
    let new_file = NewFile {
        owner_id: user.id,
        parent_id: folder_id,
        name: display_name,
        kind: processed.kind,
        mime: processed.mime.clone(),
        size: processed.data.len() as i64,
        sha256: processed.sha256.clone(),
        // The row is born INTO the resolved bucket: reads route by this
        // id from its first request on.
        bucket_id: Some(bucket_id),
        // The bucket's OWN backend tag — the row and its bytes live in
        // the same bucket from birth.
        storage_backend,
        storage_key,
        thumbnail_key,
        original_name: source_name.unwrap_or_default(),
    };
    let record = match state.store.create_file(new_file).await {
        Ok(record) => record,
        // A Conflict is either the per-owner content dedupe (return the
        // winner's record) or, with folders live, the same-layer name
        // unique index -- distinguish by looking the content up.
        Err(i0721_store::StoreError::Conflict(_)) => state
            .store
            .get_file_by_owner_and_sha256(user.id, &processed.sha256)
            .await?
            .ok_or_else(|| {
                ApiError::conflict("a row with this name already exists in this folder")
            })?,
        Err(err) => return Err(err.into()),
    };
    Ok(Uploaded {
        record,
        created: true,
    })
}

/// Map the pipeline's failure ontology onto the API's.
fn pipeline_error(err: PipelineError) -> ApiError {
    match err {
        PipelineError::TooLarge { limit, actual } => {
            ApiError::too_large(format!("upload is {actual} bytes, the limit is {limit}"))
        }
        PipelineError::Decode(reason) => {
            ApiError::bad_request(format!("undecodable image: {reason}"))
        }
        PipelineError::Encode(reason) => {
            ApiError::internal(format!("WebP encoding failed: {reason}"))
        }
    }
}
