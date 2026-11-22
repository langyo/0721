//! The router assembly — `app(state)` for serving, `router(state)` for
//! oneshot tests (the ayane split).

use axum::routing::get;
use axum::Router;
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use crate::api;
use crate::media;
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

/// The route tree alone. `/api` and `/media` own their paths; every
/// other path falls through to the static/SPA handlers (see
/// `static_files`), with client-side routes surviving a hard refresh
/// through the index.html fallback.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(api::settings::health))
        .nest("/api/v1", api::router())
        .route("/media/{key}", get(media::download))
        .route("/", get(static_files::serve_root))
        .route("/assets/{*path}", get(static_files::serve_asset))
        .fallback(static_files::fallback)
        .with_state(state)
}
