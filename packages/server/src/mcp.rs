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
//! The tool surface (five tools) rides the STORE's existing per-user
//! permission face: no admin elevation, the key's owner is the acting
//! user for every call.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::mpsc;
use uuid::Uuid;

use i0721_proto::{FileDto, NewShare};

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

/// Verify an API key: hash → lookup → active check. The hash lookup is
/// an indexed equality — timing leaks nothing about which key matched
/// (all non-matches are the same branch); the comparison itself is the
/// SHA-256 digest, not the key, so constant-time comparison of the
/// digest is unnecessary (the lookup key IS the digest).
pub(crate) async fn verify_api_key(
    state: &AppState,
    raw_key: &str,
) -> Result<i0721_proto::ApiKeyRecord, ApiError> {
    let hash = hash_api_key(raw_key);
    let record = state
        .store
        .find_api_key_by_hash(&hash)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::unauthorized("invalid API key"))?;
    // Best-effort refresh (never blocks the call).
    let store = state.store.clone();
    let id = record.id;
    tokio::spawn(async move {
        let _ = store.touch_api_key(id).await;
    });
    Ok(record)
}

// ── SSE session management ─────────────────────────────────────────────

struct McpSession {
    /// The authenticated user.
    user_id: Uuid,
    /// The response channel (SSE writes).
    tx: mpsc::Sender<String>,
    /// Last activity (TTL pruning).
    #[allow(dead_code)]
    last_active: std::time::Instant,
}

/// In-memory session table (bounded by SESSION_CAP; pruned on insert).
#[derive(Clone)]
pub struct McpSessions {
    inner: std::sync::Arc<tokio::sync::Mutex<HashMap<String, Arc<McpSession>>>>,
}

impl McpSessions {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl Default for McpSessions {
    fn default() -> Self {
        Self::new()
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

    let session_id = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<String>(64);

    // Register the session.
    {
        let mut sessions = state.mcp_sessions.inner.lock().await;
        // Prune expired + enforce cap.
        sessions.retain(|_, s| s.last_active.elapsed() < SESSION_TTL);
        if sessions.len() >= SESSION_CAP {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":"session limit reached"}"#,
            )
                .into_response();
        }
        sessions.insert(
            session_id.clone(),
            Arc::new(McpSession {
                user_id: key_record.user_id,
                tx: tx.clone(),
                last_active: std::time::Instant::now(),
            }),
        );
    }

    // SSE stream.
    let stream = async_stream::stream! {
        // First frame: the endpoint event.
        yield Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default()
            .event("endpoint")
            .data(format!("/api/mcp/message?sessionId={session_id}")));
        // Keepalive + message pump.
        loop {
            tokio::select! {
                _ = tokio::time::sleep(KEEPALIVE) => {
                    yield Ok(axum::response::sse::Event::default().comment("ping"));
                }
                msg = rx.recv() => {
                    match msg {
                        Some(data) => yield Ok(axum::response::sse::Event::default()
                            .event("message")
                            .data(data)),
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

    // Session lookup: the session carries the authenticated user; the
    // message lane does NOT re-verify the API key (the session IS the
    // credential from here on — the key was proven at the SSE handshake).
    let session = {
        let sessions = state.mcp_sessions.inner.lock().await;
        sessions.get(session_id).cloned()
    };
    let Some(session) = session else {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"unknown or expired session"}"#,
        )
            .into_response();
    };

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
            let _user = state
                .store
                .get_user(user_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("user not found")?;
            // The pipeline is sync: process bytes → detect kind → convert.
            let rules = crate::pipeline::UploadRules {
                size_limit: 8 * 1024 * 1024,
                webp_auto_convert: true,
            };
            let processed = crate::pipeline::process_upload(&data, Some(name), &rules)
                .map_err(|e| format!("pipeline: {e}"))?;
            // Dedupe per owner.
            if let Some(existing) = state
                .store
                .get_file_by_owner_and_sha256(user_id, &processed.sha256)
                .await
                .map_err(|e| e.to_string())?
            {
                return Ok(serde_json::to_value(FileDto::from(existing)).unwrap_or_default());
            }
            // Blob write.
            let storage_key = processed.sha256.clone();
            state
                .blobs
                .put(crate::blob::BlobSlot::Media, &storage_key, &processed.data)
                .await
                .map_err(|e| format!("blob put: {e}"))?;
            if let Some(thumb) = &processed.thumbnail {
                let thumb_key = format!("{storage_key}.webp");
                state
                    .blobs
                    .put(crate::blob::BlobSlot::Cache, &thumb_key, thumb)
                    .await
                    .map_err(|e| format!("thumbnail put: {e}"))?;
            }
            // Metadata row.
            let record = state
                .store
                .create_file(i0721_proto::NewFile {
                    owner_id: user_id,
                    parent_id: folder_id,
                    name: storage_key.clone(),
                    kind: processed.kind,
                    mime: processed.mime,
                    size: processed.data.len() as i64,
                    sha256: processed.sha256.clone(),
                    storage_backend: "local".to_owned(),
                    storage_key,
                    thumbnail_key: processed
                        .thumbnail
                        .as_ref()
                        .map(|_| format!("{}.webp", processed.sha256)),
                    original_name: name.to_owned(),
                })
                .await
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(FileDto::from(record)).unwrap_or_default())
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
