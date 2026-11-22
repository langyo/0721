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
