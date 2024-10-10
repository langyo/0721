use anyhow::Result;

use axum::{
    extract::{Json, Query, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use _database::{
    functions::backend::user::{count as do_count, list as do_list},
    RouteEnv,
};
use _types::request::PageArgs;

#[tracing::instrument(skip_all, parent = None)]
pub async fn count(State(env): State<RouteEnv>) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_count(env)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}

#[tracing::instrument(skip_all, parent = None)]
pub async fn list(
    State(env): State<RouteEnv>,
    args: Query<PageArgs>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_list(env, args.offset.unwrap_or(0), args.limit.unwrap_or(100))
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(ret))
}
