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
    validate_display_name, validate_file_name, validate_username, FileDto, NewUser, PageArgs,
    PageResult, Role, SiteConfigStrict, UserDto, CONFIG_SETTING_KEY,
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
                let args: PageArgs = params(&ctx)?;
                let (offset, limit) = args.sanitize();
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
                require_file_owner_or_admin(&state, &user, params.id).await?;
                validate_file_name(&params.name)
                    .map_err(|err| ApiError::bad_request(err.to_string()))
                    .map_err(api_err)?;
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
                // The gate's fetch doubles as the pre-delete snapshot
                // (the REST handler's pattern).
                let record = require_file_owner_or_admin(&state, &user, params.id).await?;
                let removed = state
                    .store
                    .soft_delete_file(params.id)
                    .await
                    .map_err(store_err)?;
                if !removed {
                    return Err(api_err(ApiError::not_found("file not found")));
                }
                if state
                    .store
                    .count_live_files_by_storage_key(&record.storage_key)
                    .await
                    .map_err(store_err)?
                    == 0
                {
                    // Compare-and-restore against a racing upload of the
                    // same content (the REST handler's guard, verbatim).
                    let snapshot = state
                        .blobs
                        .get(BlobSlot::Media, &record.storage_key)
                        .await
                        .map_err(blob_err)?;
                    let thumb_key = record.thumbnail_key.clone();
                    let thumb_snapshot = match thumb_key.as_deref() {
                        Some(key) => state
                            .blobs
                            .get(BlobSlot::Cache, key)
                            .await
                            .map_err(blob_err)?,
                        None => None,
                    };
                    state
                        .blobs
                        .delete(BlobSlot::Media, &record.storage_key)
                        .await
                        .map_err(blob_err)?;
                    if let Some(key) = thumb_key.as_deref() {
                        state
                            .blobs
                            .delete(BlobSlot::Cache, key)
                            .await
                            .map_err(blob_err)?;
                    }
                    if state
                        .store
                        .count_live_files_by_storage_key(&record.storage_key)
                        .await
                        .map_err(store_err)?
                        > 0
                    {
                        if let Some(bytes) = snapshot {
                            state
                                .blobs
                                .put(BlobSlot::Media, &record.storage_key, &bytes)
                                .await
                                .map_err(blob_err)?;
                        }
                        if let (Some(key), Some(bytes)) = (thumb_key.as_deref(), thumb_snapshot) {
                            state
                                .blobs
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
        .build()
}
