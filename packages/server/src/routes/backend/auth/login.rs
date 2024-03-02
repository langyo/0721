use anyhow::Result;
use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Json},
    response::IntoResponse,
};
use hyper::{HeaderMap, StatusCode};

use _database::{functions::frontend::auth::login as do_login, types::request::LoginInfo};

#[tracing::instrument]
pub async fn login(
    headers: HeaderMap,
    ConnectInfo(real_ip): ConnectInfo<SocketAddr>,
    args: Json<LoginInfo>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let ret = do_login(args.name.clone(), args.password_raw.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot login: {}", e),
            )
        })?;

    Ok(Json(ret))
}
