//! The slim `/api/v1` whitelist: everything else moved to the RPC lane
//! (`crate::rpc`). What stays is what cannot or should not ride a
//! JSON control lane — the login bootstrap and the multipart binary
//! upload.

pub mod auth;
pub mod files;
pub mod settings;

use axum::routing::post;
use axum::Router;

use crate::state::AppState;

/// Mount the v1 whitelist (stateless: the caller applies the state).
pub fn router() -> Router<AppState> {
    let files_api = Router::new().route("/", post(files::upload));
    Router::new()
        .route("/auth/login", post(auth::login))
        .nest("/files", files_api)
}
