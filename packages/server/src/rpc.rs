//! The `/api/rpc` lane — strict WebSocket JSON-RPC 2.0 (GET WS upgrade)
//! with an HTTP POST fallback on the same method map, both provided by
//! the family's `plana-rpc-server` framework.
//!
//! This lane is the webui's primary API: the REST surface is retired
//! down to the bootstrap/binary whitelist (`/api/health`, the password
//! login, the multipart upload and `/media/{key}` — see `routes.rs`).
//! Every method body is the semantics of the former REST handler, moved
//! verbatim: the admin gates, the owner-or-admin file gate, the strict
//! full-document settings validation.
//!
//! ## Auth: the four-channel hook (the chest order)
//!
//! Connection auth runs at every WS upgrade and every POST fallback
//! request. The token is extracted in the family's fixed order —
//! cookie → `Authorization: Bearer` → `Sec-WebSocket-Protocol: jwt.<t>`
//! → `?token=` (the last resort; the subprotocol channel exists so a
//! browser WebSocket can authenticate without a query parameter) — and
//! then proven exactly like the REST lane: HS256 verify, user load,
//! watermark check (a token minted before the user's last login is
//! expired; 5 s leeway). Denial aborts the handshake with HTTP 401
//! carrying the JSON-RPC error body.

use std::sync::Arc;

use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::Router;
use plana::jsonrpc::JsonRpcError;
use plana_rpc_server::{RpcRequestCtx, RpcServer};
use serde_json::Value;
use uuid::Uuid;

use i0721_proto::{
    validate_display_name, validate_file_name, validate_username, FileDto, NewShare, NewUser,
    PageArgs, PageResult, QuotaInfo, Role, ShareDto, SiteConfigStrict, UserDto, CONFIG_SETTING_KEY,
};
use i0721_store::CreateUser;

/// The create-user wire form (the former REST body; the ts-rs binding
/// keeps feeding the webui).
#[derive(Debug, Clone, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct CreateUserRequest {
    /// Login name.
    pub username: String,
    /// Plaintext password (hashed before storage).
    pub password: String,
    /// Human-facing name; defaults to the username.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Role; defaults to member.
    #[serde(default)]
    pub role: Option<Role>,
}
use crate::api::auth::{password_login_enabled, PASSWORD_LOGIN_SETTINGS_KEY};
use crate::api::settings::site_config;
use crate::auth::{clears_watermark, hash_password, watermark_key};
use crate::blob::BlobSlot;
use crate::error::ApiError;
use crate::oauth::{
    load_providers, save_providers, valid_provider_id, valid_provider_type, valid_provider_url,
    OAuthProvider, OAuthProviderUpsert,
};
use crate::state::AppState;

/// Extension error codes of this service.
///
/// The JSON-RPC core codes (`-32700`, `-32600`, `-32601`, `-32602`,
/// `-32603`) and the `-32001..-32005` block belong to the **plana family**
/// (`plana::jsonrpc::error_codes`): every service in the fleet speaks them,
/// so a service must not invent its own number inside that block — doing so
/// puts two unrelated failures on one code. `AUTH` is the single deliberate
/// share (both families mean "auth or permission failure"); everything else
/// here sits in the service-assignment gap above the block and maps the REST
/// statuses the former handlers emitted. The
/// `a_service_code_never_quietly_reuses_a_plana_code` test holds that line
/// mechanically.
pub mod rpc_error_codes {
    /// Auth or permission failure (401/403; the plana fleet convention).
    pub const AUTH: i64 = -32005;
    /// Entity not found (404).
    pub const NOT_FOUND: i64 = -32006;
    /// Conflict (409 — username taken, content already uploaded).
    pub const CONFLICT: i64 = -32007;
    /// Payload too large (413).
    pub const TOO_LARGE: i64 = -32008;
    /// The deferred-ops collection surface is disabled on this instance
    /// (the `ops.*` guard; see `build_server`).
    pub const OPS_DISABLED: i64 = -32009;
    /// Storage backend unavailable (B wave's Q9 addition; 503 on REST).
    pub const STORAGE_UNAVAILABLE: i64 = -32010;
}

/// The per-connection identity the auth hook establishes: the verified,
/// watermark-cleared user record.
pub struct RpcIdentity {
    /// The authenticated user (admin/member role rides inside).
    pub user: i0721_proto::UserRecord,
}

/// Map a former-REST [`ApiError`] onto the JSON-RPC error family.
fn api_err(err: ApiError) -> JsonRpcError {
    let code = match err.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => rpc_error_codes::AUTH,
        StatusCode::NOT_FOUND => rpc_error_codes::NOT_FOUND,
        StatusCode::CONFLICT => rpc_error_codes::CONFLICT,
        StatusCode::PAYLOAD_TOO_LARGE => rpc_error_codes::TOO_LARGE,
        StatusCode::SERVICE_UNAVAILABLE => rpc_error_codes::STORAGE_UNAVAILABLE,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            plana::jsonrpc::error_codes::INVALID_PARAMS
        }
        // The internal family: the storage-unavailable lane answers
        // -32010 (§7) when the failure IS the backend; every other
        // internal failure keeps the generic code.
        _ => plana::jsonrpc::error_codes::INTERNAL_ERROR,
    };
    JsonRpcError::new(code, err.error)
}

/// Parse a method's params into a typed value (`-32602` on a bad shape).
fn params<T: serde::de::DeserializeOwned>(ctx: &RpcRequestCtx) -> Result<T, JsonRpcError> {
    serde_json::from_value(ctx.params.clone())
        .map_err(|err| JsonRpcError::invalid_params(&err.to_string()))
}

// NOTE (declared deviation): this extractor and `api::auth::extract_token`
// are an intentional dual-write pending a shared auth module; both must
// keep the same four-channel order (cookie, Bearer, subprotocol, query).
// The RPC copy additionally labels WHICH channel carried the credential
// (the one-time WS ticket is honored on the query channel only — see
// `verify_identity`).

/// The session cookie name. The browser session rides this httpOnly
/// cookie (set at login); the four extraction channels in
/// `extract_token` below exist for non-browser clients (cookie, Bearer
/// header, subprotocol, query parameter).
const TOKEN_COOKIE: &str = "i0721-token";

/// Which channel a credential arrived on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CredentialChannel {
    /// The httpOnly session cookie (the browser path).
    Cookie,
    /// `Authorization: Bearer`.
    Bearer,
    /// `Sec-WebSocket-Protocol: jwt.<t>`.
    Subprotocol,
    /// `?token=` — the one-time WS admission ticket's slot.
    Query,
}

/// Extract the credential and its channel, the chest four-channel
/// order. Pure header/URL reading — proving happens in
/// `verify_identity`.
fn extract_token(headers: &HeaderMap, uri: &Uri) -> Option<(String, CredentialChannel)> {
    // Channel 1: the session cookie (each channel falls THROUGH to the
    // next when it carries nothing — no early function exit).
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    {
        for pair in cookie.split(';') {
            let pair = pair.trim();
            if let Some(value) = pair.strip_prefix(&format!("{TOKEN_COOKIE}=")) {
                if !value.is_empty() {
                    return Some((value.to_owned(), CredentialChannel::Cookie));
                }
            }
        }
    }
    if let Some(bearer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
    {
        return Some((bearer.to_owned(), CredentialChannel::Bearer));
    }
    // Sec-WebSocket-Protocol: jwt.<token> — keeps the credential out of
    // the URL (browser WebSocket cannot set headers).
    if let Some(jwt) = headers
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("jwt."))
    {
        return Some((jwt.to_owned(), CredentialChannel::Subprotocol));
    }
    // Last resort: the query parameter. On the WS upgrade this slot
    // carries the ONE-TIME admission ticket (see `ws_tickets`); a plain
    // JWT here stays valid for non-browser clients.
    uri.query()
        .and_then(|query| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("token=").filter(|t| !t.is_empty()))
        })
        .map(|value| (value.to_owned(), CredentialChannel::Query))
}

/// Verify the credential and load the user (the REST lane's
/// `require_auth` semantics, watermark check included).
async fn verify_identity(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<RpcIdentity, ApiError> {
    let (credential, channel) = extract_token(headers, uri)
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;

    // The one-time WS admission ticket rides the QUERY channel only: a
    // consumed ticket elsewhere (a Bearer header echoed by a client's
    // HTTP fallback) is not an admission. Valid + unexpired → consume
    // (single use) and establish the session as the bound user — the
    // mint already ran the full JWT + watermark proof ≤30 s ago, so no
    // re-verification here.
    if channel == CredentialChannel::Query {
        if let Some(user_id) = state.ws_tickets.consume(&credential) {
            let user = state
                .store
                .get_user(user_id)
                .await?
                .ok_or_else(|| ApiError::unauthorized("user not found"))?;
            return Ok(RpcIdentity { user });
        }
        // Not a live ticket: fall through — the query slot also accepts
        // plain JWTs for non-browser clients.
    }

    let token = credential;
    let claims = state
        .signer
        .verify(&token)
        .map_err(|err| ApiError::unauthorized(format!("invalid token: {err}")))?;
    let user_id: Uuid = claims
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
        .get_setting(&watermark_key(user.id))
        .await?
        .and_then(|value| value.as_str().map(str::to_owned));
    if !clears_watermark(watermark.as_deref(), claims.iat) {
        return Err(ApiError::unauthorized("token expired"));
    }
    Ok(RpcIdentity { user })
}

/// Prove the connection (the builder's auth hook).
async fn auth_hook(
    state: AppState,
    headers: HeaderMap,
    uri: Uri,
) -> Result<plana_rpc_server::AuthContext, JsonRpcError> {
    let identity = verify_identity(&state, &headers, &uri)
        .await
        .map_err(api_err)?;
    Ok(Arc::new(identity) as plana_rpc_server::AuthContext)
}

/// The authenticated user of a request (every method needs it).
fn user_of(ctx: &RpcRequestCtx) -> i0721_proto::UserRecord {
    ctx.auth::<RpcIdentity>()
        .expect("the auth hook always establishes an RpcIdentity")
        .user
        .clone()
}

/// `admin` gate for RPC methods over admin-only surfaces.
async fn require_admin(user: &i0721_proto::UserRecord) -> Result<(), JsonRpcError> {
    if user.role != Role::Admin {
        return Err(api_err(ApiError::forbidden("admin role required")));
    }
    Ok(())
}

/// Map a store error through the API error family.
fn store_err(err: i0721_store::StoreError) -> JsonRpcError {
    api_err(ApiError::from(err))
}

/// The §7 `files.*` bucket column, evaluated ONCE (the chain lane's
/// gate; other `files.*` methods keep their existing answers until the
/// spec's P2a list pushes the same filter into them):
/// * no bucket (NULL rows resolve through the bootstrap) or an unknown
///   bucket → `-32006`;
/// * visible (any grant bit or signup default) but without `Read`
///   → `-32005`;
/// * visible + readable but the bucket cannot serve → `-32010`.
async fn files_chain_bucket_gate(
    state: &AppState,
    user: &i0721_proto::UserRecord,
    bucket_id: Option<Uuid>,
) -> Result<(), JsonRpcError> {
    use crate::bucket_acl::resolve_access;
    // A NULL-bucket row reads through the bootstrap bucket (I4's
    // resolution, R-H): unresolvable → storage unavailable.
    let Some(bucket_id) = bucket_id else {
        return state.registry.bootstrap_store().map(|_| ()).map_err(|err| {
            JsonRpcError::new(
                rpc_error_codes::STORAGE_UNAVAILABLE,
                format!("storage backend unavailable: {err}"),
            )
        });
    };
    let Some(bucket) =
        crate::bucket_acl::bucket_row_for_id(state.store.as_ref(), &state.registry, bucket_id)
            .await
            .map_err(store_err)?
    else {
        return Err(api_err(ApiError::not_found("folder not found")));
    };
    let access = resolve_access(state.store.as_ref(), user, &bucket)
        .await
        .map_err(store_err)?;
    if !access.any() {
        return Err(api_err(ApiError::not_found("folder not found")));
    }
    if !access.read {
        return Err(api_err(ApiError::forbidden(
            "you do not have read access to this bucket",
        )));
    }
    state
        .registry
        .resolve(&bucket.slug)
        .map(|_| ())
        .map_err(|err| {
            JsonRpcError::new(
                rpc_error_codes::STORAGE_UNAVAILABLE,
                format!("storage backend unavailable: {err}"),
            )
        })
}

/// R-B's key-reference guard: `BUCKET_KEY_REFERENCE_SQL` — spec-verbatim —
/// whether ANY live (non-revoked) key's `scopes.buckets` array names
/// `slug`. `false` = the bucket's slug is free. The SQL runs on the PG
/// lane (the lane's own tests re-run the same constant against
/// hand-planted rows); the memory lane evaluates the identical predicate
/// over the parsed scopes, so both stores answer the same question.
async fn key_references_slug(state: &AppState, slug: &str) -> Result<bool, JsonRpcError> {
    state
        .store
        .a_live_key_references_bucket_slug(slug)
        .await
        .map_err(store_err)
}

/// Map a blob-store error (plain I/O) the same way.
fn blob_err(err: std::io::Error) -> JsonRpcError {
    api_err(ApiError::from(err))
}

/// A placeholder row shape used ONLY to address a bucket's store through
/// [`BlobRegistry::for_row`] during reclaim: the bucket id is what the
/// resolution consumes, every other field is inert. Folders never reach
/// the byte lanes, so the placeholder never resolves to a real store
/// decision beyond the bucket id (see `folder_rows_never_resolve_to_a_store`).
fn file_row_shape() -> i0721_proto::FileRecord {
    i0721_proto::FileRecord {
        id: uuid::Uuid::nil(),
        owner_id: uuid::Uuid::nil(),
        parent_id: None,
        name: String::new(),
        kind: i0721_proto::FileKind::Other,
        mime: String::new(),
        size: 0,
        sha256: String::new(),
        bucket_id: None,
        storage_backend: String::new(),
        storage_key: String::new(),
        thumbnail_key: None,
        original_name: String::new(),
        created_at: jiff::Timestamp::MIN,
        deleted_at: None,
    }
}

/// Serialize a handler result value.
fn to_value<T: serde::Serialize>(value: T) -> Result<Value, JsonRpcError> {
    serde_json::to_value(value).map_err(|err| JsonRpcError::internal_error(&err.to_string()))
}

/// The owner-or-admin file gate (the REST lane's rule, verbatim):
/// members may only mutate their own files, admins any file.
async fn require_file_owner_or_admin(
    state: &AppState,
    user: &i0721_proto::UserRecord,
    id: Uuid,
) -> Result<i0721_proto::FileRecord, JsonRpcError> {
    let record = state
        .store
        .get_file(id)
        .await
        .map_err(store_err)?
        .ok_or_else(|| ApiError::not_found("file not found"))
        .map_err(api_err)?;
    if user.role != Role::Admin && record.owner_id != user.id {
        return Err(api_err(ApiError::forbidden("not your file")));
    }
    Ok(record)
}

/// The wire form of `keys.create` / `keys.update` — the fields a key's
/// owner writes. `keys.update` is a WHOLE-RECORD replacement, so it
/// carries the same fields; an omitted `expires_at` clears the expiry.
///
/// `buckets` (Q7): the explicit slug array (`[]` = no buckets). An
/// OMITTED field (or a non-array) materializes `granted_buckets(actor)`
/// — the owner's CURRENT authorized set, snapshotted at write time
/// (I7: new buckets and new grants never extend an existing key).
#[derive(Debug, Clone, serde::Deserialize)]
struct KeyWriteParams {
    name: String,
    ops: Vec<i0721_proto::KeyOp>,
    #[serde(default)]
    expires_at: Option<String>,
    /// `None` = "not given" → materialize the owner's granted buckets.
    /// `Some(_)` = the caller picked (an empty list INCLUDED).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    buckets: Option<Vec<String>>,
}

/// Validate a key write, answering `(trimmed name, ops, canonical
/// expires_at)`.
///
/// Invariants (shared by create and update so the two can never drift):
/// * the name survives `trim()` non-empty;
/// * at least ONE permission — a key with no permission can only fail,
///   and granting none is what `keys.revoke` is for;
/// * a supplied `expires_at` is RFC 3339 and STRICTLY in the future, so
///   "create a key that is already dead" cannot be expressed;
/// * the stored form is the canonical instant (`…Z`), not the caller's
///   spelling of it.
///
/// Every failure is the repo's existing invalid-params shape
/// (`ApiError::bad_request` → `-32602`), the same one `settings.put` and
/// `files.move` answer.
fn validate_key_write(
    write: &KeyWriteParams,
) -> Result<(String, Vec<i0721_proto::KeyOp>, Option<String>), JsonRpcError> {
    let name = write.name.trim().to_owned();
    if name.is_empty() {
        return Err(api_err(ApiError::bad_request("name must not be empty")));
    }
    if write.ops.is_empty() {
        return Err(api_err(ApiError::bad_request(
            "at least one permission is required",
        )));
    }
    let now = jiff::Timestamp::now();
    let expires_at = match write.expires_at.as_deref().map(str::trim) {
        // An explicitly empty string is the webui's "no expiry" input.
        None | Some("") => None,
        Some(raw) => {
            let instant = raw.parse::<jiff::Timestamp>().map_err(|err| {
                api_err(ApiError::bad_request(format!(
                    "expires_at must be an RFC 3339 instant: {err}"
                )))
            })?;
            if instant <= now {
                return Err(api_err(ApiError::bad_request(
                    "expires_at must be in the future",
                )));
            }
            Some(instant.to_string())
        }
    };
    Ok((name, write.ops.clone(), expires_at))
}

/// The ONE ownership gate for the `keys.*` lanes that act on an existing
/// key (`keys.revoke`, `keys.update`).
///
/// Ownership is decided from the KEY RECORD's own `user_id`, never from
/// membership in `list_api_keys(caller.id)`: that list is the caller's
/// OWN keys, so it can only ever answer "mine" — it never loads the
/// target row for an admin acting on a foreign key, and it cannot tell
/// "someone else's key" apart from "no such key". Loading the row is what
/// makes both the owner answer and the admin answer well-defined.
///
/// The answers are the ones the lanes have always given:
/// * no such row → not found (`-32006`);
/// * a stranger (record owner ≠ caller, caller not admin) → forbidden
///   `"not your key"` (`-32005`) — the same text for every key that is
///   not yours, so the surface does not confirm which ids exist;
/// * a revoked target → not found `"key not found or already revoked"`,
///   the answer both lanes give when the guarded UPDATE matches no row;
/// * otherwise the record, for the caller to act on.
///
/// # Errors
///
/// [`JsonRpcError`] from [`api_err`] / [`store_err`], ready to return.
async fn require_key_access(
    state: &AppState,
    caller: &i0721_proto::UserRecord,
    id: Uuid,
) -> Result<i0721_proto::ApiKeyRecord, JsonRpcError> {
    let record = state
        .store
        .find_api_key_by_id(id)
        .await
        .map_err(store_err)?
        .ok_or_else(|| api_err(ApiError::not_found("key not found or already revoked")))?;
    if record.user_id != caller.id && caller.role != Role::Admin {
        return Err(api_err(ApiError::forbidden("not your key")));
    }
    if record.revoked_at.is_some() {
        return Err(api_err(ApiError::not_found(
            "key not found or already revoked",
        )));
    }
    Ok(record)
}

/// Assemble the RPC router: WS upgrade + POST fallback at `/api/rpc`.
pub fn router(state: AppState) -> Router {
    build_server(state).mount_at("/api/rpc")
}

/// The server assembly, exposed separately for tests that drive the
/// POST fallback through the full axum router (`routes::router` already
/// mounts it; this is for focused JSON-RPC frame tests).
pub fn build_server(state: AppState) -> RpcServer {
    let state_files_list = state.clone();
    let state_files_update = state.clone();
    let state_files_delete = state.clone();
    let state_users_list = state.clone();
    let state_users_create = state.clone();
    let state_users_delete = state.clone();
    let state_settings_get = state.clone();
    let state_settings_put = state.clone();
    let state_providers_list = state.clone();
    let state_providers_upsert = state.clone();
    let state_providers_delete = state.clone();
    let state_password_get = state.clone();
    let state_password_put = state.clone();
    let state_create_folder = state.clone();
    let state_files_move = state.clone();
    let state_share_create = state.clone();
    let state_share_list = state.clone();
    let state_share_delete = state.clone();
    let state_quota_get = state.clone();
    let state_quota_put = state.clone();
    let state_files_chain = state.clone();
    let state_keys_create = state.clone();
    let state_keys_update = state.clone();
    let state_keys_list = state.clone();
    let state_keys_revoke = state.clone();
    let state_storage_grants_set = state.clone();
    let state_storage_buckets_list = state.clone();
    let state_storage_buckets_upsert = state.clone();
    let state_storage_buckets_delete = state.clone();
    let state_storage_defaults_get = state.clone();
    let state_storage_defaults_set = state.clone();
    let state_storage_unregistered_get = state.clone();
    let state_storage_unregistered_put = state.clone();
    let state_storage_unregistered_list = state.clone();
    let state_storage_unregistered_adopt = state.clone();
    RpcServer::builder()
        .auth(move |headers: &HeaderMap, uri: &Uri| {
            let state = state.clone();
            let headers = headers.clone();
            let uri = uri.clone();
            async move { auth_hook(state, headers, uri).await }
        })
        // Deferred-ops collection (`ops.result` / `ops.cancel`) ships
        // built-in with the framework — this instance runs NO deferred
        // operations, so the surface is refused before dispatch. A
        // dedicated code (not the framework's -32052 unknown-op, not
        // -32601 method-not-found) keeps the refusal distinguishable
        // from "unknown op id" and "no such method" in client logs.
        .guard(
            |_ctx: plana_rpc_server::AuthContext, method: &str, _params: &Value| {
                let method = method.to_owned();
                async move {
                    if method.starts_with("ops.") {
                        Err(JsonRpcError::new(
                            rpc_error_codes::OPS_DISABLED,
                            "deferred ops are not served on this instance",
                        ))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .method_ctx("auth.verify", |ctx: RpcRequestCtx| async move {
            to_value(UserDto::from(user_of(&ctx)))
        })
        .method_ctx("files.list", move |ctx: RpcRequestCtx| {
            let state = state_files_list.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct ListParams {
                    #[serde(default)]
                    offset: Option<u64>,
                    #[serde(default)]
                    limit: Option<u64>,
                    /// The folder to list (own scope); None = root.
                    #[serde(default)]
                    folder_id: Option<uuid::Uuid>,
                    /// "timeline" (the legacy global image stream, admin
                    /// flavor) or "own" (the caller's netdisk tree).
                #[serde(default)]
                    scope: Option<String>,
                    /// Optional kind filter ("image", "document", …).
                    #[serde(default)]
                    kind: Option<String>,
                    /// Optional bucket-slug filter (P2a / Q14): narrows
                    /// the own-scope tree to ONE visible bucket. An
                    /// INVISIBLE slug (or one the caller cannot read)
                    /// makes rows AND total vanish — never an error.
                    #[serde(default)]
                    bucket: Option<String>,
                }
                let args: ListParams = params(&ctx)?;
                let user = user_of(&ctx);
                let scope = args.scope.as_deref().unwrap_or("own");
                // Members are pinned to their own tree: the timeline is
                // the admin-flavored global stream.
                let scope = if user.role != Role::Admin { "own" } else { scope };
                // §7 pushdown (Q14): the caller's VISIBLE bucket set
                // (any grant bit or the signup default) is derived
                // server-side and pushed INTO the store — a post-fetch
                // filter would let `total` leak counts. An explicit
                // slug narrows it further; a slug outside the visible
                // set simply yields an empty set (rows AND total
                // vanish; the §7 files.list cell).
                let mut buckets = crate::bucket_acl::visible_bucket_ids(
                    state.store.as_ref(),
                    &state.registry,
                    &user,
                )
                .await
                .map_err(store_err)?;
                if let Some(slug) = args.bucket.as_deref() {
                    // The explicit slug narrows the visible set to THAT
                    // bucket; an unknown slug or one outside the visible
                    // set empties it entirely (rows AND total vanish —
                    // the §7 files.list cell, never an error).
                    buckets = match state
                        .store
                        .get_storage_bucket_by_slug(slug)
                        .await
                        .map_err(store_err)?
                    {
                        Some(row) if buckets.contains(&row.id) => vec![row.id],
                        _ => Vec::new(),
                    };
                }
                let buckets_ref: Vec<uuid::Uuid> = buckets;
                if scope == "own" {
                    let (offset, limit) = PageArgs {
                        offset: args.offset,
                        limit: args.limit,
                    }
                    .sanitize();
                    // The count rides COUNT(*) (no row decoding); the page
                    // pushes offset+limit down to the store.
                    let total = state
                        .store
                        .count_folder(user.id, args.folder_id, args.kind.clone(), Some(&buckets_ref))
                        .await
                        .map_err(store_err)?;
                    let items = state
                        .store
                        .list_folder(
                            user.id,
                            args.folder_id,
                            args.kind,
                            Some(&buckets_ref),
                            offset,
                            limit,
                        )
                        .await
                        .map_err(store_err)?
                        .into_iter()
                        .map(FileDto::from)
                        .collect::<Vec<_>>();
                    return to_value(PageResult { total, offset, limit, items });
                }
                let (offset, limit) = PageArgs {
                    offset: args.offset,
                    limit: args.limit,
                }
                .sanitize();
                let total = state
                    .store
                    .count_files(Some(&buckets_ref))
                    .await
                    .map_err(store_err)?;
                let items = state
                    .store
                    .list_files(Some(&buckets_ref), offset, limit)
                    .await
                    .map_err(store_err)?
                    .into_iter()
                    .map(FileDto::from)
                    .collect::<Vec<_>>();
                to_value(PageResult {
                    total,
                    offset,
                    limit,
                    items,
                })
            }
        })
        .method_ctx("files.update", move |ctx: RpcRequestCtx| {
            let state = state_files_update.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct UpdateParams {
                    id: Uuid,
                    name: String,
                }
                let params: UpdateParams = params(&ctx)?;
                let user = user_of(&ctx);
                let existing = require_file_owner_or_admin(&state, &user, params.id).await?;
                validate_file_name(&params.name)
                    .map_err(|err| ApiError::bad_request(err.to_string()))
                    .map_err(api_err)?;
                // Same-layer duplicate pre-check (exclude self): a rename
                // into a taken name answers the same friendly conflict as
                // create/move, not a raw index rejection.
                let taken = state
                    .store
                    .folder_name_taken(existing.owner_id, existing.parent_id, existing.bucket_id, &params.name, Some(params.id))
                    .await
                    .map_err(store_err)?;
                if taken {
                    return Err(api_err(ApiError::conflict(
                        "a row with this name already exists in this folder",
                    )));
                }
                let record = state
                    .store
                    .rename_file(params.id, &params.name)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| ApiError::not_found("file not found"))
                    .map_err(api_err)?;
                to_value(FileDto::from(record))
            }
        })
        .method_ctx("files.delete", move |ctx: RpcRequestCtx| {
            let state = state_files_delete.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DeleteParams {
                    id: Uuid,
                }
                let params: DeleteParams = params(&ctx)?;
                let user = user_of(&ctx);
                // The gate's fetch doubles as the pre-delete snapshot.
                let record = require_file_owner_or_admin(&state, &user, params.id).await?;
                // Cascade: collect the live subtree first (one walk),
                // snapshot every row for the reclaim pass, then one batch
                // soft-delete — deleting a folder deletes its subtree.
                let subtree = state
                    .store
                    .subtree_live_ids(params.id)
                    .await
                    .map_err(store_err)?;
                let rows = state
                    .store
                    .get_files_by_ids(&subtree)
                    .await
                    .map_err(store_err)?;
                let removed = state
                    .store
                    .soft_delete_many(&subtree)
                    .await
                    .map_err(store_err)?;
                if removed == 0 {
                    return Err(api_err(ApiError::not_found("file not found")));
                }
                // The subtree's share rows die with it: tokens stop
                // resolving AND the "my shares" list stops listing them.
                state
                    .store
                    .delete_shares_for_files(&subtree)
                    .await
                    .map_err(store_err)?;
                if record.kind == i0721_proto::FileKind::Folder {
                    // Batch blob reclaim for the whole subtree: per key,
                    // unlink only when no other live row references it
                    // (compare-and-restore against racing re-uploads —
                    // the single-file lane's guard, applied per key).
                    use std::collections::HashMap;
                    // Collect BOTH slot keys per row: the media blob and
                    // the thumbnail (Cache slot) — the cascade must
                    // reclaim everything the single-file lane would.
                    // B5: reclaim refcounts are per (bucket_id, key) —
                    // `media/` and `cache/` are per-bucket object
                    // namespaces, so a key is collected by bucket, and
                    // every byte touch (snapshot / delete / restore)
                    // goes to THAT bucket's store.
                    let mut media_keys: HashMap<(Option<uuid::Uuid>, String), ()> = HashMap::new();
                    let mut cache_keys: HashMap<(Option<uuid::Uuid>, String), ()> = HashMap::new();
                    for row in &rows {
                        if row.kind != i0721_proto::FileKind::Folder {
                            if !row.storage_key.is_empty() {
                                media_keys.insert((row.bucket_id, row.storage_key.clone()), ());
                            }
                            if let Some(thumb) = row.thumbnail_key.as_deref() {
                                if !thumb.is_empty() {
                                    cache_keys.insert((row.bucket_id, thumb.to_owned()), ());
                                }
                            }
                        }
                    }
                    // Reclaim with the single-file lane's
                    // compare-and-restore guard, per (bucket, key).
                    async fn reclaim_media(
                        state: &AppState,
                        bucket_id: Option<uuid::Uuid>,
                        key: &str,
                    ) -> Result<(), crate::error::ApiError> {
                        let store = state
                            .registry
                            .for_row(&i0721_proto::FileRecord {
                                bucket_id,
                                ..file_row_shape()
                            })
                            .map_err(|err| {
                                crate::error::ApiError::internal(format!(
                                    "storage backend unavailable: {err}"
                                ))
                            })?;
                        let still_live = state
                            .store
                            .count_live_files_by_storage_key(bucket_id, key)
                            .await
                            .map_err(crate::error::ApiError::from)?;
                        if still_live == 0 {
                            let snapshot = store
                                .get(crate::blob::BlobSlot::Media, key)
                                .await
                                .map_err(crate::error::ApiError::from)?;
                            store
                                .delete(crate::blob::BlobSlot::Media, key)
                                .await
                                .map_err(crate::error::ApiError::from)?;
                            if state
                                .store
                                .count_live_files_by_storage_key(bucket_id, key)
                                .await
                                .map_err(crate::error::ApiError::from)?
                                > 0
                            {
                                if let Some(bytes) = snapshot {
                                    store
                                        .put(crate::blob::BlobSlot::Media, key, &bytes)
                                        .await
                                        .map_err(crate::error::ApiError::from)?;
                                }
                            }
                        }
                        Ok(())
                    }
                    async fn reclaim_cache(
                        state: &AppState,
                        bucket_id: Option<uuid::Uuid>,
                        key: &str,
                    ) -> Result<(), crate::error::ApiError> {
                        // THUMBNAIL liveness: a thumbnail survives while
                        // any live row still references it via
                        // thumbnail_key — NOT storage_key (another
                        // owner's same-content file shares the media
                        // blob but has its OWN thumbnail row; deleting
                        // one owner's tree must not kill the other's).
                        // Per bucket, like the media slot.
                        let store = state
                            .registry
                            .for_row(&i0721_proto::FileRecord {
                                bucket_id,
                                ..file_row_shape()
                            })
                            .map_err(|err| {
                                crate::error::ApiError::internal(format!(
                                    "storage backend unavailable: {err}"
                                ))
                            })?;
                        let still_live = state
                            .store
                            .count_live_files_by_thumbnail_key(bucket_id, key)
                            .await
                            .map_err(crate::error::ApiError::from)?;
                        if still_live == 0 {
                            let snapshot = store
                                .get(crate::blob::BlobSlot::Cache, key)
                                .await
                                .map_err(crate::error::ApiError::from)?;
                            store
                                .delete(crate::blob::BlobSlot::Cache, key)
                                .await
                                .map_err(crate::error::ApiError::from)?;
                            if state
                                .store
                                .count_live_files_by_thumbnail_key(bucket_id, key)
                                .await
                                .map_err(crate::error::ApiError::from)?
                                > 0
                            {
                                if let Some(bytes) = snapshot {
                                    store
                                        .put(crate::blob::BlobSlot::Cache, key, &bytes)
                                        .await
                                        .map_err(crate::error::ApiError::from)?;
                                }
                            }
                        }
                        Ok(())
                    }
                    for ((bucket_id, key), ()) in media_keys {
                        reclaim_media(&state, bucket_id, &key)
                            .await
                            .map_err(api_err)?;
                    }
                    for ((bucket_id, key), ()) in cache_keys {
                        reclaim_cache(&state, bucket_id, &key)
                            .await
                            .map_err(api_err)?;
                    }
                    return Ok(Value::Null);
                }
                // Single-file lane: same per-(bucket, key) refcount.
                let record_store = state.registry.for_row(&record).map_err(|err| {
                    api_err(ApiError::internal(format!(
                        "storage backend unavailable: {err}"
                    )))
                })?;
                if state
                    .store
                    .count_live_files_by_storage_key(record.bucket_id, &record.storage_key)
                    .await
                    .map_err(store_err)?
                    == 0
                {
                    // Compare-and-restore against a racing upload of the
                    // same content (the REST handler's guard, verbatim).
                    let snapshot = record_store
                        .get(BlobSlot::Media, &record.storage_key)
                        .await
                        .map_err(blob_err)?;
                    let thumb_key = record.thumbnail_key.clone();
                    let thumb_snapshot = match thumb_key.as_deref() {
                        Some(key) => record_store
                            .get(BlobSlot::Cache, key)
                            .await
                            .map_err(blob_err)?,
                        None => None,
                    };
                    record_store
                        .delete(BlobSlot::Media, &record.storage_key)
                        .await
                        .map_err(blob_err)?;
                    if let Some(key) = thumb_key.as_deref() {
                        record_store
                            .delete(BlobSlot::Cache, key)
                            .await
                            .map_err(blob_err)?;
                    }
                    if state
                        .store
                        .count_live_files_by_storage_key(record.bucket_id, &record.storage_key)
                        .await
                        .map_err(store_err)?
                        > 0
                    {
                        if let Some(bytes) = snapshot {
                            record_store
                                .put(BlobSlot::Media, &record.storage_key, &bytes)
                                .await
                                .map_err(blob_err)?;
                        }
                        if let (Some(key), Some(bytes)) = (thumb_key.as_deref(), thumb_snapshot) {
                            record_store
                                .put(BlobSlot::Cache, key, &bytes)
                                .await
                                .map_err(blob_err)?;
                        }
                    }
                }
                Ok(Value::Null)
            }
        })
        .method_ctx("users.list", move |ctx: RpcRequestCtx| {
            let state = state_users_list.clone();
            async move {
                let user = user_of(&ctx);
                require_admin(&user).await?;
                let args: PageArgs = params(&ctx)?;
                let (offset, limit) = args.sanitize();
                let total = state.store.count_users().await.map_err(store_err)?;
                let items = state
                    .store
                    .list_users(offset, limit)
                    .await
                    .map_err(store_err)?
                    .into_iter()
                    .map(UserDto::from)
                    .collect::<Vec<_>>();
                to_value(PageResult {
                    total,
                    offset,
                    limit,
                    items,
                })
            }
        })
        .method_ctx("users.create", move |ctx: RpcRequestCtx| {
            let state = state_users_create.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let request: CreateUserRequest = params(&ctx)?;
                let candidate = NewUser {
                    username: request.username,
                    password: request.password,
                    display_name: request.display_name,
                    role: request.role,
                };
                validate_username(&candidate.username)
                    .map_err(|err| ApiError::bad_request(err.to_string()))
                    .map_err(api_err)?;
                if candidate.password.is_empty() || candidate.password.len() > 128 {
                    return Err(api_err(ApiError::bad_request(
                        "password must be 1..=128 characters",
                    )));
                }
                let display_name = candidate
                    .display_name
                    .clone()
                    .unwrap_or_else(|| candidate.username.clone());
                validate_display_name(&display_name)
                    .map_err(|err| ApiError::bad_request(err.to_string()))
                    .map_err(api_err)?;
                let password_hash = hash_password(&candidate.password)
                    .map_err(|err| ApiError::internal(err.to_string()))
                    .map_err(api_err)?;
                let record = state
                    .store
                    .create_user(CreateUser {
                        username: candidate.username.clone(),
                        display_name,
                        role: candidate.role.unwrap_or(Role::Member),
                        password_hash: Some(password_hash),
                    })
                    .await
                    .map_err(store_err)?;
                state
                    .store
                    .set_setting(
                        &watermark_key(record.id),
                        serde_json::Value::String(jiff::Timestamp::now().to_string()),
                    )
                    .await
                    .map_err(store_err)?;
                to_value(UserDto::from(record))
            }
        })
        .method_ctx("users.delete", move |ctx: RpcRequestCtx| {
            let state = state_users_delete.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DeleteParams {
                    id: Uuid,
                }
                let params: DeleteParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                if caller.id == params.id {
                    return Err(api_err(ApiError::forbidden("cannot delete yourself")));
                }
                let removed = state
                    .store
                    .delete_user(params.id)
                    .await
                    .map_err(store_err)?;
                if removed {
                    Ok(Value::Null)
                } else {
                    Err(api_err(ApiError::not_found("user not found")))
                }
            }
        })
        .method_ctx("settings.get", move |ctx: RpcRequestCtx| {
            let state = state_settings_get.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                to_value(site_config(&state).await.map_err(api_err)?)
            }
        })
        .method_ctx("settings.put", move |ctx: RpcRequestCtx| {
            let state = state_settings_put.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // A full replacement: the strict wire form requires every
                // section and leaf and rejects unknown fields, so a
                // partial body fails with -32602 instead of silently
                // resetting omitted sections to the seed defaults.
                let strict: SiteConfigStrict = params(&ctx)?;
                let config = i0721_proto::SiteConfig::from(strict);
                config
                    .validate()
                    .map_err(ApiError::bad_request)
                    .map_err(api_err)?;
                state
                    .store
                    .set_setting(
                        CONFIG_SETTING_KEY,
                        serde_json::to_value(&config).map_err(|err| {
                            JsonRpcError::internal_error(&format!(
                                "config serialization failed: {err}"
                            ))
                        })?,
                    )
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("providers.list", move |ctx: RpcRequestCtx| {
            let state = state_providers_list.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let rows = load_providers(&state).await;
                to_value(rows.iter().map(OAuthProvider::masked).collect::<Vec<_>>())
            }
        })
        .method_ctx("providers.upsert", move |ctx: RpcRequestCtx| {
            let state = state_providers_upsert.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let upsert: OAuthProviderUpsert = params(&ctx)?;
                if !valid_provider_id(&upsert.id) {
                    return Err(api_err(ApiError::bad_request(
                        "provider id must be [a-z0-9][a-z0-9._-]* (max 48)",
                    )));
                }
                if !valid_provider_type(&upsert.provider_type) {
                    return Err(api_err(ApiError::bad_request(
                        "type must be builtin:github | builtin:feishu | builtin:erp | oidc",
                    )));
                }
                // https-only endpoints (http survives for loopback — the
                // local stub/dev lane); an http base would POST the
                // client secret in cleartext.
                if !valid_provider_url(upsert.base_url.as_deref()) {
                    return Err(api_err(ApiError::bad_request(
                        "base_url must be an absolute https URL (http only for localhost/127.0.0.1)",
                    )));
                }
                if !valid_provider_url(upsert.issuer_url.as_deref()) {
                    return Err(api_err(ApiError::bad_request(
                        "issuer_url must be an absolute https URL (http only for localhost/127.0.0.1)",
                    )));
                }
                let mut providers = load_providers(&state).await;
                // Empty secret = keep the stored one (the admin form
                // never echoes secrets back).
                let stored_secret = providers
                    .iter()
                    .find(|p| p.id == upsert.id)
                    .map(|p| p.client_secret.clone())
                    .unwrap_or_default();
                let secret = if upsert.client_secret.is_empty() {
                    stored_secret
                } else {
                    upsert.client_secret.clone()
                };
                let row = OAuthProvider {
                    id: upsert.id,
                    display_name: upsert.display_name,
                    provider_type: upsert.provider_type,
                    client_id: upsert.client_id,
                    client_secret: secret,
                    base_url: upsert.base_url,
                    issuer_url: upsert.issuer_url,
                    scopes: upsert.scopes,
                    enabled: upsert.enabled,
                    icon: upsert.icon,
                };
                match providers.iter().position(|p| p.id == row.id) {
                    Some(index) => providers[index] = row.clone(),
                    None => providers.push(row.clone()),
                }
                save_providers(&state, &providers).await.map_err(api_err)?;
                to_value(row.masked())
            }
        })
        .method_ctx("providers.delete", move |ctx: RpcRequestCtx| {
            let state = state_providers_delete.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DeleteParams {
                    id: String,
                }
                let params: DeleteParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let mut providers = load_providers(&state).await;
                let before = providers.len();
                providers.retain(|p| p.id != params.id);
                if providers.len() == before {
                    return Err(api_err(ApiError::not_found("provider not found")));
                }
                save_providers(&state, &providers).await.map_err(api_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("auth.password.get", move |ctx: RpcRequestCtx| {
            let state = state_password_get.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                to_value(password_login_enabled(&state).await)
            }
        })
        .method_ctx("auth.password.put", move |ctx: RpcRequestCtx| {
            let state = state_password_put.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct PutParams {
                    enabled: bool,
                }
                let params: PutParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // The dead-state guard: turning the password OFF with no
                // enabled provider would leave the deployment with NO
                // login method at all — refuse (the row stays; the
                // admin enables a provider first or keeps the password).
                if !params.enabled {
                    let any_provider = load_providers(&state)
                        .await
                        .iter()
                        .any(|p| p.enabled);
                    if !any_provider {
                        return Err(api_err(ApiError::bad_request(
                            "cannot disable password login while no OAuth provider is enabled",
                        )));
                    }
                }
                state
                    .store
                    .set_setting(
                        PASSWORD_LOGIN_SETTINGS_KEY,
                        serde_json::Value::Bool(params.enabled),
                    )
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("files.create_folder", move |ctx: RpcRequestCtx| {
            let state = state_create_folder.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct FolderParams {
                    name: String,
                    #[serde(default)]
                    parent_id: Option<uuid::Uuid>,
                }
                let args: FolderParams = params(&ctx)?;
                let user = user_of(&ctx);
                validate_file_name(&args.name)
                    .map_err(|err| ApiError::bad_request(err.to_string()))
                    .map_err(api_err)?;
                // R-G: the folder tree is per bucket. A child inherits
                // its parent's bucket; a root folder takes the resolved
                // target bucket (in this slice the bootstrap bucket —
                // the same outlet the upload lane reads).
                let bucket_id = match args.parent_id {
                    Some(parent) => {
                        let row = state.store.get_file(parent).await.map_err(store_err)?
                            .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
                            .ok_or_else(|| ApiError::bad_request("parent folder not found"))
                            .map_err(api_err)?;
                        if user.role != Role::Admin && row.owner_id != user.id {
                            return Err(api_err(ApiError::forbidden("not your folder")));
                        }
                        row.bucket_id
                    }
                    None => Some(
                        state
                            .registry
                            .resolve_upload_bucket(None, None)
                            .map_err(|err| {
                                api_err(ApiError::internal(format!(
                                    "storage backend unavailable: {err}"
                                )))
                            })?
                            .0,
                    ),
                };
                let record = state
                    .store
                    .create_folder(user.id, args.parent_id, &args.name, bucket_id)
                    .await
                    .map_err(|err| match err {
                        i0721_store::StoreError::Conflict(reason) => ApiError::conflict(reason),
                        other => ApiError::from(other),
                    })
                    .map_err(api_err)?;
                to_value(FileDto::from(record))
            }
        })
        .method_ctx("files.move", move |ctx: RpcRequestCtx| {
            let state = state_files_move.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct MoveParams {
                    id: uuid::Uuid,
                    #[serde(default)]
                    parent_id: Option<uuid::Uuid>,
                }
                let args: MoveParams = params(&ctx)?;
                let user = user_of(&ctx);
                let record = state.store.get_file(args.id).await.map_err(store_err)?
                    .filter(|r| r.deleted_at.is_none())
                    .ok_or_else(|| ApiError::not_found("file not found"))
                    .map_err(api_err)?;
                if user.role != Role::Admin && record.owner_id != user.id {
                    return Err(api_err(ApiError::forbidden("not your file")));
                }
                if let Some(parent) = args.parent_id {
                    let target = state.store.get_file(parent).await.map_err(store_err)?
                        .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
                        .ok_or_else(|| ApiError::bad_request("target folder not found"))
                        .map_err(api_err)?;
                    if user.role != Role::Admin && target.owner_id != user.id {
                        return Err(api_err(ApiError::forbidden("not your folder")));
                    }
                    // Cycle: the target's ancestor chain must not pass
                    // through the moved row itself.
                    let chain = state.store.ancestor_ids(parent).await.map_err(store_err)?;
                    if chain.contains(&args.id) {
                        return Err(api_err(ApiError::bad_request(
                            "cannot move a folder into its own subtree",
                        )));
                    }
                }
                // Same-layer duplicate check (exclude self: moving within
                // its own layer keeping its name is a no-op, not 409).
                let taken = state
                    .store
                    .folder_name_taken(record.owner_id, args.parent_id, record.bucket_id, &record.name, Some(args.id))
                    .await
                    .map_err(store_err)?;
                if taken {
                    return Err(api_err(ApiError::conflict(
                        "a row with this name already exists in the target folder",
                    )));
                }
                state.store.set_parent(args.id, args.parent_id).await.map_err(store_err)?;
                let moved = state.store.get_file(args.id).await.map_err(store_err)?
                    .ok_or_else(|| ApiError::not_found("file not found"))
                    .map_err(api_err)?;
                to_value(FileDto::from(moved))
            }
        })
        .method_ctx("files.chain", move |ctx: RpcRequestCtx| {
            let state = state_files_chain.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct ChainParams {
                    folder_id: uuid::Uuid,
                }
                let args: ChainParams = params(&ctx)?;
                let user = user_of(&ctx);
                // Owner-or-admin on the TARGET folder — and, from P2a on,
                // the §7 bucket column: the folder's bucket must be
                // VISIBLE (any bit), readable, and its backend servable;
                // invisible → `-32006`, visible without Read → `-32005`,
                // unservable → `-32010`.
                let target = state.store.get_file(args.folder_id).await.map_err(store_err)?
                    .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
                    .ok_or_else(|| api_err(ApiError::not_found("folder not found")))?;
                if user.role != Role::Admin && target.owner_id != user.id {
                    return Err(api_err(ApiError::not_found("folder not found")));
                }
                files_chain_bucket_gate(&state, &user, target.bucket_id).await?;
                // Walk ancestors, then reverse (root-first).
                let ancestors = state.store.ancestor_ids(args.folder_id).await.map_err(store_err)?;
                let mut chain = vec![
                    serde_json::json!({"id": args.folder_id.to_string(), "name": target.name}),
                ];
                for ancestor_id in ancestors {
                    if let Some(row) = state.store.get_file(ancestor_id).await.map_err(store_err)? {
                        chain.push(serde_json::json!({
                            "id": row.id.to_string(), "name": row.name,
                        }));
                    }
                }
                chain.reverse(); // root … → current
                to_value(chain)
            }
        })
        .method_ctx("files.share_create", move |ctx: RpcRequestCtx| {
            let state = state_share_create.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct ShareParams {
                    file_id: uuid::Uuid,
                    #[serde(default)]
                    expires_at: Option<String>,
                    #[serde(default)]
                    allow_preview: bool,
                }
                let args: ShareParams = params(&ctx)?;
                let user = user_of(&ctx);
                let record = state.store.get_file(args.file_id).await.map_err(store_err)?
                    .filter(|r| r.deleted_at.is_none() && r.kind != i0721_proto::FileKind::Folder)
                    .ok_or_else(|| ApiError::not_found("file not found"))
                    .map_err(api_err)?;
                if user.role != Role::Admin && record.owner_id != user.id {
                    return Err(api_err(ApiError::forbidden("not your file")));
                }
                // R-L: owner-or-admin is the WHO; the share still needs the
                // actor's SHARE bit on the file's bucket (a write-only grant
                // row must not publish shares through the RPC face either).
                let bucket_id = record
                    .bucket_id
                    .or_else(|| state.registry.bootstrap_bucket_id().ok());
                let Some(bucket_id) = bucket_id else {
                    return Err(api_err(ApiError::not_found("file not found")));
                };
                let bucket_row = crate::bucket_acl::bucket_row_for_id(
                    state.store.as_ref(),
                    &state.registry,
                    bucket_id,
                )
                .await
                .map_err(store_err)?
                .ok_or_else(|| ApiError::not_found("file not found"))
                .map_err(api_err)?;
                if user.role != Role::Admin {
                    let access = crate::bucket_acl::resolve_access(
                        state.store.as_ref(),
                        &user,
                        &bucket_row,
                    )
                    .await
                    .map_err(store_err)?;
                    if !access.share {
                        return Err(api_err(ApiError::forbidden(format!(
                            "you do not have share access to bucket {:?}",
                            bucket_row.slug
                        ))));
                    }
                }
                let share = state
                    .store
                    .create_share(NewShare {
                        file_id: args.file_id,
                        owner_id: record.owner_id,
                        expires_at: args.expires_at,
                        allow_preview: args.allow_preview,
                    })
                    .await
                    .map_err(store_err)?;
                to_value(ShareDto::from((share, record.name.clone(), record.size)))
            }
        })
        .method_ctx("files.share_list", move |ctx: RpcRequestCtx| {
            let state = state_share_list.clone();
            async move {
                let user = user_of(&ctx);
                let shares = state.store.list_shares(user.id).await.map_err(store_err)?;
                let mut out = Vec::with_capacity(shares.len());
                for share in shares {
                    if let Some(file) = state.store.get_file(share.file_id).await.map_err(store_err)? {
                        out.push(ShareDto::from((share, file.name, file.size)));
                    }
                }
                to_value(out)
            }
        })
        .method_ctx("files.share_delete", move |ctx: RpcRequestCtx| {
            let state = state_share_delete.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DeleteParams {
                    token: String,
                }
                let args: DeleteParams = params(&ctx)?;
                let user = user_of(&ctx);
                let share = state.store.get_share(&args.token).await.map_err(store_err)?
                    .ok_or_else(|| ApiError::not_found("share not found"))
                    .map_err(api_err)?;
                if user.role != Role::Admin && share.owner_id != user.id {
                    return Err(api_err(ApiError::forbidden("not your share")));
                }
                state.store.delete_share(&args.token).await.map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("files.quota_get", move |ctx: RpcRequestCtx| {
            let state = state_quota_get.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct QuotaParams {
                    #[serde(default)]
                    user_id: Option<uuid::Uuid>,
                }
                let args: QuotaParams = params(&ctx)?;
                let caller = user_of(&ctx);
                let target = match args.user_id {
                    Some(id) => {
                        if caller.role != Role::Admin && caller.id != id {
                            return Err(api_err(ApiError::forbidden("admin role required")));
                        }
                        state.store.get_user(id).await.map_err(store_err)?
                            .ok_or_else(|| ApiError::not_found("user not found"))
                            .map_err(api_err)?
                    }
                    None => caller,
                };
                let used = state.store.sum_live_sizes(target.id).await.map_err(store_err)?;
                // Per-user row ?? the site default; None = unlimited.
                let quota = match target.quota_bytes {
                    Some(q) => Some(q),
                    None => site_config(&state)
                        .await
                        .map_err(api_err)?
                        .upload
                        .default_quota_bytes,
                };
                to_value(QuotaInfo {
                    used_bytes: used,
                    quota_bytes: quota,
                })
            }
        })
        .method_ctx("files.quota_put", move |ctx: RpcRequestCtx| {
            let state = state_quota_put.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct QuotaPutParams {
                    user_id: uuid::Uuid,
                    #[serde(default)]
                    quota_bytes: Option<i64>,
                }
                let args: QuotaPutParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                if let Some(bytes) = args.quota_bytes {
                    if bytes < 0 {
                        return Err(api_err(ApiError::bad_request("quota_bytes must be >= 0")));
                    }
                }
                state
                    .store
                    .set_user_quota(args.user_id, args.quota_bytes)
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })        .method_ctx("keys.create", move |ctx: RpcRequestCtx| {
            let state = state_keys_create.clone();
            async move {
                let args: KeyWriteParams = params(&ctx)?;
                let caller = user_of(&ctx);
                let (name, ops, expires_at) = validate_key_write(&args)?;
                // Q7's materialization: an omitted `buckets` snapshots
                // the owner's CURRENT authorized set (`granted_buckets`);
                // an explicit array (even `[]`) is the caller's pick.
                // MUT:M-P2A7 — materializing from the DEFAULT (ignoring
                // the explicit `[]`) turns the explicit-empty arm red.
                let buckets = match args.buckets {
                    Some(explicit) => explicit,
                    None => crate::bucket_acl::granted_buckets(&state.store, &caller)
                        .await
                        .map_err(store_err)?
                        .into_iter()
                        .collect(),
                };

                let key = crate::mcp::mint_api_key();
                let hash = crate::mcp::hash_api_key(&key);
                let prefix = crate::mcp::mask_api_key(&key);
                let record = i0721_proto::ApiKeyRecord {
                    id: Uuid::new_v4(),
                    user_id: caller.id,
                    name,
                    key_prefix: prefix,
                    key_hash: hash,
                    scopes: crate::key_scopes::encode_scopes_with_buckets(ops, &buckets),
                    expires_at,
                    last_used_at: None,
                    revoked_at: None,
                    created_at: jiff::Timestamp::now(),
                };
                state.store.create_api_key(&record).await.map_err(store_err)?;
                // The plaintext key appears HERE and ONLY here.
                to_value(i0721_proto::ApiKeyCreated {
                    id: record.id,
                    name: record.name,
                    key,
                })
            }
        })
        .method_ctx("keys.update", move |ctx: RpcRequestCtx| {
            let state = state_keys_update.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct UpdateParams {
                    id: Uuid,
                    #[serde(flatten)]
                    write: KeyWriteParams,
                }
                let args: UpdateParams = params(&ctx)?;
                let caller = user_of(&ctx);
                // Owner-or-admin from the RECORD's own user_id, shared with
                // `keys.revoke` ([`require_key_access`]). An EXPIRED key may
                // be edited (renewal is the point); a REVOKED one may not —
                // dead is dead, and it answers the same not-found
                // `keys.revoke` answers (the helper's revoked arm).
                let key = require_key_access(&state, &caller, args.id).await?;
                // Whole-record replacement: all fields, and an
                // omitted `expires_at` CLEARS the expiry. The same Q7
                // materialization as `keys.create` — but resolved for the
                // KEY'S OWNER (an admin editing a member's key snapshots
                // the MEMBER's buckets, not their own).
                let (name, ops, expires_at) = validate_key_write(&args.write)?;
                let owner = state.store.get_user(key.user_id).await.map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("key owner not found")))?;
                let buckets = match args.write.buckets {
                    Some(explicit) => explicit,
                    None => crate::bucket_acl::granted_buckets(&state.store, &owner)
                        .await
                        .map_err(store_err)?
                        .into_iter()
                        .collect(),
                };
                let updated = state
                    .store
                    .update_api_key(
                        key.id,
                        &name,
                        &crate::key_scopes::encode_scopes_with_buckets(ops, &buckets),
                        expires_at.as_deref(),
                    )
                    .await
                    .map_err(store_err)?;
                if updated {
                    Ok(Value::Null)
                } else {
                    Err(api_err(ApiError::not_found("key not found or already revoked")))
                }
            }
        })
        .method_ctx("keys.list", move |ctx: RpcRequestCtx| {
            let state = state_keys_list.clone();
            async move {
                let caller = user_of(&ctx);
                let now = jiff::Timestamp::now();
                let rows = state.store.list_api_keys(caller.id).await.map_err(store_err)?;
                let items: Vec<i0721_proto::ApiKeyPublic> = rows
                    .into_iter()
                    .map(|r| {
                        let set = crate::key_scopes::parse_scopes(&r.scopes);
                        let expired =
                            crate::key_scopes::is_live(&r, now) == crate::key_scopes::Live::Expired;
                        // Q7's list projection: an explicit array — `[]`
                        // included — carries the canonicalized slugs; a
                        // scopes WITHOUT the array (K-era) reports `None`
                        // so the webui can render the「无桶」marker.
                        let buckets = if crate::key_scopes::scopes_carry_buckets_array(&r.scopes) {
                            Some(
                                crate::key_scopes::parse_scopes_buckets(&r.scopes)
                                    .into_iter()
                                    .collect::<Vec<_>>(),
                            )
                        } else {
                            None
                        };
                        i0721_proto::ApiKeyPublic {
                            id: r.id,
                            name: r.name,
                            key_prefix: r.key_prefix,
                            ops: crate::key_scopes::ordered_ops(&set),
                            buckets,
                            expires_at: r.expires_at,
                            expired,
                            last_used_at: r.last_used_at,
                            revoked_at: r.revoked_at,
                            created_at: r.created_at,
                        }
                    })
                    .collect();
                to_value(items)
            }
        })
        .method_ctx("keys.revoke", move |ctx: RpcRequestCtx| {
            let state = state_keys_revoke.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct RevokeParams {
                    id: Uuid,
                }
                let args: RevokeParams = params(&ctx)?;
                let caller = user_of(&ctx);
                // Owner-or-admin from the RECORD's own user_id, shared with
                // `keys.update` ([`require_key_access`]).
                let key = require_key_access(&state, &caller, args.id).await?;
                let removed = state.store.revoke_api_key(key.id).await.map_err(store_err)?;
                if removed {
                    Ok(Value::Null)
                } else {
                    Err(api_err(ApiError::not_found("key not found or already revoked")))
                }
            }
        })
        // ── the bucket authorization + registry surface (P2a) ──────────
        .method_ctx("storage.grants.set", move |ctx: RpcRequestCtx| {
            let state = state_storage_grants_set.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct GrantParams {
                    user: Uuid,
                    bucket_slug: String,
                    #[serde(default)]
                    read: bool,
                    #[serde(default)]
                    write: bool,
                    #[serde(default)]
                    share: bool,
                }
                let args: GrantParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // The admin arm of the R-A evaluation never reads a row:
                // an admin's rights come from the ROLE, so writing a grant
                // row for one is refused (it would also leak nothing after
                // a later demotion — the row is just dead weight).
                let target = state.store.get_user(args.user).await.map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("user not found")))?;
                if target.role == Role::Admin {
                    return Err(api_err(ApiError::forbidden(
                        "grants cannot be set for an admin (their rights come from the role)",
                    )));
                }
                let bucket = state
                    .store
                    .get_storage_bucket_by_slug(&args.bucket_slug)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("no such bucket")))?;
                state
                    .store
                    .set_bucket_grant(args.user, bucket.id, args.read, args.write, args.share)
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("storage.buckets.list", move |ctx: RpcRequestCtx| {
            let state = state_storage_buckets_list.clone();
            async move {
                let caller = user_of(&ctx);
                // The stage-A surface, extended to VISIBILITY SCOPE (the
                // §7 files.list cell mandates a selector that lists the
                // actor's visible buckets with a per-bucket permission
                // mark — a member-visible enumeration is required for
                // that, and the key-form picker reads the same lane):
                // admin keeps ALL buckets; a member gets ONLY their
                // visible buckets (R-A evaluation: any grant bit or the
                // signup default; `enabled` does not affect visibility,
                // Q14). The record shape is unchanged plus one per-row
                // `my_access` projection of the actor's own bits.
                let rows = state.store.list_storage_buckets().await.map_err(store_err)?;
                let mut out: Vec<Value> = Vec::with_capacity(rows.len());
                for bucket in rows {
                    let access =
                        crate::bucket_acl::resolve_access(state.store.as_ref(), &caller, &bucket)
                            .await
                            .map_err(store_err)?;
                    if caller.role != Role::Admin && !access.any() {
                        continue;
                    }
                    let mut value = to_value(bucket)?;
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "my_access".to_owned(),
                            serde_json::json!({
                                "read": access.read,
                                "write": access.write,
                                "share": access.share,
                            }),
                        );
                    }
                    out.push(value);
                }
                to_value(out)
            }
        })
        .method_ctx("storage.buckets.upsert", move |ctx: RpcRequestCtx| {
            let state = state_storage_buckets_upsert.clone();
            async move {
                #[derive(serde::Deserialize)]
                #[serde(deny_unknown_fields)]
                struct BucketUpsertParams {
                    slug: String,
                    /// The mutable fields are a STRICT FULL DOCUMENT: an
                    /// omitted field is a refusal, not a reset-to-default.
                    display_name: String,
                    anonymous_read: bool,
                    grant_on_signup: bool,
                    enabled: bool,
                }
                let args: BucketUpsertParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // The R-A constraint: a default-open bucket must be
                // anonymous-readable (default-open only makes sense for a
                // public bucket).
                if args.grant_on_signup && !args.anonymous_read {
                    return Err(api_err(ApiError::bad_request(
                        "grant_on_signup = TRUE requires anonymous_read = TRUE",
                    )));
                }
                let updated = state
                    .store
                    .upsert_storage_bucket(
                        &args.slug,
                        args.display_name.trim(),
                        args.anonymous_read,
                        args.grant_on_signup,
                        args.enabled,
                    )
                    .await
                    .map_err(store_err)?;
                to_value(updated)
            }
        })
        .method_ctx("storage.buckets.delete", move |ctx: RpcRequestCtx| {
            let state = state_storage_buckets_delete.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DeleteParams {
                    slug: String,
                }
                let args: DeleteParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let bucket = state
                    .store
                    .get_storage_bucket_by_slug(&args.slug)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("no such bucket")))?;
                // R-B's three directions, all before any delete:
                if bucket.is_bootstrap {
                    return Err(api_err(ApiError::forbidden(
                        "the bootstrap bucket cannot be deleted",
                    )));
                }
                let live = state
                    .store
                    .count_live_files_by_bucket(Some(bucket.id))
                    .await
                    .map_err(store_err)?;
                if live > 0 {
                    return Err(api_err(ApiError::conflict(format!(
                        "the bucket still holds {live} live rows — empty it first"
                    ))));
                }
                if key_references_slug(&state, &bucket.slug).await? {
                    return Err(api_err(ApiError::conflict(
                        "a live API key still references this bucket's slug",
                    )));
                }
                let removed = state
                    .store
                    .delete_storage_bucket(bucket.id)
                    .await
                    .map_err(store_err)?;
                if removed {
                    Ok(Value::Null)
                } else {
                    Err(api_err(ApiError::not_found("no such bucket")))
                }
            }
        })
        .method_ctx("storage.defaults.get", move |ctx: RpcRequestCtx| {
            let state = state_storage_defaults_get.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                use i0721_store::SettingsState;
                let value = match state
                    .store
                    .settings_state(crate::api::upload_core::DEFAULT_BUCKET_SETTINGS_KEY)
                    .await
                    .map_err(store_err)?
                {
                    SettingsState::Absent => None,
                    SettingsState::Value(value) => Some(value),
                    SettingsState::Corrupt { raw } | SettingsState::Unreadable { err: raw } => {
                        // R-F: the four-state read surfaces honestly
                        // instead of falling back to "unconfigured".
                        return Err(api_err(ApiError::internal(format!(
                            "the storage default bucket setting is not readable: {raw:?}"
                        ))));
                    }
                };
                to_value(serde_json::json!({ "bucket": value.and_then(|v| v.as_str().map(str::to_owned)) }))
            }
        })
        .method_ctx("storage.defaults.set", move |ctx: RpcRequestCtx| {
            let state = state_storage_defaults_set.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct DefaultsParams {
                    /// The default upload bucket's slug (R-F / R-I); the
                    /// upload lane answers 500 / -32010 when this is
                    /// unset-but-required or unreadable.
                    bucket: String,
                }
                let args: DefaultsParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // The default must name a REAL bucket.
                state
                    .store
                    .get_storage_bucket_by_slug(&args.bucket)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("no such bucket")))?;
                state
                    .store
                    .set_setting(
                        crate::api::upload_core::DEFAULT_BUCKET_SETTINGS_KEY,
                        serde_json::Value::String(args.bucket),
                    )
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("storage.unregistered.get", move |ctx: RpcRequestCtx| {
            let state = state_storage_unregistered_get.clone();
            async move {
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                let show = crate::api::unregistered::unregistered_show(&state).await.map_err(api_err)?;
                to_value(serde_json::json!({ "show": show }))
            }
        })
        .method_ctx("storage.unregistered.put", move |ctx: RpcRequestCtx| {
            let state = state_storage_unregistered_put.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct ShowParams {
                    show: bool,
                }
                let args: ShowParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                state
                    .store
                    .set_setting(
                        crate::api::unregistered::UNREGISTERED_SHOW_SETTINGS_KEY,
                        serde_json::Value::Bool(args.show),
                    )
                    .await
                    .map_err(store_err)?;
                Ok(Value::Null)
            }
        })
        .method_ctx("storage.unregistered.list", move |ctx: RpcRequestCtx| {
            let state = state_storage_unregistered_list.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct UnregisteredListParams {
                    bucket: String,
                    #[serde(default)]
                    offset: Option<u64>,
                    #[serde(default)]
                    limit: Option<u64>,
                }
                let args: UnregisteredListParams = params(&ctx)?;
                let caller = user_of(&ctx);
                // The unregistered-object surface is INVISIBLE to a
                // non-admin (B-SPEC's error table, storage.* row: "bucket
                // not visible → -32006"; a member must not learn that the
                // surface — or anything in the bucket — exists). The other
                // three unregistered methods are management ACTIONS and
                // keep require_admin's -32005.
                // MUT:P3-REQUIRE-ADMIN — dropping this gate reddens
                // `unregistered_off_rpc_list_refuses` (non-admin arm).
                if caller.role != Role::Admin {
                    return Err(api_err(ApiError::not_found(
                        "the unregistered-object surface is disabled (storage_unregistered.show)",
                    )));
                }
                // R-E: the switch gates the LIST itself — with show=false
                // even an admin learns nothing (a corrupt/unreadable
                // setting is OFF too, fail-closed).
                let show = crate::api::unregistered::unregistered_show(&state).await.map_err(api_err)?;
                if !show {
                    return Err(api_err(ApiError::not_found(
                        "the unregistered-object surface is disabled (storage_unregistered.show)",
                    )));
                }
                let bucket = state
                    .store
                    .get_storage_bucket_by_slug(&args.bucket)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("no such bucket")))?;
                let (offset, limit) = PageArgs {
                    offset: args.offset,
                    limit: args.limit,
                }
                .sanitize();
                let bucket_store = state
                    .registry
                    .resolve(&bucket.slug)
                    .map_err(|err| api_err(ApiError::internal(format!("storage backend unavailable: {err}"))))?;
                let (items, total) =
                    crate::api::unregistered::list_unregistered(&state, &bucket, &bucket_store, offset, limit)
                        .await
                        .map_err(api_err)?;
                to_value(PageResult { total, offset, limit, items })
            }
        })
        .method_ctx("storage.unregistered.adopt", move |ctx: RpcRequestCtx| {
            let state = state_storage_unregistered_adopt.clone();
            async move {
                #[derive(serde::Deserialize)]
                struct AdoptParams {
                    bucket: String,
                    key: String,
                    #[serde(default)]
                    owner_id: Option<Uuid>,
                    #[serde(default)]
                    parent_id: Option<Uuid>,
                    #[serde(default)]
                    name: Option<String>,
                }
                let args: AdoptParams = params(&ctx)?;
                let caller = user_of(&ctx);
                require_admin(&caller).await?;
                // R-E gates adopt too: an OFF switch means the lane does
                // not exist, not merely that listing is hidden.
                let show = crate::api::unregistered::unregistered_show(&state).await.map_err(api_err)?;
                if !show {
                    return Err(api_err(ApiError::not_found(
                        "the unregistered-object surface is disabled (storage_unregistered.show)",
                    )));
                }
                // The sha256 namespace gate (draft §7.4) BEFORE any store
                // touch.
                if !crate::api::unregistered::is_sha256_key(&args.key) {
                    return Err(api_err(ApiError::bad_request(
                        "the key is outside the sha256 namespace (64 lowercase hex chars required)",
                    )));
                }
                let bucket = state
                    .store
                    .get_storage_bucket_by_slug(&args.bucket)
                    .await
                    .map_err(store_err)?
                    .ok_or_else(|| api_err(ApiError::not_found("no such bucket")))?;
                // MUT:P3-ADOPT-DISABLED — a disabled or credential-less target
                // bucket must refuse the adopt with the storage-unavailable
                // fault (-32010), never re-route the row into another
                // bucket's store.
                let bucket_store = state.registry.resolve(&bucket.slug).map_err(|err| {
                    JsonRpcError::new(
                        rpc_error_codes::STORAGE_UNAVAILABLE,
                        format!("storage backend unavailable: {err}"),
                    )
                })?;
                let name = match args.name {
                    Some(name) => {
                        validate_file_name(&name)
                            .map_err(|err| api_err(ApiError::bad_request(err.to_string())))?;
                        name
                    }
                    None => args.key.clone(),
                };
                if let Some(parent_id) = args.parent_id {
                    let folder = state
                        .store
                        .get_file(parent_id)
                        .await
                        .map_err(store_err)?
                        .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
                        .ok_or_else(|| api_err(ApiError::bad_request("folder not found")))?;
                    let target_owner = args.owner_id.unwrap_or(caller.id);
                    // MUT:P3-ADOPT-PARENT-OWNER — the folder must belong to
                    // the adopt's target owner.
                    if folder.owner_id != target_owner {
                        return Err(api_err(ApiError::bad_request(
                            "the parent folder must belong to the adopt's target owner",
                        )));
                    }
                    // MUT:P3-ADOPT-PARENT-BUCKET — R-G's per-bucket folder tree:
                    // the row is born into the target bucket, so its parent
                    // must sit in the SAME bucket (the invariant `set_parent`
                    // and `create_folder` enforce on every other path).
                    if folder.bucket_id != Some(bucket.id) {
                        return Err(api_err(ApiError::conflict(
                            "the parent folder sits in another bucket",
                        )));
                    }
                }
                let record = crate::api::unregistered::adopt(
                    &state,
                    &bucket,
                    &bucket_store,
                    &caller,
                    crate::api::unregistered::AdoptTarget {
                        bucket_id: bucket.id,
                        storage_backend: bucket.backend.as_str().to_owned(),
                        owner_id: args.owner_id.unwrap_or(caller.id),
                        parent_id: args.parent_id,
                        name,
                        key: args.key,
                    },
                )
                .await
                .map_err(api_err)?;
                to_value(FileDto::from(record))
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use std::collections::BTreeMap;
    use std::path::Path;

    /// `text` with every `#[cfg(test)] mod … { … }` blanked to spaces.
    ///
    /// Blanked, not cut: a file can hold more than one test module with
    /// production items between them (`config.rs` has three), and an earlier
    /// version cut from the first `#[cfg(test)]` to the end of the file —
    /// which skipped everything after it, test or not.
    ///
    /// A test module ends at the next `}` in column zero: a top-level `mod`
    /// always does, whatever is nested inside it. That is the whole rule — no
    /// brace counting, so a `}` in a string or a nested item cannot end it
    /// early. Only an attribute that STARTS a line begins a module; the same
    /// text in a comment or a string does not (this function's own doc used to
    /// be matched first, which ended the strip before it began — measured by
    /// the scan flagging its own fixtures).
    fn strip_test_modules(text: &str) -> String {
        let mut out = Vec::new();
        // `Some(indent)` while inside a test module — the indentation of the
        // attribute, whose close is a `}` at that same indentation. A `fn`
        // under `#[cfg(test)]` has no module to end, so it is blanked only to
        // the end of its own item: the next line at the same indentation that
        // does not continue it. (A first version blanked to ANY `}` in column
        // zero, which for a `#[cfg(test)] fn` at column zero meant "to the end
        // of the file" — and a production literal below it was invisible.
        // Measured by the review.)
        let mut inside: Option<usize> = None;
        for line in text.lines() {
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            if inside.is_none() && trimmed.starts_with("#[cfg(test)]") {
                inside = Some(indent);
                out.push(String::new());
                continue;
            }
            match inside {
                Some(open) => {
                    out.push(String::new());
                    // Two ways an item ends, both checked on the line being
                    // blanked: a `}` at the item's own indentation (a `mod`
                    // written across lines, whose close carries its own
                    // trailing whitespace), or a line that opens and closes
                    // its braces itself (`fn h() { … }` on one line, or a
                    // `mod t { … }` written that way) — there is nothing left
                    // to wait for after it.
                    let closes_at_own_indent = trimmed.starts_with('}') && indent == open;
                    let self_contained = trimmed.ends_with('}')
                        && trimmed.matches('{').count() == trimmed.matches('}').count();
                    if closes_at_own_indent || self_contained {
                        inside = None;
                    }
                }
                None => out.push(line.to_owned()),
            }
        }
        out.join("\n")
    }

    /// `text` with every `/* … */` replaced by spaces (newlines kept), so what
    /// is left has the same shape and line numbers and no comment in it.
    ///
    /// Not a full lexer: it tracks `"…"` (with escapes) and line comments, so
    /// a `/*` inside a string is text. It does not track raw strings
    /// (`r#"…"#`) or byte/char literals; a `/*` inside one of those still
    /// starts a comment, which can only remove text and never add a code, so
    /// the failure direction is "a code below it is missed", not "a code is
    /// invented". The tree has no such literal today.
    fn strip_block_comments(text: &str) -> String {
        // Walk the text once, tracking whether we are inside a string literal
        // or a char literal, so a `/*` that is part of a string is text and
        // not a comment opener. (A first version did not, and a `/*` in a
        // string blanked the rest of the file — measured by this function's
        // own test.) Escapes are honoured inside a literal; a lifetime `'a`
        // looks like the start of a char literal and is ended by the next
        // `'` or by a non-char character, which is harmless here because a
        // lifetime is never followed by `/*` in a way that matters.
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut index = 0usize;
        while index < chars.len() {
            let ch = chars[index];
            let next = chars.get(index + 1).copied();
            // A string literal: copied verbatim until its unescaped quote.
            if ch == '"' {
                out.push(ch);
                index += 1;
                while index < chars.len() {
                    let c = chars[index];
                    out.push(c);
                    index += 1;
                    if c == '\\' {
                        if let Some(escaped) = chars.get(index) {
                            out.push(*escaped);
                            index += 1;
                        }
                    } else if c == '"' {
                        break;
                    }
                }
                continue;
            }
            // A line comment: verbatim to the end of the line (the scan strips
            // it later, and a `/*` inside one is not an opener either).
            if ch == '/' && next == Some('/') {
                while index < chars.len() && chars[index] != '\n' {
                    out.push(chars[index]);
                    index += 1;
                }
                continue;
            }
            if ch == '/' && next == Some('*') {
                // A block comment: blank to its close, newlines kept.
                let start = index;
                index += 2;
                while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                    index += 1;
                }
                let end = if index + 1 < chars.len() {
                    index + 2
                } else {
                    chars.len()
                };
                for c in &chars[start..end] {
                    out.push(if *c == '\n' { '\n' } else { ' ' });
                }
                index = end;
                continue;
            }
            out.push(ch);
            index += 1;
        }
        out
    }

    /// The declarations in a `rpc_error_codes` body, by name.
    ///
    /// The loop lives here once; [`our_codes`] hands it this module's own
    /// body and [`the_parser_reads_what_rust_accepts`] hands it crafted ones,
    /// so a parser change is seen by both. (It used to be a copy in the test,
    /// with a comment claiming the two were "kept in step by construction" —
    /// they were not, and nothing went red when they drifted.)
    ///
    /// What counts as a declaration is deliberately loose, because the first
    /// parser was strict in exactly the ways a real edit gets past it (all
    /// measured, all green at the time). `pub(crate) const` and bare `const`
    /// are declarations too, not just `pub const`, and spacing is whatever
    /// `rustfmt` did not normalise. The module ends at a `}` **in column
    /// zero**, not at any `}`: a helper fn or a block-valued const inside the
    /// module has indented closing braces of its own, and stopping at the
    /// first of those hid every code after it.
    ///
    /// A value that is neither a number nor an earlier name in the same body
    /// is refused by name, not skipped: a code this cannot read is exactly the
    /// one that must not slip past.
    fn parse_codes(body: &str) -> BTreeMap<String, i64> {
        let mut codes = BTreeMap::new();
        for line in body.lines() {
            if line == "}" {
                break; // the module's closing brace, and only in column zero
            }
            let trimmed = line.trim_start();
            // A declaration line: optional visibility, `const`, a name, `:`, a
            // type, `=`, a value, `;`. Matched on the words, not on spacing.
            let Some((head, value)) = trimmed.split_once('=') else {
                continue;
            };
            let words: Vec<&str> = head.split_whitespace().collect();
            let Some(name_index) = words.iter().position(|word| *word == "const") else {
                continue;
            };
            let Some(name) = words.get(name_index + 1) else {
                continue;
            };
            // The name carries the `: i64` on its shoulder when spacing is tight.
            let name = name.split(':').next().unwrap_or(name);
            let value = value.trim().trim_end_matches(';').trim();
            let number = value.parse::<i64>().unwrap_or_else(|_| {
                codes.get(value).copied().unwrap_or_else(|| {
                    panic!("`{name}` holds `{value}`, which this guard cannot read")
                })
            });
            codes.insert(name.to_owned(), number);
        }
        codes
    }

    /// Every code this service publishes, **parsed out of this module's own
    /// source** rather than listed here by hand.
    ///
    /// The hand-written list this test started as could not fail: the mutation
    /// that adds a new constant squatting on a plana number simply was not in
    /// the list, so the guard passed (measured — it did). Reading the source
    /// means a constant added tomorrow is covered without anyone remembering
    /// this file.
    fn our_codes() -> BTreeMap<String, i64> {
        const HEADER: &str = "pub mod rpc_error_codes {";
        let source = include_str!("rpc.rs");
        let start = source.find(HEADER).expect("the module this test is about");
        parse_codes(&source[start + HEADER.len()..])
    }

    /// Every code the plana crates this service links assign, by the name they
    /// publish it under — read from the crates, not copied as literals, so a
    /// family reassignment is a compile error here instead of a stale number.
    ///
    /// Two modules, not one. The first version of this list covered only
    /// `plana::jsonrpc::error_codes` and called itself "the plana family"; but
    /// this service also links `plana-rpc-server`, whose
    /// `ext_error_codes::SERVICE_BUSY` (`-32050`) is emitted on this service's
    /// own `/api/rpc` mount. A squat on `-32050` passed the one-module guard
    /// (measured). Every `-3xxxx` constant in every plana crate in the tree
    /// belongs here; if a third module appears, it is a line in this function.
    fn plana_codes() -> BTreeMap<&'static str, i64> {
        use plana::jsonrpc::error_codes as family;
        use plana_rpc_server::ext_error_codes as server;
        BTreeMap::from([
            ("PARSE_ERROR", family::PARSE_ERROR),
            ("INVALID_REQUEST", family::INVALID_REQUEST),
            ("METHOD_NOT_FOUND", family::METHOD_NOT_FOUND),
            ("INVALID_PARAMS", family::INVALID_PARAMS),
            ("INTERNAL_ERROR", family::INTERNAL_ERROR),
            ("SNAPSHOT_FAILED", family::SNAPSHOT_FAILED),
            ("AGENT_UNAVAILABLE", family::AGENT_UNAVAILABLE),
            ("CONTAINER_ERROR", family::CONTAINER_ERROR),
            ("REPL_ERROR", family::REPL_ERROR),
            ("AUTH_ERROR", family::AUTH_ERROR),
            ("OPS_UNKNOWN", family::OPS_UNKNOWN),
            ("OPS_EXPIRED", family::OPS_EXPIRED),
            ("OPS_CAPACITY", family::OPS_CAPACITY),
            ("OPS_CANCELLED", family::OPS_CANCELLED),
            ("SERVICE_BUSY", server::SERVICE_BUSY),
            ("DISPATCH_STALLED", server::DISPATCH_STALLED),
        ])
    }

    /// The one name pair both sides define, on purpose, with one meaning.
    const SHARED_ON_PURPOSE: (&str, &str) = ("AUTH", "AUTH_ERROR");

    /// The source parser has to actually find the table, or the guard below
    /// passes by finding nothing — and it has to keep finding the last code
    /// after someone adds a helper or a block-valued const above it.
    #[test]
    fn the_guard_reads_the_whole_error_code_table() {
        let codes = our_codes();
        for expected in [
            "AUTH",
            "NOT_FOUND",
            "CONFLICT",
            "TOO_LARGE",
            "OPS_DISABLED",
            "STORAGE_UNAVAILABLE",
        ] {
            assert!(
                codes.contains_key(expected),
                "the parser missed `{expected}`: {codes:?}"
            );
        }
        // The parsed numbers have to be the numbers the crate compiles with.
        assert_eq!(codes["AUTH"], super::rpc_error_codes::AUTH);
        assert_eq!(
            codes["STORAGE_UNAVAILABLE"],
            super::rpc_error_codes::STORAGE_UNAVAILABLE
        );
    }

    /// The parser is not fooled by the edit shapes that got past its first
    /// version. Each case is a real declaration or a real distractor; the
    /// expectations say which, so a parser that starts skipping real ones or
    /// inventing false ones goes red here rather than silently in the guard.
    #[test]
    fn the_parser_reads_what_rust_accepts() {
        let parse = |body: &str| Some(parse_codes(body));

        // Tight spacing is a declaration.
        assert_eq!(
            parse("    pub const A:i64=-32001;\n}").unwrap()["A"],
            -32001
        );
        // `pub(crate)` and bare `const` are declarations.
        assert_eq!(
            parse("    pub(crate) const A: i64 = -32002;\n}").unwrap()["A"],
            -32002
        );
        assert_eq!(parse("    const A: i64 = -32003;\n}").unwrap()["A"], -32003);
        // An alias of an earlier name in the same module resolves.
        let alias =
            parse("    pub const BASE: i64 = -32010;\n    pub const SAME = BASE;\n}").unwrap();
        assert_eq!(alias["SAME"], -32010);
        // A doc comment, a blank line and an attribute are not declarations.
        assert!(
            parse("    /// not one\n\n    #[allow(dead_code)]\n    pub const A: i64 = -1;\n}")
                .unwrap()["A"]
                == -1
        );
        // A helper fn inside the module has indented closing braces: the scan
        // must continue past them and still find what comes after.
        let with_helper = "    pub const A: i64 = -32006;\n    pub fn name_of(code: i64) -> &'static str {\n        match code {\n            -32006 => \"not found\",\n            _ => \"?\",\n        }\n    }\n    pub const B: i64 = -32001;\n}";
        let parsed = parse(with_helper).unwrap();
        assert_eq!(
            parsed["B"], -32001,
            "the code after the helper is read: {parsed:?}"
        );
        // A block-valued const is refused, loudly. The guard cannot read the
        // number out of an expression, and a code it cannot read is exactly
        // the one that must not slip past: the scan stops with a message that
        // names the constant, rather than counting it as something or moving
        // on. (An `fn` is fine — see the helper case above — because a `fn`
        // line has no `=`.)
        let block = "    pub const A: i64 = {\n        let base = -32011;\n        base\n    };\n    pub const B: i64 = -32001;\n}";
        let result = std::panic::catch_unwind(|| parse(block));
        let message = result
            .err()
            .and_then(|payload| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        assert!(
            message.contains("`A` holds") && message.contains("cannot read"),
            "a block-valued const names itself: {message:?}"
        );
    }

    /// A service code must never quietly reuse a number the plana crates have
    /// already assigned.
    ///
    /// The MCP lane used to answer its scope refusal with a private `-32001`,
    /// which plana assigns to `SNAPSHOT_FAILED` — so "your snapshot could not
    /// be taken" and "your key may not upload" arrived as the same code, and
    /// a client had no way to tell them apart. The fix moved that lane onto
    /// `AUTH`; this test is what keeps the next invented number out of the
    /// family's block. (The MCP lane's refusal rides `rpc_error_codes::AUTH`
    /// through its own alias; the wire is pinned by `tests/routes.rs`'s
    /// `mcp_tool_calls_require_the_mapped_permission`, which compares against
    /// that constant and not a copy of the number.)
    ///
    /// Two of this service's own codes never share a number.
    ///
    /// The plana comparison is about the family's block; this is about the
    /// table itself. Two names on one number is the collision this PR exists
    /// to remove, and a `pub(super) const` added inside the module (a form the
    /// review found the first parser's word match reads perfectly well) sailed
    /// through every test because nothing compared the service's codes with
    /// each other — only with plana's.
    #[test]
    fn no_two_service_codes_share_a_number() {
        let ours = our_codes();
        let mut by_number: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
        for (name, number) in &ours {
            by_number.entry(*number).or_default().push(name);
        }
        let clashes: Vec<String> = by_number
            .iter()
            .filter(|(_, names)| names.len() > 1)
            .map(|(number, names)| format!("{number}: {names:?}"))
            .collect();
        assert!(
            clashes.is_empty(),
            "two service codes answer with one number: {}",
            clashes.join("; ")
        );
    }

    #[test]
    fn a_service_code_never_quietly_reuses_a_plana_code() {
        let ours = our_codes();
        let theirs = plana_codes();
        for (our_name, our) in &ours {
            for (their_name, their) in &theirs {
                if (our_name.as_str(), *their_name) == SHARED_ON_PURPOSE {
                    continue;
                }
                assert_ne!(
                    our, their,
                    "our {our_name} ({our}) reuses plana's {their_name} ({their}): \
                     two different failures would answer with one code"
                );
            }
        }
    }

    /// The deliberate share is deliberate, and still shares the *meaning*:
    /// `AUTH` may equal `AUTH_ERROR` only because both mean "auth or
    /// permission failure". If the family moves its permission code, this
    /// fails and the share has to be re-decided rather than drift onto a
    /// number that means something else.
    #[test]
    fn the_one_shared_code_is_the_one_we_mean() {
        assert_eq!(
            super::rpc_error_codes::AUTH,
            plana::jsonrpc::error_codes::AUTH_ERROR,
            "AUTH is the fleet's permission code; the share is by meaning, not by number"
        );
    }

    /// No error code in the extension range is written as a bare literal.
    ///
    /// The guards above read `rpc_error_codes`, and that is all they read: a
    /// constant declared *outside* the module — at file scope in `mcp.rs`,
    /// say — was invisible to them (measured: `pub const LOOSE_CODE: i64 =
    /// -32001;` right after the module passed every test). That is exactly the
    /// shape the collision had: the MCP lane's `SCOPE_DENIED` was a file-scope
    /// `const` in another file. So the scan is the whole crate's `src/`, for
    /// any integer literal in the plana range or this service's, wherever it
    /// is written — a declaration, a call argument, a match arm.
    ///
    /// What is allowed: the numbers inside `rpc_error_codes` itself (the table
    /// is the one place they belong), and the spec's own `-3260x`/`-32700`
    /// codes, which are JSON-RPC's and not anybody's extension.
    #[test]
    fn no_extension_error_code_is_written_outside_the_table() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut queue = vec![root];
        while let Some(dir) = queue.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    queue.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    files.push(path);
                }
            }
        }
        files.sort();
        let table = our_codes();
        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("source is UTF-8");
            // Strip comments: a doc comment may quote a code to explain it.
            // Comments first, and block comments before line ones: a `/* … */`
            // can run over lines and hold a `}` in column zero, and the table
            // strip below ends at the first of those — so a block comment
            // sitting above the table hid every code after it (measured by the
            // review: a `-32001` in the comment was flagged, a real one after
            // it was not). Stripped to spaces, not removed, so line numbers
            // still name where an offender is.
            let stripped: String = strip_block_comments(&text);
            let code: String = stripped
                .lines()
                .map(|line| line.split("//").next().unwrap_or("").to_owned())
                .collect::<Vec<_>>()
                .join("\n");
            // Test-only code is not the wire: a `#[cfg(test)]` module's
            // fixtures are free to write a code as a literal (this file's own
            // parser tests do). Each module is blanked to its closing `}` in
            // column zero — not to the end of the file, which would skip any
            // production item after it (`config.rs` has three test modules
            // with production above and between them).
            let code = strip_test_modules(&code);
            // Skip the table itself, wherever it sits in its file: from its
            // header to the column-zero `}` that `parse_codes` would stop at.
            // (The table's numbers are the one place they belong, and the
            // guards above have already read them.)
            let body = match code.find("pub mod rpc_error_codes {") {
                Some(start) => {
                    let after = &code[start..];
                    let end = after
                        .lines()
                        .enumerate()
                        .find(|(index, line)| *index > 0 && *line == "}")
                        .map(|(index, _)| {
                            start
                                + after
                                    .lines()
                                    .take(index)
                                    .map(|l| l.len() + 1)
                                    .sum::<usize>()
                        })
                        .unwrap_or(code.len());
                    format!("{}{}", &code[..start], &code[end..])
                }
                None => code.clone(),
            };
            for (index, line) in body.lines().enumerate() {
                for token in line.split(|c: char| !(c.is_ascii_digit() || c == '-')) {
                    // The sign is part of the number: parse the whole token.
                    // (A first version stripped the `-` and parsed what was
                    // left, so `-32001` read as `32001`, which is in nobody's
                    // range — the scan was green on everything, including its
                    // own fixtures. Found by instrumenting the gate: it never
                    // ran for a code it was visibly handed.)
                    let Ok(number) = token.parse::<i64>() else {
                        continue;
                    };
                    let in_plana_range = (-32099..=-32000).contains(&number);
                    let ours = table.values().any(|known| *known == number);
                    if in_plana_range && !ours {
                        offenders.push(format!(
                            "{}:{}: `{number}` is written as a literal and is not in the table",
                            file.display(),
                            index + 1
                        ));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "an error code belongs in `rpc_error_codes`, once:\n{}",
            offenders.join("\n")
        );
    }

    /// The two strippers survive their own shapes.
    ///
    /// Neither had a test, and the first version of each was bricked by a
    /// shape that was in the file: the comment stripper by a `/*` inside a
    /// string (it searched for `/*` without knowing strings), and the
    /// test-module stripper by a `#[cfg(test)]` mentioned in prose — it
    /// searched for the text anywhere in a line and bailed out when the item
    /// it found was not a module, leaving the rest of the file unstripped and
    /// the scan flagging its own fixtures. (An earlier version of this
    /// comment said the mention was in the stripper's own doc comment and was
    /// matched first; a doc line starts with `///`, so that was never how it
    /// happened. The shapes below are the ones that were real.)
    #[test]
    fn the_strippers_survive_their_own_shapes() {
        // A test module between two production items: only the module goes.
        let text = "pub const A: i64 = 1;\n#[cfg(test)]\nmod tests {\n    const FIXTURE: i64 = -32001;\n    fn f() {}\n}\n pub const B: i64 = 2;\n";
        let stripped = strip_test_modules(text);
        assert!(stripped.contains("pub const A"), "{stripped:?}");
        assert!(
            stripped.contains("pub const B"),
            "the item after the module is kept: {stripped:?}"
        );
        assert!(
            !stripped.contains("FIXTURE"),
            "the module's body is gone: {stripped:?}"
        );
        // The attribute mentioned in a doc comment is not a module start.
        let doc = "/// says `#[cfg(test)]` in prose\npub const C: i64 = 3;\n";
        let stripped = strip_test_modules(doc);
        assert!(stripped.contains("pub const C"), "{stripped:?}");
        // An item that opens and closes on its own line leaves nothing
        // behind: a `#[cfg(test)] fn h() { … }` and a one-line `mod` both
        // used to swallow every line after them (the stripper waited for a
        // close that had already gone by).
        for shape in [
            "#[cfg(test)]\nfn h() { let _ = 1; }\nconst K: i64 = -1;\n",
            "#[cfg(test)]\nmod t { const F: i64 = 1; }\nconst K: i64 = -1;\n",
        ] {
            let stripped = strip_test_modules(shape);
            assert!(
                stripped.contains("const K"),
                "the item after a self-contained one is kept: {stripped:?}"
            );
            assert!(!stripped.contains("#[cfg(test)]"), "{stripped:?}");
        }
        // A `/*` inside a string literal is not a comment opener.
        let literal = "pub const S: &str = \"a /* in a string\";\npub const D: i64 = 4;\n";
        let stripped = strip_block_comments(literal);
        assert!(stripped.contains("pub const D"), "{stripped:?}");
        // And a real block comment is removed, newlines and count intact.
        let commented = "pub const E: i64 = 5;\n/* a comment\n}\n*/\npub const F: i64 = 6;\n";
        let stripped = strip_block_comments(commented);
        assert!(!stripped.contains("a comment"), "{stripped:?}");
        assert!(stripped.contains("pub const F"), "{stripped:?}");
        assert_eq!(
            stripped.lines().count(),
            commented.lines().count(),
            "line count holds"
        );
    }

    /// Every `-32xxx` constant in the plana crates this service links is in
    /// the table above — counted from the crates' own sources.
    ///
    /// The tests above only know the constants they name, so a plana crate
    /// growing a new error-code module would be a silent gap. This pins the
    /// *number* of extension codes the linked crates publish: a new one makes
    /// this red, and adding it to `plana_codes` is then a deliberate act.
    ///
    /// It reads the sources through `cargo metadata`, not by guessing a path:
    /// the manifest_dirs are the files cargo compiled. If cargo cannot be run
    /// (no toolchain in the test environment) the test says so and fails —
    /// a guard that quietly skips is worse than one that is red.
    #[test]
    fn every_plana_error_code_is_in_the_table() {
        let metadata =
            std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .args(["metadata", "--format-version", "1", "--locked"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .output()
                .expect("cargo metadata runs");
        assert!(
            metadata.status.success(),
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr)
        );
        let json: serde_json::Value =
            serde_json::from_slice(&metadata.stdout).expect("cargo metadata is JSON");
        let packages = json["packages"].as_array().expect("packages");
        // What the crates DECLARE, by name and value — not just how many.
        // Counting alone let a wrong VALUE through: `OPS_EXPIRED + 3` kept the
        // count and passed (measured by the review). So each declaration is
        // looked up by name and compared by number.
        // Keyed by (crate, name): two crates declaring the same NAME with
        // different numbers must both be seen, not collapsed into one.
        let mut declared: BTreeMap<(String, String), i64> = BTreeMap::new();
        for package in packages {
            let name = package["name"].as_str().unwrap_or_default();
            // The plana crates that define codes: the family, and the server profile.
            // By prefix, not an allow-list: a new `plana-*` crate with error
            // codes is counted without anyone remembering to add its name.
            // (A plana crate that publishes no `-32xxx` const adds zero and
            // changes nothing, so this over-includes safely.)
            if !name.starts_with("plana") {
                continue;
            }
            let crate_name = name.to_owned();
            let manifest = Path::new(package["manifest_path"].as_str().expect("manifest_path"));
            let root = manifest.parent().expect("a manifest has a parent");
            let mut stack = vec![root.join("src")];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "rs") {
                        let text = std::fs::read_to_string(&path).expect("source is UTF-8");
                        // A `const NAME: i64 = -32000..=-32999;` line, spacing aside.
                        for line in text.lines() {
                            let t = line.trim_start();
                            let is_const = t.starts_with("pub const ")
                                || t.starts_with("pub(crate) const ")
                                || t.starts_with("const ");
                            if !is_const {
                                continue;
                            }
                            let Some((_, value)) = t.split_once('=') else {
                                continue;
                            };
                            let value = value.trim().trim_end_matches(';').trim();
                            if let Ok(number) = value.parse::<i64>() {
                                if (-32999..=-32000).contains(&number) {
                                    let name = t
                                        .split_once("const ")
                                        .and_then(|(_, rest)| rest.split_once(':'))
                                        .map(|(name, _)| name.trim().to_owned())
                                        .unwrap_or_default();
                                    declared.insert((crate_name.clone(), name), number);
                                }
                            }
                        }
                    }
                }
            }
        }
        let listed = plana_codes();
        // Every declaration, by its crate and name, with the number the crate
        // gives it — and the table is asked for the same name, so a NAME that
        // exists in one plana crate and not another cannot be quietly matched
        // to whichever table entry happens to share it.
        let mut wrong: Vec<String> = Vec::new();
        for ((crate_name, name), number) in &declared {
            match listed.get(name.as_str()) {
                None => wrong.push(format!(
                    "{crate_name}::{name} ({number}) is not in `plana_codes`"
                )),
                Some(&in_table) if in_table != *number => wrong.push(format!(
                    "{crate_name}::{name}: the crate says {number}, the table says {in_table}"
                )),
                Some(_) => {}
            }
        }
        assert!(
            wrong.is_empty(),
            "`plana_codes` must say of each code what its crate says:\n{}",
            wrong.join("\n")
        );
        // And nothing in the table that no crate declares — a stale entry
        // after a code is removed upstream. Declared names, flattened: the
        // same NAME in two crates is allowed (different codespaces), and any
        // of them satisfying the table's name is enough for this check.
        let declared_names: std::collections::HashSet<&str> =
            declared.keys().map(|(_, name)| name.as_str()).collect();
        let stale: Vec<&str> = listed
            .keys()
            .copied()
            .filter(|name| !declared_names.contains(name))
            .collect();
        assert!(
            stale.is_empty(),
            "`plana_codes` lists codes no linked plana crate declares: {stale:?}"
        );
        assert_eq!(
            declared.len(),
            listed.len(),
            "the plana crates declare {} extension codes; `plana_codes` lists {}",
            declared.len(),
            listed.len()
        );
    }
}
