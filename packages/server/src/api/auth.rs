//! `POST /api/v1/auth/login|refresh`, `GET /api/v1/auth/verify` —
//! transition-era password + JWT, semantics matching the legacy engine
//! (15-day tokens, per-user watermark, 5/min/IP login limit).

use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::Json;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::auth::{clears_watermark, effective_client_ip, verify_password, watermark_key, Claims};
use crate::error::ApiError;
use crate::state::AppState;
use i0721_proto::UserDto;

/// The login request body.
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct LoginRequest {
    /// Login name.
    pub username: String,
    /// Plaintext password.
    pub password: String,
}

/// The authenticated-session response.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct AuthSession {
    /// The bearer token.
    pub token: String,
    /// Token expiry (RFC 3339).
    pub expires_at: String,
    /// The authenticated user.
    pub user: UserDto,
}

/// Extract and verify the bearer token, then load the user.
///
/// The watermark check reproduces the legacy engine: a token minted
/// before the user's last login (minus 5 s leeway) is expired.
pub(crate) async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Claims, i0721_proto::UserRecord), ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    let claims = state
        .signer
        .verify(token)
        .map_err(|err| ApiError::unauthorized(format!("invalid token: {err}")))?;
    let user_id: uuid::Uuid = claims
        .sub
        .parse()
        .map_err(|_| ApiError::unauthorized("invalid token subject"))?;
    let user = state
        .store
        .get_user(user_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("user not found"))?;
    let watermark = state
        .store
        .get_setting(&watermark_key(user.id))
        .await?
        .and_then(|value| value.as_str().map(str::to_owned));
    if !clears_watermark(watermark.as_deref(), claims.iat) {
        return Err(ApiError::unauthorized("token expired"));
    }
    Ok((claims, user))
}

/// `POST /api/v1/auth/login`
pub async fn login(
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Json<AuthSession>, ApiError> {
    let client_ip = effective_client_ip(&state.config.trusted_proxies, source.ip(), &headers);
    state.login_limiter.record(client_ip)?;

    let user = state
        .store
        .get_user_by_username(&request.username)
        .await?
        .ok_or_else(|| ApiError::unauthorized("invalid credentials"))?;
    let password_ok = user
        .password_hash
        .as_deref()
        .map(|hash| verify_password(&request.password, hash))
        .unwrap_or(false);
    if !password_ok {
        return Err(ApiError::unauthorized("invalid credentials"));
    }

    let now = Timestamp::now();
    let token = state.signer.sign(user.id, now);
    // A fresh login supersedes older sessions — the legacy behavior.
    state
        .store
        .set_setting(
            &watermark_key(user.id),
            serde_json::Value::String(now.to_string()),
        )
        .await?;

    Ok(Json(AuthSession {
        token,
        expires_at: expiry_rfc3339(now),
        user: user.into(),
    }))
}

fn expiry_rfc3339(now: Timestamp) -> String {
    let exp = now
        .checked_add(jiff::Span::new().hours(crate::auth::TOKEN_TTL_DAYS * 24))
        .unwrap_or(now);
    exp.to_string()
}
