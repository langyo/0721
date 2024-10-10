mod get;
mod set;

use anyhow::Result;

use axum::{
    middleware,
    routing::{get, put},
    Router,
};

use _database::RouteEnv;

pub async fn route(env: RouteEnv) -> Result<Router> {
    let router = Router::new()
        .route("/", get(get::get))
        .route("/", put(set::set))
        .layer(middleware::from_fn_with_state(
            env.clone(),
            super::auth_middleware,
        ))
        .with_state(env);

    Ok(router)
}
