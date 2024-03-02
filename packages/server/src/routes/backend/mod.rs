mod api;
mod auth;

use anyhow::Result;
use axum::Router;

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/auth", auth::route().await?)
        .nest("/", api::route().await?);

    Ok(router)
}
