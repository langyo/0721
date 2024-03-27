mod insert;
mod list;
mod update;

use anyhow::Result;
use axum::{
    routing::{delete, get, post, put},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route("/insert", put(insert::insert))
        .route("/update/:id", post(update::update))
        .route("/delete/:id", delete(update::delete));

    Ok(router)
}
