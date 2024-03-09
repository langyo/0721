pub mod media;
pub mod pages;
pub mod static_files;

use anyhow::Result;

use axum::{routing::get, Router};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/", static_files::route().await?)
        .nest("/", pages::route().await?)
        // TODO - Custom the perfix by global config
        .route("/media/:hash", get(media::download_media));

    Ok(router)
}
