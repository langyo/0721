use anyhow::Result;

use axum::{
    extract::{Json, Path},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::backend::user::{delete as do_delete, set as do_update},
    types::request::models::User as VO,
};

pub async fn update(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Path(name): Path<String>,
    Json(vo): Json<VO>,
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

    do_update(name, &vo.into())
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(()))
}

pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Path(name): Path<String>,
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

    do_delete(name)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(()))
}
