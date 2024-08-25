use anyhow::Result;

use axum::{extract::Path, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::functions::backend::user::delete as do_delete;

pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if let Some(info) = info.clone() {
        if !info.is_admin() {
            return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
        }

        // Ensure the user is not deleting itself
        if info.name == name {
            return Err((StatusCode::FORBIDDEN, "Cannot delete yourself".to_string()));
        }
    } else {
        return Err((StatusCode::FORBIDDEN, "No permission".to_string()));
    }

    do_delete(name)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(())
}
