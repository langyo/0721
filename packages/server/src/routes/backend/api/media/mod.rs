mod insert;
mod list;
mod update;

use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route(
            "/insert",
            // TODO - Dynamic the max size by global config
            post(insert::insert).layer(DefaultBodyLimit::max(1024 * 1024 * 8)), // 8 MiB
        )
        .route("/delete/:id", post(update::delete));

    Ok(router)
}
