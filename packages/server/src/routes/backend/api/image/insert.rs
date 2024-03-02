use anyhow::Result;

use axum::{extract::Multipart, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _database::{functions::backend::image::insert as do_insert, types::response::AuthInfo};

#[tracing::instrument]
pub async fn insert(
    ExtractAuthInfo(info): ExtractAuthInfo,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    match info {
        AuthInfo::User(info) => {
            let data = multipart
                .next_field()
                .await
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
                .ok_or((StatusCode::BAD_REQUEST, "No file".to_string()))?;
            let data = data
                .bytes()
                .await
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

            do_insert(info.name, data)
                .await
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
            Ok(())
        }
        _ => Err((StatusCode::FORBIDDEN, "No permission".to_string())),
    }
}
