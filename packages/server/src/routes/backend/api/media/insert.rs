use anyhow::Result;
use serde::{Deserialize, Serialize};

use axum::{
    extract::{Multipart, Query, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{functions::backend::media::set as do_insert, RouteEnv};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameArgs {
    pub name: Option<String>,
}

#[tracing::instrument(skip_all, parent = None)]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    State(env): State<RouteEnv>,
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

        let db_key = do_insert(env.clone(), info.name, data, args.name)
            .await
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok(db_key)
    } else {
        Err((StatusCode::FORBIDDEN, "No permission".to_string()))
    }
}
