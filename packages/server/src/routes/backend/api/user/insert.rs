use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use chrono::Utc;
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::backend::user::insert as do_insert, models::user::Model as DTO,
    types::request::models::User as VO,
};

#[tracing::instrument]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Json(vo): Json<VO>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !info.is_admin() {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    do_insert(
        vo.name,
        DTO {
            updated_at: Utc::now(),
            permission: vo.permission,
            password_hash: vo.password_hash,
            email: vo.email,
        },
    )
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(())
}
