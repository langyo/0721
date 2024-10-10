use anyhow::Result;

use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use axum_extra::{
    headers::{authorization::Bearer, Authorization},
    TypedHeader,
};
use hyper::StatusCode;

use _database::{functions::frontend::auth::refresh as do_refresh, RouteEnv};

#[tracing::instrument(skip_all, parent = None)]
pub async fn refresh(
    bearer: TypedHeader<Authorization<Bearer>>,
    State(env): State<RouteEnv>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_refresh(env, bearer.token().to_string())
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot refresh: {}", err),
            )
        })?;

    Ok(Json(ret))
}
