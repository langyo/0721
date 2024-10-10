use anyhow::Result;

use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::backend::user::{get as do_select, set as do_insert},
    RouteEnv,
};
use _types::request::RegisterParams;

#[tracing::instrument(skip_all, parent = None)]
pub async fn register(
    ExtractAuthInfo(info): ExtractAuthInfo,
    State(env): State<RouteEnv>,
    Json(vo): Json<RegisterParams>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !info
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "No permission".to_string(),
        ))?
        .is_admin()
    {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    if do_select(
        env.clone(),
        vo.name
            .clone()
            .ok_or((StatusCode::BAD_REQUEST, "No name".to_string()))?,
    )
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    .is_some()
    {
        return Err((StatusCode::BAD_REQUEST, "Duplicated name".to_string()));
    }

    do_insert(env.clone(), vo)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}
