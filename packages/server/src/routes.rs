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

use crate::api;
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
    Router::new()
        .route("/api/health", get(api::settings::health))
        .nest("/api/v1", api::router())
        .route("/media/{key}", get(media::download))
        .route("/", get(static_files::serve_root))
        .route("/assets/{*path}", get(static_files::serve_asset))
        .fallback(static_files::fallback)
        .with_state(state)
        // The RPC lane owns its state (the builder captured it); merge
        // the stateless router it produces after the stateful tree.
        .merge(rpc::router(rpc_state))
}
