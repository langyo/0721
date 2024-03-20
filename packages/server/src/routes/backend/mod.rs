mod api;
mod auth;
mod config;

use anyhow::Result;

use axum::{extract::Request, middleware::Next, response::Response, Router};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::functions::frontend::auth::verify;

pub async fn auth_middleware(
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
        .nest("/auth", auth::route().await?)
        .nest("/config", config::route().await?)
        .nest("/", api::route().await?);

    Ok(router)
}
