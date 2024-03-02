use anyhow::Result;

use axum::{
    extract::{Json, Query},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::routes::backend::api::PageArgs;
use _database::functions::backend::config::list as do_list;

#[tracing::instrument]
pub async fn list(args: Query<PageArgs>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_list()
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}
