mod insert;
mod list;
mod update;

use anyhow::Result;
use axum::{
    routing::{get, post},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route("/insert", post(|token, vo| insert::insert(token, vo)))
        .route(
            "/delete/:id",
            post(|token, path| update::delete(token, path)),
        );

    Ok(router)
}
