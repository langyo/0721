use anyhow::Result;

use axum::{extract::Multipart, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::functions::backend::{
    media::{get as do_select, set as do_insert},
    media_insert_log::insert as do_insert_log,
};

#[tracing::instrument]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if let Some(info) = info {
        let data = multipart
            .next_field()
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            .ok_or((StatusCode::BAD_REQUEST, "No file".to_string()))?;
        let data = data
            .bytes()
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let ret = do_insert(info.name, data)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        let item = do_select(&ret)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "No item after insert".to_string(),
            ))?;

        do_insert_log(&ret, item.created_at)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok(ret)
    } else {
        Err((StatusCode::FORBIDDEN, "No permission".to_string()))
    }
}
