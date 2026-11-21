//! The `/api/v1` surface.

pub mod auth;
pub mod files;
pub mod settings;
pub mod users;

use axum::routing::{get, patch, post};
use axum::Router;

use crate::state::AppState;

/// Mount the v1 API (stateless: the caller applies the state).
pub fn router() -> Router<AppState> {
    let files_api = Router::new()
        .route("/", get(files::list).post(files::upload))
        .route("/{id}", patch(files::rename).delete(files::remove));
    let users_api = Router::new()
        .route("/", get(users::list).post(users::create))
        .route("/{id}", axum::routing::delete(users::remove));
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/verify", get(auth::verify))
        .route("/auth/refresh", post(auth::refresh))
        .nest("/files", files_api)
        .nest("/users", users_api)
        .route("/settings", get(settings::get).put(settings::set))
}
