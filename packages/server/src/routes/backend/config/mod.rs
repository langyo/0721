mod get;
mod set;

use anyhow::Result;

use axum::{
    middleware,
    routing::{get, put},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/", get(get::get))
        .route("/", put(set::set))
        .layer(middleware::from_fn(super::auth_middleware));

    Ok(router)
}
