//! `GET /media/{key}` — the public path legacy image links depend on.
//!
//! Behavior carried over from the legacy engine: the referrer allow-list
//! (empty list = no check), anonymous access, `?thumbnail=true` serving
//! the cached WebP thumbnail — regenerating it on demand when the cache
//! entry is missing.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;

use crate::api::settings::site_config;
use crate::blob::BlobSlot;
use crate::bucket_acl::{resolve_access, BucketAccess};
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

/// The credential the media lane accepted (`None` = anonymous).
#[derive(Debug, Clone)]
pub(crate) enum MediaActor {
    /// No session cookie, no Bearer key: the anonymous lane (I3 — its
    /// only possible answers are 200 and 404).
    Anonymous,
    /// A proven session (JWT cookie / Bearer JWT).
    Session(Arc<i0721_proto::UserRecord>),
    /// A live API key (I6: its authority = `ops` ∩ `buckets` ∩ the
    /// owner's current grants — the owner is the acting user).
    ApiKey {
        user: Arc<i0721_proto::UserRecord>,
        buckets: std::collections::BTreeSet<String>,
    },
}

/// Resolve the media lane's credential: a Bearer API key (`i0721-…`) is
/// a live MCP key; any other Bearer or the session cookie is a JWT.
async fn media_actor(state: &AppState, headers: &HeaderMap) -> Result<MediaActor, ApiError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        });
    if let Some(raw) = bearer {
        if crate::mcp::KEY_PREFIX.is_empty() || raw.starts_with(crate::mcp::KEY_PREFIX) {
            let record = crate::mcp::verify_api_key(state, raw).await?;
            let user = state
                .store
                .get_user(record.user_id)
                .await?
                .ok_or_else(|| ApiError::unauthorized("user not found"))?;
            return Ok(MediaActor::ApiKey {
                user: Arc::new(user),
                buckets: crate::key_scopes::parse_scopes_buckets(&record.scopes),
            });
        }
        let user = session_user_from_token(state, raw).await?;
        return Ok(MediaActor::Session(user));
    }
    if let Some(token) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|pair| {
                pair.trim()
                    .strip_prefix("i0721-token=")
                    .filter(|t| !t.is_empty())
            })
        })
    {
        let user = session_user_from_token(state, token).await?;
        return Ok(MediaActor::Session(user));
    }
    Ok(MediaActor::Anonymous)
}

/// Verify a JWT and load its user — the RPC `verify_identity` semantics
/// (signature + expiry + watermark) for the REST media lane.
async fn session_user_from_token(
    state: &AppState,
    token: &str,
) -> Result<Arc<i0721_proto::UserRecord>, ApiError> {
    let claims = state
        .signer
        .verify(token)
        .map_err(|err| ApiError::unauthorized(format!("invalid token: {err}")))?;
    let user_id: uuid::Uuid = claims
        .sub
        .parse()
        .map_err(|_| ApiError::unauthorized("invalid token subject"))?;
    let user = state
        .store
        .get_user(user_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("user not found"))?;
    let watermark = state
        .store
        .get_setting(&crate::auth::watermark_key(user.id))
        .await?
        .and_then(|value| value.as_str().map(str::to_owned));
    if !crate::auth::clears_watermark(watermark.as_deref(), claims.iat) {
        return Err(ApiError::unauthorized("token expired"));
    }
    Ok(Arc::new(user))
}

/// The bucket record of one live row — the store's rows with the
/// registry's id→slug map as the fixture-lane fallback (the same
/// posture `bucket_row_for_id` uses for the per-id lookups).
async fn bucket_of_row(
    state: &AppState,
    row: &i0721_proto::FileRecord,
) -> Result<Option<i0721_proto::StorageBucketRecord>, ApiError> {
    let Some(bucket_id) = row.bucket_id else {
        // The legacy NULL-bucket shape resolves to the enabled bootstrap
        // bucket — its row when the store seeds one, otherwise the
        // registry's synthetic posture.
        let id = state
            .registry
            .bootstrap_bucket_id()
            .map_err(|err| ApiError::internal(format!("storage backend unavailable: {err}")))?;
        return crate::bucket_acl::bucket_row_for_id(state.store.as_ref(), &state.registry, id)
            .await
            .map_err(ApiError::from);
    };
    crate::bucket_acl::bucket_row_for_id(state.store.as_ref(), &state.registry, bucket_id)
        .await
        .map_err(ApiError::from)
}

/// Whether the actor READS through this row's bucket: the owner's grant
/// (the R-A resolution) — for an API key additionally intersected with
/// the key's `buckets` (I6's call-time intersection), admin = all.
async fn actor_reads_row(
    state: &AppState,
    actor: &MediaActor,
    row: &i0721_proto::FileRecord,
) -> Result<bool, ApiError> {
    let Some(bucket) = bucket_of_row(state, row).await? else {
        return Ok(false);
    };
    let user = match actor {
        MediaActor::Anonymous => return Ok(false),
        MediaActor::Session(user) => user,
        MediaActor::ApiKey { user, buckets } => {
            if !buckets.contains(&bucket.slug) {
                return Ok(false);
            }
            user
        }
    };
    if user.role == i0721_proto::Role::Admin {
        return Ok(true);
    }
    Ok(resolve_access(state.store.as_ref(), user, &bucket)
        .await?
        .read)
}

/// Whether the row's bucket is VISIBLE to the actor but holds no `Read`
/// bit — the 403 face's condition (session/key only; anonymous never
/// 403s, I3). Visibility = the R-A `any()` bit set (Q14).
async fn row_is_visible_without_read(
    state: &AppState,
    actor: &MediaActor,
    row: &i0721_proto::FileRecord,
) -> Result<bool, ApiError> {
    let Some(bucket) = bucket_of_row(state, row).await? else {
        return Ok(false);
    };
    let user = match actor {
        MediaActor::Anonymous => return Ok(false),
        MediaActor::Session(user) => user,
        MediaActor::ApiKey { user, buckets } => {
            if !buckets.contains(&bucket.slug) {
                return Ok(false);
            }
            user
        }
    };
    if user.role == i0721_proto::Role::Admin {
        return Ok(false);
    }
    let access: BucketAccess = resolve_access(state.store.as_ref(), user, &bucket).await?;
    Ok(access.any() && !access.read)
}

/// The R-J serving row: the first row by `id ASC` (the store's
/// traversal order) whose bucket is `Ready ∧ enabled` — bytes for the
/// original AND the thumbnail (including the lazy write) come from THAT
/// row's bucket. Availability = the registry's Ready state AND the
/// store's `enabled` flag (the registry snapshot can trail a runtime
/// disable; the row in the store is the authority for the flag).
async fn serving_row<'a>(
    state: &AppState,
    rows: &[&'a i0721_proto::FileRecord],
) -> Option<&'a i0721_proto::FileRecord> {
    for row in rows {
        if state.registry.for_row(row).is_err() || !row_bucket_ready(state, row) {
            continue;
        }
        if let Some(bucket) = bucket_of_row(state, row).await.ok().flatten() {
            if !bucket.enabled {
                continue;
            }
        }
        return Some(row);
    }
    None
}

/// Whether the row's bucket (the NULL shape reads through the bootstrap
/// bucket) is `Ready` in the registry.
fn row_bucket_ready(state: &AppState, row: &i0721_proto::FileRecord) -> bool {
    let bucket_id = row
        .bucket_id
        .or_else(|| state.registry.bootstrap_bucket_id().ok());
    bucket_id
        .and_then(|id| state.registry.slug_for_bucket_id(id))
        .is_some_and(|slug| {
            state
                .registry
                .health()
                .into_iter()
                .any(|h| h.slug == slug && h.state == "ready")
        })
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

    let actor = media_actor(&state, &headers).await?;

    // R-J's full form: the determination is a SET predicate over ALL
    // live rows with this key (never a short-circuit to the first row).
    // A = "some live row sits in a bucket with anonymous_read = TRUE"
    // (enabled IGNORED — a disabled public bucket still makes the key
    // anonymous-visible; availability is the serving row's business).
    let rows = state.store.list_live_rows_by_storage_key(&key).await?;
    let mut a_witnesses: Vec<&i0721_proto::FileRecord> = Vec::new();
    for row in &rows {
        if let Some(bucket) = bucket_of_row(&state, row).await? {
            if bucket.anonymous_read {
                a_witnesses.push(row);
            }
        }
    }
    let a = !a_witnesses.is_empty();

    // The serving row under the actor's authorized set.
    enum Face {
        Serve(uuid::Uuid),
        NotFound,
        Forbidden,
        Unavailable,
    }
    let authorized: Vec<&i0721_proto::FileRecord> = match &actor {
        // Anonymous: the authorized set IS A's witness rows (the key is
        // anonymous-visible); I3 — its only answers are 200 or 404.
        MediaActor::Anonymous => a_witnesses.clone(),
        // Session/API key: A's witnesses ∪ rows whose bucket grants the
        // actor Read (API key ∩ the key's `buckets`; admin = all).
        _ => {
            let mut out = a_witnesses.clone();
            for row in &rows {
                if !out.iter().any(|w| w.id == row.id)
                    && actor_reads_row(&state, &actor, row).await?
                {
                    out.push(row);
                }
            }
            out
        }
    };
    // R-J: the serving row is the first by `id ASC` WITHIN
    // authorized_rows — the set's construction order (witnesses first)
    // must not decide; the id does.
    let mut authorized = authorized;
    authorized.sort_by_key(|row| row.id);
    authorized.dedup();
    let face = if let Some(row) = serving_row(&state, &authorized).await {
        Face::Serve(row.id)
    } else if matches!(actor, MediaActor::Anonymous) {
        // A held (the set is non-empty only when it did) but no usable
        // serving row: 404 to anonymous, never a 503 (I3).
        Face::NotFound
    } else if !authorized.is_empty() {
        Face::Unavailable
    } else {
        // `authorized_rows` empty: a live row in a VISIBLE bucket
        // WITHOUT Read → 403 (session/key only); otherwise 404.
        let mut visible_denied = false;
        for row in &rows {
            if row_is_visible_without_read(&state, &actor, row).await? {
                visible_denied = true;
                break;
            }
        }
        if visible_denied {
            Face::Forbidden
        } else {
            Face::NotFound
        }
    };
    let Face::Serve(serving_id) = face else {
        return Ok(match face {
            Face::NotFound => not_found_response(),
            Face::Forbidden => forbidden_response(),
            Face::Unavailable => unavailable_response(),
            Face::Serve(_) => unreachable!(),
        });
    };
    let serving = rows
        .iter()
        .find(|row| row.id == serving_id)
        .ok_or_else(|| ApiError::not_found("media not found"))?;

    // Every byte — original and thumbnail, cached or lazily regenerated
    // — comes from the SERVING ROW's own bucket (I4).
    let store = state
        .registry
        .for_row(serving)
        .map_err(|err| ApiError::not_found(format!("media not available: {err}")))?;

    let (mime, bytes) = if args.thumbnail.unwrap_or(false) {
        if let Some(thumb_key) = serving.thumbnail_key.as_deref() {
            if let Some(cached) = store.get(BlobSlot::Cache, thumb_key).await? {
                ("image/webp".to_owned(), cached)
            } else {
                regenerate_thumbnail(&store, &serving.storage_key, thumb_key).await?
            }
        } else {
            // No thumbnail recorded (non-image): serve the original.
            let bytes = read_blob(&store, &serving.storage_key).await?;
            (serving.mime.clone(), bytes)
        }
    } else {
        let bytes = read_blob(&store, &serving.storage_key).await?;
        (serving.mime.clone(), bytes)
    };

    // R-S (I5): the cache directives live ONLY in this known-2xx branch.
    // 2xx ∧ A → the immutable public directive; 2xx ∧ ¬A → private.
    let mut response = ([(header::CONTENT_TYPE, mime)], bytes).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(if a {
            "public, max-age=31536000, immutable"
        } else {
            "private, no-store"
        }),
    );
    Ok(response)
}

/// A 404 media answer with its no-store directive (R-S: EVERY non-2xx
/// carries `no-store`).
fn not_found_response() -> Response {
    error_response(StatusCode::NOT_FOUND, "media not found")
}

/// A 403 media answer (the visible-but-unreadable face; session/key
/// only).
fn forbidden_response() -> Response {
    error_response(
        StatusCode::FORBIDDEN,
        "you do not have read access to this media",
    )
}

/// A 503 media answer (the unusable-bucket face; session/key only).
fn unavailable_response() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "storage backend unavailable",
    )
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let mut response = ApiError::new(status, message).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

async fn read_blob(
    store: &std::sync::Arc<dyn crate::blob::BlobStore>,
    key: &str,
) -> Result<Bytes, ApiError> {
    store
        .get(BlobSlot::Media, key)
        .await?
        .ok_or_else(|| ApiError::not_found("media blob missing"))
}

/// The legacy lazy path: no cache entry → regenerate from the original
/// blob, store it, serve it — all inside the serving row's bucket.
async fn regenerate_thumbnail(
    store: &std::sync::Arc<dyn crate::blob::BlobStore>,
    storage_key: &str,
    thumb_key: &str,
) -> Result<(String, Bytes), ApiError> {
    let original = read_blob(store, storage_key).await?;
    let thumbnail = thumbnail_for_media(&original)
        .map_err(|err| ApiError::internal(format!("thumbnail regeneration failed: {err}")))?;
    store.put(BlobSlot::Cache, thumb_key, &thumbnail).await?;
    Ok(("image/webp".to_owned(), thumbnail))
}
