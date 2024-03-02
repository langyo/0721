pub mod image;
pub mod pages;
pub mod static_files;

use anyhow::Result;

use axum::{routing::get, Router};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/", static_files::route().await?)
        .nest("/", pages::route().await?)
        .route("/image/:hash", get(image::download_image));

    Ok(router)
}
