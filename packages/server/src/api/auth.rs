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
/// (the XSS-exposure class that motivated the session-layer move).
///
/// `SameSite=Lax`, NOT `Strict`: this service's whole point is sharing
/// links — a recipient who is already signed in and follows a share link
/// from another site arrives on a TOP-LEVEL navigation, and `Strict`
/// would strip the session cookie from exactly that request (they would
/// look signed-out until they navigated internally). `Lax` still keeps
/// the cookie off cross-site sub-requests and off state-changing POSTs,
/// so the CSRF face the earlier comment worried about stays closed: our
/// mutations ride POST/JSON-RPC, and every one of them is a non-top-level
/// cross-site request.
///
/// `Secure` is added when the deployment is served over https (decided
/// per request from [`crate::origin::serves_https`]) so the cookie is
/// never sent over a plaintext hop in production, while local http dev
/// keeps working.
#[must_use]
pub fn cookie_attributes(secure: bool) -> &'static str {
    if secure {
        " HttpOnly; SameSite=Lax; Path=/; Secure"
    } else {
        " HttpOnly; SameSite=Lax; Path=/"
    }
}

/// Extract the credential, the family's fixed four-channel order:
/// cookie → `Authorization: Bearer` → `Sec-WebSocket-Protocol: jwt.<t>`
/// → `?token=` (the last resort). The browser path is the cookie (a
/// WS-side copy of this extractor lives in rpc.rs, where the query
/// channel additionally honors one-time admission tickets); the other
/// channels stay for non-browser clients.
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

/// system_settings key for the password kill-switch. Precedence: a
/// well-formed row first, then the env pin (`I0721_PASSWORD_LOGIN`),
/// then default ON (a fresh deployment with no OAuth configured must
/// not lock itself out).
pub const PASSWORD_LOGIN_SETTINGS_KEY: &str = "auth_password_login";

/// The wire message when the switch is off (chest-identical wording so
/// a shared frontend error table maps both).
pub const PASSWORD_LOGIN_DISABLED_MESSAGE: &str = "password login is disabled";

/// The kill-switch's decision core (pure, so the precedence matrix is
/// testable without a store): a well-formed row wins; a JUNK row
/// arrives as `None` and defers to the env pin instead of failing OPEN
/// — a malformed operator edit must not silently re-enable a disabled
/// surface. Unpinned = on.
pub(crate) fn resolve_password_login(row: Option<bool>, env_pin: Option<bool>) -> bool {
    row.or(env_pin).unwrap_or(true)
}

/// Whether the password credential surface is offered.
pub(crate) async fn password_login_enabled(state: &AppState) -> bool {
    let row = state
        .store
        .get_setting(PASSWORD_LOGIN_SETTINGS_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.as_bool());
    let env_pin = std::env::var("I0721_PASSWORD_LOGIN").ok().and_then(|raw| {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    });
    resolve_password_login(row, env_pin)
}

/// `GET /api/v1/auth/login-methods` — the public login-page descriptor:
/// the password kill-switch's state, the enabled provider summary, and the
/// family login-policy document (`kirino::auth::mode::LoginPolicy`,
/// validated before it is served). NO secret material anywhere in the
/// payload.
pub async fn login_methods(
    State(state): State<AppState>,
) -> axum::response::Json<crate::oauth::LoginMethods> {
    let password = password_login_enabled(&state).await;
    let providers = crate::oauth::load_providers(&state).await;
    axum::response::Json(crate::oauth::login_methods_document(password, &providers))
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
    // The password kill-switch: fail closed for every password attempt
    // when the switch is off (login-methods already hides the form).
    if !password_login_enabled(&state).await {
        return Err(ApiError::forbidden(PASSWORD_LOGIN_DISABLED_MESSAGE));
    }
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
    Ok(session_cookie(
        &token,
        session,
        crate::origin::serves_https(&state, &headers),
    ))
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
    Ok(session_cookie(
        &token,
        session,
        crate::origin::serves_https(&state, &headers),
    ))
}

/// `POST /api/v1/auth/logout` — expires the cookie and supersedes every
/// outstanding token for the user (the watermark bump: the next request
/// with ANY pre-logout credential fails). SameSite=Lax keeps the
/// endpoint CSRF-safe (a cross-site POST never carries the cookie); an
/// unauthenticated call merely clears a cookie that was already dead.
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
    let attributes = cookie_attributes(crate::origin::serves_https(&state, &headers));
    let cleared =
        format!("{TOKEN_COOKIE}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{attributes}");
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cleared)]).into_response())
}

/// The one-time WebSocket admission ticket response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct WsTicket {
    /// The single-use, ≤30 s ticket for the `?token=` slot of the WS
    /// upgrade. Never a session credential.
    pub ticket: String,
}

/// `POST /api/v1/auth/ws-ticket` — mint a one-time WebSocket admission
/// ticket for the authenticated user (any of the four channels; the
/// browser rides the cookie). The ticket opens the socket and dies with
/// the opening — see `ws_tickets`.
pub async fn ws_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Result<Json<WsTicket>, ApiError> {
    let (_, user) = require_auth(&state, &headers, &uri).await?;
    Ok(Json(WsTicket {
        ticket: state.ws_tickets.mint(user.id),
    }))
}

/// Wrap a session body with its Set-Cookie.
fn session_cookie(token: &str, session: AuthSession, secure: bool) -> Response {
    let cookie = format!(
        "{TOKEN_COOKIE}={token}; Max-Age={}{}",
        crate::auth::TOKEN_TTL_DAYS * 24 * 3600,
        cookie_attributes(secure)
    );
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(session),
    )
        .into_response()
}

pub(crate) fn expiry_rfc3339(now: Timestamp) -> String {
    let exp = now
        .checked_add(jiff::Span::new().hours(crate::auth::TOKEN_TTL_DAYS * 24))
        .unwrap_or(now);
    exp.to_string()
}

#[cfg(test)]
mod password_switch_tests {
    use super::*;

    #[test]
    fn decision_matrix_row_wins_then_env_then_default_on() {
        // A well-formed row wins over everything.
        assert!(resolve_password_login(Some(true), Some(false)));
        assert!(!resolve_password_login(Some(false), Some(true)));
        // Junk rows arrive as None and defer to the env pin.
        assert!(resolve_password_login(None, Some(true)));
        assert!(!resolve_password_login(None, Some(false)));
        // Unpinned defaults ON (fresh deployments must not lock out).
        assert!(resolve_password_login(None, None));
    }
}
