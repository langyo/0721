//! The router assembly — `app(state)` for serving, `router(state)` for
//! oneshot tests (the ayane split).
//!
//! Two API lanes share the tree: the WebSocket JSON-RPC surface at
//! `/api/rpc` (the webui's primary API) and a deliberately TINY REST
//! whitelist — `/api/health` (probes + container healthcheck), the
//! password login (the bootstrap: it mints the JWT every later call,
//! WS included, rides), the multipart upload (binary bodies do not
//! belong on a JSON control lane), and `/media/{key}` (large binary
//! serving). Every other former `/api/v1` route lives on as an RPC
//! method (`rpc.rs`); unknown `/api` paths answer 404 JSON (the
//! fallback's API-territory rule).

use axum::routing::get;
use axum::Router;
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use plana_error_landing::ErrorLanding;

use crate::api;
use crate::config::Config;
use crate::media;
use crate::rpc;
use crate::state::AppState;
use crate::static_files;

/// The complete app: the router plus the body limit and the dynamic
/// gzip layer. The predicate deliberately drops tower-http's default
/// "skip images" rule — the gzip-test branch's intent was compressing
/// the dynamically served media responses themselves for clients that
/// advertise `Accept-Encoding: gzip` (bodies above 1 KiB only; smaller
/// ones lose more to gzip framing than they gain).
pub fn app(state: AppState) -> Router {
    router(state)
        .layer(RequestBodyLimitLayer::new(crate::config::BODY_LIMIT))
        .layer(CompressionLayer::new().compress_when(SizeAbove::new(1024)))
}

/// The route tree alone.
pub fn router(state: AppState) -> Router {
    let rpc_state = state.clone();
    // Cheap to clone: the landing holds one `Arc<str>` and is resolved
    // once, when the state is assembled.
    let error_landing = state.error_landing.clone();
    Router::new()
        .route("/api/health", get(api::settings::health))
        .route("/api/mcp/sse", get(crate::mcp::sse))
        .route("/api/mcp/message", axum::routing::post(crate::mcp::message))
        .nest("/api/v1", api::router())
        .route("/media/{key}", get(media::download))
        .route("/", get(static_files::serve_root))
        .route("/assets/{*path}", get(static_files::serve_asset))
        .fallback(static_files::fallback)
        .with_state(state)
        // The RPC lane owns its state (the builder captured it); merge
        // the stateless router it produces after the stateful tree.
        .merge(rpc::router(rpc_state))
        // Innermost, over the WHOLE tree — every handler above and the
        // fallback included — so `app()`'s body limit and gzip sit
        // OUTSIDE it. The layer rewrites only a browser navigation that
        // ended in a 4xx/5xx bare-text body; fetch/XHR keeps its JSON.
        .layer(ErrorLanding::clone(&error_landing).layer())
}

/// The error landing's shell: the very `index.html` bytes `/` serves
/// (`static_files::index_bytes_blocking`, both the embedded and the dev
/// mode), read once while the state is assembled.
///
/// A build with no dist has no frontend to hand over, so the layer is
/// `disabled()` and every response passes through untouched — the same
/// posture as this tree's other headless paths, and never a landing
/// painted from a shell the service cannot serve.
///
/// Non-UTF-8 shell bytes are treated as no shell: the layer would have to
/// splice markup into a document it cannot read, and a direct pass-through
/// is the honest answer there.
pub(crate) fn error_landing(config: &Config) -> ErrorLanding {
    let shell =
        static_files::index_bytes_blocking(config).and_then(|bytes| String::from_utf8(bytes).ok());
    match shell {
        Some(shell) => ErrorLanding::new(shell),
        None => ErrorLanding::disabled(),
    }
}
