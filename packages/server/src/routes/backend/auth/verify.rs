use anyhow::Result;

use axum::response::IntoResponse;
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::functions::frontend::auth::verify as do_verify;

#[tracing::instrument]
pub async fn verify(
    bearer: TypedHeader<Authorization<Bearer>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    do_verify(bearer.token().to_string()).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot verify: {}", e),
        )
    })?;

    Ok(())
}
