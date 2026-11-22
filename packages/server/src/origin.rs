//! The deployment's public origin — ONE derivation shared by every lane
//! that must emit an absolute URL (the OAuth redirect URI, the MCP share
//! links).
//!
//! `I0721_PUBLIC_ORIGIN` (the deployment's A-layer fact) wins when set;
//! otherwise the request's `Host` header is used with a warning — a
//! derivation the reverse proxy's Host preservation makes right in
//! practice, but which the env pin makes explicit and auditable. `None`
//! when neither exists: a lane that cannot derive an origin must fail
//! loudly instead of inventing a `localhost` URL that no client can
//! reach.

use axum::http::{header, HeaderMap};

use crate::state::AppState;

/// The absolute origin (`scheme://host`) this request is served under:
/// the configured public origin first, else the request `Host`.
#[must_use]
pub(crate) fn public_origin(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = state.config.public_origin.as_deref() {
        return Some(origin.to_owned());
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())?;
    tracing::warn!(
        host,
        "I0721_PUBLIC_ORIGIN unset — deriving the public origin from the Host header",
    );
    Some(format!("http://{host}"))
}

/// Whether THIS request arrived over https — what the session cookie's
/// `Secure` flag keys off.
///
/// The decision is per request, not per configuration: the deployment's
/// public origin being `https://` says nothing about how the request in
/// hand reached the process. A TLS-terminating edge forwards the real
/// scheme in `X-Forwarded-Proto`; a direct LAN http request carries
/// neither that header nor TLS, and marking the cookie `Secure` there
/// would make the browser drop it (login silently "succeeds" and then
/// nothing sticks).
///
/// `I0721_COOKIE_SECURE` pins the answer for edges that terminate TLS
/// without forwarding the scheme: `always` (public https deployments
/// behind such an edge), `never`, or `auto` (default — trust the
/// header).
#[must_use]
pub(crate) fn serves_https(state: &AppState, headers: &HeaderMap) -> bool {
    match state.config.cookie_secure.as_str() {
        "always" => true,
        "never" => false,
        _ => headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(|proto| proto.split(',').next().unwrap_or(proto).trim() == "https")
            .unwrap_or(false),
    }
}
