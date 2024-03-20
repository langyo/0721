mod media;
mod user;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use axum::{middleware, Router};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/media", media::route().await?)
        .nest("/user", user::route().await?)
        .layer(middleware::from_fn(super::auth_middleware));

    Ok(router)
}
