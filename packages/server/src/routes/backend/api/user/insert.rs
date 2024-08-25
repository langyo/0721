use std::str::FromStr;

use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use chrono::Utc;
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::{
        backend::user::{get as do_select, set as do_insert},
        frontend::auth::generate_hash,
    },
    models::user::Model as DTO,
};
use _types::request::{Permission, RegisterParams};

#[tracing::instrument]
pub async fn register(
    ExtractAuthInfo(info): ExtractAuthInfo,
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

    if do_select(&vo.name)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .is_some()
    {
        return Err((StatusCode::BAD_REQUEST, "Duplicated name".to_string()));
    }

    let password_hash = generate_hash(&vo.password_raw)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    do_insert(
        vo.name,
        &DTO {
            updated_at: Utc::now(),
            permission: Permission::from_str(&vo.permission)
                .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?,
            password_hash,
            email: vo.email,
        },
    )
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}
