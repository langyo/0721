//! The unregistered-object lane (P3) — the admin surface over blobs that
//! exist in a bucket's `media/` namespace without a `files` row.
//!
//! Contract: B-SPEC v5.1 R-E / R-P + B-DESIGN-DRAFT §7. Three pieces:
//!
//! * the SHOW SWITCH (`storage_unregistered.show`, settings key; R-E —
//!   default OFF, and any non-`Value(true)` four-state read is OFF);
//! * the DIFF (listed `media/` keys − live rows' `storage_key` in the
//!   bucket; the `cache/` prefix is excluded AT ENUMERATION, never by a
//!   post-filter — a thumbnail legitimately has no row);
//! * ADOPT (the only write; there is NO delete — an unregistered object
//!   is adopted first, then removed through the row's reclaim chain).

use std::sync::Arc;

use crate::api::settings::site_config;
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::state::AppState;
use i0721_proto::{classify_kind, NewFile, UserRecord};

/// The R-E settings key. Its value is the JSON boolean `true` when the
/// unregistered surface is shown; every other four-state outcome
/// (`Absent` / `Corrupt` / `Unreadable` / any other JSON) is OFF.
pub const UNREGISTERED_SHOW_SETTINGS_KEY: &str = "storage_unregistered.show";

/// The R-F four-state read of the show switch, collapsed to the lane's
/// decision: `true` ONLY from a `Value(true)`; Absent → `false` (the
/// default); Corrupt/Unreadable/any-other → `false` as well (R-E: the
/// switch is fail-closed in the hidden direction — a broken setting can
/// never EXPOSE the unregistered surface, only hide it).
/// MUT:P3-SHOW-DEFAULT — defaulting to `true` here reddens every
/// `unregistered_off_*` test, starting with
/// `unregistered_off_rpc_list_refuses`.
pub(crate) async fn unregistered_show(state: &AppState) -> Result<bool, ApiError> {
    use i0721_store::SettingsState;
    Ok(matches!(
        state
            .store
            .settings_state(UNREGISTERED_SHOW_SETTINGS_KEY)
            .await?,
        SettingsState::Value(serde_json::Value::Bool(true))
    ))
}

/// A listed media key that has no live row in its bucket.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct UnregisteredObject {
    /// The blob key (`media/` prefix already stripped).
    pub key: String,
    /// Byte length (read from the stored object).
    pub size: i64,
    /// Last-modified instant, ISO 8601 (the backend's stamp; `"unknown"`
    /// when the backend exposes none — the memory store).
    pub modified_at: String,
    /// The MIME the bytes guess as (a hint, never authoritative).
    pub hinted_mime: String,
}

/// One adopt request's validated inputs.
pub(crate) struct AdoptTarget {
    pub bucket_id: uuid::Uuid,
    pub storage_backend: String,
    pub owner_id: uuid::Uuid,
    pub parent_id: Option<uuid::Uuid>,
    pub name: String,
    pub key: String,
}

/// The sha256 namespace gate (draft §7.4): adopt accepts ONLY a 64-char
/// lowercase hex sha256 key. Anything else is refused BEFORE any store
/// read — an uppercase, a 63-char or a non-hex key can never become a
/// row whose sha256 disagrees with its blob bytes.
/// MUT:P3-SHA-NAMESPACE — accepting any safe key here reddens
/// `adopt_refuses_keys_outside_the_sha256_namespace`.
pub(crate) fn is_sha256_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The diff (draft §7.2): listed `media/` keys − the bucket's live rows'
/// `storage_key`. Re-listed on EVERY call (adopt re-checks through this
/// same function, so the diff is never cached — TOCTOU-safe). The
/// `cache/` prefix never enters because `list_media_keys` enumerates the
/// media namespace only.
pub(crate) async fn unregistered_diff(
    state: &AppState,
    bucket: &i0721_proto::StorageBucketRecord,
    store: &Arc<dyn crate::blob::BlobStore>,
) -> Result<Vec<String>, ApiError> {
    let listed = store
        .list_media_keys()
        .await
        .map_err(|err| {
            ApiError::internal(format!("the bucket cannot be scanned: {err}"))
        })?
        // `None` = the backend cannot be scanned: a DISTINCT documented
        // refusal, never a silent empty list (which would leak the
        // opposite lie — "nothing unregistered here").
        .ok_or_else(|| {
            ApiError::bad_request(
                "this backend cannot be scanned (unregistered objects are not enumerable)",
            )
        })?;
    // The live rows of THIS bucket only — a key adopted in another
    // bucket stays unregistered here (the namespaces are per bucket).
    let live: std::collections::HashSet<String> =
        state.store.storage_keys_live_in_bucket(Some(bucket.id)).await?;
    Ok(listed.into_iter().filter(|key| !live.contains(key)).collect())
}

/// Load one blob's metadata for the listing (size + modification + MIME
/// hint). A blob that vanished between listing and read is skipped, not
/// an error — the next listing will not show it either.
async fn describe(
    store: &Arc<dyn crate::blob::BlobStore>,
    key: &str,
) -> Option<UnregisteredObject> {
    let bytes = store.get(BlobSlot::Media, key).await.ok()??;
    let hinted_mime = crate::pipeline::mime_hint_for(&bytes, key);
    Some(UnregisteredObject {
        key: key.to_owned(),
        size: bytes.len() as i64,
        modified_at: "unknown".to_owned(),
        hinted_mime,
    })
}

/// The paged listing for one bucket.
pub(crate) async fn list_unregistered(
    state: &AppState,
    bucket: &i0721_proto::StorageBucketRecord,
    store: &Arc<dyn crate::blob::BlobStore>,
    offset: u64,
    limit: u64,
) -> Result<(Vec<UnregisteredObject>, u64), ApiError> {
    let diff = unregistered_diff(state, bucket, store).await?;
    let total = diff.len() as u64;
    let page_futures: Vec<_> = diff
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|key| async move { describe(store, &key).await })
        .collect();
    let page: Vec<UnregisteredObject> = futures::future::join_all(page_futures)
        .await
        .into_iter()
        .flatten()
        .collect();
    Ok((page, total))
}

/// The adopt chain (draft §7.4, the four steps verbatim):
/// 1. the key is RE-LISTED and compared against the diff NOW — no
///    cache; a key that vanished since the admin saw it answers
///    `not_found` and NO row is inserted (TOCTOU-safe);
/// 2. `sha256 = key`, NO transcoding — the upload pipeline is never
///    called (a WebP auto-convert would change the sha256 and silently
///    drop the original bytes); only `classify_kind` + a MIME guess;
/// 3. INSERT the row: the caller's `owner_id`, `parent_id` as given,
///    name defaults to the key, `original_name` empty, the bucket's
///    id/backend, `thumbnail_key` NULL (lazy generation fills it);
/// 4. the QUOTA check mirrored from `run_upload` — the target owner
///    over quota answers `too_large` (`-32008` on the RPC lane).
/// MUT:P3-ADOPT-RELIST — dropping step 1 (trusting the caller's key)
/// reddens `unregistered_adopt_refuses_vanished_key`.
/// MUT:P3-ADOPT-PIPELINE — routing step 2 through `process_upload`
/// reddens `adopt_uses_the_key_as_sha_and_does_not_transcode`.
/// MUT:P3-ADOPT-QUOTA — dropping step 4 reddens `adopt_checks_quota`.
pub(crate) async fn adopt(
    state: &AppState,
    bucket: &i0721_proto::StorageBucketRecord,
    bucket_store: &Arc<dyn crate::blob::BlobStore>,
    _caller: &UserRecord,
    target: AdoptTarget,
) -> Result<i0721_proto::FileRecord, ApiError> {
    let AdoptTarget {
        bucket_id,
        storage_backend,
        owner_id,
        parent_id,
        name,
        key,
    } = target;

    // 1. Re-list NOW (no cache) — the diff is bucket-scoped.
    let diff = unregistered_diff(state, bucket, bucket_store).await?;
    if !diff.iter().any(|k| k == &key) {
        return Err(ApiError::not_found(
            "the key is not an unregistered object of this bucket (it may have vanished)",
        ));
    }

    // 2. The bytes are read back; sha256 = the key, no transcoding.
    let bytes = bucket_store
        .get(BlobSlot::Media, &key)
        .await?
        .ok_or_else(|| {
            ApiError::not_found("the key is not an unregistered object of this bucket")
        })?;
    let mime = crate::pipeline::mime_hint_for(&bytes, &key);
    let kind = classify_kind(&mime, &key);

    // 4. The quota check (mirrored from run_upload: user override ?? the
    //    site default; None = unlimited). BEFORE the row insert so a
    //    refusal leaves nothing behind.
    let config = site_config(state).await?;
    let target_user = state
        .store
        .get_user(owner_id)
        .await?
        .ok_or_else(|| ApiError::not_found("no such owner"))?;
    let quota = match target_user.quota_bytes {
        Some(q) => Some(q),
        None => config.upload.default_quota_bytes,
    };
    if let Some(quota) = quota {
        let used = state.store.sum_live_sizes(owner_id).await?;
        if used + bytes.len() as i64 > quota {
            return Err(ApiError::too_large(format!(
                "quota exceeded: {used} bytes used, the limit is {quota}"
            )));
        }
    }

    // 3. The row.
    let record = state
        .store
        .create_file(NewFile {
            owner_id,
            parent_id,
            name,
            kind,
            mime: mime.clone(),
            size: bytes.len() as i64,
            sha256: key.clone(),
            bucket_id: Some(bucket_id),
            storage_backend,
            storage_key: key.clone(),
            thumbnail_key: None,
            original_name: String::new(),
        })
        .await?;
    Ok(record)
}
