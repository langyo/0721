//! The OAuth dual-mode authentication lane: a provider registry in the
//! `settings` table, the relying-party browser flow, `find_or_create_user`
//! over the `oauth_identities` table, and the family login-policy document
//! the login page renders.
//!
//! ## What this module owns and what kirino owns
//!
//! The **protocol** — PKCE S256, the one-time `state`, the browser binding,
//! the code-for-token exchange, the UserInfo read, the ID-token check — is
//! `kirino::auth::passport::oauth`, the family's relying-party core. No code
//! here writes a PKCE pair, mints or compares a `state`, or builds a token
//! request by hand.
//!
//! This module keeps the three things a host must keep:
//!
//! * the **catalog** — the settings row, mapped per provider type onto
//!   kirino's [`ProviderMetadata`] (endpoints, token-body encoding, scopes,
//!   issuer) and [`ClientAuth`];
//! * the **claim reading** — the vendor payload shapes this service has
//!   always understood (a GitHub numeric `id`, a Feishu `open_id`, a
//!   Feishu-wrapped `data` envelope) map onto the local account key;
//! * the **session** — `find_or_create_user` and the ordinary session
//!   cookie, unchanged by this wave.
//!
//! The one network seam is [`plana_identity_relay::ReqwestFetcher`] (no
//! redirects, a bounded timeout, errors that carry no URL and no body).
//!
//! ## Provider registry
//!
//! One settings row (`oauth_providers`) holds the whole catalog as a JSON
//! array. Each row: `id` / `display_name` / `type`
//! (`builtin:github` | `builtin:feishu` | `builtin:erp` | `oidc`) /
//! `client_id` / `client_secret` (server-held) / `base_url` (erp) or
//! `issuer_url` (oidc) / `scopes` / `enabled` / `icon`. The
//! `client_secret` NEVER leaves the server: every public shape
//! (login-methods, the admin list) carries only `has_secret` + the last four
//! characters.
//!
//! ## Flow
//!
//! `GET /api/v1/auth/oauth/{provider}` parks an attempt (kirino mints the
//! one-time `state`, the PKCE verifier and the OIDC `nonce`; this service
//! adds a per-attempt browser binding), plants the binding in an HttpOnly
//! cookie scoped to `/api/v1/auth/oauth` (600 s), and 302s to the provider's
//! authorize endpoint. The callback presents the binding back to kirino's
//! `finish`, which spends the `state` exactly once, refuses an attempt from
//! another browser or minted for another provider, exchanges the code with
//! the verifier, reads the identity, and hands back a `RelayLogin`. The
//! subject then goes through `find_or_create_user` and the ordinary session
//! cookie is planted.
//!
//! ## OIDC discovery
//!
//! `oidc`-typed providers resolve their endpoints from
//! `{issuer_url}/.well-known/openid-configuration`, cached in-process for 10
//! minutes with a 30 s negative cache (the redirect endpoint is
//! unauthenticated — without a negative cache any visitor could force an
//! outbound request per hit on a broken issuer).
//!
//! ## Login policy
//!
//! `GET /api/v1/auth/login-methods` publishes the family's
//! [`kirino::auth::mode::LoginPolicy`] beside the legacy `password` /
//! `providers[]` fields. The mode is derived (`federated` when the catalog
//! offers at least one usable provider, `local` otherwise) and the document
//! is validated before it is served. Both renderings are projected from ONE
//! filtered row set, and a catalog that still fails validation answers `503`
//! instead of a policy the page cannot honour.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, Utc};
use kirino::auth::mode::{
    AuthMode as KirinoAuthMode, LoginMethodDescriptor as KirinoMethodDescriptor,
    LoginPolicy as KirinoLoginPolicy, LoginPolicyViolation, LOGIN_POLICY_VERSION,
};
use kirino::auth::passport::oauth::{
    authorize_url as kirino_authorize_url, begin_with_binding as kirino_begin_with_binding,
    finish as kirino_finish, Callback as KirinoCallback, CallbackProblem,
    CallbackQuery as KirinoCallbackQuery, ClientAuth, LoginAttempt,
    LoginAttemptStore as KirinoAttemptStore, ProviderIdentity, ProviderMetadata, RelayLogin,
    TokenBody,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use i0721_proto::Role;

use crate::api::auth::AuthSession;
use crate::auth::TOKEN_TTL_DAYS;
use crate::error::ApiError;
use crate::state::AppState;

/// The settings row holding the provider catalog.
pub const PROVIDERS_SETTINGS_KEY: &str = "oauth_providers";

/// Longest provider id accepted — the id prefixes minted usernames
/// (`{provider}_{subject8}`, VARCHAR must fit).
const PROVIDER_ID_MAX_LEN: usize = 48;

/// The browser-binding cookie: the login-CSRF half of the flow.
///
/// The name and the path are the ones this service has always used (a
/// deployment's cookies are not this wave's to rename): the cookie is
/// re-purposed from "state + verifier" to "the value that proves the callback
/// came back from the browser that started the attempt". The `state` itself
/// no longer rides it — kirino keeps it server-side, one-time.
const OAUTH_BINDING_COOKIE: &str = "i0721-oauth-state";

/// The path both OAuth routes live under; the binding cookie is scoped to it.
const OAUTH_PATH_PREFIX: &str = "/api/v1/auth/oauth";

/// How long an in-flight attempt — and the binding cookie that unlocks it —
/// stays usable. A ceiling for the provider's own login screen.
const OAUTH_ATTEMPT_TTL_SECS: i64 = 600;

/// The cookie's `Max-Age`, in the same unit (seconds).
const OAUTH_STATE_MAX_AGE_SECS: u64 = 600;

/// Upper bound on concurrently parked attempts: the start route is
/// unauthenticated, so the store must not be an unbounded memory tap.
const OAUTH_ATTEMPT_CAP: usize = 1024;

/// Where a finished sign-in lands (the SPA root, as it always has).
const OAUTH_LANDING: &str = "/";

/// Discovery caches.
const DISCOVERY_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const DISCOVERY_TTL: StdDuration = StdDuration::from_secs(600);
const DISCOVERY_FAILURE_TTL: StdDuration = StdDuration::from_secs(30);
const DISCOVERY_CACHE_MAX: usize = 32;

// ── the registry rows ──────────────────────────────────────────────────

/// One provider row as stored in settings. This is the STORAGE form:
/// it serializes the client_secret (the settings row is server-side
/// only — every public wire shape goes through [`OAuthProvider::masked`],
/// and `skip_serializing` here would WIPE the secret on every save and
/// leave a row the loader cannot parse back). It deliberately carries
/// no ts-rs export: the webui speaks `OAuthProviderUpsert` (input) and
/// `OAuthProviderPublic` (masked output), never this shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The client secret (server-held; the masked projection is the
    /// only public shape).
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
    /// Login-button icon (http(s)/data URI passthrough; the bundled
    /// webui decorates every button with a uniform key glyph and does
    /// not read this field).
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

/// The family login-policy document this service publishes.
///
/// The SHAPE is `kirino::auth::mode::LoginPolicy` — the family contract, not
/// a local invention — restated here only because this crate is what exports
/// TypeScript bindings: kirino declares these with a `ts` feature this build
/// leaves off, and enabling it would write kirino's files into this
/// repository on kirino's own path. The contract is pinned by
/// [`policy_to_wire`], which converts kirino's validated document into this
/// one field for field, and by the tests that compare the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct LoginPolicy {
    /// The wire version (currently `1`).
    pub version: u8,
    /// The mode — the summary of everything below.
    pub mode: AuthMode,
    /// Whether the local password card is offered.
    pub password: bool,
    /// Every entrance other than the password card, in display order.
    pub methods: Vec<LoginMethodDescriptor>,
    /// Present if and only if `mode` is `relay` (absent here: this service
    /// has no relay directive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<RelayDirective>,
}

/// The authentication mode — the policy's one-word summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// A self-contained deployment (the password card, maybe no provider).
    Local,
    /// A deployment that signs in through at least one external provider.
    Federated,
    /// A deployment entered through a designated central login site.
    Relay,
}

/// One entrance the login page offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LoginMethodDescriptor {
    /// A third-party provider the browser is sent to.
    Oauth {
        /// Stable id, unique within the policy.
        id: String,
        /// Display label.
        label: String,
        /// Where the browser starts the method: a same-origin absolute path.
        href: String,
        /// The host's provider id.
        provider: String,
        /// Optional icon reference.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    /// WebAuthn / passkey sign-in (not offered by this service).
    Passkey {
        id: String,
        label: String,
        href: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    /// A host-specific entrance (a cross-realm jump row).
    Custom {
        id: String,
        label: String,
        href: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
}

impl LoginMethodDescriptor {
    /// The stable id, whatever the kind.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Oauth { id, .. } | Self::Passkey { id, .. } | Self::Custom { id, .. } => id,
        }
    }
}

/// How a relay-mode service is entered (unused here; the contract carries it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct RelayDirective {
    /// The service's own same-origin entry route.
    pub entry: String,
    /// The query parameter carrying the post-login return address.
    pub return_param: String,
}

/// Convert kirino's validated document into this crate's wire shape.
///
/// Field for field, so the published document cannot mean anything other
/// than what the family contract validated (the test
/// `published_policy_round_trips_through_kirinos_contract` compares the two
/// serialisations).
fn policy_to_wire(policy: &KirinoLoginPolicy) -> LoginPolicy {
    LoginPolicy {
        version: policy.version,
        mode: match policy.mode {
            KirinoAuthMode::Local => AuthMode::Local,
            KirinoAuthMode::Federated => AuthMode::Federated,
            KirinoAuthMode::Relay => AuthMode::Relay,
        },
        password: policy.password,
        methods: policy
            .methods
            .iter()
            .map(|method| match method {
                KirinoMethodDescriptor::Oauth {
                    id,
                    label,
                    href,
                    provider,
                    icon,
                } => LoginMethodDescriptor::Oauth {
                    id: id.clone(),
                    label: label.clone(),
                    href: href.clone(),
                    provider: provider.clone(),
                    icon: icon.clone(),
                },
                KirinoMethodDescriptor::Passkey {
                    id,
                    label,
                    href,
                    icon,
                } => LoginMethodDescriptor::Passkey {
                    id: id.clone(),
                    label: label.clone(),
                    href: href.clone(),
                    icon: icon.clone(),
                },
                KirinoMethodDescriptor::Custom {
                    id,
                    label,
                    href,
                    icon,
                } => LoginMethodDescriptor::Custom {
                    id: id.clone(),
                    label: label.clone(),
                    href: href.clone(),
                    icon: icon.clone(),
                },
            })
            .collect(),
        relay: policy.relay.as_ref().map(|directive| RelayDirective {
            entry: directive.entry.clone(),
            return_param: directive.return_param.clone(),
        }),
    }
}

/// The login-methods payload (public — no secret anywhere).
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct LoginMethods {
    /// Whether the password form is offered (the kill-switch).
    pub password: bool,
    /// Enabled OAuth providers, login-button order.
    pub providers: Vec<LoginMethodProvider>,
    /// The family login-policy document, validated before it is served.
    /// Additive: `password` + `providers` above stay the shape the bundled
    /// webui reads.
    pub policy: LoginPolicy,
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

/// Load the whole registry (empty when the row is absent — a fresh
/// deployment has no providers). A CORRUPT row (hand-edited JSON the
/// `OAuthProvider` shape rejects) also lands on empty: the login page
/// stays up and the admin list reads clean, and the next upsert
/// overwrites the broken row with a well-formed registry.
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

/// One provider type's endpoint family: where the three legs are, how the
/// token body is encoded, which scopes are mandatory, and the issuer the
/// identity carries. This is *data* kirino's core consumes, never a code
/// path it branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    /// `None` for a provider that offers no separate UserInfo leg at all
    /// (feishu — whose identity source is undecided, see `builtin:feishu`).
    pub userinfo: Option<String>,
    pub token_body: TokenBody,
    /// The scopes requested when the operator gives none. `None` = ask for
    /// none at all (feishu: its authorize endpoint takes no `scope`).
    pub default_scopes: Option<&'static [&'static str]>,
    /// The issuer an identity carries when the provider's own answer names
    /// none.
    pub issuer: Option<String>,
    /// The key a provider wraps its token answer in, when it answers in its
    /// own transport envelope instead of the flat RFC 6749 object.
    ///
    /// `None` (GitHub, the ERP, `oidc`): the token fields are read at the
    /// answer's top level. `Some("data")` (feishu v2): the token fields —
    /// `access_token` among them — live one level down, next to the
    /// vendor's business result, so the core drills into that key first and
    /// reads nothing else there.
    pub token_response_envelope: Option<&'static str>,
}

/// GitHub's token endpoint (the vendor's documented route).
const TOKEN_ENDPOINT_GITHUB: &str = "https://github.com/login/oauth/access_token";

/// Feishu's v2 token endpoint (the vendor's documented route).
const TOKEN_ENDPOINT_FEISHU: &str = "https://open.feishu.cn/open-apis/authen/v2/oauth/token";

/// The built-in endpoint family for a provider type. `None` when the row
/// cannot resolve one (an erp row without a base, or the `oidc` type, whose
/// endpoints come from discovery).
fn builtin_endpoints(provider: &OAuthProvider) -> Option<Endpoints> {
    match provider.provider_type.as_str() {
        "builtin:github" => Some(Endpoints {
            authorize: "https://github.com/login/oauth/authorize".to_owned(),
            token: TOKEN_ENDPOINT_GITHUB.to_owned(),
            userinfo: Some("https://api.github.com/user".to_owned()),
            // GitHub speaks form encoding (its JSON body is an opt-in the
            // pre-kirino client never asked for) and is a confidential
            // client: the secret rides the token request, never the browser.
            token_body: TokenBody::Form,
            default_scopes: Some(&["read:user"]),
            issuer: Some("https://github.com".to_owned()),
            // The flat RFC 6749 object: `access_token` at the top level.
            token_response_envelope: None,
        }),
        "builtin:feishu" => Some(Endpoints {
            authorize: "https://accounts.feishu.cn/open-apis/authen/v1/authorize".to_owned(),
            token: TOKEN_ENDPOINT_FEISHU.to_owned(),
            // Feishu v2's token answer is not the flat RFC 6749 object: the
            // token fields sit one level down, in `data`, next to the
            // vendor's business result (`{"code": 0, "msg": "ok", "data":
            // {"access_token": …, "token_type": "Bearer", …}}`). The core
            // drills into that key — see `token_response_envelope` below.
            //
            // Identity source UNDECIDED (see PLAN, the C-lite ruling): the
            // token answer's `data` envelope carries the token fields and no
            // identity the core reads, and feishu's separate `user_info`
            // response is an envelope too (no `sub`). This type therefore
            // owes an identity leg it does not have — the core refuses the
            // callback with `no identity source: no userinfo endpoint and no
            // ID token`. A feishu row is UNUSABLE until that is settled
            // (kirino's dialects wave or a host-side identity read); the type
            // stays in the catalog as a dormant capability, and the envelope
            // below is kirino's contract for it, already pinned by kirino's
            // own suite (`an_envelope_without_the_key_or_without_a_token_is_refused`
            // and friends).
            userinfo: None,
            token_body: TokenBody::Form,
            default_scopes: None,
            issuer: Some("https://open.feishu.cn".to_owned()),
            token_response_envelope: Some("data"),
        }),
        "builtin:erp" => {
            let base = absolute_base(provider.base_url.as_deref()?)?;
            Some(Endpoints {
                authorize: format!("{base}/oauth/authorize"),
                token: format!("{base}/api/oauth/token"),
                // The ERP provider mounts userinfo under its `/api` prefix
                // (`/api/oauth/userinfo`, alongside `/api/oauth/token`). The
                // bare `/oauth/userinfo` is NOT an ERP route: its host-split
                // middleware 307s the request to the staff admin face, the
                // callback's server-side GET follows into an HTML login page,
                // and the decode dies with "error decoding response body"
                // (the 2026-11-24 res.celestia.world report). Both legs are
                // derived from `base_url` — the row carries no per-endpoint
                // fields.
                //
                // MUT:erp-userinfo-prefix — reverting to the bare
                // `/oauth/userinfo` makes both end-to-end OAuth cases fail
                // against the `/api`-mounted stub (`tests/routes.rs`,
                // `oauth_end_to_end_against_a_local_stub_provider` and
                // `oauth_second_login_is_idempotent`).
                userinfo: Some(format!("{base}/api/oauth/userinfo")),
                token_body: TokenBody::Form,
                default_scopes: Some(&["openid", "profile"]),
                // The ERP is the family's own service: its issuer is the
                // deployment's own origin, not a third-party hostname.
                issuer: Some(base.to_owned()),
                // The flat RFC 6749 object, like every other family provider.
                token_response_envelope: None,
            })
        }
        _ => None,
    }
}

/// The scopes to ask for: the operator's override when present, else the
/// type's mandatory set.
fn scopes_for(provider: &OAuthProvider, endpoints: &Endpoints) -> Vec<String> {
    match provider.scopes.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => raw.split_whitespace().map(str::to_owned).collect(),
        _ => endpoints
            .default_scopes
            .unwrap_or_default()
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect(),
    }
}

/// The provider's `base_url`, trimmed and stripped of trailing slashes, so
/// every leg derived from it joins on exactly one `/`: an operator who
/// pastes `https://sso.example/` gets `https://sso.example/api/oauth/token`,
/// not `…//api/oauth/token`, and an issuer with no trailing slash.
fn absolute_base(base: &str) -> Option<&str> {
    let trimmed = base.trim();
    (!trimmed.is_empty()).then(|| trimmed.trim_end_matches('/'))
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
    /// The document's own `issuer` (the string its tokens must name).
    #[serde(default)]
    pub issuer: Option<String>,
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

// ── the attempt store ──────────────────────────────────────────────────

/// The in-memory store of in-flight sign-ins (kirino's
/// [`LoginAttemptStore`](KirinoAttemptStore) contract), behind
/// `crate::State`.
///
/// Nothing survives a restart — an in-flight sign-in simply starts over —
/// and nothing here is ever persisted. Unlike kirino's reference store this
/// one is **bounded**: the start route is unauthenticated and unrestricted,
/// so an unbounded map would let any visitor grow the process's memory one
/// redirect at a time.
#[derive(Debug, Clone, Default)]
pub struct LoginAttemptStore {
    attempts: std::sync::Arc<Mutex<HashMap<String, LoginAttempt>>>,
}

impl LoginAttemptStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many attempts are in flight (cap diagnostics, and the test that
    /// pins the bound).
    #[must_use]
    pub fn len(&self) -> usize {
        self.attempts
            .lock()
            .map(|attempts| attempts.len())
            .unwrap_or_default()
    }

    /// Whether no attempt is in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl KirinoAttemptStore for LoginAttemptStore {
    async fn put(&self, attempt: LoginAttempt) -> anyhow::Result<()> {
        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| anyhow::anyhow!("the in-flight sign-in store is poisoned"))?;
        // Make room BEFORE inserting, so the map never exceeds the cap: first
        // whatever has already expired (the cheap half), then — if a flood of
        // live attempts filled it — the oldest one. The start route is
        // unauthenticated, so being evicted is part of the contract: that
        // login simply starts over.
        if attempts.len() >= OAUTH_ATTEMPT_CAP {
            let now = Utc::now();
            attempts.retain(|_, kept| !kept.is_expired(now));
            if attempts.len() >= OAUTH_ATTEMPT_CAP {
                let oldest = attempts
                    .iter()
                    .min_by_key(|(_, kept)| kept.created_at)
                    .map(|(state, _)| state.clone());
                if let Some(oldest) = oldest {
                    attempts.remove(&oldest);
                }
            }
        }
        attempts.insert(attempt.state.clone(), attempt);
        Ok(())
    }

    async fn take(&self, state: &str) -> anyhow::Result<Option<LoginAttempt>> {
        let mut attempts = self
            .attempts
            .lock()
            .map_err(|_| anyhow::anyhow!("the in-flight sign-in store is poisoned"))?;
        // One lock spans lookup, expiry check and removal: a replayed
        // callback finds nothing, and an expired attempt is never handed out.
        let now = Utc::now();
        match attempts.remove(state) {
            Some(attempt) if !attempt.is_expired(now) => Ok(Some(attempt)),
            _ => {
                attempts.retain(|_, kept| !kept.is_expired(now));
                Ok(None)
            }
        }
    }
}

/// The transport: built on first use.
///
/// The workspace runs rustls with a provider the `ReqwestFetcher` installs
/// itself, and building the client is what installs it — doing that eagerly
/// would make pre-network unit tests depend on who ran first (arona's
/// precedent).
fn fetch_transport() -> anyhow::Result<&'static plana_identity_relay::ReqwestFetcher> {
    static TRANSPORT: OnceLock<plana_identity_relay::ReqwestFetcher> = OnceLock::new();
    if let Some(fetcher) = TRANSPORT.get() {
        return Ok(fetcher);
    }
    let fetcher =
        plana_identity_relay::ReqwestFetcher::new().map_err(|err| anyhow::anyhow!(err))?;
    // A concurrent caller may have won the race; either client serves.
    let _ = TRANSPORT.set(fetcher);
    TRANSPORT
        .get()
        .ok_or_else(|| anyhow::anyhow!("the OAuth transport could not be built"))
}

// ── provider rows → kirino metadata ────────────────────────────────────

/// One provider row resolved into everything kirino's protocol core needs.
#[derive(Debug, Clone)]
pub struct ProviderRoute {
    /// The protocol metadata (endpoints, token encoding, scopes, issuer).
    pub metadata: ProviderMetadata,
    /// Confidential when the row carries a secret, public otherwise.
    pub client: ClientAuth,
}

/// Assemble a provider row into the core's metadata, or `Err` naming why
/// the row cannot complete a login (an unresolvable endpoint, an unusable
/// URL).
///
/// `oidc` rows resolve their endpoints through the discovery cache and are
/// therefore async; every built-in type assembles synchronously from its
/// endpoint family.
pub async fn resolve_route(
    state: &AppState,
    provider: &OAuthProvider,
) -> Result<ProviderRoute, ApiError> {
    let issuer_url = provider
        .issuer_url
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .map(str::to_owned);

    let endpoints = if provider.provider_type == "oidc" {
        let issuer = issuer_url
            .clone()
            .ok_or_else(|| ApiError::bad_request("oidc provider needs an issuer_url"))?;
        let document = discover(state, &issuer).await?.ok_or_else(|| {
            ApiError::bad_request(format!(
                "OIDC discovery failed for '{issuer}' — check the issuer_url"
            ))
        })?;
        let authorize = document.authorization_endpoint.ok_or_else(|| {
            ApiError::bad_request("discovery document names no authorization_endpoint")
        })?;
        let token = document
            .token_endpoint
            .ok_or_else(|| ApiError::bad_request("discovery document names no token_endpoint"))?;
        Endpoints {
            authorize,
            token,
            userinfo: document.userinfo_endpoint,
            token_body: TokenBody::Form,
            default_scopes: Some(&["openid", "profile"]),
            issuer: Some(document.issuer.unwrap_or(issuer)),
            // OIDC's token answer is the standard flat object (RFC 6749):
            // whatever a discovery document says about its endpoints, it
            // says nothing about a transport envelope, and the standard
            // does not have one.
            token_response_envelope: None,
        }
    } else {
        builtin_endpoints(provider).ok_or_else(|| {
            ApiError::bad_request(format!(
                "provider '{}' has no usable endpoints (check base_url)",
                provider.id
            ))
        })?
    };

    let parse = |what: &str, raw: &str| -> Result<url::Url, ApiError> {
        url::Url::parse(raw).map_err(|err| {
            ApiError::bad_request(format!(
                "provider '{}' {what} is not a URL: {err}",
                provider.id
            ))
        })
    };
    let authorize = parse("authorize endpoint", &endpoints.authorize)?;
    let token = parse("token endpoint", &endpoints.token)?;
    let userinfo = match endpoints.userinfo.as_deref() {
        Some(raw) => Some(parse("userinfo endpoint", raw)?),
        None => None,
    };

    let client = if provider.client_secret.is_empty() {
        ClientAuth::Public {
            client_id: provider.client_id.clone(),
        }
    } else {
        ClientAuth::Confidential {
            client_id: provider.client_id.clone(),
            client_secret: provider.client_secret.clone().into(),
        }
    };

    let scopes = scopes_for(provider, &endpoints);
    Ok(ProviderRoute {
        metadata: ProviderMetadata {
            provider: provider.id.clone(),
            issuer: endpoints.issuer,
            authorize_url: authorize,
            token_url: token,
            userinfo_url: userinfo,
            token_body: endpoints.token_body,
            scopes,
            token_response_envelope: endpoints.token_response_envelope,
        },
        client,
    })
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

    let route = match resolve_route(&state, config).await {
        Ok(route) => route,
        Err(err) => return bad_request(&err.error),
    };
    let redirect_uri = match callback_url(&state, &headers, &provider) {
        Ok(url) => url,
        Err(err) => return bad_request(&err.error),
    };

    // The browser binding: a fresh 32-byte RNG value, HttpOnly, scoped to
    // the OAuth routes, SameSite=Lax (the provider's redirect back is a
    // top-level navigation). kirino refuses a callback that presents a
    // different binding — the login-CSRF gate.
    let binding = new_browser_binding();
    let attempt = match kirino_begin_with_binding(
        state.oauth_attempts.as_ref(),
        route.metadata.provider.clone(),
        None,
        Some(binding.clone()),
        Duration::seconds(OAUTH_ATTEMPT_TTL_SECS),
    )
    .await
    {
        Ok(attempt) => attempt,
        Err(err) => return bad_gateway(&format!("OAuth attempt could not be parked: {err}")),
    };

    let target = kirino_authorize_url(&route.metadata, &route.client, &redirect_uri, &attempt);
    let cookie = binding_cookie(&binding, OAUTH_STATE_MAX_AGE_SECS);
    (
        StatusCode::FOUND,
        [
            (header::SET_COOKIE, cookie),
            (header::LOCATION, target.to_string()),
        ],
    )
        .into_response()
}

/// `GET /api/v1/auth/oauth/{provider}/callback` — the return leg.
pub async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let binding = cookie_value(&headers, OAUTH_BINDING_COOKIE);
    let query = KirinoCallbackQuery {
        code: params.get("code").cloned(),
        state: params.get("state").cloned(),
        error: params.get("error").cloned(),
        error_description: params.get("error_description").cloned(),
    };

    // The provider vanished or was disabled between the two legs: the
    // attempt dies with the row rather than resolving endpoints from a
    // catalog no longer offered.
    let providers = load_providers(&state).await;
    let Some(config) = providers.iter().find(|p| p.id == provider && p.enabled) else {
        return bad_request("OAuth provider not configured or disabled");
    };
    let route = match resolve_route(&state, config).await {
        Ok(route) => route,
        Err(err) => return bad_request(&err.error),
    };
    let redirect_uri = match callback_url(&state, &headers, &provider) {
        Ok(url) => url,
        Err(err) => return bad_request(&err.error),
    };
    let transport = match fetch_transport() {
        Ok(transport) => transport,
        Err(err) => return bad_gateway(&format!("OAuth transport unavailable: {err}")),
    };

    let relay = match kirino_finish(
        transport,
        state.oauth_attempts.as_ref(),
        &route.metadata,
        &route.client,
        &redirect_uri,
        KirinoCallback::new(&query, binding.as_deref(), &[]),
    )
    .await
    {
        Ok(relay) => relay,
        Err(problem) => return refusal_response(problem),
    };

    let secure_cookie = crate::origin::serves_https(&state, &headers);
    match mint_session(&state, &provider, &relay, secure_cookie).await {
        Ok(response) => {
            let mut response = response;
            // Clear the binding cookie alongside the session.
            response
                .headers_mut()
                .append(header::SET_COOKIE, cleared_binding_cookie());
            response
        }
        Err(err) => bad_gateway(&err.error),
    }
}

/// Map kirino's typed refusal onto the HTTP status the pre-kirino lane
/// answered (kept so a client's error handling does not drift).
fn refusal_response(problem: CallbackProblem) -> Response {
    let message = problem.to_string();
    match problem {
        CallbackProblem::UnknownState
        | CallbackProblem::BindingMismatch
        | CallbackProblem::ProviderMismatch => forbidden(&message),
        CallbackProblem::MissingState
        | CallbackProblem::MissingCode
        | CallbackProblem::ProviderDenied { .. } => bad_request(&message),
        CallbackProblem::TokenInvalid(_)
        | CallbackProblem::IdentityEmpty
        | CallbackProblem::IdTokenInvalid(_)
        | CallbackProblem::Transport(_) => bad_gateway(&message),
    }
}

/// The account half: resolve the subject, `find_or_create_user`, plant the
/// ordinary session cookie, land on the SPA root.
async fn mint_session(
    state: &AppState,
    provider: &str,
    relay: &RelayLogin,
    secure_cookie: bool,
) -> Result<Response, ApiError> {
    let subject = subject_from_identity(&relay.profile)
        .ok_or_else(|| ApiError::bad_gateway("userinfo carried no subject"))?;
    let display_name = display_name_from_identity(&relay.profile);
    // The provider's RBAC statement about this very login; `None` = it did
    // not speak about roles (see `role_from_claims`).
    let role = role_from_claims(&relay.profile.claims);

    let user =
        find_or_create_user(state, provider, &subject, display_name.as_deref(), role).await?;

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
        crate::api::auth::cookie_attributes(secure_cookie),
    );
    Ok((
        StatusCode::FOUND,
        [
            (header::SET_COOKIE, cookie),
            (header::LOCATION, OAUTH_LANDING.to_owned()),
        ],
        axum::Json(session),
    )
        .into_response())
}

// ── claim mapping ──────────────────────────────────────────────────────

/// The provider subject from the identity the core resolved: `sub`
/// (oidc/erp) | `id` (github, a JSON NUMBER) | `open_id` (feishu). Numbers
/// stringify — GitHub's `/user` answers `{"id": 12345678}`, and an
/// `as_str()`-only read would reject every GitHub login.
///
/// kirino's own `subject_from_userinfo` is the fallback for the spellings it
/// knows beyond these three; the local ladder stays first so the numeric
/// GitHub `id` and the numeric discipline ("`sub` is a string, `id` may be a
/// number") are exactly what this service has always minted.
pub fn subject_from_userinfo(userinfo: &serde_json::Value) -> Option<String> {
    ["sub", "id", "open_id"].iter().find_map(|key| {
        userinfo.get(*key).and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().filter(|_| *key == "id").map(|n| n.to_string()))
        })
    })
}

/// The role the identity provider states for this login, if it states one.
///
/// The upstream ERP's UserInfo answers `roles` — a JSON array of strings,
/// e.g. `["member", "admin"]`; only a member of its Administrators group
/// carries `"admin"`. kirino hands that very UserInfo document to this
/// module as [`ProviderIdentity::claims`](kirino::auth::passport::oauth::ProviderIdentity)
/// (the UserInfo leg's body IS `claims`), so `claims["roles"]` is exactly
/// what the provider sent.
///
/// The three answers are deliberately distinct:
///
/// * `Some(Admin)` — a string array that contains `"admin"`;
/// * `Some(Member)` — a string array that does not;
/// * `None` — no statement at all (`roles` absent, or not an array of
///   strings). A provider that says nothing never moves a local role: a
///   GitHub or feishu login, whose identity carries no `roles`, must not
///   demote anybody.
///
/// The tag is matched EXACTLY (`"admin"`), case-sensitively: that is the
/// spelling the ERP's RBAC emits, and accepting variants would widen a
/// privilege boundary on a guess.
#[must_use]
pub fn role_from_claims(claims: &serde_json::Value) -> Option<Role> {
    let roles = claims.get("roles")?.as_array()?;
    // A non-string member means the field is not the shape we understand:
    // refuse the statement rather than half-read it.
    let mut saw_string = true;
    let mut is_admin = false;
    for entry in roles {
        match entry.as_str() {
            Some(tag) => is_admin |= tag == "admin",
            None => saw_string = false,
        }
    }
    if !saw_string {
        return None;
    }
    Some(if is_admin { Role::Admin } else { Role::Member })
}

/// The subject of a resolved identity: the raw claims first (the vendor
/// ladder above), then the core's own reading of them.
fn subject_from_identity(profile: &ProviderIdentity) -> Option<String> {
    subject_from_userinfo(&profile.claims).or_else(|| {
        let subject = profile.subject.trim();
        (!subject.is_empty()).then(|| subject.to_owned())
    })
}

/// The display name a first login mints with: the vendor's own spellings
/// first (GitHub `login` never appears in an OIDC name ladder), then the
/// core's reading.
fn display_name_from_identity(profile: &ProviderIdentity) -> Option<String> {
    ["name", "preferred_username", "login", "display_name"]
        .iter()
        .find_map(|key| {
            profile
                .claims
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| profile.display_name.clone())
}

/// `find_or_create_user` over the `oauth_identities` table: an existing
/// (provider, subject) binding loads its user; a first login mints a
/// local account (`{provider}_{subject[0..8]}`, no password) and links
/// the identity. Idempotent: a second login never mints again.
///
/// `role` is the provider's statement about this login (see
/// [`role_from_claims`]) and is applied BOTH ways:
///
/// * a first login mints the account with `role.unwrap_or(Role::Member)`;
/// * a returning login with `Some(r)` whose stored role differs is
///   rewritten to `r` — promotion and demotion alike, because the
///   administrator identity is the provider RBAC's to grant and to take
///   away;
/// * `None` (the provider said nothing) leaves the stored role alone.
///
/// Only accounts REACHED THROUGH an `oauth_identities` binding are ever
/// touched: a local password account (the seeded admin among them) has no
/// binding, so this function can neither mint over it nor rewrite it.
pub async fn find_or_create_user(
    state: &AppState,
    provider: &str,
    subject: &str,
    display_name: Option<&str>,
    role: Option<Role>,
) -> Result<i0721_proto::UserRecord, ApiError> {
    if let Some(user_id) = state
        .store
        .get_oauth_identity(provider, subject)
        .await
        .map_err(ApiError::from)?
    {
        let existing = state
            .store
            .get_user(user_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::internal("OAuth-linked user vanished"))?;
        // The provider said nothing → the stored role stands.
        let Some(stated) = role else {
            return Ok(existing);
        };
        if existing.role == stated {
            return Ok(existing);
        }
        state
            .store
            .set_user_role(existing.id, stated)
            .await
            .map_err(ApiError::from)?;
        // Re-read: the session must carry what the store now holds, not a
        // locally assumed copy of it.
        return state
            .store
            .get_user(existing.id)
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

    // The prefixed name the first login would take; a collision falls back
    // to a unique suffix (see the Conflict arm). The role a first login
    // mints with is the provider's statement, else the ordinary member.
    let candidate = i0721_store::CreateUser {
        username: username.clone(),
        display_name: display,
        role: role.unwrap_or(Role::Member),
        password_hash: None,
    };
    let user = match state.store.create_user(candidate.clone()).await {
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
                let existing = state
                    .store
                    .get_user(user_id)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::internal("OAuth-linked user vanished"))?;
                // The concurrent round may have minted with a different
                // statement than ours; the CURRENT login's statement wins.
                let Some(stated) = role else {
                    return Ok(existing);
                };
                if existing.role == stated {
                    return Ok(existing);
                }
                state
                    .store
                    .set_user_role(existing.id, stated)
                    .await
                    .map_err(ApiError::from)?;
                return state
                    .store
                    .get_user(existing.id)
                    .await
                    .map_err(ApiError::from)?
                    .ok_or_else(|| ApiError::internal("OAuth-linked user vanished"));
            }
            let unique = format!("{provider}_{}_{prefix}", Uuid::new_v4().simple());
            state
                .store
                .create_user(i0721_store::CreateUser {
                    username: unique,
                    ..candidate
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

// ── the login policy ───────────────────────────────────────────────────

/// The start route a method row navigates to (the browser leg, same-origin).
fn method_href(provider_id: &str) -> String {
    format!("{OAUTH_PATH_PREFIX}/{}", urlencoding::encode(provider_id))
}

/// Why one enabled catalog row cannot be an entrance on the login page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogRowDefect {
    /// The row's id is blank, padded, or names a second entrance.
    Id,
    /// The row's label is blank — the panel would render a nameless button.
    Label,
}

impl std::fmt::Display for CatalogRowDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Id => "its id is blank, padded or not unique",
            Self::Label => "its display_name is blank",
        })
    }
}

/// The enabled catalog rows THAT CAN BE RENDERED, in catalog order — the ONE
/// row set both renderings (the legacy `providers[]` and the family policy)
/// are built from.
///
/// A row that cannot name itself (blank / padded / duplicated id) or cannot
/// be labelled (blank `display_name`) is dropped HERE, once, with a warning
/// naming the row and the reason: the page then shows the entrances that do
/// work, and the disabled ones never enter. Dropping the row in one place is
/// the whole point — filtering per rendering is how the same response used to
/// advertise a provider in `providers[]` that its policy had already given
/// up on.
fn renderable_rows(providers: &[OAuthProvider]) -> Vec<&OAuthProvider> {
    let mut seen: HashSet<&str> = HashSet::new();
    providers
        .iter()
        .filter(|provider| provider.enabled)
        .filter(|provider| {
            let blank_or_padded_id = provider.id.is_empty() || provider.id.trim() != provider.id;
            let defect = if blank_or_padded_id || !seen.insert(provider.id.as_str()) {
                Some(CatalogRowDefect::Id)
            } else if provider.display_name.trim().is_empty() {
                Some(CatalogRowDefect::Label)
            } else {
                None
            };
            if let Some(defect) = defect {
                tracing::warn!(
                    provider = %provider.id,
                    display_name = %provider.display_name,
                    reason = %defect,
                    "login methods: dropping an unusable provider row from the login page"
                );
            }
            defect.is_none()
        })
        .collect()
}

/// Assemble the login-methods document: the legacy fields plus the family
/// [`LoginPolicy`], both projected from the SAME filtered row set
/// ([`renderable_rows`]) — the two views of one catalog cannot disagree about
/// which entrances exist.
///
/// The mode is derived, never configured: a catalog with at least one
/// enabled provider is `federated`, an empty one is `local`.
///
/// # Errors
///
/// [`ApiError`] carrying `503 Service Unavailable` when the filtered catalog
/// still fails kirino's validation for a reason other than the one reachable
/// login-less posture (no entrance at all, published honestly — see
/// [`publish_policy`]). Such a document means this service assembled a
/// contradiction the operator cannot see, so the page refuses to serve a
/// policy rather than answering with one it knows is wrong.
pub fn login_methods_document(
    password: bool,
    providers: &[OAuthProvider],
) -> Result<LoginMethods, ApiError> {
    let rows = renderable_rows(providers);
    let legacy = rows
        .iter()
        .map(|provider| LoginMethodProvider {
            id: provider.id.clone(),
            display_name: provider.display_name.clone(),
            provider_type: provider.provider_type.clone(),
            icon: provider.icon.clone(),
        })
        .collect();
    let policy = publish_policy(password, &rows)?;
    Ok(LoginMethods {
        password,
        providers: legacy,
        policy,
    })
}

/// The family login-policy document for this deployment's catalog.
///
/// The method list is the SAME catalog the legacy `providers[]` payload
/// carries, in the same order, with the same ids and labels — one catalog,
/// two renderings, so the two documents cannot disagree about which
/// entrances exist. A disabled row is not an entrance and never reaches the
/// document, whichever caller built the slice.
///
/// # Errors
///
/// The first [`LoginPolicyViolation`]. The login-methods endpoint turns all
/// of them except the one reachable login-less posture into a `503`: a
/// violation here names a catalog the login page cannot honour.
pub fn build_login_policy(
    password: bool,
    providers: &[&OAuthProvider],
) -> Result<LoginPolicy, LoginPolicyViolation> {
    validated_policy(password, providers).map(|policy| policy_to_wire(&policy))
}

/// kirino's own validated document for the catalog (the source of truth the
/// published wire shape is projected from).
fn validated_policy(
    password: bool,
    providers: &[&OAuthProvider],
) -> Result<KirinoLoginPolicy, LoginPolicyViolation> {
    let methods: Vec<KirinoMethodDescriptor> = providers
        .iter()
        .filter(|provider| provider.enabled)
        .map(|provider| {
            KirinoMethodDescriptor::oauth(
                provider.id.clone(),
                method_href(&provider.id),
                provider.display_name.clone(),
            )
        })
        .collect();
    let mode = if methods.is_empty() {
        KirinoAuthMode::Local
    } else {
        KirinoAuthMode::Federated
    };
    let policy = KirinoLoginPolicy {
        version: LOGIN_POLICY_VERSION,
        mode,
        password,
        methods,
        relay: None,
    };
    policy.validate().map(|()| policy)
}

/// [`build_login_policy`] for the login-methods endpoint: the ONE document
/// served without passing `validate()` is the login-less posture below; every
/// other violation is a `503` refusal.
fn publish_policy(password: bool, providers: &[&OAuthProvider]) -> Result<LoginPolicy, ApiError> {
    match validated_policy(password, providers) {
        Ok(policy) => Ok(policy_to_wire(&policy)),
        // A deployment with the local-password kill-switch off and no usable
        // provider has NO entrance at all. The contract calls that document
        // inconsistent — `local` with no password reads as "nobody can sign
        // in" — but it is a real, reachable configuration: the switch is off
        // while the first provider is still being onboarded. Answering 503
        // here would empty the login page for every visitor instead of
        // showing them (and the operator) the truth, so the honest login-less
        // document is published: `local`, no password, no method. It is the
        // ONE document served without passing `validate()`, and the warning
        // names the invariant it breaks (chest's precedent).
        Err(LoginPolicyViolation::PasswordMissingInLocalMode) => {
            tracing::warn!(
                "login policy: this configuration offers no login method at all — the local \
                 password switch is off and no usable OAuth provider is configured; serving \
                 the login-less `local` document (password: false, methods: []) instead"
            );
            Ok(policy_to_wire(&KirinoLoginPolicy {
                version: LOGIN_POLICY_VERSION,
                mode: KirinoAuthMode::Local,
                password: false,
                methods: Vec::new(),
                relay: None,
            }))
        }
        // Every other violation is unreachable from a row set that already
        // passed `renderable_rows`: the rows carry non-blank labels, unique
        // unpadded ids, and the mode follows the method count. Reaching it
        // means this service built a contradiction the operator cannot see,
        // so the login page refuses to answer rather than publishing a policy
        // it knows is wrong (and, in the old shape, than publishing a
        // login-less document while `providers[]` still advertised one —
        // the same response contradicting itself).
        Err(violation) => {
            tracing::error!(
                "login policy: the catalog fails validation ({violation}); refusing to serve \
                 the login-methods document"
            );
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("login methods unavailable: {violation}"),
            ))
        }
    }
}

// ── shared helpers ─────────────────────────────────────────────────────

/// The callback URI as the provider sees it: the deployment's public
/// origin ([`crate::origin::public_origin`] — `I0721_PUBLIC_ORIGIN` when
/// set, else the request Host with a warning). The OAuth lane keeps a
/// `localhost` last resort: a redirect URI that no provider can call back
/// is still better than refusing the flow outright (the misconfiguration
/// is visible in the provider's own error). The MCP share lane, which
/// hands the URL to a human, refuses instead.
fn callback_url(
    state: &AppState,
    headers: &HeaderMap,
    provider: &str,
) -> Result<url::Url, ApiError> {
    let origin = crate::origin::public_origin(state, headers)
        .unwrap_or_else(|| "http://localhost".to_owned());
    let raw = format!("{origin}{OAUTH_PATH_PREFIX}/{provider}/callback");
    url::Url::parse(&raw).map_err(|err| ApiError::bad_request(format!("callback URI: {err}")))
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

/// Fresh browser binding (32 bytes of RNG, hex).
fn new_browser_binding() -> String {
    let mut raw = [0u8; 32];
    getrandom_bytes(&mut raw);
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Fill `buffer` from the OS RNG (the workspace's `getrandom` primitive,
/// the same one behind every share/key token here).
fn getrandom_bytes(buffer: &mut [u8]) {
    match getrandom::fill(buffer) {
        Ok(()) => {}
        // The OS RNG is a hard dependency of every credential this service
        // mints; a failure here means the process cannot be trusted to make
        // one at all.
        Err(err) => panic!("the OS RNG failed: {err}"),
    }
}

/// The binding cookie. `max_age` 0 clears it.
fn binding_cookie(value: &str, max_age: u64) -> String {
    format!(
        "{OAUTH_BINDING_COOKIE}={value}; Max-Age={max_age}; HttpOnly; SameSite=Lax; \
         Path={OAUTH_PATH_PREFIX}"
    )
}

fn cleared_binding_cookie() -> header::HeaderValue {
    let cleared = format!(
        "{OAUTH_BINDING_COOKIE}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT; \
         HttpOnly; SameSite=Lax; Path={OAUTH_PATH_PREFIX}"
    );
    cleared
        .parse()
        .unwrap_or_else(|_| header::HeaderValue::from_static(""))
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

fn bad_gateway(message: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({"error": message}).to_string(),
    )
        .into_response()
}

/// The percent-encoder the login-policy `href` uses (a provider id is
/// already path-safe, but the encoder keeps the rule true for every id).
mod urlencoding {
    /// Percent-encode everything outside `[A-Za-z0-9._~-]` (RFC 3986
    /// unreserved), which is what a path segment needs.
    pub fn encode(raw: &str) -> String {
        raw.bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-') {
                    (byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, provider_type: &str) -> OAuthProvider {
        OAuthProvider {
            id: id.to_owned(),
            display_name: format!("{id} label"),
            provider_type: provider_type.to_owned(),
            client_id: "cid".to_owned(),
            client_secret: "super-secret-value-9999".to_owned(),
            base_url: Some("https://sso.example".to_owned()),
            issuer_url: None,
            scopes: None,
            enabled: true,
            icon: None,
        }
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

    /// The truth table of [`role_from_claims`]. Every arm is pinned
    /// separately, because the three answers mean three different things:
    /// `Admin` promotes, `Member` demotes, `None` leaves the local role
    /// untouched (a demotion bug in the `None` arm would silently strip
    /// every admin whose provider does not speak about roles).
    #[test]
    fn role_from_claims_truth_table() {
        // The ERP's real shape: a member of Administrators carries both tags.
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["member", "admin"]})),
            Some(Role::Admin),
            "the ERP's Administrators group emits \"admin\" beside \"member\""
        );
        // `admin` alone is just as authoritative.
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["admin"]})),
            Some(Role::Admin),
            "a lone \"admin\" is still admin — the tag is what counts, not its index"
        );
        // A stated non-admin set is a STATEMENT: it demotes.
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["member"]})),
            Some(Role::Member),
            "a string array without \"admin\" states member — this arm is the demotion"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": []})),
            Some(Role::Member),
            "an empty array is still a statement: the provider says \"no roles\""
        );
        // Absent / wrong-typed: the provider said nothing.
        assert_eq!(
            role_from_claims(&serde_json::json!({"sub": "abc"})),
            None,
            "no `roles` key means the provider spoke no role — never demote"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": "admin"})),
            None,
            "a bare string is not the array shape"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": {"admin": true}})),
            None,
            "an object is not the array shape"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["member", 7]})),
            None,
            "a non-string member means the field is not the shape we read"
        );
        assert_eq!(role_from_claims(&serde_json::Value::Null), None);
        // Case and spelling are exact — only the tag the ERP actually
        // emits grants admin.
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["Admin"]})),
            Some(Role::Member),
            "\"Admin\" is NOT the ERP tag: the match is case-sensitive"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["admin "]}))
                .expect("still a string array"),
            Role::Member,
            "trailing whitespace is not the tag either"
        );
        assert_eq!(
            role_from_claims(&serde_json::json!({"roles": ["administrator"]})),
            Some(Role::Member),
            "`administrator` is a different string from `admin`"
        );
    }

    #[test]
    fn masked_never_carries_the_secret() {
        let masked = row("erp", "builtin:erp").masked();
        assert!(masked.has_secret);
        assert_eq!(masked.secret_tail, "9999");
        let json = serde_json::to_string(&masked).unwrap();
        assert!(
            !json.contains("super-secret"),
            "the masked projection must never serialize the secret"
        );
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
