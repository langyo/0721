mod api;
mod auth;
mod config;

use anyhow::Result;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
    Router,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::{functions::frontend::auth::verify, RouteEnv};

pub async fn auth_middleware(
    bearer: TypedHeader<Authorization<Bearer>>,
    State(env): State<RouteEnv>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    if let Err(err) = verify(env.clone(), bearer.token().to_string()).await {
        return Err((StatusCode::UNAUTHORIZED, format!("Unauthorized: {}", err)));
    }

    Ok(next.run(request).await)
}

pub async fn route(env: RouteEnv) -> Result<Router> {
    let router = Router::new()
        .nest("/auth", auth::route(env.clone()).await?)
        .nest("/config", config::route(env.clone()).await?)
        .nest("/", api::route(env.clone()).await?);

    Ok(router)
}
