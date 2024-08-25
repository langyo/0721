pub mod media;
pub mod pages;
pub mod static_files;

use anyhow::Result;

use axum::{routing::get, Router};

use _types::config::load_config;

pub async fn route() -> Result<Router> {
    let config = load_config()?;
    let media_path = config.router.media_entry_path.clone();

    let router = Router::new()
        .nest("/", static_files::route().await?)
        .nest("/", pages::route().await?)
        .route(
            format!("{}/:hash", media_path).as_str(),
            get(media::download_media),
        );

    Ok(router)
}
