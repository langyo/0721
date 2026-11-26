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
//! future handshakes. "Live" now covers EXPIRY too (`expires_at`): an
//! expired key cannot open a stream, and a session whose key expires
//! mid-flight is removed and refused on its next frame.
//!
//! Permissions: every key carries a per-key operation set (the
//! [`crate::key_scopes`] vocabulary — `read`, `upload`, `share`).
//! `tools/list` advertises only the tools the key is allowed to call, and
//! `tools/call` refuses a tool outside that set with the plana family's
//! JSON-RPC permission code `-32005` (a PROTOCOL-level refusal, not an
//! `isError` result: the call never ran). `initialize` is never
//! permission-gated.
//!
//! The tool surface (five tools) rides the STORE's existing per-user
//! permission face: the key's owner is the acting user for every call, and a
//! key is never given more than its owner has (it is given less, by its own
//! ops and `buckets`). Within those it acts as its owner, so an admin's key
//! does what that admin would (`upload_file` targets any user's folder, and
//! `get_share_link` reaches any user's file — the share BIT is not asked of
//! an admin, on this lane or on RPC). `upload_file` runs the SAME chain as the REST
//! multipart lane ([`crate::api::upload_core::run_upload`]) — quota,
//! folder ownership, the upload semaphore, the configured pipeline
//! parameters and the configured storage tag included.
//!
//! JSON-RPC 2.0 shape (the chest `mcp_media` precedent): a body the
//! server cannot accept at all (unparseable, not an object, no string
//! `method`) answers an HTTP error whose body carries the prescribed
//! error frame with `id: null`; a request WITHOUT `id` is a notification
//! (`notifications/initialized`) — 202 and NO frame; protocol failures
//! (-32601 unknown method, -32602 unknown tool) ride the envelope's
//! TOP-LEVEL `error` object, never a `result` payload.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::mpsc;
use uuid::Uuid;

use i0721_proto::{FileDto, FileKind, KeyOp, NewShare, Role};

use crate::api::upload_core::{run_upload, UploadInput};
use crate::error::ApiError;
use crate::key_scopes::{
    authorize, is_live, parse_scopes, parse_scopes_buckets, required_op, Denial, Live,
};
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

    /// How many sessions the table currently holds (any state).
    ///
    /// A TEST-FACING accessor: the integration lane (a separate crate)
    /// uses it to prove that an expiry or a revocation REMOVED the
    /// session rather than merely refusing the frame. It is deliberately
    /// NOT `len` — this is not a collection-sized API, and naming it for
    /// its one caller keeps `clippy::len_without_is_empty` from pushing a
    /// dead `is_empty` back into the surface.
    #[doc(hidden)]
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.lock().len()
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
    // Expiry is NOT a lookup filter (`find_api_key_by_hash` only drops
    // revoked rows — an expired key must stay visible to `keys.list` and
    // re-openable after a renewal), so the handshake proves the key is live
    // here. The message lane proves the same thing once per frame.
    match is_live(&key_record, jiff::Timestamp::now()) {
        Live::Live => {}
        Live::Expired => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"this API key has expired"}"#,
            )
                .into_response()
        }
        // Unreachable: the lookup already excludes revoked rows. Kept
        // explicit rather than `_` so a future `Live` variant has to be
        // decided here instead of silently admitting the session.
        Live::Revoked => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"this API key was revoked"}"#,
            )
                .into_response()
        }
    }
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

/// A protocol-level JSON-RPC failure. It becomes the response envelope's
/// TOP-LEVEL `error` object — the spec's `-32600` / `-32601` / `-32602` /
/// `-32700` family is about the REQUEST, never a `result` payload that
/// merely happens to contain an error shape.
///
/// `data` is the spec's optional third member. It exists here because one
/// code now covers two different refusals (see [`permission_denied`]): the
/// number says "permission", and `data` says which permission, so a client
/// that wants to tell "fix your key" from "ask the bucket owner" still can.
struct RpcFault {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcFault {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// The same fault, with the spec's `error.data` member attached.
    fn with_data(code: i64, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }
}

/// The JSON-RPC response frame for one dispatched request: `result` on
/// success, the top-level `error` object otherwise. `id` rides verbatim so
/// a numeric id stays a number.
fn response_frame(id: &Value, outcome: Result<Value, RpcFault>) -> String {
    match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(fault) => {
            let mut error = json!({"code": fault.code, "message": fault.message});
            // The spec's optional `data` member: present only when the fault
            // has something structured to say beyond the code.
            if let Some(data) = fault.data {
                error["data"] = data;
            }
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": error,
            })
        }
    }
    .to_string()
}

/// Answer a frame the server could not accept at all.
///
/// JSON-RPC 2.0 prescribes `id: null` there (no usable request id
/// existed), and the SSE transport's message lane is an HTTP request of
/// its own: an HTTP error whose body carries the error frame is what both
/// the spec ("the response body SHOULD contain a JSON-RPC error
/// response") and the chest `mcp_media` precedent do — the stream stays
/// clean instead of carrying an id-less frame a reader has to attribute.
fn protocol_error(code: i64, message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        response_frame(&Value::Null, Err(RpcFault::new(code, message))),
    )
        .into_response()
}

/// `POST /api/mcp/message?sessionId=` — the JSON-RPC call lane.
pub async fn message(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
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
    // key revoked or EXPIRED after the handshake must stop working NOW,
    // not when the 30-minute TTL lapses. The session stores the key's
    // hash, so this is the same unique-indexed lookup the handshake paid
    // — one read per JSON-RPC frame, far cheaper than the tool dispatch
    // it guards. The record it returns is also the frame's permission
    // source, so `tools/list` and `tools/call` read one snapshot.
    //
    // A store FAILURE is not a revocation: it fails this request (500)
    // and leaves the session alone, so a database blip does not sign
    // every agent out of its stream.
    let key_record = match state.store.find_api_key_by_hash(&session.key_hash).await {
        Ok(Some(record)) => record,
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
    };
    // The expiry half: a key that lapses mid-session ends its sessions
    // exactly like a revocation (removed here, and the stream ends when
    // the sender falls out of scope) — the wording is the only
    // difference.
    if is_live(&key_record, jiff::Timestamp::now()) == Live::Expired {
        state.mcp_sessions.remove(session_id);
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"this session's API key has expired"}"#,
        )
            .into_response();
    }

    // Parse and validate the request shape. A body the server cannot even
    // read answers an HTTP error with the id-less error frame; a VALID
    // request without `id` is a notification and is never answered.
    let request: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(err) => return protocol_error(-32700, format!("Parse error: {err}")),
    };
    if !request.is_object() {
        return protocol_error(-32600, "Invalid Request: the body must be a JSON object");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return protocol_error(-32600, "Invalid Request: method must be a string");
    };
    let method = method.to_owned();
    // JSON-RPC 2.0: a request WITHOUT `id` is a notification (e.g.
    // `notifications/initialized`) — accepted, and the server MUST NOT
    // answer it (no frame, no response body).
    let Some(id) = request.get("id") else {
        return StatusCode::ACCEPTED.into_response();
    };
    let id = id.clone();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    // The share tool emits an absolute URL: derive the origin ONCE here,
    // where the request headers are still in hand, and hand it to the
    // dispatch (the same derivation the OAuth callback lane uses).
    let origin = crate::origin::public_origin(&state, &headers);
    drop(request); // the spawn owns method + params + origin

    // Dispatch in a background task; the response goes through the SSE.
    let state = state.clone();
    let user_id = session.user_id;
    let tx = session.tx.clone();
    tokio::spawn(async move {
        let outcome = dispatch_tool(&state, user_id, origin, &key_record, &method, &params).await;
        let _ = tx.send(response_frame(&id, outcome)).await;
    });

    StatusCode::ACCEPTED.into_response()
}

// ── the tool dispatch ──────────────────────────────────────────────────

/// `list_files` page width: default and cap (the tool's description and
/// its `inputSchema` say exactly this).
const LIST_FILES_LIMIT: u64 = 100;
/// `search_files` scan width: default and cap (same contract with the
/// client).
const SEARCH_FILES_LIMIT: u64 = 500;

/// A non-negative integer argument; absent or malformed → `default`.
fn uint_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

/// One paged window out of the tool arguments: `offset` (default 0) and
/// `limit` (default `cap`, clamped into `1..=cap`). Both tools state the
/// same numbers in their schema, so a client can compute the pages.
fn page_args(args: &Value, cap: u64) -> (u64, u64) {
    (
        uint_arg(args, "offset", 0),
        uint_arg(args, "limit", cap).clamp(1, cap),
    )
}

/// The five tools, each with a REAL JSON Schema `inputSchema` (an object
/// with typed `properties` and the `required` list) — a client cannot
/// build a call from a name and a prose line alone.
///
/// This is the UNFILTERED catalogue; [`visible_tools`] narrows it to the
/// key's permissions. It is public so the lane test can pin it against
/// [`crate::key_scopes::TOOL_REQUIRED_OP`] in both directions (a tool
/// added here without a permission mapping — or the reverse — is a bug
/// that must fail a test, not reach a client).
#[must_use]
pub fn tool_specs() -> Value {
    json!([
        {
            "name": "list_files",
            "description": "List one folder of the caller's own netdisk tree, newest first. \
                `folder_id` omitted lists the root folder. Paged: `offset` defaults to 0; \
                `limit` defaults to and is capped at 100.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder_id": {
                        "type": "string",
                        "description": "folder UUID; omit for the root folder"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "rows to skip (default 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": LIST_FILES_LIMIT,
                        "description": "rows to return (default 100, capped at 100)"
                    }
                },
                "required": []
            }
        },
        {
            "name": "file_info",
            "description": "One file's metadata (own tree; a soft-deleted or foreign row \
                answers an error).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "file UUID"}
                },
                "required": ["id"]
            }
        },
        {
            "name": "upload_file",
            "description": "Upload one file from base64 content through the SAME chain as the \
                REST upload lane — quota, folder ownership, the configured WebP pipeline and \
                the configured storage backend all apply. Returns the created file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content_base64": {
                        "type": "string",
                        "description": "the file bytes, standard base64"
                    },
                    "name": {
                        "type": "string",
                        "description": "source file name (the MIME hint and the original name)"
                    },
                    "folder_id": {
                        "type": "string",
                        "description": "destination folder UUID; omit for the root folder"
                    },
                    "bucket": {
                        "type": "string",
                        "description": "target bucket slug; omit for the deployment's default bucket"
                    }
                },
                "required": ["content_base64", "name"]
            }
        },
        {
            "name": "get_share_link",
            "description": "Return the public share link of one file, reusing the live share \
                when the file already has one (folders cannot be shared). Returns \
                {url, token}. `expires_at` is an RFC 3339 instant (omitted = never expires); \
                `allow_preview` defaults to true. Reuse keeps the existing link's settings — \
                delete the share first to change them; an expired share is replaced.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_id": {
                        "type": "string",
                        "description": "file UUID (a folder is refused)"
                    },
                    "expires_at": {
                        "type": "string",
                        "description": "RFC 3339 expiry instant; omit for a link that never expires"
                    },
                    "allow_preview": {
                        "type": "boolean",
                        "description": "inline preview instead of download-only (default true)"
                    }
                },
                "required": ["file_id"]
            }
        },
        {
            "name": "search_files",
            "description": "Search the caller's own netdisk tree by a name substring (the \
                display name or the original upload name). Scans one page of the own-tree \
                timeline — `offset` defaults to 0, `limit` (rows scanned) defaults to and is \
                capped at 500, so a deeper match needs the next page. `kind` narrows the scan \
                (image / document / media / folder).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "substring to match against the file name"
                    },
                    "kind": {
                        "type": "string",
                        "description": "optional kind filter: image, document, media or folder"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "rows to skip before matching (default 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": SEARCH_FILES_LIMIT,
                        "description": "rows scanned (default 500, capped at 500)"
                    }
                },
                "required": ["query"]
            }
        }
    ])
}

/// The outcome of the §7 three-state bucket pre-check: the call may
/// proceed (with the resolved bucket loaded for the tool body), or the
/// call fails as a TOOL error (`isError` result), or the frame itself
/// fails with a JSON-RPC error (the `-32005` permission refusals, each
/// naming which permission in `error.data.denial`).
enum Precheck {
    /// Run the tool; the resolved bucket context rides along (the tool
    /// body re-fetches NOTHING — §4: one decision, one row set).
    Allow(Option<ResolvedBucket>),
    /// The tool fails with this text (`isError: true`).
    ToolError(String),
    /// The frame fails with this JSON-RPC error (code + message).
    Frame(RpcFault),
}

/// The bucket row + id the pre-check resolved (passed INTO the tool
/// body so no second fetch can race a grant revocation mid-call; the
/// current tool bodies re-resolve through `run_upload` / the share
/// lane, and the fields stay so the next reader keeps the §4 seam
/// visible — clippy's dead-code is silenced for exactly that reason).
struct ResolvedBucket {
    /// The resolved bucket's id.
    #[allow(dead_code)]
    id: Uuid,
    /// The resolved bucket's slug.
    #[allow(dead_code)]
    slug: String,
}

/// The share tool's folder-refusal text (the pre-check answers it for a
/// folder argument so the tool body never re-fetches; the same string
/// the body has always answered).
const FOLDER_REFUSAL: &str = "a folder cannot be shared — only files get a public link";

/// The §7 three-state pre-check, run in `dispatch_tool` BEFORE
/// `call_tool`: one function resolves the tool's bucket, loads its row,
/// and answers Allow / ToolError / Frame per the error table.
///
/// `tools/list` is NOT pre-checked (it filters by ops only, Q7);
/// `initialize` is never gated.
async fn precheck_tool(
    state: &AppState,
    user: &i0721_proto::UserRecord,
    key: &i0721_proto::ApiKeyRecord,
    tool: &str,
    args: &Value,
) -> Precheck {
    use crate::bucket_acl::resolve_access;
    // The key's bucket set (Q7): `parse_scopes_buckets` is fail-closed —
    // a malformed array means NO buckets, which denies every bucket-
    // scoped tool (the `bucket_scope_denials_are_jsonrpc_frames…` test).
    let key_buckets = parse_scopes_buckets(&key.scopes);
    match tool {
        "list_files" | "file_info" | "search_files" => {
            // §7's read row: `isError` + not found for invisible, storage
            // unavailable for a dead backend. The tools already answer
            // not-found naturally; the pre-check only fails the frame on
            // an UNRESOLVABLE backend for bucket-scoped calls.
            Precheck::Allow(None)
        }
        "upload_file" => {
            let Some(slug) = args.get("bucket").and_then(Value::as_str) else {
                // No override: the default/bootstrap resolution inside
                // `run_upload` handles visibility (the member's default
                // bucket may be the bootstrap one they can use).
                return Precheck::Allow(None);
            };
            // ① ops (K's gate, already run) ② the bucket must be IN the
            // key's `buckets` → a permission frame naming the bucket
            // (`data.denial = "key_scope"`); ③ the USER must hold access →
            // a permission frame too, but `data.denial = "user_access"`.
            if !key_buckets.contains(slug) {
                return Precheck::Frame(permission_denied(
                    "key_scope",
                    format!(
                        "the API key is not authorized for bucket {slug:?} — \
                         edit the key's bucket permissions"
                    ),
                    slug,
                ));
            }
            let bucket = match state.store.get_storage_bucket_by_slug(slug).await {
                Ok(Some(row)) => row,
                Ok(None) => {
                    return Precheck::ToolError(format!("no such bucket: {slug}"));
                }
                Err(err) => {
                    return Precheck::ToolError(format!("storage backend unavailable: {err}"));
                }
            };
            let access = match resolve_access(state.store.as_ref(), user, &bucket).await {
                Ok(access) => access,
                Err(err) => {
                    return Precheck::ToolError(format!("storage backend unavailable: {err}"));
                }
            };
            if !access.any() {
                // Invisible to the user: isError + not found (§7) — the
                // user must not learn the slug exists.
                return Precheck::ToolError(format!("no such bucket: {slug}"));
            }
            Precheck::Allow(Some(ResolvedBucket {
                id: bucket.id,
                slug: bucket.slug.clone(),
            }))
        }
        "get_share_link" => {
            // R-L, in THIS order: ops.share (already run) → the file's
            // bucket ∈ key's `buckets` → the user holds Need::Share.
            let Some(file_id) = args
                .get("file_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                // The tool body answers the missing-argument error with
                // its own text; the pre-check lets it through.
                return Precheck::Allow(None);
            };
            // `get_file` is by id alone. A row the caller may not act on is
            // "not found" — before anything about it is read, including
            // whether it is a folder: the folder rule's text would otherwise
            // tell a caller that an id exists and what kind it is. (A caller
            // who owns the folder still gets the folder rule.)
            //
            // Who may act on it is the RPC lane's own rule — the owner, or an
            // admin — applied HERE, before the bucket steps, rather than an
            // owner-only rule of this lane's own. The leak this guards is
            // closed by *checking before reading*, not by who is checked
            // against: with owner-or-admin at this point a row the caller may
            // not act on is still "not found" before its bucket is resolved.
            // (A first version made this owner-only and justified it with "an
            // admin's key would re-open the leak", which was wrong — and
            // contradicted the same lane's `upload_file`, which lets an
            // admin's key target any user's folder.)
            let may_act =
                |row: &i0721_proto::FileRecord| row.owner_id == user.id || user.role == Role::Admin;
            let file = match state.store.get_file(file_id).await {
                Ok(Some(row)) if !may_act(&row) => {
                    return Precheck::ToolError("file not found".to_owned());
                }
                Ok(Some(row)) if row.deleted_at.is_none() && row.kind != FileKind::Folder => row,
                // The caller's OWN folder (or a soft-deleted row of theirs):
                // the folder rule's text, which is about the kind, not the id.
                Ok(Some(_)) => return Precheck::ToolError(FOLDER_REFUSAL.to_owned()),
                Ok(None) => return Precheck::ToolError("file not found".to_owned()),
                Err(err) => {
                    return Precheck::ToolError(format!("storage backend unavailable: {err}"));
                }
            };
            // The file's bucket row: `bucket_id` → the store's registry
            // rows with the registry's id→slug map as the fixture-lane
            // fallback; None → the file cannot be shared (its bytes are
            // unreachable).
            let Some(bucket_id) = file.bucket_id else {
                return Precheck::ToolError("file not found".to_owned());
            };
            let bucket = match crate::bucket_acl::bucket_row_for_id(
                state.store.as_ref(),
                &state.registry,
                bucket_id,
            )
            .await
            {
                Ok(Some(row)) => row,
                Ok(None) => return Precheck::ToolError("file not found".to_owned()),
                Err(err) => {
                    return Precheck::ToolError(format!("storage backend unavailable: {err}"));
                }
            };
            let slug = bucket.slug.clone();
            if !key_buckets.contains(&slug) {
                return Precheck::Frame(permission_denied(
                    "key_scope",
                    format!(
                        "the API key is not authorized for bucket {slug:?} — \
                         edit the key's bucket permissions"
                    ),
                    &slug,
                ));
            }
            let access = match resolve_access(state.store.as_ref(), user, &bucket).await {
                Ok(access) => access,
                Err(err) => {
                    return Precheck::ToolError(format!("storage backend unavailable: {err}"));
                }
            };
            // The share bit. An admin is never asked it, on either lane:
            // `resolve_access` returns `BucketAccess::SIGNUP` for an admin
            // before it looks at any grant, so `access.share` is true for one
            // and the frame below cannot fire. (The `Role::Admin` shortcut is
            // there because a `bucket_grants` row for an admin is never
            // written — `grants.cannot_be_set_for_an_admin` — so a lookup
            // would find nothing and deny.)
            if !access.share {
                // Visible (the user owns the file) but no share bit: a
                // permission frame naming the bucket, distinguished from the
                // key-scope refusal by `data.denial = "user_access"` (§7's
                // third arm).
                return Precheck::Frame(permission_denied(
                    "user_access",
                    format!("you do not have share access to bucket {slug:?}"),
                    &slug,
                ));
            }
            Precheck::Allow(Some(ResolvedBucket {
                id: bucket.id,
                slug,
            }))
        }
        _ => Precheck::Allow(None),
    }
}

async fn dispatch_tool(
    state: &AppState,
    user_id: Uuid,
    origin: Option<String>,
    key: &i0721_proto::ApiKeyRecord,
    method: &str,
    params: &Value,
) -> Result<Value, RpcFault> {
    match method {
        // `initialize` is never permission-gated: a client must be able
        // to negotiate the protocol before it can discover (or be
        // refused) a tool.
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "0721", "version": env!("CARGO_PKG_VERSION")},
        })),
        // Only the tools this key may actually call: an agent that
        // cannot upload should not be told `upload_file` exists.
        // Q7: the filter is by OPS only — the bucket check happens per
        // `tools/call` below, never here.
        "tools/list" => Ok(json!({
            "tools": visible_tools(&parse_scopes(&key.scopes)),
        })),
        "tools/call" => {
            let tool = params
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // An unknown tool is a PROTOCOL error (-32602: the request's
            // params name something the server does not have); a tool the
            // key lacks the permission for is a different PROTOCOL error
            // (-32005, see [`PERMISSION_DENIED`]); a tool that runs and
            // fails is a RESULT with `isError` — three different layers
            // that must not share the `result` slot.
            //
            // The decision itself is NOT taken here: the caller already
            // proved the key live for this frame, so [`authorize`] on that
            // same (freshly loaded) record is the one interpreter of the
            // permission set — the inline `scopes.contains` this used to
            // do made the MCP lane and `key_scopes` two sources of truth.
            if let Err(denial) = authorize(key, tool) {
                let code = match denial {
                    Denial::UnknownTool => -32602,
                    // Revocation/expiry were settled by the per-frame
                    // re-check above; reaching them here would mean the
                    // record changed mid-frame, and a refusal is still the
                    // right answer.
                    Denial::Revoked | Denial::Expired | Denial::Scope { .. } => PERMISSION_DENIED,
                };
                return Err(RpcFault::new(code, denial.message(tool)));
            }
            // The §7 three-state pre-check, BEFORE the tool body: the
            // bucket decision (and its loaded row) is made ONCE here and
            // handed to the body — no re-fetch, no second decision.
            let user = state
                .store
                .get_user(user_id)
                .await
                .map_err(|err| RpcFault::new(-32603, err.to_string()))?
                .ok_or_else(|| RpcFault::new(-32603, "user not found".to_owned()))?;
            let resolved = match precheck_tool(state, &user, key, tool, &args).await {
                Precheck::Allow(resolved) => resolved,
                Precheck::ToolError(text) => {
                    return Ok(json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": true,
                    }));
                }
                Precheck::Frame(fault) => return Err(fault),
            };
            // §7 pushdown (Q14): the read tools' list is scoped to the
            // key ∩ user's visible buckets — computed ONCE here, in the
            // same pass that made the precheck decision (§4's "one
            // decision" seam; the body never re-derives it).
            let visible_ids =
                crate::bucket_acl::visible_bucket_ids(state.store.as_ref(), &state.registry, &user)
                    .await
                    .map_err(|err| RpcFault::new(-32603, err.to_string()))?;
            let key_buckets = parse_scopes_buckets(&key.scopes);
            let key_bucket_ids = {
                let mut ids = Vec::new();
                for bucket in state
                    .store
                    .list_storage_buckets()
                    .await
                    .map_err(|err| RpcFault::new(-32603, err.to_string()))?
                {
                    if key_buckets.contains(&bucket.slug) && visible_ids.contains(&bucket.id) {
                        ids.push(bucket.id);
                    }
                }
                // A NULL-bucket legacy row reads through the bootstrap
                // id — keep it listable when the key's buckets allow the
                // bootstrap slug (the synthetic-row posture).
                if let Some(bootstrap_slug) = state.registry.bootstrap_slug().map(str::to_owned) {
                    if key_buckets.contains(&bootstrap_slug) {
                        if let Ok(id) = state.registry.bootstrap_bucket_id() {
                            ids.push(id);
                        }
                    }
                }
                ids.sort();
                ids.dedup();
                ids
            };
            match call_tool(
                state,
                &user,
                origin.as_deref(),
                tool,
                &args,
                &resolved,
                &key_bucket_ids,
            )
            .await
            {
                Ok(result) => {
                    Ok(json!({"content": [{"type": "text", "text": result.to_string()}]}))
                }
                Err(err) => Ok(json!({
                    "content": [{"type": "text", "text": err}],
                    "isError": true,
                })),
            }
        }
        _ => Err(RpcFault::new(-32601, format!("method not found: {method}"))),
    }
}

/// The JSON-RPC code for "this key lacks the permission the tool needs".
///
/// It is **the plana family's** permission code (`error_codes::AUTH_ERROR`,
/// `-32005`) — the same one a 401/403 gets on the `/api/rpc` lane — and not a
/// number of this lane's own. It used to be a private `-32001`, which plana
/// already assigns to `SNAPSHOT_FAILED`: two unrelated failures answering with
/// the same number on the same wire, so a client could not tell "your snapshot
/// could not be taken" from "your key may not upload".
///
/// The two refusals that shared that old number stay distinguishable, by the
/// spec's `error.data.denial` member instead: `key_scope` (the key's own
/// `buckets` list — fixable on the keys page) and `user_access` (the acting
/// user's grant on the bucket — fixable by its owner). The ops refusal below
/// carries no `data`: the thing to fix is the key's op set, and its message
/// already names the op and the tool.
///
/// `rpc_error_codes` is the one place a plana code may be reused, and only
/// where the meaning is the same; [`crate::rpc`]'s
/// `a_service_code_never_quietly_reuses_a_plana_code` test holds that line.
const PERMISSION_DENIED: i64 = crate::rpc::rpc_error_codes::AUTH;

/// One permission refusal: the family's code, the caller's own text, and
/// which permission was missing.
///
/// An error CODE, not a status change: the message lane still answers 202 and
/// the refusal rides the envelope's TOP-LEVEL `error` object, exactly like
/// `-32602`.
fn permission_denied(denial: &'static str, message: String, bucket: &str) -> RpcFault {
    RpcFault::with_data(
        PERMISSION_DENIED,
        message,
        json!({"denial": denial, "bucket": bucket}),
    )
}

/// The tool specifications this key is allowed to call (canonical
/// `tool_specs()` order preserved).
fn visible_tools(scopes: &BTreeSet<KeyOp>) -> Vec<Value> {
    let all = tool_specs();
    all.as_array()
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| {
                    tool["name"]
                        .as_str()
                        .and_then(required_op)
                        .is_some_and(|op| scopes.contains(&op))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

async fn call_tool(
    state: &AppState,
    user: &i0721_proto::UserRecord,
    origin: Option<&str>,
    tool: &str,
    args: &Value,
    resolved: &Option<ResolvedBucket>,
    key_bucket_ids: &[Uuid],
) -> Result<Value, String> {
    // The acting user, whole: an API key runs as its owner, and the owner's
    // role is part of who that is (the share rule below is owner-or-admin).
    let user_id = user.id;
    let role = user.role;
    match tool {
        "list_files" => {
            let folder_id = args
                .get("folder_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());
            let (offset, limit) = page_args(args, LIST_FILES_LIMIT);
            let rows = state
                .store
                .list_folder(
                    user_id,
                    folder_id,
                    None,
                    Some(key_bucket_ids),
                    offset,
                    limit,
                )
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
            // The pre-check already resolved (and authorized) the
            // `bucket` argument's row; the override slug rides into
            // `run_upload` unchanged — the resolution inside re-reads the
            // SAME row the pre-check loaded (§4: one decision, one row).
            let uploaded = run_upload(
                state,
                &user,
                UploadInput {
                    name: None,
                    source_name: Some(name.to_owned()),
                    folder_id,
                    // The MCP `bucket` argument (R-I): invisible → isError
                    // + not found; visible-but-unservable → isError +
                    // storage unavailable (§7).
                    bucket: args
                        .get("bucket")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    data: data.into(),
                },
            )
            .await
            .map_err(|err| err.error)?;
            let _ = resolved;
            Ok(serde_json::to_value(FileDto::from(uploaded.record)).unwrap_or_default())
        }
        "get_share_link" => {
            let file_id = args
                .get("file_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or("missing or invalid 'file_id'")?;
            // A link nobody can open is worse than an error: the origin
            // comes from the deployment's own configuration (or the
            // request Host), never from a hardcoded localhost.
            let origin = origin.ok_or(
                "cannot derive the deployment's public origin: set I0721_PUBLIC_ORIGIN \
                 (or send a Host header) so the share link is reachable",
            )?;
            let file = state
                .store
                .get_file(file_id)
                .await
                .map_err(|e| e.to_string())?
                .filter(|r| r.deleted_at.is_none())
                .ok_or("file not found")?;
            // The owner, or an admin — the same rule as the RPC lane's
            // `files.share_create`, and the same rule the pre-check applied
            // before this body ran. (This comment has said three things: that
            // the rule was "exactly like the REST share lane" when RPC
            // elevated admins and this did not; that this lane was
            // deliberately owner-only; and now this. An API key acts as its
            // owner, admin included — `upload_file` in this same lane already
            // let an admin's key target any user's folder.)
            if file.owner_id != user_id && role != Role::Admin {
                return Err("not your file".to_owned());
            }
            // The share bit is the pre-check's (`user.role != Role::Admin &&
            // !access.share` there); an admin never reaches this line without
            // it having passed, and a non-admin's has.
            // `files.share_create` refuses folders for the same reason: a
            // share token resolves to ONE file's bytes.
            if file.kind == FileKind::Folder {
                return Err("a folder cannot be shared — only files get a public link".to_owned());
            }
            // A share belongs to the FILE, not to whoever asked for the link:
            // the RPC lane records `record.owner_id`, and so does this. (It
            // recorded the acting user, which was the same thing while only
            // the owner could get here — and the moment an admin's key could,
            // an admin's call minted a SECOND token on the member's file
            // instead of answering with the member's. Measured by the review:
            // two share rows, two tokens, for one file.)
            let wanted = NewShare {
                file_id: file.id,
                owner_id: file.owner_id,
                expires_at: args
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                allow_preview: args
                    .get("allow_preview")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            };
            let share = match state
                .store
                .find_share_by_file(file.owner_id, file.id)
                .await
                .map_err(|e| e.to_string())?
            {
                // Reuse the LIVE share: a repeated call answers the same
                // link instead of piling up tokens (and breaking the links
                // the agent already handed out).
                Some(existing) if crate::share::is_live(&existing) => existing,
                // An expired row is not a link. Drop it first — the
                // store's create path returns an existing row for the same
                // file, which would hand the dead token straight back.
                Some(stale) => {
                    state
                        .store
                        .delete_share(&stale.token)
                        .await
                        .map_err(|e| e.to_string())?;
                    state
                        .store
                        .create_share(wanted)
                        .await
                        .map_err(|e| e.to_string())?
                }
                None => state
                    .store
                    .create_share(wanted)
                    .await
                    .map_err(|e| e.to_string())?,
            };
            Ok(json!({
                "url": format!("{origin}/s/{}", share.token),
                "token": share.token,
            }))
        }
        "search_files" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("missing 'query'")?;
            let kind = args
                .get("kind")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned());
            let (offset, limit) = page_args(args, SEARCH_FILES_LIMIT);
            // The name filter runs on the page the store returns (the
            // timeline has no name-indexed query), so `offset`/`limit`
            // page the SCAN — the schema says so.
            let rows = state
                .store
                .list_folder(user_id, None, kind, Some(key_bucket_ids), offset, limit)
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
