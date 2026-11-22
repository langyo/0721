//! The MCP (Model Context Protocol) surface: API-key authenticated SSE
//! transport + JSON-RPC tool calls for AI agents.
//!
//! Transport (the chest mcp_media shape, adapted):
//! - `GET /api/mcp/sse` — Bearer API key; SSE stream; first frame
//!   `event: endpoint` data=`/api/mcp/message?sessionId=<uuid>`; 25 s
//!   keepalive `: ping`; sessions in-memory TTL 30 min, cap 512.
//! - `POST /api/mcp/message?sessionId=` — 202; the JSON-RPC response is
//!   pushed back through the corresponding SSE stream.
//!
//! Session lifecycle (the W7 hardening): a session is removed the moment
//! its stream is dropped (`SessionGuard` — the disconnect hook), the
//! stream ENDS at its `expires_at` instead of squatting on a zombie
//! keepalive, and every message re-checks that the session's API key is
//! still live — revoking a key kills the sessions it opened, not just
//! future handshakes.
//!
//! The tool surface (five tools) rides the STORE's existing per-user
//! permission face: no admin elevation, the key's owner is the acting
//! user for every call. `upload_file` runs the SAME chain as the REST
//! multipart lane ([`crate::api::upload_core::run_upload`]) — quota,
//! folder ownership, the upload semaphore, the configured pipeline
//! parameters and the configured storage tag included.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::mpsc;
use uuid::Uuid;

use i0721_proto::{FileDto, NewShare};

use crate::api::upload_core::{run_upload, UploadInput};
use crate::error::ApiError;
use crate::state::AppState;

/// Session TTL and cap (chest-matched).
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);
const SESSION_CAP: usize = 512;
const KEEPALIVE: Duration = Duration::from_secs(25);

/// The API key prefix.
pub const KEY_PREFIX: &str = "i0721-";

/// Mint a fresh API key: `i0721-<32 hex>` (~122 random bits from a v4 UUID).
#[must_use]
pub fn mint_api_key() -> String {
    format!("{KEY_PREFIX}{}", Uuid::new_v4().simple())
}

/// SHA-256 hex hash of a full key (for storage; the plaintext is never
/// stored).
#[must_use]
pub fn hash_api_key(key: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(key.as_bytes()))
}

/// The visible prefix for masked listing: first 12 characters + `****`.
#[must_use]
pub fn mask_api_key(key: &str) -> String {
    let visible = key.chars().take(12).collect::<String>();
    format!("{visible}****")
}

/// The live-key lookup: hash → non-revoked row. Side-effect free — the
/// handshake adds the last-used refresh, the per-message re-check
/// deliberately does not (one write per SSE frame would be noise).
/// The hash lookup is an indexed equality — timing leaks nothing about
/// which key matched (all non-matches are the same branch); the
/// comparison itself is the SHA-256 digest, not the key, so constant-time
/// comparison of the digest is unnecessary (the lookup key IS the digest).
async fn lookup_live_key(
    state: &AppState,
    key_hash: &str,
) -> Result<i0721_proto::ApiKeyRecord, ApiError> {
    state
        .store
        .find_api_key_by_hash(key_hash)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("invalid API key"))
}

/// Verify an API key at the handshake: the live lookup plus a best-effort
/// `last_used_at` refresh (spawned, never blocks the call).
pub(crate) async fn verify_api_key(
    state: &AppState,
    raw_key: &str,
) -> Result<i0721_proto::ApiKeyRecord, ApiError> {
    let record = lookup_live_key(state, &hash_api_key(raw_key)).await?;
    let store = state.store.clone();
    let id = record.id;
    tokio::spawn(async move {
        let _ = store.touch_api_key(id).await;
    });
    Ok(record)
}

// ── SSE session management ─────────────────────────────────────────────

struct McpSession {
    /// The authenticated user (the key owner at handshake time).
    user_id: Uuid,
    /// The HASH of the key that opened this session — the re-check handle.
    ///
    /// Why the hash and not `api_key_id` + a new `get_api_key(id)` store
    /// method: the hash is already the unique-indexed lookup key
    /// (`find_api_key_by_hash`), and that query already filters revoked
    /// rows — so the re-check needs NO new store surface, no schema
    /// change and no second revocation predicate to keep in sync. It is
    /// also no more sensitive than the database row, which stores exactly
    /// this digest. The plaintext key never enters the session.
    key_hash: String,
    /// The response channel (SSE writes).
    tx: mpsc::Sender<String>,
    /// Hard TTL end for BOTH sides: the stream stops here and the table
    /// stops re-admitting it here (no sliding window — chest-matched).
    expires_at: Instant,
}

/// In-memory session table (bounded by SESSION_CAP; pruned on insert).
///
/// The lock is a `std::sync::Mutex`, not `tokio`'s: every critical
/// section is a few HashMap operations with no `.await` inside, and the
/// synchronous `Drop` hook below MUST be able to take it (a `tokio` lock
/// cannot be acquired from a `Drop`).
#[derive(Clone)]
pub struct McpSessions {
    inner: Arc<std::sync::Mutex<HashMap<String, Arc<McpSession>>>>,
}

impl McpSessions {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Borrow the table. Poisoning only means a panic unwound while the
    /// map was borrowed; the map itself is still structurally sound, so
    /// recover it instead of cascading the panic into every later session.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<McpSession>>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a session. `false` when the cap is already reached (the
    /// caller answers 429). Expired entries are reclaimed here — there is
    /// no background sweeper: the streams end themselves at `expires_at`,
    /// and this sweep only catches holders that vanished without a Drop.
    fn insert(&self, session_id: String, session: Arc<McpSession>) -> bool {
        let mut sessions = self.lock();
        let now = Instant::now();
        sessions.retain(|_, s| s.expires_at > now);
        if sessions.len() >= SESSION_CAP {
            return false;
        }
        sessions.insert(session_id, session);
        true
    }

    /// One live session (a clone of the `Arc` — the caller drops the lock
    /// before it awaits anything). An entry past its `expires_at` is dead
    /// even when nothing has swept it yet: the TTL is enforced HERE as
    /// well as on the stream, so a client that lost its stream cannot
    /// keep POSTing against a table entry the sweep has not reached.
    fn get(&self, session_id: &str) -> Option<Arc<McpSession>> {
        let sessions = self.lock();
        let session = sessions.get(session_id)?;
        (session.expires_at > Instant::now()).then(|| Arc::clone(session))
    }

    /// Drop a session from the table (the Drop hook + the revocation
    /// path). Idempotent: a missing entry is not an error.
    fn remove(&self, session_id: &str) {
        self.lock().remove(session_id);
    }
}

impl Default for McpSessions {
    fn default() -> Self {
        Self::new()
    }
}

/// The disconnect hook, and the ONLY reliable one.
///
/// This guard is a local INSIDE the stream body, so it dies exactly when
/// the body does: the client goes away → axum drops the response body →
/// the `async_stream` generator is dropped → the guard drops → the
/// session leaves the table immediately instead of squatting until the
/// next insert's sweep. (The chest precedent wraps the stream in a
/// `Drop` impl; a guard local is the same hook without a manual `Stream`
/// impl to keep in sync.)
struct SessionGuard {
    session_id: String,
    sessions: McpSessions,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.sessions.remove(&self.session_id);
    }
}

// ── the SSE endpoint ───────────────────────────────────────────────────

/// `GET /api/mcp/sse` — the SSE transport handshake.
pub async fn sse(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Bearer API key authentication.
    let raw_key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let key_record = match verify_api_key(&state, raw_key).await {
        Ok(record) => record,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"invalid API key"}"#,
            )
                .into_response()
        }
    };
    // The handle the message lane re-verifies this session against.
    let key_hash = hash_api_key(raw_key);

    let session_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let expires_at = Instant::now() + SESSION_TTL;

    // Register the session.
    let registered = state.mcp_sessions.insert(
        session_id.clone(),
        Arc::new(McpSession {
            user_id: key_record.user_id,
            key_hash,
            tx: tx.clone(),
            expires_at,
        }),
    );
    if !registered {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"session limit reached"}"#,
        )
            .into_response();
    }

    // The guard and the table handle are cloned OUT of `state` so the
    // stream body owns them for its whole life (it must be `'static`).
    let sessions = state.mcp_sessions.clone();
    let guard_id = session_id.clone();

    // SSE stream.
    let stream = async_stream::stream! {
        // The disconnect hook: dropped with the body, i.e. the moment the
        // client goes away.
        let _guard = SessionGuard {
            session_id: guard_id,
            sessions,
        };
        // First frame: the endpoint event.
        yield Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default()
            .event("endpoint")
            .data(format!("/api/mcp/message?sessionId={session_id}")));
        // Keepalive + message pump.
        loop {
            // TTL enforcement at the TOP of every iteration, not only on
            // the keepalive tick: a session that keeps receiving frames
            // abandons each fresh `sleep` before it elapses, so a check
            // living in the sleep branch would never run for a busy
            // session. Here it runs once per frame and once per ping (at
            // most KEEPALIVE late), and the stream ENDS rather than
            // pinging a zombie until the client notices.
            if Instant::now() >= expires_at {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(KEEPALIVE) => {
                    yield Ok(axum::response::sse::Event::default().comment("ping"));
                }
                msg = rx.recv() => {
                    match msg {
                        Some(data) => yield Ok(axum::response::sse::Event::default()
                            .event("message")
                            .data(data)),
                        // Every sender is gone: the session was removed
                        // (revoked key) — end the stream.
                        None => break,
                    }
                }
            }
        }
    };

    axum::response::sse::Sse::new(stream).into_response()
}

/// `POST /api/mcp/message?sessionId=` — the JSON-RPC call lane.
pub async fn message(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let Some(session_id) = params.get("sessionId") else {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"missing sessionId"}"#,
        )
            .into_response();
    };

    // Session lookup: the session carries the authenticated user.
    let Some(session) = state.mcp_sessions.get(session_id) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"unknown or expired session"}"#,
        )
            .into_response();
    };

    // Key re-verification. The session is NOT a permanent credential: a
    // key revoked after the handshake must stop working NOW, not when the
    // 30-minute TTL lapses. The session stores the key's hash, so this is
    // the same unique-indexed lookup the handshake paid — one read per
    // JSON-RPC frame, far cheaper than the tool dispatch it guards.
    //
    // A store FAILURE is not a revocation: it fails this request (500)
    // and leaves the session alone, so a database blip does not sign
    // every agent out of its stream.
    match state.store.find_api_key_by_hash(&session.key_hash).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            // Revoked. Drop the entry: this `Arc` (plus the stream's
            // clone of the sender) is the only holder, so the SSE stream
            // ends as soon as it falls out of scope here — the client
            // cannot keep riding it.
            state.mcp_sessions.remove(session_id);
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"this session's API key was revoked"}"#,
            )
                .into_response();
        }
        Err(err) => return ApiError::from(err).into_response(),
    }

    // Parse and dispatch the JSON-RPC request.
    let request: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(err) => {
            let response = json!({
                "jsonrpc": "2.0", "id": null,
                "error": {"code": -32700, "message": format!("parse error: {err}")}
            });
            let _ = session.tx.send(response.to_string()).await;
            return StatusCode::ACCEPTED.into_response();
        }
    };

    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_owned();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    drop(request); // the spawn owns method + params

    // Dispatch in a background task; the response goes through the SSE.
    let state = state.clone();
    let user_id = session.user_id;
    let tx = session.tx.clone();
    tokio::spawn(async move {
        let result = dispatch_tool(&state, user_id, &method, &params).await;
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        let _ = tx.send(response.to_string()).await;
    });

    StatusCode::ACCEPTED.into_response()
}

// ── the tool dispatch ──────────────────────────────────────────────────

async fn dispatch_tool(state: &AppState, user_id: Uuid, method: &str, params: &Value) -> Value {
    match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "0721", "version": env!("CARGO_PKG_VERSION")},
        }),
        "tools/list" => json!({"tools": [
            {"name": "list_files", "description": "List files in a folder (or root). Params: folder_id (optional), offset, limit"},
            {"name": "file_info", "description": "Get one file's metadata. Params: id"},
            {"name": "upload_file", "description": "Upload a file. Params: content_base64, name, folder_id (optional)"},
            {"name": "get_share_link", "description": "Get or create a public share link. Params: file_id"},
            {"name": "search_files", "description": "Search files by name substring. Params: query, kind (optional), limit"},
        ]}),
        "tools/call" => {
            let tool = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(state, user_id, tool, &args).await {
                Ok(result) => json!({"content": [{"type": "text", "text": result.to_string()}]}),
                Err(err) => json!({
                    "content": [{"type": "text", "text": err}],
                    "isError": true,
                }),
            }
        }
        _ => json!({"error": {"code": -32601, "message": format!("method not found: {method}")}}),
    }
}

async fn call_tool(
    state: &AppState,
    user_id: Uuid,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    match tool {
        "list_files" => {
            let folder_id = args
                .get("folder_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            let rows = state
                .store
                .list_folder(user_id, folder_id, None, 0, 100)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<FileDto> = rows.into_iter().map(FileDto::from).collect();
            Ok(serde_json::to_value(items).unwrap_or_default())
        }
        "file_info" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or("missing or invalid 'id'")?;
            let record = state
                .store
                .get_file(id)
                .await
                .map_err(|e| e.to_string())?
                .filter(|r| r.deleted_at.is_none() && r.owner_id == user_id)
                .ok_or("file not found")?;
            Ok(serde_json::to_value(FileDto::from(record)).unwrap_or_default())
        }
        "upload_file" => {
            let content_b64 = args
                .get("content_base64")
                .and_then(|v| v.as_str())
                .ok_or("missing 'content_base64'")?;
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("missing 'name'")?;
            let folder_id = args
                .get("folder_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD
                .decode(content_b64)
                .map_err(|e| format!("bad base64: {e}"))?;
            let user = state
                .store
                .get_user(user_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("user not found")?;
            // The SAME chain the REST multipart lane runs: quota, folder
            // ownership, the upload semaphore, the configured size/WebP
            // pipeline parameters and the configured storage-backend tag
            // all apply here (this tool used to skip every one of them and
            // hardcode "local").
            // `name` is the SOURCE name (the pipeline's MIME hint and the
            // row's `original_name`); the display name resolves through
            // the site config exactly as the REST lane's does — with
            // `upload.use_source_file_name` off (the default) the row is
            // named by the content hash, the behavior this tool had.
            let uploaded = run_upload(
                state,
                &user,
                UploadInput {
                    name: None,
                    source_name: Some(name.to_owned()),
                    folder_id,
                    data: data.into(),
                },
            )
            .await
            .map_err(|err| err.error)?;
            Ok(serde_json::to_value(FileDto::from(uploaded.record)).unwrap_or_default())
        }
        "get_share_link" => {
            let file_id = args
                .get("file_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or("missing or invalid 'file_id'")?;
            let file = state
                .store
                .get_file(file_id)
                .await
                .map_err(|e| e.to_string())?
                .filter(|r| r.deleted_at.is_none() && r.owner_id == user_id)
                .ok_or("file not found")?;
            let share = state
                .store
                .create_share(NewShare {
                    file_id: file.id,
                    owner_id: user_id,
                    expires_at: None,
                    allow_preview: true,
                })
                .await
                .map_err(|e| e.to_string())?;
            let origin = state
                .config
                .public_origin
                .clone()
                .unwrap_or_else(|| "http://localhost".to_owned());
            Ok(json!({"url": format!("{origin}/s/{}", share.token)}))
        }
        "search_files" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let kind = args
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned());
            let rows = state
                .store
                .list_folder(user_id, None, kind, 0, 500)
                .await
                .map_err(|e| e.to_string())?;
            let items: Vec<FileDto> = rows
                .into_iter()
                .filter(|r| r.name.contains(query) || r.original_name.contains(query))
                .map(FileDto::from)
                .collect();
            Ok(serde_json::to_value(items).unwrap_or_default())
        }
        _ => Err(format!("unknown tool: {tool}")),
    }
}
