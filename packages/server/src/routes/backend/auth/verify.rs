use anyhow::Result;

use axum::{extract::State, response::IntoResponse};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::{functions::frontend::auth::verify as do_verify, RouteEnv};

#[tracing::instrument(skip_all, parent = None)]
pub async fn verify(
    bearer: TypedHeader<Authorization<Bearer>>,
    State(env): State<RouteEnv>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    do_verify(env, bearer.token().to_string())
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot verify: {}", err),
            )
        })?;

    Ok(())
}
