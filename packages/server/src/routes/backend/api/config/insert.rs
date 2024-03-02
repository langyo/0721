use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::backend::config::insert as do_insert, types::request::models::Config as VO,
};

#[tracing::instrument]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Json(vo): Json<VO>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !info.is_admin() {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    do_insert(vo.label, vo.value)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}
