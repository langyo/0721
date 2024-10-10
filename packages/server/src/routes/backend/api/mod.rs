mod media;
mod user;

use anyhow::Result;

use axum::{middleware, Router};

use _database::RouteEnv;

pub async fn route(env: RouteEnv) -> Result<Router> {
    let router = Router::new()
        .nest("/media", media::route(env.clone()).await?)
        .nest("/user", user::route(env.clone()).await?)
        .layer(middleware::from_fn_with_state(
            env.clone(),
            super::auth_middleware,
        ));

    Ok(router)
}
