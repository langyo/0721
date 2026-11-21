pub mod media;
pub mod static_files;

use anyhow::Result;

use axum::{response::IntoResponse, routing::get, Router};

use _database::RouteEnv;
use _types::config::load_config;

async fn not_found() -> impl IntoResponse {
    axum::http::StatusCode::NOT_FOUND
}

pub async fn route(env: RouteEnv) -> Result<Router> {
    let config = load_config()?;
    let media_path = config.router.media_entry_path.clone();

    let router = Router::new()
        .nest("/", static_files::route(env.clone()).await?)
        .route(
            format!("{}/:hash", media_path).as_str(),
            get(media::download_media).with_state(env),
        )
        .fallback(get(not_found));

    Ok(router)
}
