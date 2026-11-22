//! `POST /api/v1/auth/login|refresh`, `GET /api/v1/auth/verify` —
//! transition-era password + JWT, semantics matching the legacy engine
//! (15-day tokens, per-user watermark, 5/min/IP login limit).

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

use crate::auth::{clears_watermark, effective_client_ip, verify_password, watermark_key, Claims};

/// The cookie the login sets (the browser session's credential — see
/// [`COOKIE_ATTRIBUTES`]).
pub const TOKEN_COOKIE: &str = "i0721-token";

/// The cookie's attributes. `HttpOnly` keeps the JWT out of JS reach
/// (the XSS-exposure class that motivated the session-layer move);
/// `SameSite=Strict` rides the same-origin deployment (the reverse
/// proxy fronts the service, so there is no cross-site legit caller —
/// and CSRF on the cookie lane dies with Strict). No `Secure` flag:
/// the backend serves plain HTTP behind the TLS-terminating proxy, and
/// a Secure cookie would never be set over that hop (revisit if the
/// service is ever exposed directly).
pub const COOKIE_ATTRIBUTES: &str = " HttpOnly; SameSite=Strict; Path=/";

/// Extract the credential, the family's fixed four-channel order:
/// cookie → `Authorization: Bearer` → `Sec-WebSocket-Protocol: jwt.<t>`
/// → `?token=` (the last resort). The browser path is the cookie (the WS-side copy of this extractor lives in rpc.rs); the
/// other channels stay for non-browser clients.
pub(crate) fn extract_token(headers: &HeaderMap, uri: &axum::http::Uri) -> Option<String> {
    if let Some(cookie) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
    {
        for pair in cookie.split(';') {
            let pair = pair.trim();
            if let Some(value) = pair.strip_prefix(&format!("{TOKEN_COOKIE}=")) {
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    if let Some(bearer) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
        })
    {
        return Some(bearer.to_owned());
    }
    // Sec-WebSocket-Protocol: jwt.<token> — keeps the credential out of
    // the URL (browser WebSocket cannot set headers).
    if let Some(jwt) = headers
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("jwt."))
    {
        return Some(jwt.to_owned());
    }
    // Last resort: the query parameter.
    uri.query()
        .and_then(|query| {
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("token=").filter(|t| !t.is_empty()))
        })
        .map(str::to_owned)
}
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

/// The authenticated-session response. The credential itself rides the
/// httpOnly cookie the login/refresh handlers set — the body carries
/// only the expiry and the user (a token in the body would hand the
/// JWT straight back to any script on the page, defeating HttpOnly).
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct AuthSession {
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
    uri: &axum::http::Uri,
) -> Result<(Claims, i0721_proto::UserRecord), ApiError> {
    let token = extract_token(headers, uri)
        .ok_or_else(|| ApiError::unauthorized("missing bearer token"))?;
    let claims = state
        .signer
        .verify(&token)
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

/// `POST /api/v1/auth/login` — mints the session and plants it as an
/// httpOnly cookie; the body carries only the expiry and the user (the
/// cookie is the credential — see [`AuthSession`]).
pub async fn login(
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, ApiError> {
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

    let session = AuthSession {
        expires_at: expiry_rfc3339(now),
        user: user.into(),
    };
    Ok(session_cookie(&token, session))
}

/// `POST /api/v1/auth/refresh` — cookie rotation: verifies the current
/// session (any of the four channels; browsers ride the cookie), mints
/// a fresh token and re-plants the cookie. No watermark bump — a
/// refresh keeps the current session alive (the deliberate semantics
/// carried over from the REST era).
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Result<Response, ApiError> {
    let (_, user) = require_auth(&state, &headers, &uri).await?;
    let now = Timestamp::now();
    let token = state.signer.sign(user.id, now);
    let session = AuthSession {
        expires_at: expiry_rfc3339(now),
        user: user.into(),
    };
    Ok(session_cookie(&token, session))
}

/// `POST /api/v1/auth/logout` — expires the cookie and supersedes every
/// outstanding token for the user (the watermark bump: the next request
/// with ANY pre-logout credential fails). SameSite=Strict keeps the
/// endpoint CSRF-safe; an unauthenticated call merely clears a cookie
/// that was already dead.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Result<Response, ApiError> {
    if let Ok((_, user)) = require_auth(&state, &headers, &uri).await {
        // Stamp PAST the 5 s leeway: the watermark check tolerates
        // tokens minted within 5 s of it (legacy login-skew allowance),
        // so a plain `now` stamp would leave a just-logged-in session
        // alive for the leeway window. Logout must kill immediately.
        let now = Timestamp::now();
        let effective = now
            .checked_add(jiff::Span::new().seconds(crate::auth::WATERMARK_LEEWAY_SECONDS + 1))
            .unwrap_or(now);
        state
            .store
            .set_setting(
                &watermark_key(user.id),
                serde_json::Value::String(effective.to_string()),
            )
            .await?;
    }
    let cleared = format!(
        "{TOKEN_COOKIE}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{COOKIE_ATTRIBUTES}"
    );
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cleared)]).into_response())
}

/// Wrap a session body with its Set-Cookie.
fn session_cookie(token: &str, session: AuthSession) -> Response {
    let cookie = format!(
        "{TOKEN_COOKIE}={token}; Max-Age={}{}",
        crate::auth::TOKEN_TTL_DAYS * 24 * 3600,
        COOKIE_ATTRIBUTES
    );
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(session),
    )
        .into_response()
}

fn expiry_rfc3339(now: Timestamp) -> String {
    let exp = now
        .checked_add(jiff::Span::new().hours(crate::auth::TOKEN_TTL_DAYS * 24))
        .unwrap_or(now);
    exp.to_string()
}
