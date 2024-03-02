use anyhow::Result;

use axum::{extract::Json, response::IntoResponse};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::functions::frontend::auth::refresh as do_refresh;

#[tracing::instrument]
pub async fn refresh(
    bearer: TypedHeader<Authorization<Bearer>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_refresh(bearer.token().to_string()).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot refresh: {}", e),
        )
    })?;

    Ok(Json(ret))
}
