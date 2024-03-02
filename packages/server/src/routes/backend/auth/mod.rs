mod login;
mod refresh;
mod verify;

use anyhow::Result;

use axum::{
    routing::{get, post},
    Router,
};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/login", post(login::login))
        .route("/refresh", get(refresh::refresh))
        .route("/verify", get(verify::verify));

    Ok(router)
}
