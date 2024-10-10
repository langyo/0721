use anyhow::Result;

use axum::{
    extract::{Path, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{functions::backend::user::delete as do_delete, RouteEnv};

pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    State(env): State<RouteEnv>,
    Path(email): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if let Some(info) = info.clone() {
        if !info.is_admin() {
            return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
        }

        // Ensure the user is not deleting itself
        if info.email == email {
            return Err((StatusCode::FORBIDDEN, "Cannot delete yourself".to_string()));
        }
    } else {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    do_delete(env, email)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(())
}
