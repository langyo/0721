mod insert;
mod list;
mod update;

use anyhow::Result;
use axum::{
    routing::{delete, get, post},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route("/register", post(insert::register))
        .route("/delete/:id", delete(update::delete));

    Ok(router)
}
