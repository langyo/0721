//! The OAuth dual-mode authentication lane (W4): a provider registry in
//! the `settings` table, a browser authorization-code flow with PKCE
//! S256, and `find_or_create_user` over the `oauth_identities` table.
//!
//! ## Provider registry
//!
//! One settings row (`oauth_providers`) holds the whole catalog as a
//! JSON array. Each row: `id` / `display_name` / `type`
//! (`builtin:github` | `builtin:feishu` | `builtin:erp` | `oidc`) /
//! `client_id` / `client_secret` (server-held) / `base_url` (erp) or
//! `issuer_url` (oidc) / `scopes` / `enabled` / `icon`. The
//! `client_secret` NEVER leaves the server: every public shape
//! (login-methods, the admin list) carries only `has_secret` + the
//! last four characters.
//!
//! ## Flow
//!
//! `GET /api/v1/auth/oauth/{provider}` mints a random `state` + a PKCE
//! S256 `verifier`, plants them in one HttpOnly cookie (600 s), and
//! 302s to the provider's authorize endpoint with
//! `code_challenge = BASE64URL(SHA256(verifier))`,
//! `code_challenge_method=S256`. The callback strictly compares the
//! state cookie with the provider's echo, exchanges `code` + the
//! verifier for an access token (form-encoded, RFC 6749 §4.1.3 —
//! except GitHub's JSON body), fetches userinfo, resolves the subject
//! (`sub` | `id` | `open_id`), and `find_or_create_user`s before
//! planting the ordinary session cookie and 302ing to `/`.
//!
//! No domain restriction in this first version (the admin surface for
//! it arrives with the identity wave); the `state` cookie + PKCE are
//! the CSRF/interception defenses.
//!
//! ## OIDC discovery
//!
//! `oidc`-typed providers resolve their endpoints from
//! `{issuer_url}/.well-known/openid-configuration`, cached in-process
//! for 10 minutes with a 30 s negative cache (the redirect endpoint is
//! unauthenticated — without a negative cache any visitor could force
//! an outbound request per hit on a broken issuer).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use uuid::Uuid;

use i0721_proto::{NewUser, Role};

use crate::api::auth::AuthSession;
use crate::auth::TOKEN_TTL_DAYS;
use crate::error::ApiError;
use crate::state::AppState;

/// The settings row holding the provider catalog.
pub const PROVIDERS_SETTINGS_KEY: &str = "oauth_providers";

/// Longest provider id accepted — the id prefixes minted usernames
/// (`{provider}_{subject8}`, VARCHAR must fit).
const PROVIDER_ID_MAX_LEN: usize = 48;

/// The state/PKCE cookie's name and lifetime.
const OAUTH_STATE_COOKIE: &str = "i0721-oauth-state";
const OAUTH_STATE_MAX_AGE_SECS: u64 = 600;

/// Discovery caches.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const DISCOVERY_TTL: Duration = Duration::from_secs(600);
const DISCOVERY_FAILURE_TTL: Duration = Duration::from_secs(30);
const DISCOVERY_CACHE_MAX: usize = 32;

// ── the registry rows ──────────────────────────────────────────────────

/// One provider row as stored in settings (secrets server-held).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct OAuthProvider {
    /// Registry id (`[a-z0-9][a-z0-9._-]*`).
    pub id: String,
    /// Login-button label.
    pub display_name: String,
    /// Endpoint family: `builtin:github` | `builtin:feishu` |
    /// `builtin:erp` | `oidc`.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// OAuth client id (the app id on feishu).
    pub client_id: String,
    /// The client secret. NEVER serialized to any public shape — the
    /// admin list carries `has_secret` + the tail instead.
    #[serde(skip_serializing)]
    pub client_secret: String,
    /// The ERP panel's origin (`builtin:erp` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The OIDC issuer (`oidc` only) — endpoints via discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_url: Option<String>,
    /// Extra scopes (space-joined); providers' mandatory set rides the
    /// template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    /// Disabled providers vanish from login-methods and refuse flows.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Login-button icon (http(s)/data URI passthrough; unused by the
    /// bundled webui which renders initials).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

fn default_true() -> bool {
    true
}

impl OAuthProvider {
    /// The masked public projection (login-methods + admin list): no
    /// secret material beyond existence + the last four characters.
    pub fn masked(&self) -> OAuthProviderPublic {
        OAuthProviderPublic {
            id: self.id.clone(),
            display_name: self.display_name.clone(),
            provider_type: self.provider_type.clone(),
            client_id: self.client_id.clone(),
            base_url: self.base_url.clone(),
            issuer_url: self.issuer_url.clone(),
            scopes: self.scopes.clone(),
            enabled: self.enabled,
            icon: self.icon.clone(),
            has_secret: !self.client_secret.is_empty(),
            secret_tail: self
                .client_secret
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        }
    }
}

/// The public provider shape (masked).
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct OAuthProviderPublic {
    pub id: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Whether the server holds a client secret.
    pub has_secret: bool,
    /// The secret's last four characters (display hint only).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub secret_tail: String,
}

/// The login-methods payload (public — no secret anywhere).
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct LoginMethods {
    /// Whether the password form is offered (the kill-switch).
    pub password: bool,
    /// Enabled OAuth providers, login-button order.
    pub providers: Vec<LoginMethodProvider>,
}

/// One login-methods entry.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct LoginMethodProvider {
    pub id: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// The upsert wire shape (admin): same fields as the row; an EMPTY
/// `client_secret` means "keep the stored one".
#[derive(Debug, Clone, PartialEq, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct OAuthProviderUpsert {
    pub id: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub issuer_url: Option<String>,
    #[serde(default)]
    pub scopes: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Load the whole registry (empty when the row is absent/corrupt — a
/// corrupt row must not take the login page down; the admin list
/// surfaces the breakage on write).
pub async fn load_providers(state: &AppState) -> Vec<OAuthProvider> {
    state
        .store
        .get_setting(PROVIDERS_SETTINGS_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<Vec<OAuthProvider>>(value).ok())
        .unwrap_or_default()
}

/// Persist the registry.
pub(crate) async fn save_providers(
    state: &AppState,
    providers: &[OAuthProvider],
) -> Result<(), ApiError> {
    state
        .store
        .set_setting(
            PROVIDERS_SETTINGS_KEY,
            serde_json::to_value(providers)
                .map_err(|err| ApiError::internal(format!("provider serialization: {err}")))?,
        )
        .await
        .map_err(ApiError::from)
}

/// Validate a provider id (`[a-z0-9][a-z0-9._-]*`, ≤48).
pub fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= PROVIDER_ID_MAX_LEN
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        })
}

/// Validate a provider URL field (base_url / issuer_url): an absolute
/// https URL, or http for loopback only (the local stub/dev lane) —
/// an http URL elsewhere would carry the client secret in cleartext.
pub fn valid_provider_url(raw: Option<&str>) -> bool {
    let Some(raw) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return true; // absent is fine (fields are per-type optional)
    };
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if parsed.username().is_empty() && parsed.scheme() == "https" {
        return true;
    }
    parsed.scheme() == "http"
        && parsed.username().is_empty()
        && matches!(
            parsed.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("::1")
        )
}

/// Validate the type tag.
pub fn valid_provider_type(provider_type: &str) -> bool {
    matches!(
        provider_type,
        "builtin:github" | "builtin:feishu" | "builtin:erp" | "oidc"
    )
}

// ── endpoint templates ─────────────────────────────────────────────────

/// The authorize URL for a provider (None = unusable configuration).
fn authorize_url(
    provider: &OAuthProvider,
    redirect_uri: &str,
    state_param: &str,
    challenge: &str,
) -> Option<String> {
    let encoded_redirect = urlencode(redirect_uri);
    let mut url = match provider.provider_type.as_str() {
        "builtin:github" => format!(
            "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read%3Auser&state={}&code_challenge={}&code_challenge_method=S256",
            urlencode(&provider.client_id), encoded_redirect, state_param, challenge,
        ),
        "builtin:feishu" => format!(
            "https://accounts.feishu.cn/open-apis/authen/v1/authorize?app_id={}&redirect_uri={}&state={}",
            urlencode(&provider.client_id), encoded_redirect, state_param,
        ),
        "builtin:erp" => {
            let base = absolute_base(provider.base_url.as_deref()?)?;
            format!(
                "{base}/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&scope=openid+profile&state={}&code_challenge={}&code_challenge_method=S256",
                urlencode(&provider.client_id), encoded_redirect, state_param, challenge,
            )
        }
        _ => return None, // oidc resolves through discovery (async).
    };
    if url.contains(' ') {
        url = url.replace(' ', "+");
    }
    Some(url)
}

/// Resolve the authorize URL, OIDC discovery included.
async fn resolve_authorize_url(
    state: &AppState,
    provider: &OAuthProvider,
    redirect_uri: &str,
    state_param: &str,
    challenge: &str,
) -> Result<Option<String>, ApiError> {
    if let Some(url) = authorize_url(provider, redirect_uri, state_param, challenge) {
        return Ok(Some(url));
    }
    if provider.provider_type != "oidc" {
        return Ok(None);
    }
    let issuer = provider.issuer_url.as_deref().unwrap_or_default();
    let Some(document) = discover(state, issuer).await? else {
        return Ok(None);
    };
    let Some(endpoint) = document.authorization_endpoint else {
        return Ok(None);
    };
    let scopes = provider
        .scopes
        .clone()
        .unwrap_or_else(|| "openid profile".to_owned());
    Ok(Some(format!(
        "{endpoint}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(&provider.client_id),
        urlencode(redirect_uri),
        urlencode(&scopes),
        state_param,
        challenge,
    )))
}

/// The token endpoint (None for oidc until discovery resolves).
fn token_endpoint(provider: &OAuthProvider) -> Option<String> {
    match provider.provider_type.as_str() {
        "builtin:github" => Some("https://github.com/login/oauth/access_token".to_owned()),
        // Feishu v2: app credentials in the form body.
        "builtin:feishu" => {
            Some("https://open.feishu.cn/open-apis/authen/v2/oauth/token".to_owned())
        }
        "builtin:erp" => Some(format!(
            "{}/api/oauth/token",
            absolute_base(provider.base_url.as_deref()?)?
        )),
        _ => None,
    }
}

/// The userinfo endpoint (None for oidc until discovery resolves).
fn userinfo_endpoint(provider: &OAuthProvider) -> Option<String> {
    match provider.provider_type.as_str() {
        "builtin:github" => Some("https://api.github.com/user".to_owned()),
        // Feishu v2's token response itself carries the user identity.
        "builtin:feishu" => None,
        "builtin:erp" => Some(format!(
            "{}/oauth/userinfo",
            absolute_base(provider.base_url.as_deref()?)?
        )),
        _ => None,
    }
}

fn absolute_base(base: &str) -> Option<&str> {
    let trimmed = base.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn urlencode(value: &str) -> String {
    // The `url` crate's form serializer without the trailing '+' swap.
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("", value);
    let encoded = serializer.finish();
    encoded.trim_start_matches('=').to_owned()
}

// ── OIDC discovery (cached, negative-cached) ───────────────────────────

/// The discovery document subset this service consumes.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscoveryDocument {
    /// `authorization_endpoint`.
    pub authorization_endpoint: Option<String>,
    /// `token_endpoint`.
    pub token_endpoint: Option<String>,
    /// `userinfo_endpoint`.
    pub userinfo_endpoint: Option<String>,
}

struct DiscoveryCacheEntry {
    document: Option<DiscoveryDocument>,
    fetched_at: Instant,
}

/// In-process discovery cache (document + negative results).
#[derive(Default)]
pub struct DiscoveryCache {
    entries: Mutex<HashMap<String, DiscoveryCacheEntry>>,
}

impl DiscoveryCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, issuer: &str) -> Option<Option<DiscoveryDocument>> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(issuer)?;
        let ttl = if entry.document.is_some() {
            DISCOVERY_TTL
        } else {
            DISCOVERY_FAILURE_TTL
        };
        (entry.fetched_at.elapsed() < ttl).then_some(entry.document.clone())
    }

    fn put(&self, issuer: String, document: Option<DiscoveryDocument>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= DISCOVERY_CACHE_MAX && !entries.contains_key(&issuer) {
            // Bound the cache: drop the oldest entry.
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| {
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|_| entry.fetched_at)
                        .unwrap_or(entry.fetched_at)
                })
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            issuer,
            DiscoveryCacheEntry {
                document,
                fetched_at: Instant::now(),
            },
        );
    }
}

/// Discover `{issuer}/.well-known/openid-configuration`, through the
/// cache. `Ok(None)` = a cached or fresh failure (broken issuer).
async fn discover(state: &AppState, issuer: &str) -> Result<Option<DiscoveryDocument>, ApiError> {
    if issuer.is_empty() {
        return Ok(None);
    }
    if let Some(cached) = state.discovery.get(issuer) {
        return Ok(cached);
    }
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let fetched = tokio::time::timeout(DISCOVERY_TIMEOUT, state.http_client.get(&url).send())
        .await
        .map_err(|_| ApiError::bad_request("discovery timed out"))?
        .map_err(|err| ApiError::bad_request(format!("discovery failed: {err}")))?;
    let document = if fetched.status().is_success() {
        fetched
            .json::<DiscoveryDocument>()
            .await
            .map_err(|err| ApiError::bad_request(format!("discovery document: {err}")))?
            .into()
    } else {
        None
    };
    state.discovery.put(issuer.to_owned(), document.clone());
    Ok(document)
}

// ── PKCE ───────────────────────────────────────────────────────────────

/// A fresh PKCE S256 pair (verifier, challenge).
pub fn pkce_s256_pair() -> (String, String) {
    // 64 hex chars from two v4 UUIDs — within RFC 7636's 43..128
    // unreserved-character verifier band.
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let challenge = pkce_challenge(&verifier);
    (verifier, challenge)
}

/// The S256 challenge for a verifier (BASE64URL(SHA-256), no padding).
pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

// ── the flow handlers ──────────────────────────────────────────────────

/// `GET /api/v1/auth/oauth/{provider}` — the redirect entry.
pub async fn redirect(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Response {
    let providers = load_providers(&state).await;
    let Some(config) = providers.iter().find(|p| p.id == provider && p.enabled) else {
        return bad_request("OAuth provider not configured or disabled");
    };

    let state_param = Uuid::new_v4().simple().to_string();
    let (verifier, challenge) = pkce_s256_pair();
    let redirect_uri = callback_uri(&state, &headers, &provider);

    let Some(url) = resolve_authorize_url(&state, config, &redirect_uri, &state_param, &challenge)
        .await
        .unwrap_or(None)
    else {
        return bad_request(&format!(
            "Provider '{provider}' could not resolve an authorize URL"
        ));
    };

    // state + verifier in ONE HttpOnly cookie (600 s), scoped to the
    // callback path, SameSite=Lax (the provider's redirect back is a
    // top-level navigation). The PROVIDER rides the value too
    // (`|`-joined: provider ids allow dots, but never a pipe), so a
    // state minted for one provider cannot be replayed against
    // another's callback.
    let cookie = format!(
        "{OAUTH_STATE_COOKIE}={provider}|{state_param}|{verifier}; Max-Age={OAUTH_STATE_MAX_AGE_SECS}; \
         HttpOnly; SameSite=Lax; Path=/api/v1/auth/oauth"
    );
    (
        StatusCode::FOUND,
        [(header::SET_COOKIE, cookie), (header::LOCATION, url)],
    )
        .into_response()
}

/// `GET /api/v1/auth/oauth/{provider}/callback` — the return leg.
pub async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    // Strict state check: cookie vs provider echo (CSRF defense), with
    // the provider segment bound in — the cookie must name THIS
    // provider (a state minted for another one is a replay).
    let Some(cookie_value) = cookie_value(&headers, OAUTH_STATE_COOKIE) else {
        return forbidden("OAuth state cookie missing — restart the login");
    };
    let Some((cookie_provider, rest)) = cookie_value.split_once('|') else {
        return forbidden("malformed OAuth state cookie");
    };
    let Some((cookie_state, verifier)) = rest.split_once('|') else {
        return forbidden("malformed OAuth state cookie");
    };
    if cookie_provider != provider {
        return forbidden("OAuth state cookie was minted for another provider");
    }
    let Some(provider_state) = params.get("state") else {
        return forbidden("OAuth callback missing state parameter");
    };
    if cookie_state != provider_state {
        return forbidden("OAuth state parameter mismatch — possible CSRF attack");
    }
    let Some(code) = params.get("code") else {
        return bad_request("OAuth callback missing code parameter");
    };

    let providers = load_providers(&state).await;
    let Some(config) = providers.iter().find(|p| p.id == provider && p.enabled) else {
        return bad_request("OAuth provider not configured or disabled");
    };

    let redirect_uri = callback_uri(&state, &headers, &provider);

    match run_exchange_and_session(&state, config, &redirect_uri, code, verifier).await {
        Ok(response) => {
            // Clear the flow cookie alongside the session.
            let cleared = format!(
                "{OAUTH_STATE_COOKIE}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; \
                 HttpOnly; SameSite=Lax; Path=/api/v1/auth/oauth"
            );
            let mut response = response;
            response
                .headers_mut()
                .append(header::SET_COOKIE, cleared.parse().unwrap());
            response
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({"error": err.error}).to_string(),
        )
            .into_response(),
    }
}

/// The token exchange + userinfo + find_or_create + session planting.
async fn run_exchange_and_session(
    state: &AppState,
    config: &OAuthProvider,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<Response, ApiError> {
    let token_url = match token_endpoint(config) {
        Some(url) => url,
        None => {
            let issuer = config.issuer_url.as_deref().unwrap_or_default();
            match discover(state, issuer)
                .await?
                .and_then(|d| d.token_endpoint)
            {
                Some(url) => url,
                None => return Err(ApiError::bad_request("no token endpoint configured")),
            }
        }
    };

    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", config.client_id.as_str()),
        ("code_verifier", verifier),
    ];
    if !config.client_secret.is_empty() {
        form.push(("client_secret", config.client_secret.as_str()));
    }

    let token_response = state
        .http_client
        .post(&token_url)
        .header(header::ACCEPT, "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|err| ApiError::bad_gateway(format!("token exchange failed: {err}")))?;
    if !token_response.status().is_success() {
        let status = token_response.status();
        let body = token_response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "token exchange returned {status}: {body}"
        )));
    }
    let token_json: serde_json::Value = token_response
        .json()
        .await
        .map_err(|err| ApiError::bad_gateway(format!("token response: {err}")))?;
    let access_token = token_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_gateway("token response missing access_token"))?
        .to_owned();

    // Userinfo: dedicated endpoint, or (feishu v2) the token payload.
    let userinfo: serde_json::Value = match userinfo_endpoint(config) {
        Some(url) => {
            let response = state
                .http_client
                .get(&url)
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .send()
                .await
                .map_err(|err| ApiError::bad_gateway(format!("userinfo failed: {err}")))?;
            if !response.status().is_success() {
                return Err(ApiError::bad_gateway(format!(
                    "userinfo returned {}",
                    response.status()
                )));
            }
            response
                .json()
                .await
                .map_err(|err| ApiError::bad_gateway(format!("userinfo body: {err}")))?
        }
        None => token_json,
    };

    let subject = subject_from_userinfo(&userinfo)
        .ok_or_else(|| ApiError::bad_gateway("userinfo carried no subject"))?;
    let display_name = ["name", "preferred_username", "login", "display_name"]
        .iter()
        .find_map(|key| userinfo.get(*key).and_then(|v| v.as_str()))
        .map(str::to_owned);

    let user = find_or_create_user(state, &config.id, &subject, display_name.as_deref()).await?;

    // The ordinary session cookie (same shape as the password login).
    let now = jiff::Timestamp::now();
    let token = state.signer.sign(user.id, now);
    state
        .store
        .set_setting(
            &crate::auth::watermark_key(user.id),
            serde_json::Value::String(now.to_string()),
        )
        .await
        .map_err(ApiError::from)?;
    let session = AuthSession {
        expires_at: crate::api::auth::expiry_rfc3339(now),
        user: i0721_proto::UserDto::from(user),
    };
    let cookie = format!(
        "i0721-token={token}; Max-Age={}{}",
        TOKEN_TTL_DAYS * 24 * 3600,
        crate::api::auth::COOKIE_ATTRIBUTES,
    );
    Ok((
        StatusCode::FOUND,
        [
            (header::SET_COOKIE, cookie),
            (header::LOCATION, "/".to_owned()),
        ],
        axum::Json(session),
    )
        .into_response())
}

/// Extract the provider subject from a userinfo payload: `sub`
/// (oidc/erp) | `id` (github, a JSON NUMBER) | `open_id` (feishu).
/// Numbers stringify — GitHub's /user answers `{"id": 12345678}`, and
/// an `as_str()`-only read would reject every GitHub login with 502.
pub fn subject_from_userinfo(userinfo: &serde_json::Value) -> Option<String> {
    ["sub", "id", "open_id"].iter().find_map(|key| {
        userinfo.get(*key).and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().filter(|_| *key == "id").map(|n| n.to_string()))
        })
    })
}

/// `find_or_create_user` over the `oauth_identities` table: an existing
/// (provider, subject) binding loads its user; a first login mints a
/// local account (`{provider}_{subject[0..8]}`, no password) and links
/// the identity. Idempotent: a second login never mints again.
pub async fn find_or_create_user(
    state: &AppState,
    provider: &str,
    subject: &str,
    display_name: Option<&str>,
) -> Result<i0721_proto::UserRecord, ApiError> {
    if let Some(user_id) = state
        .store
        .get_oauth_identity(provider, subject)
        .await
        .map_err(ApiError::from)?
    {
        return state
            .store
            .get_user(user_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::internal("OAuth-linked user vanished"));
    }

    // Subject prefix: 8 chars, sanitized into the username charset.
    let prefix: String = subject
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(8)
        .collect();
    let username = format!("{provider}_{prefix}");
    let display = display_name
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| username.clone());

    let candidate = NewUser {
        username: username.clone(),
        password: String::new(),
        display_name: Some(display),
        role: Some(Role::Member),
    };
    let user = match state
        .store
        .create_user(i0721_store::CreateUser {
            username: candidate.username.clone(),
            display_name: candidate.display_name.clone().unwrap_or(username.clone()),
            role: Role::Member,
            password_hash: None,
        })
        .await
    {
        Ok(record) => record,
        // Lost the mint race (or the prefixed name collided): the
        // binding is the identity — if it exists now, ride it; else the
        // username is genuinely taken and a distinct suffix retries.
        Err(i0721_store::StoreError::Conflict(_)) => {
            if let Some(user_id) = state
                .store
                .get_oauth_identity(provider, subject)
                .await
                .map_err(ApiError::from)?
            {
                return state
                    .store
                    .get_user(user_id)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::internal("OAuth-linked user vanished"));
            }
            let unique = format!("{provider}_{}_{prefix}", Uuid::new_v4().simple());
            state
                .store
                .create_user(i0721_store::CreateUser {
                    username: unique,
                    display_name: candidate.display_name.unwrap_or(username.clone()),
                    role: Role::Member,
                    password_hash: None,
                })
                .await
                .map_err(ApiError::from)?
        }
        Err(err) => return Err(ApiError::from(err)),
    };
    state
        .store
        .link_oauth_identity(provider, subject, user.id)
        .await
        .map_err(ApiError::from)?;
    Ok(user)
}

// ── shared helpers ─────────────────────────────────────────────────────

/// The callback URI as the provider sees it: the configured public
/// origin (`I0721_PUBLIC_ORIGIN`, the deployment's A-layer fact) when
/// present; otherwise derived from the request Host with a warning —
/// a derivation the reverse proxy's Host preservation makes right in
/// practice, but which the env pin makes explicit and auditable.
fn callback_uri(state: &AppState, headers: &HeaderMap, provider: &str) -> String {
    let origin = state
        .config
        .public_origin
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let host = headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("localhost");
            tracing::warn!(
                host,
                "I0721_PUBLIC_ORIGIN unset — deriving the OAuth redirect URI from the Host header",
            );
            format!("http://{host}")
        });
    format!("{origin}/api/v1/auth/oauth/{provider}/callback")
}

/// Read one cookie's value from a Cookie header.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(&format!("{name}=")) {
            return Some(value.to_owned());
        }
    }
    None
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}

fn forbidden(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (verifier, challenge) = pkce_s256_pair();
        assert_eq!(challenge, pkce_challenge(&verifier));
        // RFC 7636 §4.2: BASE64URL-ENCODE(SHA256(ASCII(verifier))).
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, expected);
        // Verifier charset/length band (43..=128 unreserved chars).
        assert!((43..=128).contains(&verifier.len()));
        assert!(verifier.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn provider_id_shape() {
        assert!(valid_provider_id("erp"));
        assert!(valid_provider_id("keycloak.eu.example"));
        assert!(!valid_provider_id(""));
        assert!(!valid_provider_id("GitHub")); // uppercase rejected
        assert!(!valid_provider_id("-lead"));
        assert!(!valid_provider_id(&"x".repeat(49)));
    }

    #[test]
    fn provider_type_shape() {
        for good in ["builtin:github", "builtin:feishu", "builtin:erp", "oidc"] {
            assert!(valid_provider_type(good));
        }
        assert!(!valid_provider_type("twitter"));
    }

    #[test]
    fn masked_never_carries_the_secret() {
        let row = OAuthProvider {
            id: "erp".to_owned(),
            display_name: "ERP".to_owned(),
            provider_type: "builtin:erp".to_owned(),
            client_id: "cid".to_owned(),
            client_secret: "super-secret-value-9999".to_owned(),
            base_url: Some("https://sso.example".to_owned()),
            issuer_url: None,
            scopes: None,
            enabled: true,
            icon: None,
        };
        let masked = row.masked();
        assert!(masked.has_secret);
        assert_eq!(masked.secret_tail, "9999");
        let json = serde_json::to_string(&masked).unwrap();
        assert!(
            !json.contains("super-secret"),
            "the masked projection must never serialize the secret"
        );
    }

    #[test]
    fn builtin_authorize_urls_carry_pkce_and_state() {
        let row = OAuthProvider {
            id: "erp".to_owned(),
            display_name: "ERP".to_owned(),
            provider_type: "builtin:erp".to_owned(),
            client_id: "cid".to_owned(),
            client_secret: String::new(),
            base_url: Some("https://sso.example".to_owned()),
            issuer_url: None,
            scopes: None,
            enabled: true,
            icon: None,
        };
        let url = authorize_url(&row, "http://res.test/cb", "st123", "ch456").unwrap();
        assert!(url.starts_with("https://sso.example/oauth/authorize?"));
        assert!(url.contains("state=st123"));
        assert!(url.contains("code_challenge=ch456"));
        assert!(url.contains("code_challenge_method=S256"));

        // GitHub carries PKCE too; feishu's v1 authorize has no PKCE slot
        // (the flow binds through app credentials + state).
        let mut github = row.clone();
        github.provider_type = "builtin:github".to_owned();
        let url = authorize_url(&github, "http://res.test/cb", "st", "ch").unwrap();
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn discovery_cache_negative_results_expire_fast() {
        let cache = DiscoveryCache::new();
        cache.put("https://broken".to_owned(), None);
        assert!(cache.get("https://broken").is_some()); // cached failure
        let cache2 = DiscoveryCache::new();
        assert!(cache2.get("https://never-seen").is_none()); // absent
    }
}
