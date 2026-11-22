//! The slim `/api/v1` whitelist: everything else moved to the RPC lane
//! (`crate::rpc`). What stays is what cannot or should not ride a
//! JSON control lane: the session lifecycle (login plants the httpOnly
//! cookie, refresh rotates it, logout expires it — Set-Cookie needs
//! HTTP headers a WS method cannot carry) and the multipart binary
//! upload.

pub mod auth;
pub mod files;
pub mod settings;
/// The upload chain shared by the multipart lane and the MCP tool.
pub(crate) mod upload_core;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

/// Mount the v1 whitelist (stateless: the caller applies the state).
pub fn router() -> Router<AppState> {
    let files_api = Router::new().route("/", post(files::upload));
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/refresh", post(auth::refresh))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/ws-ticket", post(auth::ws_ticket))
        .route("/auth/login-methods", get(auth::login_methods))
        .route("/share/{token}/meta", get(crate::share::meta))
        .route("/share/{token}/download", get(crate::share::download))
        .route("/auth/oauth/{provider}", get(crate::oauth::redirect))
        .route(
            "/auth/oauth/{provider}/callback",
            get(crate::oauth::callback),
        )
        .nest("/files", files_api)
}
