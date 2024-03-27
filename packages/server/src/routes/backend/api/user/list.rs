use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use hyper::StatusCode;

use _database::functions::backend::user::{count as do_count, list as do_list};

#[tracing::instrument]
pub async fn count() -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_count()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}

#[tracing::instrument]
pub async fn list() -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_list()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}
