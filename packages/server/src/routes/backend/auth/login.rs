use anyhow::Result;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{ConnectInfo, Json, State},
    response::IntoResponse,
};
use hyper::StatusCode;

use _database::{functions::frontend::auth::login as do_login, RouteEnv};
use _types::request::LoginInfo;

type LogItem = (SocketAddr, DateTime<Utc>);
static LOGIN_LOG: Lazy<Arc<Mutex<Vec<LogItem>>>> = Lazy::new(|| Arc::new(Mutex::new(Vec::new())));

#[tracing::instrument(skip_all, parent = None, fields(ip = %real_ip, name = %args.name))]
pub async fn login(
    ConnectInfo(real_ip): ConnectInfo<SocketAddr>,
    State(env): State<RouteEnv>,
    args: Json<LoginInfo>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Write the login log
    let now = Utc::now();
    LOGIN_LOG
        .lock()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot lock login log: {}", err),
            )
        })?
        .push((real_ip, now));

    // Clear the login log that is older than 1 minute
    LOGIN_LOG
        .lock()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot lock login log: {}", err),
            )
        })?
        .retain(|(_, time)| now.signed_duration_since(*time).num_seconds() < 60);

    // Check if the user is trying to login too frequently
    // Limit to 5 times per minute
    let count = LOGIN_LOG
        .lock()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot lock login log: {}", err),
            )
        })?
        .iter()
        .filter(|(ip, time)| ip == &real_ip && now.signed_duration_since(*time).num_seconds() < 60)
        .count();
    if count > 5 {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests".to_string(),
        ));
    }

    let ret = do_login(env.clone(), args.name.clone(), args.password_raw.clone())
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot login: {}", err),
            )
        })?;

    Ok(Json(ret))
}
