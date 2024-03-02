mod config;
mod image;
mod user;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use axum::{
    extract::Request,
    middleware::{self, Next},
    response::Response,
    Router,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::functions::frontend::auth::verify;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

async fn auth_middleware(
    bearer: TypedHeader<Authorization<Bearer>>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    if let Err(err) = verify(bearer.token().to_string()).await {
        return Err((
            StatusCode::UNAUTHORIZED,
            format!("Unauthorized: {}", err.to_string()),
        ));
    }

    Ok(next.run(request).await)
}

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .nest("/config", config::route().await?)
        .nest("/image", image::route().await?)
        .nest("/user", user::route().await?)
        .layer(middleware::from_fn(auth_middleware));

    Ok(router)
}
