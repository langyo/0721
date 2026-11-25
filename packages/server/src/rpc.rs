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

/// Extension error codes of this service (the plana standard family:
/// -32700..-32603 reserved, -32005 = auth failure; the codes below sit
/// in the service-assignment gap and map the REST statuses the former
/// handlers emitted).
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
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            plana::jsonrpc::error_codes::INVALID_PARAMS
        }
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

/// The wire form of `keys.create` / `keys.update` — the three fields a
/// key's owner writes. `keys.update` is a WHOLE-RECORD replacement, so it
/// carries the same three; an omitted `expires_at` clears the expiry.
#[derive(Debug, Clone, serde::Deserialize)]
struct KeyWriteParams {
    name: String,
    ops: Vec<i0721_proto::KeyOp>,
    #[serde(default)]
    expires_at: Option<String>,
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
                }
                let args: ListParams = params(&ctx)?;
                let user = user_of(&ctx);
                let scope = args.scope.as_deref().unwrap_or("own");
                // Members are pinned to their own tree: the timeline is
                // the admin-flavored global stream.
                let scope = if user.role != Role::Admin { "own" } else { scope };
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
                        .count_folder(user.id, args.folder_id, args.kind.clone())
                        .await
                        .map_err(store_err)?;
                    let items = state
                        .store
                        .list_folder(user.id, args.folder_id, args.kind, offset, limit)
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
                let total = state.store.count_files().await.map_err(store_err)?;
                let items = state
                    .store
                    .list_files(offset, limit)
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
                            .resolve_upload_bucket(None)
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
                // Owner-or-admin on the TARGET folder.
                let target = state.store.get_file(args.folder_id).await.map_err(store_err)?
                    .filter(|r| r.deleted_at.is_none() && r.kind == i0721_proto::FileKind::Folder)
                    .ok_or_else(|| ApiError::not_found("folder not found"))
                    .map_err(api_err)?;
                if user.role != Role::Admin && target.owner_id != user.id {
                    return Err(api_err(ApiError::forbidden("not your folder")));
                }
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

                let key = crate::mcp::mint_api_key();
                let hash = crate::mcp::hash_api_key(&key);
                let prefix = crate::mcp::mask_api_key(&key);
                let record = i0721_proto::ApiKeyRecord {
                    id: Uuid::new_v4(),
                    user_id: caller.id,
                    name,
                    key_prefix: prefix,
                    key_hash: hash,
                    scopes: crate::key_scopes::encode_scopes(ops),
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
                // Whole-record replacement: all three fields, and an
                // omitted `expires_at` CLEARS the expiry.
                let (name, ops, expires_at) = validate_key_write(&args.write)?;
                let updated = state
                    .store
                    .update_api_key(
                        key.id,
                        &name,
                        &crate::key_scopes::encode_scopes(ops),
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
                        i0721_proto::ApiKeyPublic {
                            id: r.id,
                            name: r.name,
                            key_prefix: r.key_prefix,
                            ops: crate::key_scopes::ordered_ops(&set),
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
        .build()
}
