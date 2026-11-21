//! The router assembly — `app(state)` for serving, `router(state)` for
//! oneshot tests (the ayane split).

use axum::routing::get;
use axum::routing::get_service;
use axum::Router;
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeFile;

use crate::api;
use crate::media;
use crate::state::AppState;

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
    let static_dir = state.config.static_dir.clone();
    let index = ServeFile::new(static_dir.join("index.html"));
    let favicon = ServeFile::new(static_dir.join("favicon.ico"));
    let bg = ServeFile::new(static_dir.join("bg.webp"));
    let logo = ServeFile::new(static_dir.join("logo.webp"));

    Router::new()
        .route("/api/health", get(api::settings::health))
        .nest("/api/v1", api::router())
        .route("/media/{key}", get(media::download))
        .route_service("/", get_service(index))
        .route_service("/favicon.ico", get_service(favicon))
        .route_service("/bg.webp", get_service(bg))
        .route_service("/logo.webp", get_service(logo))
        .with_state(state)
}
