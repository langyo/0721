use anyhow::Result;
use serde::{Deserialize, Serialize};

use axum::{
    extract::{Multipart, Query},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::functions::backend::{
    media::{get as do_select, set as do_insert},
    media_insert_log::insert as do_insert_log,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameArgs {
    pub name: Option<String>,
}

#[tracing::instrument]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    Query(args): Query<NameArgs>,
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

        let db_key = do_insert(info.name, data, args.name)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        log::debug!("do_insert db_key: {:?}", db_key);
        let item = do_select(&db_key)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "No item after insert".to_string(),
            ))?;

        do_insert_log(&db_key, item.created_at)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok(db_key)
    } else {
        Err((StatusCode::FORBIDDEN, "No permission".to_string()))
    }
}
