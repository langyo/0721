use anyhow::Result;

use axum::{
    extract::{Json, Path},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::functions::backend::image::delete as do_delete;

#[tracing::instrument]
pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !info.is_admin() {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    do_delete(id)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(()))
}
