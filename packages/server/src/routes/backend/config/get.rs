use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use hyper::StatusCode;

use _types::config::load_config;

#[tracing::instrument(skip_all, parent = None)]
pub async fn get() -> Result<impl IntoResponse, (StatusCode, String)> {
    Ok(Json(load_config().map_err(|err| {
        (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    })?))
}
