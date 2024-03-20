use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use hyper::StatusCode;

use _database::types::config::{update_config, Config as VO};

#[tracing::instrument]
pub async fn set(Json(vo): Json<VO>) -> Result<impl IntoResponse, (StatusCode, String)> {
    update_config(vo).map_err(|err| {
        tracing::error!("Failed to update config: {}", err);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to update config".to_string(),
        )
    })?;

    Ok(())
}
