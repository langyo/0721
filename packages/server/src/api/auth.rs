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

/// The account-first ladder's first-step body: just the name the caller
/// typed (no password — that comes after the factors are known).
#[derive(Debug, Clone, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct AccountMethodsRequest {
    /// Login name as typed; surrounding whitespace is trimmed server-side.
    pub username: String,
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
///
/// `503` when the catalog yields a policy this service cannot honour (see
/// [`crate::oauth::login_methods_document`]): a login page that cannot
/// render the truth is refused, not silently downgraded.
pub async fn login_methods(
    State(state): State<AppState>,
) -> Result<axum::response::Json<crate::oauth::LoginMethods>, ApiError> {
    let password = password_login_enabled(&state).await;
    let providers = crate::oauth::load_providers(&state).await;
    let document = crate::oauth::login_methods_document(password, &providers)?;
    Ok(axum::response::Json(document))
}

/// `POST /api/v1/auth/account-methods` — the account-first ladder's first
/// step: the caller knows a name and asks how THAT account signs in.
///
/// ## The two facts this endpoint must not leak
///
/// This is a PUBLIC, unauthenticated surface taking a caller-chosen
/// username, so it is a textbook account-enumeration oracle. Two rules
/// keep it shut:
///
/// 1. **Shape parity** — a name that matches no user answers exactly like
///    a name that matches a password-carrying one: `password` factor
///    present, zero oauth rows. Never an empty list, never a 404. See
///    [`account_factors`] for the construction and `routes.rs` for the
///    test that compares the two responses as whole documents.
/// 2. **Provider parity** — the "this account has these linked
///    providers" answer exists only when the account really has linked
///    providers; it comes from the account's own binding rows, never by
///    joining against the global provider list (a global question would
///    answer identically for every name and make the honest absence
///    indistinguishable from a bug).
///
/// Rate limiting rides the same per-IP budget as [`login`]: both are
/// guess surfaces for the same secret, and sharing the log means this
/// endpoint cannot be used to widen the budget.
pub async fn account_methods(
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AccountMethodsRequest>,
) -> Result<Json<AccountMethods>, ApiError> {
    let client_ip = effective_client_ip(&state.config.trusted_proxies, source.ip(), &headers);
    state.login_limiter.record(client_ip)?;

    let password_switch = password_login_enabled(&state).await;
    let user = state
        .store
        .get_user_by_username(request.username.trim())
        .await?;
    let linked = match &user {
        Some(user) => state.store.oauth_providers_for_user(user.id).await?,
        None => Vec::new(),
    };
    let has_password_hash = user
        .as_ref()
        .is_some_and(|user| user.password_hash.is_some());

    // Label resolution reads the provider registry (authentication state,
    // not a secret): an enabled row names itself, a bound provider whose
    // row is gone still gets a row so the account's real options survive
    // an operator's edit.
    let providers = crate::oauth::load_providers(&state).await;
    let enabled: Vec<_> = providers.iter().filter(|row| row.enabled).collect();
    let factors = account_factors(
        &AccountFacts {
            has_password_hash,
            password_switch,
            linked_providers: &linked,
        },
        &ProviderLabels::new(&enabled),
    );

    Ok(Json(AccountMethods {
        factors,
        password: password_switch,
    }))
}

/// One sign-in factor an account has.
///
/// Externally tagged on `kind`: `{"kind":"password"}` and
/// `{"kind":"oauth","provider":"erp","label":"Celestia ID"}` — the ladder
/// front end switches on `kind` and never has to read a nullable pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind")]
#[ts(export, export_to = "../../webui/src/types/")]
pub enum LoginFactor {
    /// The account carries a password hash AND the deployment's password
    /// switch is on.
    #[serde(rename = "password")]
    Password,
    /// A third-party identity is linked to the account; `provider` is the
    /// registry id of the row to start the OAuth flow for.
    ///
    /// Renamed explicitly: the default `rename_all` spelling of this
    /// variant would be `o_auth`, which is not what the ladder front end
    /// and the generated TS binding read.
    #[serde(rename = "oauth")]
    OAuth {
        /// Registry id (`erp`, `github`, …).
        provider: String,
        /// Display label for the ladder row.
        label: String,
    },
}

/// The `account-methods` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct AccountMethods {
    /// At least one entry, always — see [`account_factors`].
    pub factors: Vec<LoginFactor>,
    /// The deployment-wide password switch, echoed so the ladder can tell
    /// "this account has no password" from "passwords are off here".
    pub password: bool,
}

/// The facts the factor decision needs, separated from the store so the
/// anti-enumeration parity is testable without a database.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccountFacts<'a> {
    /// `users.password_hash IS NOT NULL`.
    pub has_password_hash: bool,
    /// The global password kill-switch (see [`password_login_enabled`]).
    pub password_switch: bool,
    /// Provider ids bound to the account (`oauth_identities` rows).
    pub linked_providers: &'a [String],
}

/// The display labels this deployment can put on a provider row.
pub(crate) struct ProviderLabels<'a> {
    providers: &'a [&'a crate::oauth::OAuthProvider],
}

/// The brand name of the family's central identity provider. Its registry
/// `display_name` is operator-editable, and an empty one would render as
/// a nameless row, so the brand is the fallback.
pub(crate) const CELESTIA_ID_LABEL: &str = "Celestia ID";

impl<'a> ProviderLabels<'a> {
    /// Wrap the ENABLED provider rows (disabled ones must not be offered).
    pub(crate) fn new(providers: &'a [&'a crate::oauth::OAuthProvider]) -> Self {
        Self { providers }
    }

    /// The row label for `provider_id`.
    fn label(&self, provider_id: &str) -> String {
        let row = self.providers.iter().find(|row| row.id == provider_id);
        match row {
            Some(row) if !row.display_name.trim().is_empty() => row.display_name.clone(),
            Some(row) if row.provider_type == "builtin:erp" => CELESTIA_ID_LABEL.to_owned(),
            // A bound provider whose registry row is gone (or disabled):
            // the account really has this option, so name it by its id
            // rather than dropping it — the flow entry would 404 anyway,
            // and hiding a real path is worse than naming it plainly.
            _ => provider_id.to_owned(),
        }
    }
}

/// Decide an account's factors — the anti-enumeration core.
///
/// The invariant, stated as the property the tests pin: **the returned set
/// is never empty, and every empty "unknown account" answer is exactly the
/// answer a password-carrying account with no linked provider gets.**
///
/// The parity is structural rather than conditional: a missing user
/// arrives here as `has_password_hash = false, linked_providers = []`, and
/// the FIRST branch below is reached by every account whose password is
/// usable. So an unknown name and a normal password account produce the
/// same `vec![Password]` — there is no `if found_anything { … } else {
/// unreachable }` seam that a later edit could fill differently. The only
/// accounts that differ from that baseline are the ones that really have
/// a linked provider, which is the fact this endpoint exists to report.
///
/// The floor is what makes this hold for every account, not just the
/// common one: an OAuth-only account (NULL hash, one linked identity)
/// announces exactly its provider, and an unknown name — which has no
/// provider — announces the password baseline. So the answer space is
/// `{password} ∪ {that account's providers}`, and the ONLY difference an
/// unknown name can produce is the absence of providers, which is a fact
/// any caller can also produce by testing an account it has already seen.
///
/// Note the deliberate asymmetry of the baseline: it is emitted from the
/// deployment switch alone, never from anything account-specific, so the
/// shapes available to an unlinked name are identical for every input.
pub(crate) fn account_factors(
    facts: &AccountFacts<'_>,
    labels: &ProviderLabels<'_>,
) -> Vec<LoginFactor> {
    let mut factors = Vec::new();
    // The password factor is the account's OWN fact first: an account with
    // no hash never announces a password, whatever the deployment switch
    // says. The switch only gates the switch-independent baseline below.
    if facts.has_password_hash && facts.password_switch {
        factors.push(LoginFactor::Password);
    }
    for provider in facts.linked_providers {
        // Dedupe per provider id, not per label: the store already folds
        // multiple subjects of one provider into one id, and a duplicate
        // row would render twice.
        let seen = factors.iter().any(|factor| {
            matches!(factor, LoginFactor::OAuth { provider: bound, .. } if bound == provider)
        });
        if seen {
            continue;
        }
        factors.push(LoginFactor::OAuth {
            provider: provider.clone(),
            label: labels.label(provider),
        });
    }
    if factors.is_empty() {
        // Only reachable with the password switch OFF and nothing linked —
        // i.e. the shape a `password:false` deployment needs. `Password`
        // here is the switch-independent baseline: it is what a password
        // account's ladder would have shown anyway, so an unknown name
        // keeps the same answer while the switch is on.
        factors.push(LoginFactor::Password);
    }
    factors
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

#[cfg(test)]
mod account_factor_tests {
    use super::*;

    fn provider(id: &str, display_name: &str, provider_type: &str) -> crate::oauth::OAuthProvider {
        crate::oauth::OAuthProvider {
            id: id.to_owned(),
            display_name: display_name.to_owned(),
            provider_type: provider_type.to_owned(),
            client_id: "client".to_owned(),
            client_secret: String::new(),
            base_url: None,
            issuer_url: None,
            scopes: None,
            enabled: true,
            icon: None,
        }
    }

    /// The rows every case below resolves against: one erp (the family
    /// identity) and one generic provider.
    fn registry() -> Vec<crate::oauth::OAuthProvider> {
        vec![
            provider("erp", "Celestia ID", "builtin:erp"),
            provider("gh", "GitHub", "builtin:github"),
        ]
    }

    fn factors_for(
        has_password_hash: bool,
        password_switch: bool,
        linked: &[String],
    ) -> Vec<LoginFactor> {
        let rows = registry();
        let enabled: Vec<&crate::oauth::OAuthProvider> = rows.iter().collect();
        account_factors(
            &AccountFacts {
                has_password_hash,
                password_switch,
                linked_providers: linked,
            },
            &ProviderLabels::new(&enabled),
        )
    }

    #[test]
    fn password_account_lists_the_password_factor() {
        assert_eq!(
            factors_for(true, true, &[]),
            vec![LoginFactor::Password],
            "a password-carrying account announces exactly the password factor"
        );
    }

    #[test]
    fn unknown_account_has_the_same_shape_as_a_password_account() {
        // MUT:enumeration-parity — this is the anti-enumeration pin at the
        // decision layer. A missing user reaches `account_factors` as
        // (false, [], switch) and MUST render what a password account
        // renders; turning the fallback into an empty vec (or a 404 at the
        // route) reddens here.
        assert_eq!(
            factors_for(false, true, &[]),
            factors_for(true, true, &[]),
            "unknown and password accounts must be indistinguishable"
        );
        assert_eq!(factors_for(false, true, &[]), vec![LoginFactor::Password]);
    }

    #[test]
    fn passwordless_account_without_links_still_answers_with_a_factor() {
        // Reachable state: an OAuth-only account whose provider link was
        // removed, or an admin-created user without a password. An empty
        // factor list would render a dead ladder AND become the one shape
        // that means "no such account" — so the ladder always has a floor.
        assert_eq!(factors_for(false, true, &[]), vec![LoginFactor::Password]);
        // With the password switch OFF the floor is the same factor: the
        // baseline must not change with the switch, or the switch itself
        // becomes an oracle for "does this name exist".
        assert_eq!(factors_for(false, false, &[]), vec![LoginFactor::Password]);
        assert_eq!(factors_for(true, false, &[]), vec![LoginFactor::Password]);
    }

    #[test]
    fn linked_providers_append_oauth_factors_with_brand_labels() {
        let linked = vec!["erp".to_owned(), "gh".to_owned()];
        assert_eq!(
            factors_for(true, true, &linked),
            vec![
                LoginFactor::Password,
                LoginFactor::OAuth {
                    provider: "erp".to_owned(),
                    label: "Celestia ID".to_owned(),
                },
                LoginFactor::OAuth {
                    provider: "gh".to_owned(),
                    label: "GitHub".to_owned(),
                },
            ],
            "each bound provider is one factor, labelled by its registry row"
        );
    }

    #[test]
    fn a_passwordless_oauth_account_announces_only_its_providers() {
        // The state the endpoint exists for: no password hash (NULL), one
        // linked identity. The password switch is ON deployment-wide, yet
        // no password factor may appear — the account genuinely cannot use
        // one.
        let linked = vec!["erp".to_owned()];
        assert_eq!(
            factors_for(false, true, &linked),
            vec![LoginFactor::OAuth {
                provider: "erp".to_owned(),
                label: "Celestia ID".to_owned(),
            }]
        );
    }

    #[test]
    fn duplicate_and_unregistered_providers_still_render_once() {
        // The store folds subjects; a store bug that sends the same id
        // twice must not put two identical rows on the ladder.
        let linked = vec!["erp".to_owned(), "erp".to_owned()];
        assert_eq!(
            factors_for(false, true, &linked),
            vec![LoginFactor::OAuth {
                provider: "erp".to_owned(),
                label: "Celestia ID".to_owned(),
            }]
        );

        // MUT:factor-dedupe-key — the dedupe must compare the provider id,
        // not the label. Two rows with the SAME label are still two
        // providers; keying on the label would let one row swallow the
        // other and the account would lose a real option.
        let same_label = ["gh".to_owned(), "tw".to_owned()];
        assert_eq!(
            factors_for(false, true, &same_label),
            vec![
                LoginFactor::OAuth {
                    provider: "gh".to_owned(),
                    label: "GitHub".to_owned(),
                },
                LoginFactor::OAuth {
                    provider: "tw".to_owned(),
                    // not in the registry: named by its id, exactly once.
                    label: "tw".to_owned(),
                },
            ]
        );

        // A bound provider whose registry row was deleted or renamed: the
        // account's real option survives, named by its id.
        let unregistered = vec!["gone".to_owned()];
        assert_eq!(
            factors_for(false, true, &unregistered),
            vec![LoginFactor::OAuth {
                provider: "gone".to_owned(),
                label: "gone".to_owned(),
            }]
        );
    }

    #[test]
    fn label_falls_back_to_the_brand_when_the_operator_blanked_it() {
        let rows = [provider("erp", "  ", "builtin:erp")];
        let enabled: Vec<&crate::oauth::OAuthProvider> = rows.iter().collect();
        let labels = ProviderLabels::new(&enabled);
        assert_eq!(labels.label("erp"), CELESTIA_ID_LABEL);
    }

    #[test]
    fn wire_shape_switches_on_kind() {
        // The TS binding is generated from this shape, so the tag/spelling
        // is contract: `{"kind":"password"}` and
        // `{"kind":"oauth","provider":…,"label":…}` with no null fields.
        let password = serde_json::to_value(LoginFactor::Password).unwrap();
        assert_eq!(password, serde_json::json!({"kind": "password"}));
        let oauth = serde_json::to_value(LoginFactor::OAuth {
            provider: "erp".to_owned(),
            label: "Celestia ID".to_owned(),
        })
        .unwrap();
        assert_eq!(
            oauth,
            serde_json::json!({"kind": "oauth", "provider": "erp", "label": "Celestia ID"})
        );
    }
}
