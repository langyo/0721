mod media;
mod user;

use anyhow::Result;

use axum::{middleware, Router};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/media", media::route().await?)
        .nest("/user", user::route().await?)
        .layer(middleware::from_fn(super::auth_middleware));

    Ok(router)
}
