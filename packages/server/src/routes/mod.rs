use anyhow::Result;

use axum::{middleware::from_extractor, routing::get, Router};

use crate::utils::ExtractAuthInfo;

pub mod backend;
pub mod frontend;

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/", frontend::route().await?)
        .nest("/api", backend::route().await?)
        .fallback(get(frontend::pages::not_found::query))
        .layer(from_extractor::<ExtractAuthInfo>());

    Ok(router)
}
