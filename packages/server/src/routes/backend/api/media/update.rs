use anyhow::Result;

use axum::{extract::Path, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::functions::backend::{
    media::{delete as do_delete, get as do_select},
    media_insert_log::delete as do_delete_log,
};

#[tracing::instrument]
pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Path(id): Path<String>,
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

    let item = do_select(id.clone())
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "Not found".to_string()))?;

    do_delete_log(&item.created_at.to_rfc3339())
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    do_delete(id)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(())
}
