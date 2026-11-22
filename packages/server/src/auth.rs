//! Transition-era authentication: argon2id passwords and hand-rolled
//! HS256 JWTs (verified strictly — the only accepted algorithm).
//!
//! The OAuth provider directory arrives in wave 4 and replaces the
//! password lane; the token layer is expected to carry over.

use std::net::IpAddr;
use std::sync::Mutex;

use axum::http::HeaderMap;
use ipnet::IpNet;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use i0721_proto::settings_keys;
use jiff::Timestamp;
use password_hash::rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::str::FromStr;
use uuid::Uuid;

use crate::error::ApiError;

/// Token lifetime — the legacy engine issued 15-day tokens.
pub const TOKEN_TTL_DAYS: i64 = 15;

/// Leeway when comparing a token's `iat` against the user's watermark
/// (the legacy engine allowed 5 s of clock skew).
pub const WATERMARK_LEEWAY_SECONDS: i64 = 5;

/// The login rate limit: at most this many attempts per source IP …
pub const LOGIN_ATTEMPT_LIMIT: usize = 5;
/// … inside this window.
pub const LOGIN_WINDOW: Duration = Duration::from_secs(60);

type HmacSha256 = Hmac<Sha256>;

/// Why a token did not verify.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    /// Not three `base64url` segments.
    #[error("malformed token")]
    Malformed,
    /// The header is not `{"alg":"HS256","typ":"JWT"}`.
    #[error("wrong token header")]
    WrongHeader,
    /// The payload did not decode into claims.
    #[error("unreadable token payload")]
    BadPayload,
    /// The signature did not match (constant-time comparison failed).
    #[error("bad signature")]
    BadSignature,
    /// The token is expired.
    #[error("token expired")]
    Expired,
}

/// HS256 JWT claims. `sub` is the user id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — the user id.
    pub sub: String,
    /// Issued-at (unix seconds).
    pub iat: i64,
    /// Expiry (unix seconds).
    pub exp: i64,
}

/// Signs and verifies the transition-era HS256 JWTs.
#[derive(Debug, Clone)]
pub struct JwtSigner {
    secret: Vec<u8>,
}

impl JwtSigner {
    /// Build a signer from the configured secret.
    #[must_use]
    pub fn new(secret: &str) -> Self {
        Self {
            secret: secret.as_bytes().to_vec(),
        }
    }

    /// Issue a token for `user_id` valid for [`TOKEN_TTL_DAYS`] days.
    #[must_use]
    pub fn sign(&self, user_id: Uuid, now: Timestamp) -> String {
        let exp = now.as_second() + TOKEN_TTL_DAYS * 24 * 3600;
        let header = B64.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let claims = Claims {
            sub: user_id.to_string(),
            iat: now.as_second(),
            exp,
        };
        let payload = B64.encode(serde_json::to_string(&claims).expect("claims serialize"));
        let signing_input = format!("{header}.{payload}");
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("hmac takes any key");
        mac.update(signing_input.as_bytes());
        let signature = B64.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{signature}")
    }

    /// Verify a token and return its claims.
    ///
    /// # Errors
    ///
    /// [`TokenError`] for every rejection; `alg` confusion and tampered
    /// signatures can never pass (the header must be exactly HS256, the
    /// signature is compared in constant time by the HMAC verifier).
    pub fn verify(&self, token: &str) -> Result<Claims, TokenError> {
        let mut parts = token.split('.');
        let (Some(header_b64), Some(payload_b64), Some(signature_b64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TokenError::Malformed);
        };
        let header = B64
            .decode(header_b64)
            .map_err(|_| TokenError::WrongHeader)?;
        if header != b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}" {
            return Err(TokenError::WrongHeader);
        }
        // Verify over the raw segments: re-encoding could normalize
        // padding/canonicalization the signer never produced.
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("hmac takes any key");
        mac.update(format!("{header_b64}.{payload_b64}").as_bytes());
        let expected = mac.finalize().into_bytes();
        let given = B64
            .decode(signature_b64)
            .map_err(|_| TokenError::BadSignature)?;
        // Constant-time comparison of the full tag.
        if expected.as_slice() != given.as_slice() {
            return Err(TokenError::BadSignature);
        }
        let claims: Claims = serde_json::from_slice(
            &B64.decode(payload_b64)
                .map_err(|_| TokenError::BadPayload)?,
        )
        .map_err(|_| TokenError::BadPayload)?;
        if claims.exp <= jiff::Timestamp::now().as_second() {
            return Err(TokenError::Expired);
        }
        Ok(claims)
    }
}

/// Hash a password with argon2id.
///
/// # Errors
///
/// [`ApiError::internal`] when the OS RNG or the hasher fails (both are
/// not expected to fail on a healthy host).
pub fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| ApiError::internal(format!("password hashing failed: {err}")))
}

/// Verify a password against its argon2id hash.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// The login rate limiter — the legacy engine's in-memory log: count the
/// attempts of one source IP inside [`LOGIN_WINDOW`] and refuse past
/// [`LOGIN_ATTEMPT_LIMIT`].
#[derive(Debug, Default)]
pub struct LoginLimiter {
    attempts: Mutex<Vec<(IpAddr, Instant)>>,
}

impl LoginLimiter {
    /// Record an attempt from `ip` and tell whether it is allowed.
    ///
    /// # Errors
    ///
    /// [`ApiError::too_many_requests`] past the limit.
    pub fn record(&self, ip: IpAddr) -> Result<(), ApiError> {
        let mut attempts = self.attempts.lock().expect("login log mutex");
        let now = Instant::now();
        attempts.retain(|(_, at)| now.duration_since(*at) < LOGIN_WINDOW);
        attempts.push((ip, now));
        let count = attempts
            .iter()
            .filter(|(attempt_ip, at)| *attempt_ip == ip && now.duration_since(*at) < LOGIN_WINDOW)
            .count();
        if count > LOGIN_ATTEMPT_LIMIT {
            return Err(ApiError::too_many_requests("Too many requests"));
        }
        Ok(())
    }
}

/// Derive the client IP for rate limiting.
///
/// Behind a reverse proxy the socket address is the proxy's, so
/// `X-Forwarded-For` is the only signal carrying the real client — but
/// it is trivially forgeable. The rule: only when the **socket** peer is
/// inside [`Config::trusted_proxies`](crate::config::Config) is the
/// header honored at all, and then the *rightmost* parseable entry (the
/// one appended by the proxy closest to us) identifies the client. An
/// untrusted — or absent/invalid — header leaves the socket address in
/// charge.
#[must_use]
pub fn effective_client_ip(
    trusted_proxies: &[IpNet],
    socket: IpAddr,
    headers: &HeaderMap,
) -> IpAddr {
    let trusted = trusted_proxies
        .iter()
        .any(|net| crate::config::net_contains(net, socket));
    if !trusted {
        return socket;
    }
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(',')
                .rev()
                .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
                .next()
        })
        .unwrap_or(socket)
}

/// Read the token watermark — the RFC 3339 timestamp of the user's most
/// recent login — and tell whether a token's `iat` still clears it.
///
/// Missing watermark = no login ever recorded = the token clears.
#[must_use]
pub fn clears_watermark(watermark: Option<&str>, iat: i64) -> bool {
    match watermark.and_then(|raw| Timestamp::from_str(raw).ok()) {
        Some(wm) => iat >= wm.as_second() - WATERMARK_LEEWAY_SECONDS,
        None => true,
    }
}

/// The settings key of a user's token watermark.
#[must_use]
pub fn watermark_key(user_id: Uuid) -> String {
    settings_keys::token_watermark(&user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_signer() -> JwtSigner {
        JwtSigner::new("test-secret")
    }

    #[test]
    fn sign_verify_round_trip() {
        let signer = fixed_signer();
        let now = Timestamp::now();
        let token = signer.sign(Uuid::new_v4(), now);
        let claims = signer.verify(&token).expect("round trip");
        assert_eq!(claims.exp - claims.iat, TOKEN_TTL_DAYS * 24 * 3600);
    }

    #[test]
    fn alg_confusion_is_rejected() {
        let signer = fixed_signer();
        let now = Timestamp::now();
        let token = signer.sign(Uuid::new_v4(), now);
        // Swap the header to "none" while keeping payload+signature.
        let mut parts = token.split('.');
        let _header = parts.next().unwrap();
        let payload = parts.next().unwrap();
        let signature = parts.next().unwrap();
        let forged = format!(
            "{}.{payload}.{signature}",
            B64.encode(r#"{"alg":"none","typ":"JWT"}"#)
        );
        assert_eq!(signer.verify(&forged), Err(TokenError::WrongHeader));
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let signer = fixed_signer();
        let token = signer.sign(Uuid::new_v4(), Timestamp::now());
        let tampered = format!("{token}x");
        assert!(matches!(
            signer.verify(&tampered),
            Err(TokenError::BadSignature | TokenError::Malformed)
        ));
    }

    #[test]
    fn foreign_secret_is_rejected() {
        let token = fixed_signer().sign(Uuid::new_v4(), Timestamp::now());
        assert_eq!(
            JwtSigner::new("other-secret").verify(&token),
            Err(TokenError::BadSignature)
        );
    }

    #[test]
    fn password_hash_round_trip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("wrong", &hash));
        assert!(!verify_password("x", "not-a-hash"));
    }

    #[test]
    fn login_limiter_allows_five_refuses_sixth() {
        let limiter = LoginLimiter::default();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        for _ in 0..LOGIN_ATTEMPT_LIMIT {
            limiter.record(ip).unwrap();
        }
        assert!(
            limiter.record(ip).is_err(),
            "attempt {} is the first refused one",
            LOGIN_ATTEMPT_LIMIT + 1
        );
        // A different IP is unaffected.
        let other: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(limiter.record(other).is_ok());
    }

    #[test]
    fn xff_is_ignored_without_trust_and_honored_with_it() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_static("9.9.9.9, 8.8.8.8"),
        );
        let socket: IpAddr = "127.0.0.1".parse().unwrap();

        // Untrusted socket: the forged header changes nothing.
        assert_eq!(effective_client_ip(&[], socket, &headers), socket);

        // Trusted socket: the rightmost parseable entry identifies the
        // client.
        let loopback: IpNet = "127.0.0.0/8".parse().unwrap();
        assert_eq!(
            effective_client_ip(&[loopback], socket, &headers),
            "8.8.8.8".parse::<IpAddr>().unwrap()
        );

        // Garbage entries are skipped; with none left, the socket wins.
        let garbage = HeaderMap::new();
        assert_eq!(effective_client_ip(&[loopback], socket, &garbage), socket);
    }

    #[test]
    fn watermark_semantics_match_the_legacy_engine() {
        let now = Timestamp::now().to_string();
        let now_s = Timestamp::now().as_second();
        assert!(clears_watermark(Some(&now), now_s));
        assert!(clears_watermark(
            Some(&now),
            now_s - WATERMARK_LEEWAY_SECONDS
        ));
        assert!(!clears_watermark(
            Some(&now),
            now_s - WATERMARK_LEEWAY_SECONDS - 1
        ));
        assert!(clears_watermark(None, 0));
        assert!(clears_watermark(Some("garbage"), 0));
    }
}
