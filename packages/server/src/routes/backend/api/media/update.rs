use anyhow::Result;

use axum::{
    extract::{Path, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{functions::backend::media::delete as do_delete, RouteEnv};

#[tracing::instrument(skip_all, parent = None)]
pub async fn delete(
    ExtractAuthInfo(info): ExtractAuthInfo,
    State(env): State<RouteEnv>,
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

    do_delete(env, id)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(())
}
