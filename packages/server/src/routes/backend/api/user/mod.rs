mod insert;
mod list;
mod update;

use anyhow::Result;
use axum::{
    routing::{delete, get, post},
    Router,
};

use _database::RouteEnv;

pub async fn route(env: RouteEnv) -> Result<Router> {
    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route("/register", post(insert::register))
        .route("/delete/:id", delete(update::delete))
        .with_state(env);

    Ok(router)
}
