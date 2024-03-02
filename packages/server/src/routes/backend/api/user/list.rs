use anyhow::Result;

use axum::{
    extract::{Json, Query},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::routes::backend::api::PageArgs;
use _database::functions::backend::user::{count as do_count, list as do_list};

#[tracing::instrument]
pub async fn count() -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_count()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}

#[tracing::instrument]
pub async fn list(args: Query<PageArgs>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_list(args.offset.unwrap_or(0), args.limit.unwrap_or(10))
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}
