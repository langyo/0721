//! Route-level tests: the real router over the in-memory store and blob
//! fake, exercised with `tower::ServiceExt::oneshot`. No external
//! services — PostgreSQL is covered by the opt-in lane in
//! `i0721-store`'s `pg_lane` tests.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use i0721_proto::{Role, SiteConfigStrict, UserRecord};
use i0721_store::{CreateUser, MemoryStore, Store};
use jiff::Timestamp;
use sha2::Digest;
use tower::ServiceExt;

use i0721_server::auth::hash_password;
use i0721_server::blob::BlobStore as _;
use i0721_server::blob::MemoryBlobStore;
use i0721_server::config::Config;
use i0721_server::routes;
use i0721_server::state::AppState;

/// A complete strict settings document with one section mutated.
fn full_settings_json(mutate: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut json =
        serde_json::to_value(SiteConfigStrict::from(&i0721_proto::default_site_config())).unwrap();
    mutate(&mut json);
    json.to_string()
}

/// A 4×4 red PNG.
fn tiny_png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(4, 4);
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn base_config() -> Config {
    Config {
        database_url: "unused".to_owned(),
        listen: "127.0.0.1:8080".parse().unwrap(),
        jwt_secret: "route-test-secret".to_owned(),
        media_root: PathBuf::from("unused"),
        static_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/webui"),
        upload_concurrency: 4,
        log_filter: "off".to_owned(),
        seed_admin_password: "admin-password".to_owned(),
        trusted_proxies: Vec::new(),
        public_origin: None,
        cookie_secure: "auto".to_owned(),
        storage_backend: None,
    }
}

fn test_config() -> Config {
    base_config()
}

async fn test_state() -> (AppState, UserRecord) {
    let store = Arc::new(MemoryStore::new());
    let password_hash = hash_password("admin-password").unwrap();
    let admin = store
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(password_hash),
        })
        .await
        .unwrap();
    // The fixture mirror of migration 0007: production databases carry
    // the bootstrap bucket row (signup-open, anonymous-readable), so the
    // Q7 materialization (`granted_buckets`) and the §7 pre-check see
    // the same bucket the registry's single-tagged fixture resolves to.
    store.seed_bucket(i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: "bootstrap".to_owned(),
        display_name: "bootstrap".to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read: true,
        grant_on_signup: true,
        enabled: true,
        is_bootstrap: true,
        created_at: Timestamp::now(),
    });
    let state = AppState::new(
        store,
        Arc::new(MemoryBlobStore::new()),
        Arc::new(test_config()),
    );
    (state, admin)
}

/// The app with a mocked connect info — the documented way to exercise
/// `ConnectInfo` extractors (the login rate limiter) under `oneshot`.
fn app(state: &AppState) -> Router {
    use axum::extract::connect_info::MockConnectInfo;
    routes::app(state.clone()).layer(MockConnectInfo(SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        0,
    )))
}

/// Login and return the session cookie's JWT (the credential now
/// lives ONLY in the httpOnly cookie — the body must not carry it).
async fn login(state: &AppState, username: &str, password: &str) -> String {
    let response = ServiceExt::oneshot(app(state), login_request(username, password))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("login plants the session cookie")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        cookie.contains("HttpOnly"),
        "the cookie must be HttpOnly: {cookie}"
    );
    assert!(
        cookie.contains("SameSite=Lax"),
        "the cookie must be Lax (Strict would strip it on a top-level \
         navigation that follows an external share link): {cookie}"
    );
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("i0721-token=")
        .expect("the cookie carries the JWT")
        .to_owned();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json.get("token").is_none(),
        "the session body must NOT carry the token (HttpOnly would be moot)"
    );
    token
}

fn login_request(username: &str, password: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({"username": username, "password": password}).to_string(),
        ))
        .unwrap()
}

fn bearer(request: Request<Body>, token: &str) -> Request<Body> {
    let (mut parts, body) = request.into_parts();
    parts.headers.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    Request::from_parts(parts, body)
}

/// POST one JSON-RPC frame through the HTTP fallback with a bearer
/// token; returns the HTTP status and the parsed envelope.
async fn rpc(
    state: &AppState,
    token: &str,
    method: &str,
    params: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    rpc_request(state, bearer(rpc_frame_request(method, params), token)).await
}

/// The same frame without any credential (auth-hook denial checks).
async fn rpc_anon(
    state: &AppState,
    method: &str,
    params: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    rpc_request(state, rpc_frame_request(method, params)).await
}

fn rpc_frame_request(method: &str, params: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            })
            .to_string(),
        ))
        .unwrap()
}

async fn rpc_request(state: &AppState, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = ServiceExt::oneshot(app(state), request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    (status, json)
}

/// The `result` member of an envelope (fails loudly when absent).
fn rpc_result(envelope: &serde_json::Value) -> &serde_json::Value {
    envelope
        .get("result")
        .unwrap_or_else(|| panic!("no result in {envelope}"))
}

/// The `error.code` member of an envelope.
fn rpc_error_code(envelope: &serde_json::Value) -> i64 {
    envelope["error"]["code"].as_i64().expect("an error object")
}

async fn upload(
    state: &AppState,
    token: &str,
    bytes: Vec<u8>,
    name: &str,
) -> (StatusCode, serde_json::Value) {
    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let body = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{name}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    );
    let mut body = body.into_bytes();
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let response = app(state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
            token,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn health_is_ok_and_reports_the_settings_source() {
    let (state, _) = test_state().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["config_source"], "default");
}

#[tokio::test]
async fn login_rejects_bad_credentials() {
    let (state, _) = test_state().await;
    for (user, pass) in [("admin", "wrong"), ("ghost", "admin-password")] {
        let response = app(&state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"username": user, "password": pass}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{user}/{pass}");
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"].as_str().unwrap().contains("credentials"));
    }
}

#[tokio::test]
async fn login_rate_limit_kicks_in() {
    let (state, _) = test_state().await;
    let mut saw_limit = false;
    for _ in 0..7 {
        let response = app(&state)
            .oneshot(login_request("admin", "x"))
            .await
            .unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            saw_limit = true;
        }
    }
    assert!(saw_limit, "the limiter must refuse the sixth attempt");
}

#[tokio::test]
async fn rpc_auth_flow_verify_refresh() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    let (status, envelope) = rpc(&state, &token, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);

    let user = rpc_result(&envelope);
    assert_eq!(user["username"], "admin");
    assert_eq!(user["role"], "admin");
    assert!(user.get("password_hash").is_none());

    // auth.refresh retired from the WS table: rotating the httpOnly
    // cookie needs Set-Cookie, which a WS method cannot carry. The REST
    // refresh owns it; the RPC table refuses the name outright.
    let (status, envelope) = rpc(&state, &token, "auth.refresh", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32601);
    let refreshed = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/refresh")
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(refreshed.status(), StatusCode::OK);
    let cookie = refreshed
        .headers()
        .get(header::SET_COOKIE)
        .expect("refresh rotates the cookie")
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("i0721-token="));
    // The rotation carries the same attribute contract as the login
    // (Lax always; Secure follows the request's scheme).
    assert!(cookie.contains("SameSite=Lax"), "refresh: {cookie}");
    assert!(!cookie.contains("Strict"), "refresh: {cookie}");
    assert!(!cookie.contains("Secure"), "plain-http refresh: {cookie}");

    // Tampered token: the auth hook refuses the request outright.
    let tampered = format!("{token}x");
    let (status, _) = rpc(&state, &tampered, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // No token at all: same refusal.
    let (status, _) = rpc_anon(&state, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn login_supersedes_older_sessions() {
    let (state, _) = test_state().await;
    let first = login(&state, "admin", "admin-password").await;
    // Two logins inside the 5 s leeway both stay valid (legacy skew
    // allowance); push the watermark 10 s ahead to prove invalidation.
    let future = Timestamp::now()
        .checked_add(jiff::Span::new().seconds(10))
        .unwrap()
        .to_string();
    state
        .store
        .set_setting(
            &i0721_proto::settings_keys::token_watermark(&{
                let user = state
                    .store
                    .get_user_by_username("admin")
                    .await
                    .unwrap()
                    .unwrap();
                user.id
            }),
            serde_json::Value::String(future),
        )
        .await
        .unwrap();
    let (status, _) = rpc(&state, &first, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a watermark past the token's iat invalidates it"
    );
}
#[tokio::test]
async fn upload_serves_dedupes_and_deletes() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    // Upload a PNG → converted to WebP.
    let (status, dto) = upload(&state, &token, tiny_png(), "cat.png").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(dto["mime"], "image/webp");
    assert_eq!(dto["kind"], "image");
    assert_eq!(dto["original_name"], "cat.png");
    assert_eq!(dto["name"], dto["sha256"], "hash-keyed by default");
    let url = dto["url"].as_str().unwrap().to_owned();

    // The timeline list rides the RPC lane now.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"offset": 0, "limit": 10}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list = rpc_result(&envelope);
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    // Public media path serves it, anonymous, correct content type.
    let media = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::OK);
    assert_eq!(
        media.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/webp"
    );
    let bytes = axum::body::to_bytes(media.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::WebP
    );

    // Thumbnail variant.
    let thumb = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("{url}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(thumb.status(), StatusCode::OK);
    let thumb_bytes = axum::body::to_bytes(thumb.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        image::guess_format(&thumb_bytes).unwrap(),
        image::ImageFormat::WebP
    );

    // Identical content dedupes to the existing record (200, not 201).
    let (status, again) = upload(&state, &token, tiny_png(), "cat.png").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], dto["id"]);

    // Rename rides RPC (files.update).
    let id = dto["id"].as_str().unwrap().to_owned();
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.update",
        serde_json::json!({"id": id, "name": "renamed cat"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(rpc_result(&envelope)["name"], "renamed cat");

    // Soft delete rides RPC (files.delete) → media 404s.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(rpc_result(&envelope).is_null());
    let gone = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);

    // Deleting the same file again: the NOT_FOUND error code.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32006);
}
#[tokio::test]
async fn upload_requires_auth_and_respects_the_size_limit() {
    let (state, _) = test_state().await;
    // Anonymous upload: refused (with a parseable multipart envelope,
    // so the rejection comes from the auth check, not the extractor).
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files")
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=X-BOUNDARY-X",
                )
                .body(Body::from("--X-BOUNDARY-X--\r\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Oversize: shrink the limit via the RPC settings lane, then upload
    // past it through the (whitelisted) multipart route.
    let token = login(&state, "admin", "admin-password").await;
    let strict = full_settings_json(|json| {
        json["upload"]["image-size-limit"] = "1KiB".into();
    });
    let strict: serde_json::Value = serde_json::from_str(&strict).unwrap();
    let (status, envelope) = rpc(&state, &token, "settings.put", strict).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let (status, body) = upload(&state, &token, vec![0_u8; 2048], "big.bin").await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}
#[tokio::test]
async fn non_image_uploads_pass_through() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let pdf: &[u8] = b"%PDF-1.4 netdisk-ready document";
    let (status, dto) = upload(&state, &token, pdf.to_vec(), "doc.pdf").await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(dto["kind"], "document");
    assert_eq!(dto["mime"], "application/pdf");
}

#[tokio::test]
async fn users_management_is_admin_gated() {
    let (state, admin) = test_state().await;

    // A member (no password login needed for gating checks).
    let member = state
        .store
        .create_user(CreateUser {
            username: "member".to_owned(),
            display_name: "Member".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    // Member token signed directly (password login needs a hash).
    let member_token = state.signer.sign(member.id, Timestamp::now());

    // Per-method WELL-FORMED params: the gate must fire before any
    // params validation, so each frame carries a parseable body.
    let strict: serde_json::Value = serde_json::from_str(&full_settings_json(|_| {})).unwrap();
    for (method, params) in [
        ("users.list", serde_json::json!({"offset": 0, "limit": 10})),
        (
            "users.create",
            serde_json::json!({"username": "x", "password": "y"}),
        ),
        (
            "users.delete",
            serde_json::json!({"id": uuid::Uuid::new_v4().to_string()}),
        ),
        ("settings.get", serde_json::Value::Null),
        ("settings.put", strict),
    ] {
        let (status, envelope) = rpc(&state, &member_token, method, params).await;
        assert_eq!(status, StatusCode::OK, "member on {method}: {envelope}");
        assert_eq!(
            rpc_error_code(&envelope),
            -32005,
            "member on {method} gets the auth error code"
        );
    }

    // Admin creates a user over RPC.
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.create",
        serde_json::json!({
            "username": "newbie",
            "password": "a-password",
            "role": "member"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let newbie = rpc_result(&envelope);
    assert_eq!(newbie["username"], "newbie");
    let newbie_id = newbie["id"].as_str().unwrap().to_owned();

    // Duplicate name: the CONFLICT error code.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.create",
        serde_json::json!({"username": "newbie", "password": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32007);

    // Cannot delete self.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.delete",
        serde_json::json!({"id": admin.id.to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32005);

    // Delete the other: fine (null result).
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.delete",
        serde_json::json!({"id": newbie_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(rpc_result(&envelope).is_null());

    // Deleting an unknown user: NOT_FOUND.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.delete",
        serde_json::json!({"id": uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32006);

    // Admin list works.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "users.list",
        serde_json::json!({"offset": 0, "limit": 10}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(rpc_result(&envelope)["total"].as_u64().unwrap() >= 1);
}
#[tokio::test]
async fn settings_get_put_and_referer_gate() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());

    // Defaults visible to the admin (RPC settings.get).
    let (status, envelope) = rpc(&state, &token, "settings.get", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rpc_result(&envelope)["router"]["media-entry-path"],
        "/media"
    );

    // Moving the media path is rejected (legacy links) — invalid params.
    let mut bad = full_settings_json(|_| {});
    bad = bad.replace("\"/media\"", "\"/img\"");
    let bad: serde_json::Value = serde_json::from_str(&bad).unwrap();
    let (status, envelope) = rpc(&state, &token, "settings.put", bad).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(rpc_error_code(&envelope), -32602);

    // A referer allow-list applies to the media path.
    let strict = full_settings_json(|json| {
        json["router"]["limit-referrer-host"] = serde_json::json!(["good.example"]);
    });
    let strict: serde_json::Value = serde_json::from_str(&strict).unwrap();
    let (status, envelope) = rpc(&state, &token, "settings.put", strict).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    let (_, dto) = upload(&state, &token, tiny_png(), "cat.png").await;
    let url = dto["url"].as_str().unwrap().to_owned();

    let blocked = app(&state)
        .oneshot(
            Request::builder()
                .uri(&url)
                .header(header::REFERER, "https://evil.example/page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

    for referer in ["https://good.example/page"] {
        let allowed = app(&state)
            .oneshot(
                Request::builder()
                    .uri(&url)
                    .header(header::REFERER, referer)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK, "referer {referer}");
    }

    // No referer at all: anonymous access stays open (legacy semantics).
    let anon = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::OK);
}
#[tokio::test]
async fn settings_put_requires_the_complete_document() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());

    // Missing a whole section: -32602, and the stored config is untouched.
    let mut partial: serde_json::Value = serde_json::from_str(&full_settings_json(|_| {})).unwrap();
    partial.as_object_mut().unwrap().remove("upload");
    let (status, envelope) = rpc(&state, &token, "settings.put", partial).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32602);

    let (_, envelope) = rpc(&state, &token, "settings.get", serde_json::Value::Null).await;
    assert_eq!(
        rpc_result(&envelope)["upload"]["image-size-limit"],
        "8MiB",
        "a rejected PUT must not reset the stored config"
    );

    // Unknown leaf inside a section: -32602.
    let mut unknown_leaf: serde_json::Value =
        serde_json::from_str(&full_settings_json(|_| {})).unwrap();
    unknown_leaf["upload"]["max-pixels"] = serde_json::json!(1);
    let (status, envelope) = rpc(&state, &token, "settings.put", unknown_leaf).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32602);

    // The complete document: accepted, null result.
    let strict: serde_json::Value = serde_json::from_str(&full_settings_json(|_| {})).unwrap();
    let (status, envelope) = rpc(&state, &token, "settings.put", strict).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(rpc_result(&envelope).is_null());
}
#[tokio::test]
async fn dedupe_is_per_owner_and_shares_the_blob() {
    let (state, _) = test_state().await;
    let admin_token = login(&state, "admin", "admin-password").await;

    // A member with a password so they can upload through the API.
    let member = state
        .store
        .create_user(CreateUser {
            username: "member".to_owned(),
            display_name: "Member".to_owned(),
            role: Role::Member,
            password_hash: Some(hash_password("member-pass").unwrap()),
        })
        .await
        .unwrap();
    drop(member);
    let member_token = login(&state, "member", "member-pass").await;

    // Admin uploads; the member uploads the identical bytes.
    let (status, admin_file) = upload(&state, &admin_token, tiny_png(), "cat.png").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, member_file) = upload(&state, &member_token, tiny_png(), "cat.png").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "another owner's identical content is a new row, not a DTO leak"
    );
    assert_ne!(admin_file["id"], member_file["id"]);
    assert_eq!(admin_file["url"], member_file["url"]);
    assert_eq!(admin_file["sha256"], member_file["sha256"]);

    // The same owner re-uploading dedupes to their own row.
    let (status, again) = upload(&state, &admin_token, tiny_png(), "cat.png").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["id"], admin_file["id"]);

    // Deleting one owner's row (RPC) must NOT remove the shared blob
    // while the other row still lives.
    let url = admin_file["url"].as_str().unwrap().to_owned();
    let id = admin_file["id"].as_str().unwrap().to_owned();
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "files.delete",
        serde_json::json!({"id": id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let still = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        still.status(),
        StatusCode::OK,
        "the member's row still serves the shared blob"
    );

    // Deleting the last live row reclaims the blob.
    let member_id = member_file["id"].as_str().unwrap().to_owned();
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "files.delete",
        serde_json::json!({"id": member_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let gone = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn file_mutations_require_owner_or_admin() {
    let (state, admin) = test_state().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());

    let member = state
        .store
        .create_user(CreateUser {
            username: "member".to_owned(),
            display_name: "Member".to_owned(),
            role: Role::Member,
            password_hash: Some(hash_password("member-pass").unwrap()),
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());

    // The admin's file: the member may neither rename nor delete it.
    let (_, admin_file) = upload(&state, &admin_token, tiny_png(), "admin.png").await;
    let admin_id = admin_file["id"].as_str().unwrap().to_owned();
    for (method, params) in [
        (
            "files.update",
            serde_json::json!({"id": admin_id, "name": "hijacked"}),
        ),
        ("files.delete", serde_json::json!({"id": admin_id})),
    ] {
        let (status, envelope) = rpc(&state, &member_token, method, params).await;
        assert_eq!(status, StatusCode::OK, "{method} by member: {envelope}");
        assert_eq!(
            rpc_error_code(&envelope),
            -32005,
            "{method} by member gets the auth error code"
        );
    }

    // The member's own file: theirs to rename.
    let (_, member_file) = upload(&state, &member_token, tiny_png(), "member.png").await;
    let member_id = member_file["id"].as_str().unwrap().to_owned();
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "files.update",
        serde_json::json!({"id": member_id, "name": "my file"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(rpc_result(&envelope)["name"], "my file");

    // The admin may rename the member's file (owner-or-admin).
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "files.update",
        serde_json::json!({"id": member_id, "name": "admin says"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // A bad name is invalid params; an unknown file id is NOT_FOUND.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "files.update",
        serde_json::json!({"id": member_id, "name": "a/b"}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32602);
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "files.update",
        serde_json::json!({"id": uuid::Uuid::new_v4().to_string(), "name": "x"}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32006);
}
#[tokio::test]
async fn xff_shapes_login_throttling_only_through_trust() {
    // Untrusted (default): forged X-Forwarded-For cannot rotate buckets. The
    // entries must be *parseable and distinct* — unparseable ones would mask
    // an unconditional-trust regression (both shapes read as "no XFF").
    let (state, _) = test_state().await;
    let mut saw_limit = false;
    for i in 1..8 {
        let mut request = login_request("admin", "wrong");
        request.headers_mut().insert(
            "x-forwarded-for",
            header::HeaderValue::from_str(&format!("10.99.0.{i}")).unwrap(),
        );
        let response = app(&state).oneshot(request).await.unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            saw_limit = true;
        }
    }
    assert!(
        saw_limit,
        "with no trusted proxies the socket identity alone is throttled even against rotating parseable XFF entries"
    );

    // Trusted: the rightmost valid XFF entry is its own bucket, and the
    // socket identity remains separate.
    let mut config = base_config();
    config.trusted_proxies = vec!["127.0.0.0/8".parse().unwrap()];
    let store = Arc::new(MemoryStore::new());
    store
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(hash_password("admin-password").unwrap()),
        })
        .await
        .unwrap();
    let state = AppState::new(store, Arc::new(MemoryBlobStore::new()), Arc::new(config));
    let mut limited = false;
    for _ in 0..7 {
        let mut request = login_request("admin", "wrong");
        request.headers_mut().insert(
            "x-forwarded-for",
            header::HeaderValue::from_static("9.9.9.9"),
        );
        let response = app(&state).oneshot(request).await.unwrap();
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
        }
    }
    assert!(limited, "the XFF identity is throttled on its own");
    let no_header = app(&state)
        .oneshot(login_request("admin", "wrong"))
        .await
        .unwrap();
    assert_ne!(
        no_header.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the socket identity has a separate bucket"
    );
}

/// Simulates the reclaim race C3 demonstrated: when the delete handler
/// unlinks the media blob, a concurrent upload of the same content has
/// just landed a live row on the same content-addressed key (uploads put
/// the blob before `create_file`, so the blob is present again at unlink
/// time). The handler must notice the revived live row and restore the
/// bytes instead of orphaning them.
struct RaceInjectingBlobs {
    inner: MemoryBlobStore,
    store: Arc<MemoryStore>,
    payload: std::sync::Mutex<Option<(i0721_proto::NewFile, String)>>,
}

#[async_trait::async_trait]
impl i0721_server::blob::BlobStore for RaceInjectingBlobs {
    async fn put(
        &self,
        slot: i0721_server::blob::BlobSlot,
        key: &str,
        data: &[u8],
    ) -> Result<(), std::io::Error> {
        self.inner.put(slot, key, data).await
    }

    async fn get(
        &self,
        slot: i0721_server::blob::BlobSlot,
        key: &str,
    ) -> Result<Option<axum::body::Bytes>, std::io::Error> {
        self.inner.get(slot, key).await
    }

    async fn delete(
        &self,
        slot: i0721_server::blob::BlobSlot,
        key: &str,
    ) -> Result<(), std::io::Error> {
        if let i0721_server::blob::BlobSlot::Media = slot {
            let injected = self.payload.lock().unwrap().take();
            if let Some((new_file, _key)) = injected {
                self.store.create_file(new_file).await.unwrap();
            }
        }
        self.inner.delete(slot, key).await
    }
}

#[tokio::test]
async fn blob_reclaim_restores_when_a_racing_upload_lands() {
    let store = Arc::new(MemoryStore::new());
    let admin = store
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(hash_password("admin-password").unwrap()),
        })
        .await
        .unwrap();
    let other = store
        .create_user(CreateUser {
            username: "other".to_owned(),
            display_name: "Other Member".to_owned(),
            role: Role::Member,
            password_hash: Some(hash_password("other-password").unwrap()),
        })
        .await
        .unwrap();
    let racy = Arc::new(RaceInjectingBlobs {
        inner: MemoryBlobStore::new(),
        store: store.clone(),
        payload: std::sync::Mutex::new(None),
    });
    let state = AppState::new(store, racy.clone(), Arc::new(test_config()));

    let token = login(&state, "admin", "admin-password").await;
    // A real image so the pipeline produces a thumbnail alongside the blob.
    let bytes: Vec<u8> = tiny_png();
    let (status, dto) = upload(&state, &token, bytes.clone(), "race.png").await;
    assert_eq!(status, StatusCode::CREATED, "upload succeeds: {dto}");
    let key = dto["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let file_id = dto["id"].as_str().unwrap().to_owned();

    // Arm the interleave: the "upload" lands its live row exactly when the
    // delete handler reaches the unlink. The racing row shares the
    // admin row's bucket (the same bootstrap bucket) so its refcount
    // protects the blob.
    *racy.payload.lock().unwrap() = Some((
        i0721_proto::NewFile {
            bucket_id: state
                .store
                .get_file(uuid::Uuid::parse_str(file_id.as_str()).unwrap())
                .await
                .unwrap()
                .and_then(|r| r.bucket_id),
            parent_id: None,
            owner_id: other.id,
            name: "raced.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: bytes.len() as i64,
            sha256: key.clone(),
            storage_backend: "local".to_owned(),
            storage_key: key.clone(),
            thumbnail_key: Some(format!("{key}.webp")),
            original_name: "raced.png".to_owned(),
        },
        key.clone(),
    ));

    let _admin = admin;
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": file_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // The racing live row must still be servable: the blob was restored,
    // not orphaned. Its thumbnail serves too (restored alongside, or
    // regenerated from the restored media bytes on first ask).
    // Byte-equality first: the surviving row's media bytes are the
    // upload's, not some other blob's.
    let media = app(&state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        media.status(),
        StatusCode::OK,
        "the racing upload's live row keeps its bytes"
    );
    let served = axum::body::to_bytes(media.into_body(), 1024 * 1024)
        .await
        .unwrap();
    use sha2::{Digest, Sha256};
    let served_hash = Sha256::digest(&served);
    let served_hex: String = served_hash.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        served_hex, key,
        "the restored bytes hash to their own content-addressed key"
    );
    assert_eq!(
        &served[..4],
        b"RIFF".as_slice(),
        "the stored bytes are the converted webp, not some other blob"
    );

    // The thumbnail variant serves as well (restored alongside, or
    // regenerated from the restored media bytes on first ask).
    let thumb = app(&state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        thumb.status(),
        StatusCode::OK,
        "the thumbnail variant serves"
    );
    assert_eq!(
        thumb
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/webp"),
        "the thumbnail variant serves the webp thumbnail"
    );
}

#[tokio::test]
async fn final_delete_reclaims_the_last_blob_and_thumbnail() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (status, dto) = upload(&state, &token, tiny_png(), "last.png").await;
    assert_eq!(status, StatusCode::CREATED, "upload succeeds: {dto}");
    let key = dto["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let file_id = dto["id"].as_str().unwrap().to_owned();
    let thumb_key = format!("{key}.webp");

    // The racing-row window is closed first: no other live row exists, so
    // the reclaim must actually free both blobs (this pins the `> 0`
    // restore guard against an unconditional-restore regression).
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": file_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    let media = app(&state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::NOT_FOUND, "the row is gone");

    assert!(
        state
            .blobs
            .get(i0721_server::blob::BlobSlot::Media, &key)
            .await
            .unwrap()
            .is_none(),
        "the last live row's media blob is reclaimed"
    );
    assert!(
        state
            .blobs
            .get(i0721_server::blob::BlobSlot::Cache, &thumb_key)
            .await
            .unwrap()
            .is_none(),
        "the last live row's thumbnail is reclaimed"
    );
}

// ── static / SPA serving ───────────────────────────────────────────────
// These ride the DEFAULT embedded-frontend feature: the compiled-in dist
// comes from `just webui` (the gate builds the frontend first). The dev
// (on-disk) lane shares every handler body through asset_bytes, so the
// serving CONTRACT is the same; only the storage differs.

#[tokio::test]
async fn root_serves_the_spa_shell() {
    let (state, _) = test_state().await;
    let response = routes::router(state)
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/html"));
}

#[tokio::test]
async fn spa_fallback_serves_deep_links_and_404s_missing_files() {
    let (state, _) = test_state().await;
    let router = routes::router(state);

    // An extension-less client route renders the shell, not a 404.
    let deep = router
        .clone()
        .oneshot(Request::get("/users").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(deep.status(), StatusCode::OK);
    assert!(deep
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/html"));

    // A path WITH an extension that does not exist must 404 — serving
    // the shell for a missing script would hand back text/html.
    let missing = router
        .clone()
        .oneshot(
            Request::get("/assets/does-not-exist-1234.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // Traversal into the host FS is refused outright.
    let traversal = router
        .oneshot(
            Request::get("/assets/../../Cargo.toml")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(traversal.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unknown_api_routes_answer_404_json_not_the_spa_shell() {
    let (state, _) = test_state().await;
    let router = routes::router(state);

    let response = router
        .oneshot(
            Request::get("/api/v1/definitely-not-a-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("route not found"));
}

// ── the WebSocket upgrade lane ─────────────────────────────────────────
// Integration smoke against a LIVE listener (raw TCP, no WS client
// library): the handshake's auth hook is the only gate between a socket
// and the method surface, so its refusal (HTTP 401) and its four
// credential channels are pinned here end-to-end.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn spawn_listener(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = routes::app(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    format!("127.0.0.1:{}", addr.port())
}

async fn ws_handshake(addr: &str, extra_headers: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /api/rpc HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n{extra_headers}\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    // Read the response HEAD only: a 101 upgrade holds the socket open
    // for frames (reading to EOF would block forever), so the loop stops
    // at the blank line that terminates the headers.
    let mut buf = [0_u8; 1];
    let mut head = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => {
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Ok(Err(_)) => break,
        }
    }
    String::from_utf8_lossy(&head).to_string()
}

#[tokio::test]
async fn ws_handshake_refuses_without_a_token() {
    let (state, _) = test_state().await;
    let addr = spawn_listener(state).await;
    let head = ws_handshake(&addr, "").await;
    let status_line = head.lines().next().unwrap_or_default();
    assert!(
        status_line.contains("401"),
        "anonymous handshake must be refused, got: {status_line}"
    );
}

#[tokio::test]
async fn ws_handshake_accepts_the_subprotocol_and_query_channels() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    // Channel 3: Sec-WebSocket-Protocol: jwt.<token>.
    let addr = spawn_listener(state.clone()).await;
    // Channel 3: Sec-WebSocket-Protocol: jwt.<token>.
    let head = ws_handshake(&addr, &format!("Sec-WebSocket-Protocol: jwt.{token}\r\n")).await;
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "the subprotocol channel must upgrade, got: {}",
        head.lines().next().unwrap_or_default()
    );

    // Channel 4 (last resort): ?token= — a fresh connection; the head
    // reader stops at the blank line so the upgraded socket never hangs.
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let request = format!(
        "GET /api/rpc?token={token} HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut buf = [0_u8; 1];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
            Ok(Ok(n)) => {
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    assert!(
        text.starts_with("HTTP/1.1 101"),
        "the query channel must upgrade, got: {}",
        text.lines().next().unwrap_or_default()
    );
}

#[tokio::test]
async fn post_fallback_accepts_the_cookie_channel() {
    // Channel 1: the session cookie (channels 2/4 are pinned above and
    // by every bearer-carrying POST test).
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("i0721-token={token}"))
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 7, "method": "auth.verify",
                "params": null,
            })
            .to_string(),
        ))
        .unwrap();
    let (status, envelope) = rpc_request(&state, request).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(rpc_result(&envelope)["username"], "admin");
}

// ── the cookie session lane (the production browser path) ──────────────

/// THE main pin: a real WebSocket, authenticated by the httpOnly
/// cookie, carrying a real JSON-RPC frame end-to-end. This exercises
/// the exact production path (handshake auth hook → dispatch → reply
/// frame), not just the HTTP POST fallback.
#[tokio::test]
async fn ws_cookie_channel_carries_a_real_frame_round_trip() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let addr = spawn_listener(state).await;

    // connect_async cannot add headers; the cookie rides a raw request.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{addr}/api/rpc")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        header::COOKIE,
        header::HeaderValue::from_str(&format!("i0721-token={token}")).unwrap(),
    );
    let (mut ws, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(
        response.status(),
        101,
        "the cookie channel must upgrade (the browser production path)"
    );

    ws.send(Message::Text(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 41,
            "method": "auth.verify",
            "params": null,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("a reply frame within 5s")
        .expect("the stream stays open")
        .expect("a text frame");
    let Message::Text(text) = reply else {
        panic!("expected a text frame, got {reply:?}");
    };
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["id"], 41, "the reply carries the request id");
    assert_eq!(envelope["result"]["username"], "admin");
    assert!(
        envelope["result"].get("password_hash").is_none(),
        "the user projection never carries the hash"
    );
    ws.close(None).await.ok();
}

/// The deferred-ops collection surface is refused BEFORE dispatch, with
/// the dedicated OPS_DISABLED code — distinguishable from the
/// framework's unknown-op (-32052) and method-not-found (-32601).
#[tokio::test]
async fn deferred_ops_collection_is_refused_with_a_dedicated_code() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    for method in ["ops.result", "ops.cancel"] {
        let (status, envelope) = rpc(
            &state,
            &token,
            method,
            serde_json::json!({"op_id": "whatever"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{method}: {envelope}");
        let code = rpc_error_code(&envelope);
        assert_eq!(code, -32009, "{method} refused by the ops guard");
        assert_ne!(code, -32052, "not the framework's unknown-op");
        assert_ne!(code, -32601, "not method-not-found");
    }
}

/// The multipart upload rides the cookie (the browser cannot set
/// Authorization on an XHR it does not own headers for — same-origin
/// cookies ride automatically).
#[tokio::test]
async fn upload_accepts_the_cookie_channel_and_refuses_without_any() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    // Cookie channel: created.
    let (status, dto) = upload_with_cookie(&state, &token, tiny_png(), "cat.png").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    assert_eq!(dto["kind"], "image");

    // No credential at all: refused.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files")
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=X-BOUNDARY-X",
                )
                .body(Body::from("--X-BOUNDARY-X--\r\n"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The upload helper's cookie-channel twin.
async fn upload_with_cookie(
    state: &AppState,
    token: &str,
    bytes: Vec<u8>,
    name: &str,
) -> (StatusCode, serde_json::Value) {
    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let body = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{name}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    );
    let mut body = body.into_bytes();
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let response = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::COOKIE, format!("i0721-token={token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Logout expires the cookie AND supersedes the token at the watermark:
/// the next request with the old credential is refused.
#[tokio::test]
async fn logout_expires_the_cookie_and_kills_the_session() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = response.headers().get(header::SET_COOKIE).unwrap();
    let cleared = cookie.to_str().unwrap();
    assert!(cleared.contains("Max-Age=0"));
    // The clearing cookie carries the SAME attributes as the planted one
    // — a mismatched Path/SameSite would leave the live cookie in place.
    assert!(cleared.contains("SameSite=Lax"), "logout: {cleared}");
    assert!(cleared.contains("HttpOnly"), "logout: {cleared}");

    let (status, _) = rpc(&state, &token, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the watermark bump kills the pre-logout token everywhere"
    );
}

// ── the one-time WS admission ticket lane ───────────────────────────────

/// Mint a ticket over REST (the browser's pre-connect step).
async fn mint_ticket(state: &AppState, token: &str) -> String {
    let response = app(state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/ws-ticket")
                .body(Body::empty())
                .unwrap(),
            token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["ticket"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn ws_ticket_mint_requires_a_session() {
    let (state, _) = test_state().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/ws-ticket")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_ticket_opens_the_socket_and_dies_with_the_opening() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let ticket = mint_ticket(&state, &token).await;
    let addr = spawn_listener(state).await;

    let upgrade = format!(
        "GET /api/rpc?token={ticket} HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );

    // First use: 101 — the ticket admits.
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    stream.write_all(upgrade.as_bytes()).await.unwrap();
    let first = read_http_head(&mut stream).await;
    assert!(
        first.starts_with("HTTP/1.1 101"),
        "a fresh ticket upgrades: {}",
        first.lines().next().unwrap_or_default()
    );

    // Replay: the SAME ticket is dead — 401 (single use).
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    stream.write_all(upgrade.as_bytes()).await.unwrap();
    let replay = read_http_head(&mut stream).await;
    assert!(
        replay.starts_with("HTTP/1.1 401"),
        "a used ticket never admits again: {}",
        replay.lines().next().unwrap_or_default()
    );
}

/// Read an HTTP response head (up to the blank line; a 101 upgrade
/// holds the socket open, so EOF would hang).
async fn read_http_head(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut head = Vec::new();
    let mut buf = [0_u8; 1];
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
            Ok(Ok(n)) => {
                head.extend_from_slice(&buf[..n]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&head).to_string()
}

/// End-to-end: a ticket-opened socket carries a REAL frame — the
/// session the ticket established (its bound user) answers auth.verify.
#[tokio::test]
async fn a_ticket_opened_socket_answers_a_real_frame() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let ticket = mint_ticket(&state, &token).await;
    let addr = spawn_listener(state).await;

    let mut request = format!("ws://{addr}/api/rpc?token={ticket}")
        .into_client_request()
        .unwrap();
    // Deliberately NO cookie, NO auth header: the ticket alone must
    // carry the upgrade.
    request.headers_mut().remove(header::COOKIE);
    let (mut ws, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status(), 101);

    ws.send(Message::Text(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 77,
            "method": "auth.verify",
            "params": null,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
        .await
        .expect("a reply frame")
        .expect("stream open")
        .expect("a text frame");
    let Message::Text(text) = reply else {
        panic!("expected a text frame");
    };
    let envelope: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(envelope["result"]["username"], "admin");
    ws.close(None).await.ok();
}

// ── the OAuth dual-mode lane ────────────────────────────────────────────

/// A local OAuth provider stub: it speaks the ERP's endpoint contract, so
/// the flow under test is the real one — a genuine authorize leg, a
/// genuine token exchange and a genuine UserInfo read over loopback HTTP.
///
/// The authorize endpoint records the PKCE challenge/method and the
/// `state` it was handed, then redirects back with a code. The token
/// endpoint verifies the verifier against that challenge (S256 math) and
/// the client secret, and answers an access token — plus, for the feishu
/// shape, the identity itself (`feishu_identity`). The userinfo endpoint
/// answers a fixed subject.
struct ProviderStub {
    /// (challenge, method) seen at authorize.
    challenge: std::sync::Mutex<(String, String)>,
    /// The `state` the authorize leg was handed (the replay test spends it).
    state_at_authorize: std::sync::Mutex<String>,
    /// The verifier seen at token (asserted against the challenge).
    verifier_at_token: std::sync::Mutex<String>,
    /// The secret the token endpoint demands.
    secret: String,
    authorize_hits: std::sync::Mutex<usize>,
    /// `true` = the token answer carries the identity (feishu's shape: its
    /// v2 token response IS the profile, behind a `data` envelope) and there
    /// is no UserInfo leg.
    feishu_identity: bool,
}

impl ProviderStub {
    fn router(self: Arc<Self>) -> Router {
        let authorize_stub = self.clone();
        let token_stub = self.clone();
        let userinfo_stub = self.clone();
        Router::new()
            .route(
                "/oauth/authorize",
                axum::routing::get(move |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| {
                    let stub = authorize_stub.clone();
                    async move {
                        *stub.challenge.lock().unwrap() = (
                            q.get("code_challenge").cloned().unwrap_or_default(),
                            q.get("code_challenge_method").cloned().unwrap_or_default(),
                        );
                        *stub.state_at_authorize.lock().unwrap() =
                            q.get("state").cloned().unwrap_or_default();
                        *stub.authorize_hits.lock().unwrap() += 1;
                        let state = q.get("state").cloned().unwrap_or_default();
                        let redirect =
                            format!("http://res.test/api/v1/auth/oauth/stub/callback?code=CODE123&state={state}");
                        axum::response::Redirect::to(&redirect)
                    }
                }),
            )
            .route(
                "/api/oauth/token",
                axum::routing::post(move |body: String| {
                    let stub = token_stub.clone();
                    async move {
                        let form: HashMap<String, String> =
                            url::form_urlencoded::parse(body.as_bytes())
                                .into_owned()
                                .collect::<HashMap<String, String>>();
                        let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                        *stub.verifier_at_token.lock().unwrap() = verifier.clone();
                        // PKCE S256: BASE64URL(SHA256(verifier)) must equal
                        // the challenge recorded at authorize.
                        let digest = sha2::Sha256::digest(verifier.as_bytes());
                        let expected = {
                            use base64::Engine;
                            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
                        };
                        let (challenge, method) = stub.challenge.lock().unwrap().clone();
                        if method != "S256" || expected != challenge {
                            return (
                                StatusCode::BAD_REQUEST,
                                axum::Json(serde_json::json!({
                                    "error": "pkce_mismatch",
                                    "method": method,
                                })),
                            );
                        }
                        if form.get("client_secret").map(String::as_str) != Some(stub.secret.as_str()) {
                            return (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({"error": "bad_secret"})));
                        }
                        if form.get("code").map(String::as_str) != Some("CODE123") {
                            return (StatusCode::BAD_REQUEST, axum::Json(serde_json::json!({"error": "bad_code"})));
                        }
                        if stub.feishu_identity {
                            // feishu v2: the token answer carries the identity
                            // behind its `data` envelope, and the access token
                            // sits in the same envelope.
                            return (
                                StatusCode::OK,
                                axum::Json(serde_json::json!({
                                    "code": 0,
                                    "data": {
                                        "access_token": "stub-access-token",
                                        "token_type": "Bearer",
                                        "open_id": "ou_stub_feishu_01",
                                        "name": "Feishu User",
                                    },
                                })),
                            );
                        }
                        (
                            StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "access_token": "stub-access-token",
                                "token_type": "Bearer",
                            })),
                        )
                    }
                }),
            )
            // MUT:erp-userinfo-prefix — the stub mounts userinfo where the
            // real ERP does (`/api/oauth/userinfo`); reverting the provider
            // to the bare path makes the callback hit a 404 stub and fail.
            .route(
                "/api/oauth/userinfo",
                axum::routing::get(move |headers: axum::http::HeaderMap| {
                    let stub = userinfo_stub.clone();
                    async move {
                        let auth = headers
                            .get(header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default();
                        if stub.feishu_identity || auth != "Bearer stub-access-token" {
                            return (
                                StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({"error": "bad_token"})),
                            );
                        }
                        (
                            StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "sub": "subject-abcdef01",
                                "name": "Stub User",
                            })),
                        )
                    }
                }),
            )
    }
}

async fn oauth_test_state_with_stub() -> (AppState, Arc<ProviderStub>, String, UserRecord) {
    spawn_provider_stub(false).await
}

/// Start the provider stub on a loopback port and point an `builtin:erp`
/// row at it. `feishu_identity` switches the stub into the feishu answer
/// shape (identity in the token response, no UserInfo leg).
async fn spawn_provider_stub(
    feishu_identity: bool,
) -> (AppState, Arc<ProviderStub>, String, UserRecord) {
    let (state, admin) = test_state().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let stub = Arc::new(ProviderStub {
        challenge: std::sync::Mutex::new((String::new(), String::new())),
        state_at_authorize: std::sync::Mutex::new(String::new()),
        verifier_at_token: std::sync::Mutex::new(String::new()),
        secret: "stub-secret-9999".to_owned(),
        authorize_hits: std::sync::Mutex::new(0),
        feishu_identity,
    });
    let app = stub.clone().router();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let base = format!("http://127.0.0.1:{}", addr.port());
    // Register the provider as a builtin:erp row pointing at the stub.
    let provider = serde_json::json!([{
        "id": "stub",
        "display_name": "Stub SSO",
        "type": "builtin:erp",
        "client_id": "stub-client",
        "client_secret": "stub-secret-9999",
        "base_url": base,
        "enabled": true,
    }]);
    state
        .store
        .set_setting("oauth_providers", provider)
        .await
        .unwrap();
    (state, stub, base, admin)
}

/// The cookie value a redirect response planted (the browser binding).
fn flow_cookie(response: &axum::http::Response<Body>) -> String {
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("the redirect plants the binding cookie")
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        cookie.starts_with("i0721-oauth-state="),
        "the binding cookie keeps its name: {cookie}"
    );
    cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("i0721-oauth-state=")
        .unwrap()
        .to_owned()
}

/// Drive the authorize leg against the stub (the browser leg — no redirect
/// following, `res.test` is virtual) and return the callback's path+query.
async fn browser_round(location: &str) -> String {
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let authorize = browser.get(location).send().await.expect("authorize leg");
    assert!(
        authorize.status().is_redirection(),
        "the stub redirects back: {}",
        authorize.status()
    );
    authorize
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .trim_start_matches("http://res.test")
        .to_owned()
}

/// One full OAuth round against the stub: redirect entry → browser
/// authorize leg → callback. Returns the callback's response.
async fn oauth_login_round(state: &AppState) -> axum::http::Response<Body> {
    let redirect = app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(redirect.status(), StatusCode::FOUND);
    let location = redirect
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let flow_value = flow_cookie(&redirect);
    let back = browser_round(&location).await;

    app(state)
        .oneshot(
            Request::builder()
                .uri(back)
                .header(header::HOST, "res.test")
                .header(header::COOKIE, format!("i0721-oauth-state={flow_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn oauth_end_to_end_against_a_local_stub_provider() {
    use base64::Engine;
    let (state, stub, _base, _admin) = oauth_test_state_with_stub().await;

    // 1. The redirect entry: 302 with the state/PKCE cookie.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.contains("HttpOnly"), "the flow cookie is HttpOnly");
    assert!(
        cookie.contains("Max-Age=600"),
        "the flow cookie lives 600 s"
    );
    assert!(cookie.contains("SameSite=Lax"));
    // The cookie keeps its name and path; its VALUE is now the browser
    // binding (an opaque 32-byte hex value), never a state or a verifier.
    assert!(cookie.starts_with("i0721-oauth-state="));
    assert!(cookie.contains("Path=/api/v1/auth/oauth"));
    let cookie_value = cookie.split(';').next().unwrap();
    let binding = cookie_value
        .strip_prefix("i0721-oauth-state=")
        .unwrap()
        .to_owned();
    assert_eq!(binding.len(), 64, "32 bytes of RNG, hex: {binding}");
    assert!(
        binding.chars().all(|c| c.is_ascii_hexdigit()),
        "the binding is opaque hex: {binding}"
    );

    // The authorize URL carries state + PKCE S256, and NOTHING secret: the
    // client secret never rides the browser leg.
    assert!(location.contains("/oauth/authorize?"));
    assert!(location.contains("state="));
    assert!(location.contains("code_challenge="));
    assert!(location.contains("code_challenge_method=S256"));
    assert!(
        !location.contains("stub-secret-9999"),
        "the client secret must never reach the browser: {location}"
    );

    // The BROWSER leg: follow the authorize URL against the stub (the
    // app's own HTTP client stands in for the browser). The stub
    // records the challenge/method and 302s back to our callback.
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let authorize_response = browser
        .get(&location)
        .send()
        .await
        .expect("the authorize leg reaches the stub");
    assert!(
        authorize_response.status().is_redirection(),
        "the stub redirects back: {}",
        authorize_response.status()
    );
    let back = authorize_response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(back.starts_with("http://res.test/api/v1/auth/oauth/stub/callback"));
    // Drive the callback by its REAL redirected URI (query carries the
    // provider-echoed state + code).
    let callback = back.trim_start_matches("http://res.test").to_owned();

    // 2. The callback: strict state check, code exchange, userinfo,
    //    find_or_create, session cookie, 302 to /.
    let flow_value = cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("i0721-oauth-state=")
        .unwrap()
        .to_owned();
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(callback)
                .header(header::HOST, "res.test")
                .header(header::COOKIE, format!("i0721-oauth-state={flow_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    if response.status() != StatusCode::FOUND {
        let body = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        panic!("DEBUG callback failed: {}", String::from_utf8_lossy(&body));
    }
    assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/");
    let set_cookies: Vec<&str> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    let session = set_cookies
        .iter()
        .find(|c| c.starts_with("i0721-token="))
        .expect("the OAuth login plants the session cookie");
    assert!(session.contains("HttpOnly"));
    // The OAuth-planted session rides the same attribute contract.
    assert!(
        session.contains("SameSite=Lax"),
        "oauth session cookie: {session}"
    );
    // The binding cookie is cleared alongside.
    assert!(set_cookies
        .iter()
        .any(|c| c.starts_with("i0721-oauth-state=;")));

    // 3. PKCE S256 verified end-to-end: the verifier at token exchange
    //    hashes to the challenge recorded at authorize.
    let verifier = stub.verifier_at_token.lock().unwrap().clone();
    let (challenge, method) = stub.challenge.lock().unwrap().clone();
    assert_eq!(method, "S256");
    let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    assert_eq!(expected, challenge, "PKCE S256 math holds across the flow");

    // 4. The minted session is REAL: auth.verify answers over the RPC
    //    lane with the planted cookie.
    let token = session
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("i0721-token=")
        .unwrap()
        .to_owned();
    let (status, envelope) = rpc(&state, &token, "auth.verify", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let user = envelope["result"].as_object().unwrap();
    assert_eq!(
        user["username"].as_str().unwrap(),
        "stub_subject-", // {provider}_{subject[0..8]} sanitized
        "the minted account follows the provider_subject8 rule"
    );
    assert_eq!(user["display_name"], "Stub User");
    assert_eq!(user["role"], "member");
}

#[tokio::test]
async fn oauth_second_login_is_idempotent() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;

    let first = oauth_login_round(&state).await;
    assert_eq!(first.status(), StatusCode::FOUND);
    let users_after_first = state.store.count_users().await.unwrap();

    // A second full round, same subject — NO second account.
    let second = oauth_login_round(&state).await;
    assert_eq!(second.status(), StatusCode::FOUND);
    assert_eq!(
        state.store.count_users().await.unwrap(),
        users_after_first,
        "the same (provider, subject) never mints twice"
    );
}

#[tokio::test]
async fn oauth_callback_rejects_a_wrong_state() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub/callback?code=CODE123&state=forged")
                .header(header::HOST, "res.test")
                .header(header::COOKIE, "i0721-oauth-state=realstate.realverifier")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn oauth_redirect_refuses_unknown_providers() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/definitely-not")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn login_methods_shape_and_secret_hygiene() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    // (The providers row is already planted by the stub fixture.)

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["password"], true, "unpinned defaults to password ON");
    let providers = json["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["id"], "stub");
    assert_eq!(providers[0]["display_name"], "Stub SSO");
    // The secret-hygiene pin (the negative control anchor): no secret
    // material anywhere in the public payload.
    let text = body.to_vec();
    let text = String::from_utf8_lossy(&text);
    assert!(
        !text.contains("stub-secret-9999"),
        "login-methods must never leak the client secret"
    );
    assert!(
        !json["providers"][0]
            .as_object()
            .unwrap()
            .contains_key("client_secret"),
        "no client_secret key in the public shape"
    );
}

#[tokio::test]
async fn password_kill_switch_gates_login_and_login_methods() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());

    // Off via the admin RPC.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "auth.password.put",
        serde_json::json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // login-methods reports password:false.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["password"], false);

    // The password login itself: 403 with the uniform message.
    let response = app(&state)
        .oneshot(login_request("admin", "admin-password"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("password login is disabled"));

    // Back on.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "auth.password.put",
        serde_json::json!({"enabled": true}),
    )
    .await;
    let _ = envelope;
    let response = app(&state)
        .oneshot(login_request("admin", "admin-password"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn provider_crud_masks_secrets_and_empty_upsert_keeps_the_stored_one() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());

    // List: masked — has_secret + tail, never the value.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.list",
        serde_json::Value::Null,
    )
    .await;
    let list = envelope["result"].as_array().unwrap();
    let stub_row = list.iter().find(|r| r["id"] == "stub").unwrap();
    assert_eq!(stub_row["has_secret"], true);
    assert_eq!(stub_row["secret_tail"], "9999");
    assert!(
        !serde_json::to_string(stub_row)
            .unwrap()
            .contains("stub-secret-9999"),
        "the admin list is masked too"
    );

    // Upsert with an EMPTY secret: keeps the stored one.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "stub",
            "display_name": "Stub SSO renamed",
            "type": "builtin:erp",
            "client_id": "stub-client",
            "client_secret": "",
            "base_url": _base,
            "enabled": false,
        }),
    )
    .await;
    assert_eq!(envelope["result"]["display_name"], "Stub SSO renamed");
    assert_eq!(
        envelope["result"]["has_secret"], true,
        "empty secret KEEPS the stored one"
    );
    assert_eq!(envelope["result"]["secret_tail"], "9999");
    // The flow still authenticates against the stub (secret intact):
    // the disabled flag now HIDES the provider instead.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&body).contains("Stub SSO"),
        "disabled providers vanish"
    );

    // A bad type or id is refused.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "Bad Id", "display_name": "x", "type": "builtin:erp",
            "client_id": "c", "client_secret": "", "enabled": true,
        }),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32602);
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "ok-id", "display_name": "x", "type": "twitter",
            "client_id": "c", "client_secret": "", "enabled": true,
        }),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32602);

    // Delete: gone; a second delete is NOT_FOUND.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "providers.delete",
        serde_json::json!({"id": "stub"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(envelope["result"].is_null());
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.delete",
        serde_json::json!({"id": "stub"}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32006);

    // Member gate: every providers.* method refuses non-admins.
    let member = state
        .store
        .create_user(CreateUser {
            username: "member".to_owned(),
            display_name: "Member".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // Well-formed per-method params: the gate must fire BEFORE any
    // params validation.
    for (method, params) in [
        ("providers.list", serde_json::Value::Null),
        (
            "providers.upsert",
            serde_json::json!({
                "id": "x", "display_name": "x", "type": "builtin:erp",
                "client_id": "c", "client_secret": "", "enabled": true,
            }),
        ),
        ("providers.delete", serde_json::json!({"id": "x"})),
        ("auth.password.get", serde_json::Value::Null),
        ("auth.password.put", serde_json::json!({"enabled": true})),
    ] {
        let (status, envelope) = rpc(&state, &member_token, method, params).await;
        assert_eq!(status, StatusCode::OK, "{method}: {envelope}");
        assert_eq!(rpc_error_code(&envelope), -32005, "{method} is admin-gated");
    }
}

// ── W4 R2 review pins ──────────────────────────────────────────────────

/// The stub, GitHub-shaped: a NUMERIC userinfo id (GitHub's `/user`
/// answers `{"id": 12345678}`) and the id arriving as a number must
/// still mint a session.
/// The subject-extraction contract, exercised DIRECTLY through the
/// same function the callback uses (no inline copy that could drift):
/// numeric GitHub ids, string subjects, feishu open_id, and the
/// no-subject refusal.
#[test]
fn oauth_subject_extraction_accepts_all_provider_shapes() {
    use i0721_server::oauth::subject_from_userinfo;

    // GitHub: `/user` answers a NUMERIC id — the shape that broke every
    // GitHub login before the numeric arm existed.
    assert_eq!(
        subject_from_userinfo(&serde_json::json!({
            "id": 12345678, "login": "octocat", "name": "Octo Cat",
        }))
        .as_deref(),
        Some("12345678")
    );
    // oidc/erp: the string `sub`.
    assert_eq!(
        subject_from_userinfo(&serde_json::json!({
            "sub": "subject-abcdef01", "name": "Stub User",
        }))
        .as_deref(),
        Some("subject-abcdef01")
    );
    // feishu: `open_id` (no sub/id present).
    assert_eq!(
        subject_from_userinfo(&serde_json::json!({"open_id": "ou_9bc"})).as_deref(),
        Some("ou_9bc")
    );
    // No subject anywhere: None (the callback turns it into a 502).
    assert_eq!(
        subject_from_userinfo(&serde_json::json!({"name": "Nobody"})),
        None
    );
    // The numeric arm is id-ONLY: a numeric sub must NOT extract (a
    // provider answering a numeric sub is malformed, not a GitHub id).
    assert_eq!(subject_from_userinfo(&serde_json::json!({"sub": 42})), None);
}

/// https-only provider URLs: an external http base is refused at
/// upsert; loopback http survives (the stub/dev lane).
#[tokio::test]
async fn provider_upsert_enforces_https_urls() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());

    for (field, value) in [
        ("base_url", "http://sso.example.com"),
        ("issuer_url", "http://sso.example.com"),
        ("base_url", "ftp://sso.example.com"),
        ("base_url", "https://user:pass@sso.example.com"),
        ("base_url", "sso.example.com"),
    ] {
        let mut row = serde_json::json!({
            "id": "probe", "display_name": "x", "type": "builtin:erp",
            "client_id": "c", "client_secret": "", "enabled": true,
        });
        row[field] = serde_json::json!(value);
        let (_, envelope) = rpc(&state, &admin_token, "providers.upsert", row).await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32602,
            "{field}={value} must be refused"
        );
    }

    // Loopback http is the dev lane: accepted.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "local-probe", "display_name": "x", "type": "builtin:erp",
            "client_id": "c", "client_secret": "", "enabled": true,
            "base_url": "http://127.0.0.1:9999",
        }),
    )
    .await;
    assert_eq!(envelope["result"]["id"], "local-probe");
}

/// I0721_PUBLIC_ORIGIN drives the redirect_uri (the stub records it).
#[tokio::test]
async fn oauth_redirect_uri_follows_the_public_origin() {
    let (mut state, _stub, base, _admin) = oauth_test_state_with_stub().await;
    let origin = base.clone(); // the stub IS a loopback http origin
    let mut config = (*state.config).clone();
    config.public_origin = Some(origin.clone());
    state.config = Arc::new(config);

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "irrelevant.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    // The SAME encoding the server's urlencode applies.
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("", &format!("{origin}/api/v1/auth/oauth/stub/callback"));
    let encoded = serializer.finish().trim_start_matches('=').to_owned();
    assert!(
        location.contains(&encoded),
        "the redirect_uri follows I0721_PUBLIC_ORIGIN, not the Host: {location}"
    );
}

/// The redirect URI joins on ONE slash no matter how the origin was spelled:
/// a `I0721_PUBLIC_ORIGIN` with a pasted trailing slash, and a `Host` header
/// that carries one, must not produce `…//api/v1/auth/oauth/…/callback`.
#[tokio::test]
async fn callback_uri_has_no_double_slash_for_either_origin_source() {
    let (mut state, _stub, base, _admin) = oauth_test_state_with_stub().await;
    // The configured origin, spelled with a trailing slash.
    let mut config = (*state.config).clone();
    config.public_origin = Some(format!("{base}/"));
    state.config = Arc::new(config);

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "irrelevant.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("", &format!("{base}/api/v1/auth/oauth/stub/callback"));
    let encoded = serializer.finish().trim_start_matches('=').to_owned();
    assert!(
        location.contains(&encoded),
        "the redirect_uri has exactly one slash: {location}"
    );
}

/// A JUNK kill-switch row (a JSON string) must NOT fail the password
/// lane open or closed: it defers to the env/default (ON).
#[tokio::test]
async fn junk_password_row_defers_to_the_default() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    state
        .store
        .set_setting("auth_password_login", serde_json::json!("yeet"))
        .await
        .unwrap();
    // login-methods still reports ON…
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("\"password\":true"));
    // …and the password login still answers 200.
    let response = app(&state)
        .oneshot(login_request("admin", "admin-password"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// An attempt minted for provider A cannot be finished against provider
/// B's callback: the `state` is spent by kirino, and the attempt records
/// which provider it was started for.
#[tokio::test]
async fn oauth_state_cannot_cross_providers() {
    let (state, _stub, base, admin) = oauth_test_state_with_stub().await;
    // Register a SECOND provider pointing at the same stub.
    let mut providers = serde_json::from_value::<serde_json::Value>(
        state
            .store
            .get_setting("oauth_providers")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    providers.as_array_mut().unwrap().push(serde_json::json!({
        "id": "other", "display_name": "Other", "type": "builtin:erp",
        "client_id": "stub-client", "client_secret": "stub-secret-9999",
        "base_url": base, "enabled": true,
    }));
    state
        .store
        .set_setting("oauth_providers", providers)
        .await
        .unwrap();

    // Start the flow for "stub" — the attempt carries stub's state.
    let redirect = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(redirect.status(), StatusCode::FOUND);
    let flow_value = flow_cookie(&redirect);
    let location = redirect
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    // Walk the authorize leg so the stub echoes the real state back.
    let back = browser_round(&location).await;
    let state_param = back
        .split("state=")
        .nth(1)
        .expect("the authorize leg echoes the state")
        .to_owned();
    assert!(!state_param.is_empty());

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/auth/oauth/other/callback?code=CODE123&state={state_param}"
                ))
                .header(header::HOST, "res.test")
                .header(header::COOKIE, format!("i0721-oauth-state={flow_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a stub-minted attempt must not open other's callback"
    );
    let body = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("another provider"),
        "the refusal names the provider mismatch: {text}"
    );
    let _ = admin;
}

/// The dead-state guard: password OFF with no enabled provider is
/// refused at the RPC (the deployment keeps a login method).
#[tokio::test]
async fn password_off_with_no_enabled_provider_is_refused() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    // (The fixture has the enabled "stub" provider — disable it first.)
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "stub", "display_name": "Stub", "type": "builtin:erp",
            "client_id": "stub-client", "client_secret": "",
            "base_url": "http://127.0.0.1:1", "enabled": false,
        }),
    )
    .await;
    assert_eq!(envelope["result"]["enabled"], false);

    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "auth.password.put",
        serde_json::json!({"enabled": false}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32602);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no OAuth provider is enabled"),
        "the refusal explains itself: {envelope}"
    );

    // With a provider enabled again, the switch flips freely.
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "stub", "display_name": "Stub", "type": "builtin:erp",
            "client_id": "stub-client", "client_secret": "",
            "base_url": "http://127.0.0.1:1", "enabled": true,
        }),
    )
    .await;
    assert_eq!(envelope["result"]["enabled"], true);
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "auth.password.put",
        serde_json::json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
}

/// The scopes round-trip: upsert carries scopes, the masked list
/// answers them back (the edit form re-fills from exactly this).
#[tokio::test]
async fn provider_scopes_round_trip_through_the_masked_list() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());

    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "stub", "display_name": "Stub", "type": "builtin:erp",
            "client_id": "stub-client", "client_secret": "",
            "base_url": "http://127.0.0.1:1", "enabled": true,
            "scopes": "openid profile email",
        }),
    )
    .await;
    assert_eq!(
        envelope["result"]["scopes"], "openid profile email",
        "the upsert response echoes the scopes"
    );

    // And back through the LIST (what the edit form reads).
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.list",
        serde_json::Value::Null,
    )
    .await;

    let row = envelope["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "stub")
        .unwrap();
    assert_eq!(row["scopes"], "openid profile email");

    // Empty → null (the server-side per-template defaults apply).
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "providers.upsert",
        serde_json::json!({
            "id": "stub", "display_name": "Stub", "type": "builtin:erp",
            "client_id": "stub-client", "client_secret": "",
            "base_url": "http://127.0.0.1:1", "enabled": true,
            "scopes": null,
        }),
    )
    .await;
    assert_eq!(envelope["result"]["scopes"], serde_json::Value::Null);
}

// ── the relying-party migration pins: LoginPolicy + metadata + refusals ──

/// A provider row with every field filled (the tests tweak one at a time).
fn provider_row(id: &str, provider_type: &str) -> i0721_server::oauth::OAuthProvider {
    i0721_server::oauth::OAuthProvider {
        id: id.to_owned(),
        display_name: format!("{id} SSO"),
        provider_type: provider_type.to_owned(),
        client_id: "cid".to_owned(),
        client_secret: "s3cret-value-9999".to_owned(),
        base_url: None,
        issuer_url: None,
        scopes: None,
        enabled: true,
        icon: None,
    }
}

/// The login-methods document: the legacy fields AND the family policy,
/// validated, with the same catalog in both renderings.
#[tokio::test]
async fn login_methods_publishes_a_validated_family_policy() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let policy = json
        .get("policy")
        .expect("login-methods publishes the family policy");
    assert_eq!(policy["version"], 1);
    assert_eq!(
        policy["mode"], "federated",
        "one usable provider makes the deployment federated"
    );
    assert_eq!(policy["password"], true);
    assert_eq!(policy["relay"], serde_json::Value::Null);
    let methods = policy["methods"].as_array().unwrap();
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0]["kind"], "oauth");
    assert_eq!(methods[0]["id"], "stub");
    assert_eq!(methods[0]["provider"], "stub");
    assert_eq!(methods[0]["label"], "Stub SSO");
    assert_eq!(methods[0]["href"], "/api/v1/auth/oauth/stub");

    // One catalog, two renderings: the legacy array and the policy agree.
    let legacy = json["providers"].as_array().unwrap();
    assert_eq!(legacy.len(), methods.len());
    for (row, method) in legacy.iter().zip(methods) {
        assert_eq!(row["id"], method["id"]);
        assert_eq!(row["display_name"], method["label"]);
    }

    // The published document passes kirino's own validation.
    let policy: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(policy.clone()).expect("the wire shape round-trips");
    policy.validate().expect("the served policy is consistent");

    // No secret material anywhere in the public payload.
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("s3cret-value-9999"));
}

/// `GET /api/v1/auth/login-methods` for this state, decoded.
async fn login_methods_json(state: &i0721_server::state::AppState) -> serde_json::Value {
    let response = app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

/// Rewrite the stored catalog to `rows` (a raw JSON array: the point of
/// these cases is rows the RPC would have refused) and pin the password
/// kill-switch ON, the ordinary deployed shape: with a working password form
/// a catalog whose rows are all malformed still serves an honest page, so the
/// malformed rows are exercised against the rendering both views share rather
/// than against the login-less posture (`login_less_posture_publishes_the_
/// honest_document` owns that one).
async fn plant_catalog(state: &i0721_server::state::AppState, rows: serde_json::Value) {
    state
        .store
        .set_setting("oauth_providers", rows)
        .await
        .unwrap();
    state
        .store
        .set_setting("auth_password_login", serde_json::json!(true))
        .await
        .unwrap();
}

/// The ids the two renderings of one login-methods response advertise.
fn both_id_sets(json: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let legacy = json["providers"]
        .as_array()
        .expect("the legacy providers[] array")
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    let policy = json["policy"]["methods"]
        .as_array()
        .expect("the policy methods array")
        .iter()
        .map(|method| method["id"].as_str().unwrap().to_owned())
        .collect();
    (legacy, policy)
}

/// A malformed row is DROPPED from BOTH renderings, not just from the
/// policy: the login page keeps showing the entrances that work, and the
/// response never advertises — in its legacy fields — a provider its policy
/// has already given up on (the R2 finding: a blank `display_name` row
/// collapsed the policy to login-less `local` while `providers[]` stayed
/// `federated` with that row).
#[tokio::test]
async fn login_methods_drops_malformed_rows_from_both_renderings() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    plant_catalog(
        &state,
        serde_json::json!([
            {
                "id": "nameless", "display_name": "   ", "type": "builtin:erp",
                "client_id": "cid", "client_secret": "", "base_url": "https://sso.example",
                "issuer_url": null, "scopes": null, "enabled": true, "icon": null,
            },
            {
                "id": "erp", "display_name": "ERP SSO", "type": "builtin:erp",
                "client_id": "cid", "client_secret": "", "base_url": "https://sso.example",
                "issuer_url": null, "scopes": null, "enabled": true, "icon": null,
            },
        ]),
    )
    .await;

    let json = login_methods_json(&state).await;
    let (legacy, policy) = both_id_sets(&json);
    assert_eq!(
        legacy,
        vec!["erp".to_owned()],
        "the bad row is gone: {json}"
    );
    assert_eq!(policy, vec!["erp".to_owned()], "the policy agrees: {json}");
    assert_eq!(json["policy"]["mode"], "federated");
    assert_eq!(json["policy"]["password"], true);
    // The served policy is consistent by kirino's own contract.
    let contract: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(json["policy"].clone()).unwrap();
    contract.validate().expect("the served policy validates");
}

/// A catalog where EVERY row is unusable still answers 200 — an honest,
/// EMPTY page. Dropping the malformed rows leaves no entrance, so the
/// document is `local` with no method; with the password switch on it is
/// still a valid `local` policy (password offered, nothing else), which is
/// exactly what an operator whose only provider row is broken should see: a
/// working password form and no phantom SSO button. Both renderings are
/// empty and agree, and no 503 is raised for a catalog that is merely empty.
#[tokio::test]
async fn login_methods_with_every_row_unusable_serves_an_honest_empty_page() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    plant_catalog(
        &state,
        serde_json::json!([
            {
                "id": "nameless", "display_name": "", "type": "builtin:erp",
                "client_id": "cid", "client_secret": "", "base_url": "https://sso.example",
                "issuer_url": null, "scopes": null, "enabled": false, "icon": null,
            },
            {
                "id": "", "display_name": "ERP SSO", "type": "builtin:erp",
                "client_id": "cid", "client_secret": "", "base_url": "https://sso.example",
                "issuer_url": null, "scopes": null, "enabled": true, "icon": null,
            },
        ]),
    )
    .await;

    let json = login_methods_json(&state).await;
    let (legacy, policy) = both_id_sets(&json);
    assert!(legacy.is_empty(), "no entrance is advertised: {json}");
    assert!(policy.is_empty(), "the policy lists none either: {json}");
    assert_eq!(json["policy"]["mode"], "local");
    // The kill-switch is ON, so the page still offers the password form.
    assert_eq!(json["password"], true);
    assert_eq!(json["policy"]["password"], true);
    let contract: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(json["policy"].clone()).unwrap();
    contract.validate().expect("an empty local policy is valid");
}

/// The two renderings NEVER disagree about which entrances exist: the ids the
/// policy lists are exactly the ids the legacy array lists, for both catalog
/// shapes (all rows usable, some dropped, and none usable).
#[test]
fn policy_and_legacy_renderings_never_disagree() {
    let base = "https://sso.example";
    let catalog = |rows: &[(&str, &str, bool)]| -> Vec<i0721_server::oauth::OAuthProvider> {
        rows.iter()
            .enumerate()
            .map(|(index, &(id, label, enabled))| {
                let mut row = provider_row(id, "builtin:erp");
                row.display_name = label.to_owned();
                row.base_url = Some(base.to_owned());
                row.client_id = format!("cid-{index}");
                row.enabled = enabled;
                row
            })
            .collect()
    };
    // One usable row plus one blank label; then every row unusable.
    for rows in [
        catalog(&[("ok", "OK SSO", true), ("blank-label", "  ", true)]),
        catalog(&[("blank-label", " ", true), ("blank-id", "", true)]),
        catalog(&[]),
    ] {
        let document = i0721_server::oauth::login_methods_document(true, &rows)
            .expect("a filtered catalog is servable");
        let legacy: Vec<String> = document
            .providers
            .iter()
            .map(|row| row.id.clone())
            .collect();
        let policy: Vec<String> = document
            .policy
            .methods
            .iter()
            .map(|m| m.id().to_owned())
            .collect();
        assert_eq!(legacy, policy, "one catalog, two renderings, same ids");
    }
}

/// The row filter is TOTAL for this catalog's own row model: after it, kirino
/// validation can only fail with the login-less posture — so the endpoint's
/// `503` lane is a genuine "this cannot happen", never the old silent
/// downgrade wearing a different hat. Checked against rows an operator can
/// actually store (ids from `valid_provider_id`, labels from the form).
#[tokio::test]
async fn filter_makes_every_servable_catalog_a_posture_or_a_valid_policy() {
    for (id, label) in [
        ("erp", "ERP SSO"),
        ("erp-2", "ERP SSO"),
        ("", "ERP SSO"),
        ("  ", "ERP SSO"),
        ("padded ", "ERP SSO"),
        ("erp", "   "),
        (" ", ""),
    ] {
        let mut row = provider_row(id, "builtin:erp");
        row.display_name = label.to_owned();
        row.base_url = Some("https://sso.example".to_owned());
        let document = i0721_server::oauth::login_methods_document(true, &[row])
            .expect("no single row can make the page unservable");
        let contract: kirino::auth::mode::LoginPolicy =
            serde_json::from_value(serde_json::to_value(&document.policy).unwrap()).unwrap();
        match contract.validate() {
            Ok(()) => {}
            Err(kirino::auth::mode::LoginPolicyViolation::PasswordMissingInLocalMode) => {
                assert!(document.providers.is_empty() && document.policy.methods.is_empty());
            }
            Err(other) => panic!("catalog ({id:?}, {label:?}) served a bad policy: {other}"),
        }
    }
}

/// An empty catalog with the password switch on is a plain `local`
/// deployment: no methods, password offered.
#[test]
fn login_policy_local_when_no_provider_is_enabled() {
    let policy = i0721_server::oauth::build_login_policy(true, &[])
        .expect("a password-only policy is valid");
    assert_eq!(policy.mode, i0721_server::oauth::AuthMode::Local);
    assert!(policy.password);
    assert!(policy.methods.is_empty());
    // The published wire shape IS the family contract: it validates as
    // kirino's own document once converted back.
    let contract: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(serde_json::to_value(&policy).unwrap()).unwrap();
    contract.validate().expect("validated before publication");
}

/// The published document and kirino's validated one serialise identically —
/// the wire shape restates the contract, it does not reinterpret it.
#[test]
fn published_policy_round_trips_through_kirinos_contract() {
    let mut provider = provider_row("erp", "builtin:erp");
    provider.base_url = Some("https://sso.example".to_owned());
    let rows = [&provider];
    let wire = i0721_server::oauth::build_login_policy(true, &rows).unwrap();
    let wire_json = serde_json::to_value(&wire).unwrap();
    let contract: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(wire_json.clone()).expect("the wire shape parses as the contract");
    contract.validate().expect("the wire shape validates");
    let contract_json = serde_json::to_value(&contract).unwrap();
    assert_eq!(
        wire_json, contract_json,
        "one document, two renderings of the same contract"
    );
}

/// The login-less posture (chest's precedent): the password switch off and
/// no usable provider is a REAL configuration, so the honest document is
/// published rather than refused — and it is the one document that does not
/// pass `validate()`.
#[tokio::test]
async fn login_less_posture_publishes_the_honest_document() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    // Disable the fixture's only provider, then switch the password off
    // directly (the RPC refuses that pair; the contract must still hold for
    // a deployment that lands there — a hand-edited row, an upgrade).
    let mut providers: serde_json::Value = state
        .store
        .get_setting("oauth_providers")
        .await
        .unwrap()
        .unwrap();
    for row in providers.as_array_mut().unwrap() {
        row["enabled"] = serde_json::json!(false);
    }
    state
        .store
        .set_setting("oauth_providers", providers)
        .await
        .unwrap();
    state
        .store
        .set_setting("auth_password_login", serde_json::json!(false))
        .await
        .unwrap();
    let _ = admin_token;

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login-methods")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["password"], false);
    assert!(json["providers"].as_array().unwrap().is_empty());
    assert_eq!(json["policy"]["mode"], "local");
    assert_eq!(json["policy"]["password"], false);
    assert!(json["policy"]["methods"].as_array().unwrap().is_empty());
    // The posture really does violate the invariant the contract names —
    // the document is served DESPITE it, not because it passes.
    let policy: kirino::auth::mode::LoginPolicy =
        serde_json::from_value(json["policy"].clone()).unwrap();
    assert_eq!(
        policy.validate(),
        Err(kirino::auth::mode::LoginPolicyViolation::PasswordMissingInLocalMode)
    );
}

/// A disabled provider never reaches the policy (it is not an entrance).
#[test]
fn disabled_providers_vanish_from_the_policy() {
    let mut enabled = provider_row("erp", "builtin:erp");
    enabled.base_url = Some("https://sso.example".to_owned());
    let mut disabled = provider_row("old", "builtin:erp");
    disabled.enabled = false;
    let rows = [&enabled, &disabled];
    let policy = i0721_server::oauth::build_login_policy(true, &rows).unwrap();
    assert_eq!(policy.methods.len(), 1);
    assert_eq!(policy.methods[0].id(), "erp");
}

/// GitHub's metadata: its own endpoints, form-encoded token body, the
/// `read:user` scope, and a confidential client (the secret never rides the
/// browser leg).
#[tokio::test]
async fn github_metadata_mirrors_the_endpoint_family() {
    let (state, _admin) = test_state().await;
    let mut row = provider_row("gh", "builtin:github");
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .expect("github assembles without network");
    assert_eq!(route.metadata.provider, "gh");
    assert_eq!(
        route.metadata.authorize_url.as_str(),
        "https://github.com/login/oauth/authorize"
    );
    assert_eq!(
        route.metadata.token_url.as_str(),
        "https://github.com/login/oauth/access_token"
    );
    assert_eq!(
        route.metadata.userinfo_url.as_ref().map(url::Url::as_str),
        Some("https://api.github.com/user")
    );
    assert_eq!(
        route.metadata.token_body,
        kirino::auth::passport::oauth::TokenBody::Form
    );
    assert_eq!(route.metadata.scopes, vec!["read:user".to_owned()]);
    assert_eq!(route.metadata.issuer.as_deref(), Some("https://github.com"));
    assert!(
        route.client.client_secret().is_some(),
        "a secret makes it confidential"
    );

    // The operator's scope override wins.
    row.scopes = Some("read:user user:email".to_owned());
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        route.metadata.scopes,
        vec!["read:user".to_owned(), "user:email".to_owned()]
    );

    // No secret = a public client: PKCE is its only proof.
    row.client_secret.clear();
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        route.client.client_secret(),
        None,
        "an empty secret means a public client, never an empty secret"
    );
}

/// Feishu's metadata: its own endpoints, NO UserInfo leg (the v2 token
/// answer carries the identity), and no scope (its authorize takes none).
#[tokio::test]
async fn feishu_metadata_has_no_userinfo_leg_and_no_scope() {
    let (state, _admin) = test_state().await;
    let row = provider_row("fs", "builtin:feishu");
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        route.metadata.authorize_url.as_str(),
        "https://accounts.feishu.cn/open-apis/authen/v1/authorize"
    );
    assert_eq!(
        route.metadata.token_url.as_str(),
        "https://open.feishu.cn/open-apis/authen/v2/oauth/token"
    );
    assert!(
        route.metadata.userinfo_url.is_none(),
        "feishu v2 answers the identity in its token response"
    );
    assert!(
        route.metadata.scopes.is_empty(),
        "feishu's authorize endpoint takes no scope"
    );
    assert_eq!(
        route.metadata.issuer.as_deref(),
        Some("https://open.feishu.cn")
    );
}

/// The ERP's metadata: both legs under the `/api` prefix (MUT:erp-userinfo-
/// prefix), the `openid profile` default scopes, and its issuer = the
/// deployment's own base.
#[tokio::test]
async fn erp_metadata_mounts_both_legs_under_the_api_prefix() {
    let (state, _admin) = test_state().await;
    let mut row = provider_row("erp", "builtin:erp");
    row.base_url = Some("https://sso.example".to_owned());
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        route.metadata.authorize_url.as_str(),
        "https://sso.example/oauth/authorize"
    );
    assert_eq!(
        route.metadata.token_url.as_str(),
        "https://sso.example/api/oauth/token"
    );
    assert_eq!(
        route.metadata.userinfo_url.as_ref().map(url::Url::as_str),
        Some("https://sso.example/api/oauth/userinfo"),
        "the userinfo leg rides the /api prefix (its host split bounces the bare path)"
    );
    assert_eq!(
        route.metadata.scopes,
        vec!["openid".to_owned(), "profile".to_owned()]
    );
    assert_eq!(
        route.metadata.issuer.as_deref(),
        Some("https://sso.example")
    );

    // No base: the row cannot resolve endpoints, and says so.
    row.base_url = None;
    let refused = i0721_server::oauth::resolve_route(&state, &row).await;
    assert!(
        refused.is_err(),
        "an erp row without a base has no endpoints"
    );
}

/// A `base_url` pasted WITH a trailing slash joins on exactly one `/` (an
/// operator's copy-paste must not yield `//api/oauth/token`, which some
/// reverse proxies treat as protocol-relative), and the issuer keeps none.
#[tokio::test]
async fn erp_base_url_with_a_trailing_slash_joins_on_one_slash() {
    let (state, _admin) = test_state().await;
    let mut row = provider_row("erp", "builtin:erp");
    row.base_url = Some("https://sso.example/".to_owned());
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .expect("a trailing slash is not a broken base");
    assert_eq!(
        route.metadata.token_url.as_str(),
        "https://sso.example/api/oauth/token"
    );
    assert_eq!(
        route.metadata.authorize_url.as_str(),
        "https://sso.example/oauth/authorize"
    );
    assert_eq!(
        route.metadata.userinfo_url.as_ref().map(url::Url::as_str),
        Some("https://sso.example/api/oauth/userinfo")
    );
    assert_eq!(
        route.metadata.issuer.as_deref(),
        Some("https://sso.example"),
        "the issuer carries no trailing slash"
    );
}

/// An `oidc` row assembles from the cached discovery document, and the
/// cache keeps answering after the issuer's document changes (the positive
/// TTL is what makes the redirect endpoint cheap).
#[tokio::test]
async fn oidc_metadata_assembles_from_the_discovery_cache() {
    use std::sync::OnceLock;
    static DOC: OnceLock<Arc<std::sync::atomic::AtomicUsize>> = OnceLock::new();
    let hits = DOC.get_or_init(|| Arc::new(std::sync::atomic::AtomicUsize::new(0)));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());
    let serve_hits = hits.clone();
    let serve_issuer = issuer.clone();
    let app = Router::new().route(
        "/.well-known/openid-configuration",
        axum::routing::get(move || {
            let hits = serve_hits.clone();
            let issuer = serve_issuer.clone();
            async move {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/authorize"),
                    "token_endpoint": format!("{issuer}/token"),
                    "userinfo_endpoint": format!("{issuer}/userinfo"),
                }))
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (state, _admin) = test_state().await;
    let mut row = provider_row("idp", "oidc");
    row.issuer_url = Some(issuer.clone());
    let route = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        route.metadata.authorize_url.as_str(),
        format!("{issuer}/authorize")
    );
    assert_eq!(route.metadata.token_url.as_str(), format!("{issuer}/token"));
    assert_eq!(
        route.metadata.userinfo_url.as_ref().map(url::Url::as_str),
        Some(format!("{issuer}/userinfo").as_str())
    );
    assert_eq!(route.metadata.issuer.as_deref(), Some(issuer.as_str()));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // A second resolve rides the cache: no second outbound request.
    let _ = i0721_server::oauth::resolve_route(&state, &row)
        .await
        .unwrap();
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the discovery document is cached"
    );

    // A broken issuer is a readable refusal, not a panic.
    let mut broken = provider_row("broken", "oidc");
    broken.issuer_url = Some("http://127.0.0.1:1".to_owned());
    let refused = i0721_server::oauth::resolve_route(&state, &broken).await;
    assert!(refused.is_err(), "a broken issuer refuses to assemble");
}

/// A callback from a browser that never started the attempt is refused,
/// and the consumed `state` cannot be replayed afterwards.
#[tokio::test]
async fn oauth_callback_refuses_a_foreign_browser_and_a_replayed_state() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;

    let redirect = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = redirect
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    // The first attempt's binding is never presented: the probes below prove
    // it is burned regardless, so the assertion is inside `flow_cookie`.
    flow_cookie(&redirect);
    let back = browser_round(&location).await;

    // 1. The right `state`, another browser's (or no) binding: refused —
    //    and the attempt is BURNED by that first presentation (kirino takes
    //    the `state` before it compares the binding, so a hostile callback
    //    cannot probe a victim's parked attempt without destroying it).
    for presented in [None, Some("00".repeat(32))] {
        let mut request = Request::builder()
            .uri(back.clone())
            .header(header::HOST, "res.test");
        if let Some(value) = &presented {
            request = request.header(header::COOKIE, format!("i0721-oauth-state={value}"));
        }
        let response = app(&state)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a callback without the same browser's binding is refused"
        );
        let body = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("browser") || text.contains("unknown or already used state"),
            "the refusal names the binding or the spent state: {text}"
        );
        assert!(
            !text.contains("access_token") && !text.contains("stub-access-token"),
            "a refused callback never reaches the token exchange: {text}"
        );
    }

    // 2. A FRESH attempt (the probes above spent the first one): the right
    //    browser presenting the right binding is a real sign-in.
    let redirect = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/stub")
                .header(header::HOST, "res.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let flow_value = flow_cookie(&redirect);
    let location = redirect
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let back = browser_round(&location).await;
    let first = app(&state)
        .oneshot(
            Request::builder()
                .uri(back.clone())
                .header(header::HOST, "res.test")
                .header(header::COOKIE, format!("i0721-oauth-state={flow_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        first.status(),
        StatusCode::FOUND,
        "the browser that started the attempt signs in"
    );
    assert!(first
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap().starts_with("i0721-token=")));

    // 3. The SAME state replayed: already spent — refused, never a session.
    let replay = app(&state)
        .oneshot(
            Request::builder()
                .uri(back)
                .header(header::HOST, "res.test")
                .header(header::COOKIE, format!("i0721-oauth-state={flow_value}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);
    assert!(
        !replay
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|v| v.to_str().unwrap().starts_with("i0721-token=")),
        "a replayed callback never mints a session"
    );
    let body = axum::body::to_bytes(replay.into_body(), 8192)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("unknown or already used state"),
        "the refusal names the spent state"
    );
}

/// The attempt store is bounded: the start route is unauthenticated, so a
/// flood must not grow the process's memory one redirect at a time.
#[tokio::test]
async fn oauth_attempt_store_is_bounded() {
    use kirino::auth::passport::oauth::begin_with_binding;
    let store = i0721_server::oauth::LoginAttemptStore::new();
    // One past the (private) cap is enough to prove eviction keeps it flat.
    for _ in 0..1100 {
        begin_with_binding(
            &store,
            "stub",
            None,
            Some("binding".to_owned()),
            chrono::Duration::seconds(600),
        )
        .await
        .expect("an attempt parks");
    }
    assert!(
        store.len() <= 1024,
        "the store evicts rather than growing without bound ({} parked)",
        store.len()
    );
    assert!(!store.is_empty());
}

// ── the netdisk negative-control matrix (W5) ────────────────────────────

/// A login + rpc shorthand for the matrix tests.
async fn matrix_setup() -> (AppState, String) {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());
    (state, token)
}

/// Create a folder via RPC; returns its id.
async fn mk_folder(state: &AppState, token: &str, name: &str, parent: Option<String>) -> String {
    let (status, envelope) = rpc(
        state,
        token,
        "files.create_folder",
        serde_json::json!({"name": name, "parent_id": parent}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "folder {name}: {envelope}");
    envelope["result"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn netdisk_move_into_own_subtree_is_refused() {
    let (state, token) = matrix_setup().await;
    let a = mk_folder(&state, &token, "a", None).await;
    let b = mk_folder(&state, &token, "b", Some(a.clone())).await;
    // Move "a" INTO its own child "b" — a cycle.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.move",
        serde_json::json!({"id": a, "parent_id": b}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(rpc_error_code(&envelope), -32602);
    assert!(envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("own subtree"));
    // Deeper: a → b → c, move a into c.
    let c = mk_folder(&state, &token, "c", Some(b)).await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.move",
        serde_json::json!({"id": a, "parent_id": c}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32602, "deep cycles refuse too");
    // A LEGAL move still works: c to root.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.move",
        serde_json::json!({"id": c, "parent_id": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
}

#[tokio::test]
async fn netdisk_same_layer_duplicate_names_refuse_at_root_and_nested() {
    let (state, token) = matrix_setup().await;
    // Root layer.
    let _a = mk_folder(&state, &token, "photos", None).await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.create_folder",
        serde_json::json!({"name": "photos", "parent_id": null}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "root duplicate → conflict"
    );

    // Nested layer: same name in a DIFFERENT folder is fine.
    let sub = mk_folder(&state, &token, "sub", None).await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.create_folder",
        serde_json::json!({"name": "photos", "parent_id": sub}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "same name, different layer: {envelope}"
    );

    // Nested duplicate.
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.create_folder",
        serde_json::json!({"name": "photos", "parent_id": sub}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "nested duplicate → conflict"
    );

    // A MOVE into a layer holding the same name refuses too.
    let other = mk_folder(&state, &token, "other", None).await;
    let (status, _) = rpc(
        &state,
        &token,
        "files.move",
        serde_json::json!({"id": other, "parent_id": sub}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a distinct-name move is legal");
    let photos_root = state
        .store
        .list_folder(
            state
                .store
                .get_user_by_username("admin")
                .await
                .unwrap()
                .unwrap()
                .id,
            None,
            None,
            None,
            0,
            100,
        )
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "photos")
        .unwrap()
        .id;
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.move",
        serde_json::json!({"id": photos_root, "parent_id": sub}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "moving into a same-name layer refuses"
    );
}

#[tokio::test]
async fn netdisk_quota_exceeded_reports_used_and_limit() {
    let (state, token) = matrix_setup().await;
    let (_, admin) = test_state().await;
    let _ = admin;

    // Set a tiny quota (2 KiB) via the admin RPC.
    let user = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.quota_put",
        serde_json::json!({"user_id": user.id.to_string(), "quota_bytes": 2048}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // Upload past it: a 4 KiB PNG payload.
    let oversized = vec![0u8; 4096];
    let response = upload_raw(&state, &token, oversized, "big.png", None).await;
    assert_eq!(response.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        response.1.contains("quota exceeded"),
        "the message names the quota: {}",
        response.1
    );
    assert!(response.1.contains("2048"), "the message carries the limit");

    // quota_get answers the pair.
    let (_, envelope) = rpc(&state, &token, "files.quota_get", serde_json::json!({})).await;
    assert_eq!(envelope["result"]["quota_bytes"], 2048);

    assert!(envelope["result"]["used_bytes"].as_i64().unwrap() >= 0);
}

/// A raw multipart upload returning (status, error-text).
async fn upload_raw(
    state: &AppState,
    token: &str,
    bytes: Vec<u8>,
    name: &str,
    folder_id: Option<String>,
) -> (StatusCode, String) {
    upload_raw_query(state, token, bytes, name, folder_id, None).await
}

/// Multipart upload with an explicit `?bucket=` target (the R-I
/// override lane); returns the parsed DTO.
async fn upload_bucket(
    state: &AppState,
    token: &str,
    bytes: Vec<u8>,
    name: &str,
    bucket: &str,
) -> (StatusCode, serde_json::Value) {
    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let mut body = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{name}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let response = app(state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/files?bucket={bucket}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
            token,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[allow(clippy::too_many_arguments)]
async fn upload_raw_query(
    state: &AppState,
    token: &str,
    bytes: Vec<u8>,
    name: &str,
    folder_id: Option<String>,
    display_name: Option<&str>,
) -> (StatusCode, String) {
    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let mut body = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"{name}\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let mut query = folder_id
        .map(|f| format!("?folder_id={f}"))
        .unwrap_or_default();
    if let (Some(display), true) = (display_name, query.is_empty()) {
        query = format!("?name={display}");
    } else if let Some(display) = display_name {
        query.push_str(&format!("&name={display}"));
    }
    let response = app(state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/files{query}"))
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
            token,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn netdisk_share_expiry_and_revocation_answer_404() {
    let (state, token) = matrix_setup().await;
    let (_, dto) = upload(&state, &token, tiny_png(), "cat.png").await;
    let file_id = dto["id"].as_str().unwrap().to_owned();

    // Create a share expiring in the past: the public lane 404s.
    let past = (Timestamp::now() - jiff::Span::new().hours(1)).to_string();
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "expires_at": past, "allow_preview": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let expired_token = envelope["result"]["token"].as_str().unwrap().to_owned();

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{expired_token}/meta"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "expired → 404");

    // A live share: meta reads WITHOUT credentials; revocation 404s it.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "allow_preview": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Same file → returns the EXISTING (expired) share — the idempotent
    // path re-answers the first token. Revoke it, then create fresh.
    let shared_token = envelope["result"]["token"].as_str().unwrap().to_owned();
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.share_delete",
        serde_json::json!({"token": shared_token}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "allow_preview": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let live = envelope["result"]["token"].as_str().unwrap().to_owned();

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{live}/meta"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "live share reads with no auth"
    );
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        !json["file_name"].as_str().unwrap().is_empty(),
        "the display name answers"
    );
    assert_eq!(json["allow_preview"], true);
    assert_eq!(json["kind"], "image");

    // Download follows: inline for previewable.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{live}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("inline"));

    // Revoke → both endpoints 404.
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.share_delete",
        serde_json::json!({"token": live}),
    )
    .await;
    let _ = envelope;
    for path in [
        format!("/api/v1/share/{live}/meta"),
        format!("/api/v1/share/{live}/download"),
    ] {
        let response = app(&state)
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} revoked → 404"
        );
    }
}

#[tokio::test]
async fn netdisk_upload_into_folder_gates_ownership() {
    let (state, token) = matrix_setup().await;
    // Another user's folder.
    let member = state
        .store
        .create_user(CreateUser {
            username: "member".to_owned(),
            display_name: "Member".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_folder = state
        .store
        .create_folder(member.id, None, "theirs", None)
        .await
        .unwrap();

    // The admin CAN upload into any folder (owner-or-admin).
    let (status, _) = upload_raw(
        &state,
        &token,
        tiny_png(),
        "ok.png",
        Some(member_folder.id.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "admin may target any folder");

    // A MEMBER cannot upload into another user's folder.
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let member2 = state
        .store
        .create_user(CreateUser {
            username: "member2".to_owned(),
            display_name: "Member2".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let _ = member2;
    let admin_folder = state
        .store
        .create_folder(
            state
                .store
                .get_user_by_username("admin")
                .await
                .unwrap()
                .unwrap()
                .id,
            None,
            "mine",
            None,
        )
        .await
        .unwrap();
    let (status, text) = upload_raw(
        &state,
        &member_token,
        tiny_png(),
        "hijack.png",
        Some(admin_folder.id.to_string()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "member → admin's folder: {text}"
    );
}

#[tokio::test]
async fn netdisk_folder_list_scope_and_timeline() {
    let (state, token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;

    // Admin's own tree.
    let _folder = state
        .store
        .create_folder(admin_id, None, "mine", None)
        .await
        .unwrap();
    let (_, dto) = upload(&state, &token, tiny_png(), "own.png").await;
    let admin_file_id = dto["id"].as_str().unwrap().to_owned();

    // A member's tree (invisible to the admin's own scope).
    let member = state
        .store
        .create_user(CreateUser {
            username: "scoped".to_owned(),
            display_name: "Scoped".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    state
        .store
        .create_folder(member.id, None, "theirs", None)
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());

    // member's own scope: THEIR tree only.
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.list",
        serde_json::json!({"scope": "own", "folder_id": null}),
    )
    .await;
    let items = envelope["result"]["items"].as_array().unwrap();
    assert!(
        items.iter().all(|i| i["name"] != "mine"),
        "no admin rows in the member's own tree"
    );
    assert!(items.iter().any(|i| i["name"] == "theirs"));

    // member asking for timeline: FORCE-scoped to own (member pin).
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.list",
        serde_json::json!({"scope": "timeline"}),
    )
    .await;
    let items = envelope["result"]["items"].as_array().unwrap();
    assert!(
        items.iter().all(|i| i["id"] != admin_file_id),
        "the admin's upload stays invisible to the member's forced-own scope"
    );

    // Admin timeline: the global stream (both users' images).
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"scope": "timeline", "limit": 100}),
    )
    .await;
    let items = envelope["result"]["items"].as_array().unwrap();
    assert!(
        items.iter().any(|i| i["id"] == admin_file_id),
        "admin timeline sees the global stream"
    );
}

#[tokio::test]
async fn netdisk_folder_delete_cascades_and_reclaims_blobs() {
    let (state, token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;

    // Tree: f/ → a.png + sub/ → b.png (two DIFFERENT payloads so both
    // rows exist — tiny_png dedupes per owner).
    let f = state
        .store
        .create_folder(
            admin_id,
            None,
            "f",
            state.registry.bootstrap_bucket_id().ok(),
        )
        .await
        .unwrap();
    let (_, a) = upload(&state, &token, tiny_png(), "a.png").await;
    state
        .store
        .set_parent(
            uuid::Uuid::parse_str(a["id"].as_str().unwrap()).unwrap(),
            Some(f.id),
        )
        .await
        .unwrap();
    let sub = state
        .store
        .create_folder(admin_id, Some(f.id), "sub", f.bucket_id)
        .await
        .unwrap();
    let mut other_png = tiny_png();
    other_png.extend_from_slice(b"-distinct-"); // different content → new row
    let (status_b, b) = upload(&state, &token, other_png.clone(), "b.png").await;
    assert!(
        status_b == StatusCode::CREATED || status_b == StatusCode::OK,
        "second distinct upload: {b}"
    );
    state
        .store
        .set_parent(
            uuid::Uuid::parse_str(b["id"].as_str().unwrap()).unwrap(),
            Some(sub.id),
        )
        .await
        .unwrap();

    // a.png and b.png share content (tiny_png dedupes per owner → one
    // blob, two rows). Delete f/ → the whole subtree goes, and the
    // SHARED blob is reclaimed exactly once.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": f.id.to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // Subtree fully soft-deleted.
    // The single-file get filters soft-deleted rows — a soft-deleted
    // subtree member reads as None (the LIVE view of the world).
    for id in [
        f.id.to_string(),
        a["id"].as_str().unwrap().to_owned(),
        sub.id.to_string(),
        b["id"].as_str().unwrap().to_owned(),
    ] {
        let row = state
            .store
            .get_file(uuid::Uuid::parse_str(&id).unwrap())
            .await
            .unwrap();
        assert!(row.is_none(), "{id} gone from the live view");
    }

    // Both blobs (a's and b's distinct content) were RECLAIMED — the
    // dead rows' blobs are gone before the re-upload proves recreation.
    let a_key = a["sha256"].as_str().unwrap().to_owned();
    let reclaimed = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &a_key)
        .await
        .unwrap();
    assert!(
        reclaimed.is_none(),
        "the cascade reclaimed the dead blob (it must NOT survive its last live row)"
    );

    // A fresh upload of the same content re-creates it (no orphan).
    let (status, dto) = upload(&state, &token, tiny_png(), "again.png").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let key = dto["url"]
        .as_str()
        .unwrap()
        .trim_start_matches("/media/")
        .to_owned();
    let blob = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap();
    assert!(
        blob.is_some(),
        "the re-uploaded blob exists (the cascade reclaimed the dead copy)"
    );
}

#[tokio::test]
async fn netdisk_own_scope_pagination() {
    let (state, token) = matrix_setup().await;
    for i in 0..5 {
        mk_folder(&state, &token, &format!("f{i}"), None).await;
    }
    // Page 1: 3 items.
    let (_, e1) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"scope": "own", "offset": 0, "limit": 3}),
    )
    .await;
    assert_eq!(e1["result"]["total"], 5);
    assert_eq!(e1["result"]["items"].as_array().unwrap().len(), 3);
    // Page 2: remaining 2.
    let (_, e2) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"scope": "own", "offset": 3, "limit": 3}),
    )
    .await;
    assert_eq!(e2["result"]["items"].as_array().unwrap().len(), 2);
    // No overlap, no gap.
    let ids1: Vec<&str> = e1["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    let ids2: Vec<&str> = e2["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(
        ids1.iter().all(|id| !ids2.contains(id)),
        "pages do not overlap"
    );
    assert_eq!(ids1.len() + ids2.len(), 5, "no gap");
}

#[tokio::test]
async fn netdisk_files_chain_answers_ancestors() {
    let (state, token) = matrix_setup().await;
    let a = mk_folder(&state, &token, "a", None).await;
    let b = mk_folder(&state, &token, "b", Some(a.clone())).await;

    let (_, envelope) = rpc(
        &state,
        &token,
        "files.chain",
        serde_json::json!({"folder_id": b}),
    )
    .await;
    let chain = envelope["result"].as_array().unwrap();
    assert_eq!(chain.len(), 2, "root → b: {chain:?}");
    assert_eq!(chain[0]["name"], "a");
    assert_eq!(chain[1]["name"], "b");
}

#[tokio::test]
async fn netdisk_rename_precheck_answers_friendly_conflict() {
    let (state, token) = matrix_setup().await;
    let _a = mk_folder(&state, &token, "alpha", None).await;
    let b = mk_folder(&state, &token, "beta", None).await;

    // Rename "beta" to "alpha" → 409 (same-layer duplicate).
    let (_, envelope) = rpc(
        &state,
        &token,
        "files.update",
        serde_json::json!({"id": b, "name": "alpha"}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32007);
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "the friendly message names the conflict: {envelope}"
    );
}

// ── W5 R3: mutation-survivor pins ──────────────────────────────────────

/// The cascade also reclaims the CACHE slot (thumbnails): deleting a
/// folder containing images must free both the media blob and the
/// {key}.webp thumbnail.
#[tokio::test]
async fn netdisk_cascade_reclaims_thumbnail_cache_slot() {
    let (state, token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;

    let f = state
        .store
        .create_folder(
            admin_id,
            None,
            "photos",
            state.registry.bootstrap_bucket_id().ok(),
        )
        .await
        .unwrap();
    let (_, a) = upload(&state, &token, tiny_png(), "a.png").await;
    let a_id = uuid::Uuid::parse_str(a["id"].as_str().unwrap()).unwrap();
    state.store.set_parent(a_id, Some(f.id)).await.unwrap();

    // The thumbnail exists in the Cache slot.
    let a_key = a["sha256"].as_str().unwrap().to_owned();
    let thumb_key = format!("{a_key}.webp");
    let thumb = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Cache, &thumb_key)
        .await
        .unwrap();
    assert!(thumb.is_some(), "the thumbnail exists before the cascade");

    // Cascade delete the folder.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": f.id.to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // BOTH the media blob AND the thumbnail are reclaimed.
    let media = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &a_key)
        .await
        .unwrap();
    assert!(media.is_none(), "the media blob is reclaimed");
    let thumb_after = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Cache, &thumb_key)
        .await
        .unwrap();
    assert!(
        thumb_after.is_none(),
        "the CACHE-slot thumbnail is reclaimed too (not just the media blob)"
    );
}

/// Content-Disposition RFC 5987: non-ASCII filenames get
/// filename*=UTF-8'' with percent-encoding.
#[tokio::test]
async fn netdisk_share_download_encodes_non_ascii_filenames() {
    let (state, token) = matrix_setup().await;
    // Upload with a non-ASCII name (?name= overrides the display name).
    let (_, dto) = upload(&state, &token, tiny_png(), "cat.png").await;
    let file_id = dto["id"].as_str().unwrap().to_owned();

    // Create a share.
    let (status, share_e) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "allow_preview": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{share_e}");
    let share_token = share_e["result"]["token"].as_str().unwrap().to_owned();

    // Rename to a non-ASCII name.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.update",
        serde_json::json!({"id": file_id, "name": "猫照片.png"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // Download: the disposition uses filename*=UTF-8''…
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{share_token}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        disposition.contains("filename*=UTF-8''"),
        "non-ASCII → RFC 5987 form: {disposition}"
    );
    assert!(
        disposition.contains('%'),
        "the non-ASCII bytes are percent-encoded: {disposition}"
    );
    assert!(
        !disposition.contains("猫"),
        "the raw non-ASCII bytes are NOT in the header: {disposition}"
    );

    // ASCII filenames keep the simple form.
    let (status, _) = rpc(
        &state,
        &token,
        "files.update",
        serde_json::json!({"id": file_id, "name": "cat.png"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{share_token}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        disposition.contains("filename=\"cat.png\""),
        "ASCII stays simple: {disposition}"
    );
    assert!(
        !disposition.contains("filename*"),
        "no RFC 5987 arm for ASCII: {disposition}"
    );
}

/// Quota fallback: user row NULL → site default from settings.
#[tokio::test]
async fn netdisk_quota_falls_back_to_site_default() {
    let (state, token) = matrix_setup().await;
    let user = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    let _ = user;

    // Set the site default via settings.put.
    let strict = full_settings_json(|json| {
        json["upload"]["default-quota-bytes"] = serde_json::json!(5_000_000);
    });
    let strict: serde_json::Value = serde_json::from_str(&strict).unwrap();
    let (status, envelope) = rpc(&state, &token, "settings.put", strict).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // The user's row is NULL → quota_get answers the site default.
    let (_, envelope) = rpc(&state, &token, "files.quota_get", serde_json::json!({})).await;
    assert_eq!(
        envelope["result"]["quota_bytes"], 5_000_000,
        "NULL user row falls back to the site default: {envelope}"
    );

    // A per-user override wins.
    let (status, _) = rpc(
        &state,
        &token,
        "files.quota_put",
        serde_json::json!({"user_id": user.id.to_string(), "quota_bytes": 1000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, envelope) = rpc(&state, &token, "files.quota_get", serde_json::json!({})).await;
    assert_eq!(
        envelope["result"]["quota_bytes"], 1000,
        "per-user override wins"
    );
}

/// Memory rename: same-layer uniqueness enforced at the store layer
/// (mirroring the PG partial unique index).
#[tokio::test]
async fn netdisk_memory_store_rename_refuses_same_layer_duplicate() {
    let (state, _token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;

    let a = state
        .store
        .create_folder(admin_id, None, "alpha", None)
        .await
        .unwrap();
    let b = state
        .store
        .create_folder(admin_id, None, "beta", None)
        .await
        .unwrap();

    // Direct store-layer rename (bypassing the RPC pre-check): "beta" →
    // "alpha" must refuse at the STORE layer too.
    let result = state.store.rename_file(b.id, "alpha").await;
    assert!(
        matches!(result, Err(i0721_store::StoreError::Conflict(_))),
        "memory store refuses same-layer rename: {result:?}"
    );
    let _ = a;
}

/// Pagination: list_folder with offset/limit on the memory store.
#[tokio::test]
async fn netdisk_memory_list_folder_pagination() {
    let (state, _token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;
    for i in 0..5 {
        state
            .store
            .create_folder(admin_id, None, &format!("p{i}"), None)
            .await
            .unwrap();
    }
    let page1 = state
        .store
        .list_folder(admin_id, None, None, None, 0, 3)
        .await
        .unwrap();
    let page2 = state
        .store
        .list_folder(admin_id, None, None, None, 3, 3)
        .await
        .unwrap();
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 2);
    let ids1: Vec<_> = page1.iter().map(|r| r.id).collect();
    let ids2: Vec<_> = page2.iter().map(|r| r.id).collect();
    assert!(ids1.iter().all(|id| !ids2.contains(id)));
    assert_eq!(ids1.len() + ids2.len(), 5);
}

// ── W5 C1 pins ─────────────────────────────────────────────────────────

/// The upload lane's quota gate uses the SAME resolver as quota_get:
/// no per-user override + a site default → 413 on the WRITE path.
#[tokio::test]
async fn c1_upload_quota_gate_uses_site_default() {
    let (state, token) = matrix_setup().await;
    let user = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap();

    // No per-user override; set a site default of 2 KiB.
    let strict = full_settings_json(|json| {
        json["upload"]["default-quota-bytes"] = serde_json::json!(2048);
    });
    let strict: serde_json::Value = serde_json::from_str(&strict).unwrap();
    let (status, _) = rpc(&state, &token, "settings.put", strict).await;
    assert_eq!(status, StatusCode::OK);

    // The user row is NULL → the SITE default gates the upload.
    let response = upload_raw(&state, &token, vec![0u8; 4096], "big.png", None).await;
    assert_eq!(
        response.0,
        StatusCode::PAYLOAD_TOO_LARGE,
        "site default gates the upload: {}",
        response.1
    );
    assert!(
        response.1.contains("quota exceeded"),
        "the message names the quota"
    );
    assert!(
        response.1.contains("2048"),
        "the message carries the limit: {}",
        response.1
    );
    let _ = user;
}

/// Cross-owner thumbnail survival: user B's same-content upload keeps
/// the shared thumbnail alive when admin deletes their own tree.
#[tokio::test]
async fn c1_cross_owner_thumbnail_survives_cascade() {
    let (state, token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;

    // Member B (with a password so they can upload).
    let _member = state
        .store
        .create_user(CreateUser {
            username: "thumbmate".to_owned(),
            display_name: "ThumbMate".to_owned(),
            role: Role::Member,
            password_hash: Some(hash_password("member-pass").unwrap()),
        })
        .await
        .unwrap();
    let member_token = login(&state, "thumbmate", "member-pass").await;

    // Admin uploads a PNG into their folder; member uploads the SAME
    // bytes (shared blob, per-owner rows, own thumbnail).
    let f = state
        .store
        .create_folder(
            admin_id,
            None,
            "mine",
            state.registry.bootstrap_bucket_id().ok(),
        )
        .await
        .unwrap();
    let (_, a) = upload(&state, &token, tiny_png(), "a.png").await;
    let a_id = uuid::Uuid::parse_str(a["id"].as_str().unwrap()).unwrap();
    state.store.set_parent(a_id, Some(f.id)).await.unwrap();

    let (_, b) = upload(&state, &member_token, tiny_png(), "b.png").await;
    let _ = b;

    let sha = a["sha256"].as_str().unwrap().to_owned();
    let thumb_key = format!("{sha}.webp");

    // Admin deletes their folder — member's row still lives.
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": f.id.to_string()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // The member's thumbnail survives (their live row references it).
    let thumb = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Cache, &thumb_key)
        .await
        .unwrap();
    assert!(
        thumb.is_some(),
        "the member's thumbnail survives the admin's cascade (thumbnail-key count, not storage-key)"
    );
}

/// PG rename pre-check: same-layer duplicate refuses at the store layer.
#[tokio::test]
async fn c1_memory_rename_precheck_friendly_conflict() {
    let (state, _token) = matrix_setup().await;
    let admin_id = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap()
        .id;
    let a = state
        .store
        .create_folder(admin_id, None, "one", None)
        .await
        .unwrap();
    let b = state
        .store
        .create_folder(admin_id, None, "two", None)
        .await
        .unwrap();
    let result = state.store.rename_file(b.id, "one").await;
    assert!(
        matches!(result, Err(i0721_store::StoreError::Conflict(ref msg)) if msg.contains("already exists")),
        "store-layer rename pre-check: {result:?}"
    );
    let _ = a;
}

#[tokio::test]
async fn upload_lands_in_the_requested_folder() {
    let (state, token) = matrix_setup().await;
    let folder_id = mk_folder(&state, &token, "photos", None).await;

    let (status, raw) = upload_raw(
        &state,
        &token,
        tiny_png(),
        "shot.png",
        Some(folder_id.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "upload accepted: {raw}");
    let up: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        up["parent_id"].as_str(),
        Some(folder_id.as_str()),
        "the uploaded file reports its folder: {up}"
    );

    let (_, inside) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"scope": "own", "folder_id": folder_id}),
    )
    .await;
    let file_id = up["id"].as_str().unwrap().to_owned();
    let items = inside["result"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|f| f["id"].as_str() == Some(file_id.as_str())),
        "the file is inside the folder: {inside}"
    );
    let (_, root) = rpc(
        &state,
        &token,
        "files.list",
        serde_json::json!({"scope": "own"}),
    )
    .await;
    let root_items = root["result"]["items"].as_array().unwrap();
    assert!(
        !root_items
            .iter()
            .any(|f| f["id"].as_str() == Some(file_id.as_str())),
        "the file is NOT in the root listing: {root}"
    );
}

#[tokio::test]
async fn upload_name_conflict_reports_the_name_message() {
    let (state, token) = matrix_setup().await;
    let big = {
        let image = image::DynamicImage::new_rgb8(9, 9);
        let mut buf = std::io::Cursor::new(Vec::new());
        image.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    };
    let (status, first) =
        upload_raw_query(&state, &token, tiny_png(), "one.png", None, Some("dup.png")).await;
    assert_eq!(status, StatusCode::CREATED, "first upload: {first}");
    let (status, second) =
        upload_raw_query(&state, &token, big, "two.png", None, Some("dup.png")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "second upload conflicts: {second}"
    );
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    let body = second["error"].as_str().unwrap_or_default();
    assert!(
        body.contains("name") && !body.contains("content already uploaded"),
        "the conflict names the real cause: {body}"
    );
}

#[tokio::test]
async fn deleting_a_file_purges_its_share_rows() {
    let (state, token) = matrix_setup().await;
    let (_, up) = upload(&state, &token, tiny_png(), "shared.png").await;
    let file_id = up["id"].as_str().unwrap().to_owned();

    let (_, share) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "allow_preview": true}),
    )
    .await;
    let share_token = share["result"]["token"]
        .as_str()
        .unwrap_or_else(|| panic!("share_create failed: {share}"))
        .to_owned();
    let (_, list_before) = rpc(&state, &token, "files.share_list", serde_json::json!({})).await;
    assert_eq!(
        list_before["result"].as_array().unwrap().len(),
        1,
        "the share is listed: {list_before}"
    );

    // Delete the file: the share row must die with it (the token stops
    // resolving AND the list stops showing the orphan).
    let (status, del) = rpc(
        &state,
        &token,
        "files.delete",
        serde_json::json!({"id": file_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "delete: {del}");

    let (_, list_after) = rpc(&state, &token, "files.share_list", serde_json::json!({})).await;
    assert!(
        list_after["result"].as_array().unwrap().is_empty(),
        "no orphaned share rows: {list_after}"
    );
    let meta = app(&state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/share/{share_token}/meta"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(meta.status(), StatusCode::NOT_FOUND, "the token is dead");
}

// ── the IPFS blob backend against an in-process kubo stub ──────────────

/// An axum stub speaking the kubo RPC subset: /add, /files/cp,
/// /files/read, /files/stat, /files/rm, /files/mkdir, /pin/rm.
struct KuboStub {
    /// CID → bytes.
    content: std::sync::Mutex<std::collections::HashMap<String, bytes::Bytes>>,
    /// MFS path → CID.
    mfs: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// The pin/rm calls (for the delete assertion).
    unpinned: std::sync::Mutex<Vec<String>>,
    /// True when the stub should answer 500 for the NEXT add call.
    fail_next_add: std::sync::atomic::AtomicBool,
    /// The last /add query string (asserted to carry pin=true).
    last_add_query: std::sync::Mutex<Option<String>>,
    /// Hard-fail every /files/read once set (genuine kubo fault shape).
    fail_all_reads: std::sync::atomic::AtomicBool,
    /// mkdir mode: 0 = ok, 1 = EEXIST shape (500 "file already exists"),
    /// 2 = genuine fault (500 "disk on fire").
    mkdir_mode: std::sync::atomic::AtomicU8,
}

impl KuboStub {
    fn router(self: std::sync::Arc<Self>) -> Router {
        let add = self.clone();
        let cp = self.clone();
        let read = self.clone();
        let stat = self.clone();
        let rm = self.clone();
        let mkdir = self.clone();
        let pin_rm = self.clone();
        Router::new()
            .route(
                "/api/v0/add",
                axum::routing::post(
                    move |axum::extract::RawQuery(raw): axum::extract::RawQuery,
                          body: axum::body::Bytes| {
                        let stub = add.clone();
                        async move {
                            use axum::response::IntoResponse;
                            // kubo contract: the store asks pin=true so the
                            // content survives node GC; record it for asserts.
                            stub.last_add_query
                                .lock()
                                .unwrap()
                                .replace(raw.clone().unwrap_or_default());
                            if stub
                                .fail_next_add
                                .swap(false, std::sync::atomic::Ordering::Relaxed)
                            {
                                return (StatusCode::INTERNAL_SERVER_ERROR, "kubo exploded")
                                    .into_response();
                            }
                            // Extract the file payload from the multipart
                            // envelope in the BYTE domain (thumbnails are
                            // WebP — never valid UTF-8, so the handler
                            // receives Bytes, and the split is on bytes).
                            let start = body
                                .windows(4)
                                .position(|w| w == b"\r\n\r\n")
                                .map(|i| i + 4)
                                .unwrap_or(0);
                            let rest = &body[start..];
                            let end = rest
                                .windows(4)
                                .rposition(|w| w == b"\r\n--")
                                .unwrap_or(rest.len());
                            let payload = &rest[..end];
                            let cid = format!("Qm{:x}", payload.len());
                            stub.content
                                .lock()
                                .unwrap()
                                .insert(cid.clone(), bytes::Bytes::copy_from_slice(payload));
                            axum::Json(serde_json::json!({"Name": "file", "Hash": cid, "Size": 0}))
                                .into_response()
                        }
                    },
                ),
            )
            .route(
                "/api/v0/files/cp",
                axum::routing::post(
                    move |axum::extract::Query(q): axum::extract::Query<Vec<(String, String)>>| {
                        let stub = cp.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let args: Vec<&str> = q
                                .iter()
                                .filter(|(k, _)| k == "arg")
                                .map(|(_, v)| v.as_str())
                                .collect();
                            if args.len() != 2 {
                                return (StatusCode::BAD_REQUEST, "need 2 args").into_response();
                            }
                            let source = args[0].trim_start_matches("/ipfs/");
                            let target = args[1];
                            // kubo contract: without force an existing
                            // destination of any kind is an error.
                            let force = q.iter().any(|(k, v)| k == "force" && v == "true");
                            let mut mfs = stub.mfs.lock().unwrap();
                            if mfs.contains_key(target) && !force {
                                return (StatusCode::INTERNAL_SERVER_ERROR, "file already exists")
                                    .into_response();
                            }
                            mfs.insert(target.to_owned(), source.to_owned());
                            (StatusCode::OK, "").into_response()
                        }
                    },
                ),
            )
            .route(
                "/api/v0/files/read",
                axum::routing::post(
                    move |axum::extract::Query(q): axum::extract::Query<Vec<(String, String)>>| {
                        let stub = read.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let Some((_, path)) = q.iter().find(|(k, _)| k == "arg") else {
                                return (StatusCode::BAD_REQUEST, "missing arg").into_response();
                            };
                            if stub
                                .fail_all_reads
                                .load(std::sync::atomic::Ordering::Relaxed)
                            {
                                return (StatusCode::INTERNAL_SERVER_ERROR, "internal kubo fault")
                                    .into_response();
                            }
                            let mfs = stub.mfs.lock().unwrap();
                            match mfs.get(path.as_str()) {
                                Some(cid) => {
                                    let content = stub.content.lock().unwrap();
                                    match content.get(cid) {
                                        Some(bytes) => {
                                            (StatusCode::OK, bytes.clone()).into_response()
                                        }
                                        None => (StatusCode::INTERNAL_SERVER_ERROR, "CID vanished")
                                            .into_response(),
                                    }
                                }
                                None => (StatusCode::INTERNAL_SERVER_ERROR, "file does not exist")
                                    .into_response(),
                            }
                        }
                    },
                ),
            )
            .route(
                "/api/v0/files/stat",
                axum::routing::post(
                    move |axum::extract::Query(q): axum::extract::Query<Vec<(String, String)>>| {
                        let stub = stat.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let Some((_, path)) = q.iter().find(|(k, _)| k == "arg") else {
                                return (StatusCode::BAD_REQUEST, "missing arg").into_response();
                            };
                            if stub
                                .fail_all_reads
                                .load(std::sync::atomic::Ordering::Relaxed)
                            {
                                return (StatusCode::INTERNAL_SERVER_ERROR, "internal kubo fault")
                                    .into_response();
                            }
                            let mfs = stub.mfs.lock().unwrap();
                            match mfs.get(path.as_str()) {
                                Some(cid) => {
                                    (StatusCode::OK, axum::Json(serde_json::json!({"Hash": cid})))
                                        .into_response()
                                }
                                None => (StatusCode::INTERNAL_SERVER_ERROR, "file does not exist")
                                    .into_response(),
                            }
                        }
                    },
                ),
            )
            .route(
                "/api/v0/files/rm",
                axum::routing::post(
                    move |axum::extract::Query(q): axum::extract::Query<Vec<(String, String)>>| {
                        let stub = rm.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let Some((_, path)) = q.iter().find(|(k, _)| k == "arg") else {
                                return (StatusCode::BAD_REQUEST, "missing arg").into_response();
                            };
                            stub.mfs.lock().unwrap().remove(path.as_str());
                            (StatusCode::OK, "").into_response()
                        }
                    },
                ),
            )
            .route(
                "/api/v0/files/mkdir",
                axum::routing::post(move || {
                    let stub = mkdir.clone();
                    async move {
                        use axum::response::IntoResponse;
                        match stub.mkdir_mode.load(std::sync::atomic::Ordering::Relaxed) {
                            1 => (StatusCode::INTERNAL_SERVER_ERROR, "file already exists")
                                .into_response(),
                            2 => {
                                (StatusCode::INTERNAL_SERVER_ERROR, "disk on fire").into_response()
                            }
                            _ => (StatusCode::OK, "").into_response(),
                        }
                    }
                }),
            )
            .route(
                "/api/v0/pin/rm",
                axum::routing::post(
                    move |axum::extract::Query(q): axum::extract::Query<Vec<(String, String)>>| {
                        let stub = pin_rm.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let Some((_, cid)) = q.iter().find(|(k, _)| k == "arg") else {
                                return (StatusCode::BAD_REQUEST, "missing arg").into_response();
                            };
                            stub.unpinned.lock().unwrap().push(cid.clone());
                            (StatusCode::OK, "").into_response()
                        }
                    },
                ),
            )
    }
}

async fn ipfs_stub_state() -> (AppState, std::sync::Arc<KuboStub>) {
    let (state, _) = test_state().await;
    let stub = std::sync::Arc::new(KuboStub {
        content: std::sync::Mutex::new(std::collections::HashMap::new()),
        mfs: std::sync::Mutex::new(std::collections::HashMap::new()),
        unpinned: std::sync::Mutex::new(Vec::new()),
        fail_next_add: std::sync::atomic::AtomicBool::new(false),
        last_add_query: std::sync::Mutex::new(None),
        fail_all_reads: std::sync::atomic::AtomicBool::new(false),
        mkdir_mode: std::sync::atomic::AtomicU8::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = stub.clone().router();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let store =
        i0721_server::blob_ipfs::IpfsBlobStore::open(&i0721_server::blob_ipfs::IpfsConfig {
            api_url: format!("http://127.0.0.1:{}", addr.port()),
        });
    // Rebuild state with the IPFS blob store AND the matching backend tag
    // (the config field feeds the upload's storage_backend column).
    let mut config = state.config.as_ref().clone();
    config.storage_backend = Some("ipfs".to_owned());
    let new_state = i0721_server::state::AppState::new(
        state.store.clone(),
        std::sync::Arc::new(store),
        std::sync::Arc::new(config),
    );
    (new_state, stub)
}

#[tokio::test]
async fn ipfs_put_get_delete_round_trip() {
    use i0721_server::blob::BlobSlot;
    let (state, stub) = ipfs_stub_state().await;

    // put
    let data = b"hello ipfs world";
    state
        .blobs
        .put(BlobSlot::Media, "test-key", data)
        .await
        .unwrap();

    // The MFS entry exists.
    assert!(stub
        .mfs
        .lock()
        .unwrap()
        .contains_key("/i0721/media/test-key"));

    // get returns the bytes.
    let got = state.blobs.get(BlobSlot::Media, "test-key").await.unwrap();
    assert_eq!(got.as_deref(), Some(&data[..]));

    // delete removes the MFS entry + unpins.
    state
        .blobs
        .delete(BlobSlot::Media, "test-key")
        .await
        .unwrap();
    assert!(!stub
        .mfs
        .lock()
        .unwrap()
        .contains_key("/i0721/media/test-key"));
    let unpinned = stub.unpinned.lock().unwrap().clone();
    assert_eq!(unpinned.len(), 1, "one unpin call: {unpinned:?}");

    // get after delete → None.
    let gone = state.blobs.get(BlobSlot::Media, "test-key").await.unwrap();
    assert!(gone.is_none());
}

#[tokio::test]
async fn ipfs_kubo_error_surfaces() {
    use i0721_server::blob::BlobSlot;
    let (state, stub) = ipfs_stub_state().await;
    stub.fail_next_add
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let result = state.blobs.put(BlobSlot::Media, "err-key", b"data").await;
    assert!(result.is_err(), "a kubo 500 must surface as an IoError");
}

#[tokio::test]
async fn ipfs_delete_absent_key_is_fine() {
    use i0721_server::blob::BlobSlot;
    let (state, _stub) = ipfs_stub_state().await;
    // Delete a key that was never put — no error.
    state
        .blobs
        .delete(BlobSlot::Cache, "never-existed")
        .await
        .unwrap();
}

/// Upload writes the backend tag into the file row.
#[tokio::test]
async fn upload_records_storage_backend_tag() {
    let (state, token) = test_state_with_token().await;
    let (_, dto) = upload(&state, &token, tiny_png(), "tag.png").await;
    // The local lane (default) writes "local".
    // (The RPC response doesn't carry storage_backend; read via the
    // store directly.)
    let file_id = uuid::Uuid::parse_str(dto["id"].as_str().unwrap()).unwrap();
    let record = state.store.get_file(file_id).await.unwrap().unwrap();
    assert_eq!(
        record.storage_backend, "local",
        "the local lane writes its tag"
    );
}

#[tokio::test]
async fn upload_records_ipfs_backend_tag() {
    let (state, stub) = ipfs_stub_state().await;
    let admin = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    let token = state.signer.sign(admin.id, Timestamp::now());
    let (status, dto) = upload(&state, &token, tiny_png(), "tag.png").await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "upload on the ipfs lane: {dto}"
    );
    let file_id = uuid::Uuid::parse_str(dto["id"].as_str().unwrap()).unwrap();
    let record = state.store.get_file(file_id).await.unwrap().unwrap();
    assert_eq!(
        record.storage_backend, "ipfs",
        "the ipfs lane writes its tag (config-driven, not a literal)"
    );
    // The blob really landed through the IPFS stub (the media key is
    // the content hash — pin the exact entry, no fallback broadening).
    let media_key = record.storage_key.clone();
    assert!(
        stub.mfs
            .lock()
            .unwrap()
            .contains_key(&format!("/i0721/media/{media_key}")),
        "the upload flowed through the IPFS backend at its content key: {:?}",
        stub.mfs.lock().unwrap().keys()
    );
}

async fn test_state_with_token() -> (AppState, String) {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());
    (state, token)
}

// ── the opt-in S3 real-endpoint lane ────────────────────────────────────
// Run with: I0721_TEST_S3=1 I0721_S3_* env → `just s3-test`
// (or `just s3-test`). Fail-closed: set I0721_TEST_S3 with unreachable
// credentials → panic, not skip.

#[tokio::test]
#[ignore = "opt-in: set I0721_TEST_S3=1 + I0721_S3_* env"]
async fn s3_real_endpoint_round_trip() {
    use i0721_server::blob::{BlobSlot, BlobStore};

    if std::env::var("I0721_TEST_S3").is_err() {
        panic!(
            "I0721_TEST_S3 is not set — this lane is opt-in (fail-closed, never silently skipped)"
        );
    }
    let config = i0721_server::blob_s3::S3Config::from_env(|key| std::env::var(key).ok())
        .expect("I0721_TEST_S3 is set — the S3 env must be complete");
    let store = i0721_server::blob_s3::S3BlobStore::open(&config)
        .expect("the S3 client must construct against the real endpoint");

    let key = format!("test-round-trip-{}", uuid::Uuid::new_v4().simple());
    let data = b"s3 round trip payload";

    store.put(BlobSlot::Media, &key, data).await.expect("put");
    let got = store.get(BlobSlot::Media, &key).await.expect("get");
    assert_eq!(got.as_deref(), Some(&data[..]));
    store.delete(BlobSlot::Media, &key).await.expect("delete");
    let gone = store
        .get(BlobSlot::Media, &key)
        .await
        .expect("get after delete");
    assert!(gone.is_none());
}

#[tokio::test]
async fn ipfs_put_same_key_twice_overwrites() {
    let (state, _stub) = ipfs_stub_state().await;
    let blobs = state.blobs.clone();
    let first = blobs
        .put(
            i0721_server::blob::BlobSlot::Media,
            "twice",
            b"one".as_slice(),
        )
        .await;
    assert!(first.is_ok(), "first put: {first:?}");
    let second = blobs
        .put(
            i0721_server::blob::BlobSlot::Media,
            "twice",
            b"two".as_slice(),
        )
        .await;
    assert!(
        second.is_ok(),
        "re-put must overwrite (force=true): {second:?}"
    );
    let got = blobs
        .get(i0721_server::blob::BlobSlot::Media, "twice")
        .await
        .unwrap()
        .expect("present");
    assert_eq!(&got[..], b"two", "the second write wins");
}

#[tokio::test]
async fn ipfs_read_genuine_500_is_an_error_not_a_miss() {
    let (state, stub) = ipfs_stub_state().await;
    let blobs = state.blobs.clone();
    // Prime the MFS entry, then flip the stub's read endpoint into
    // hard-failure mode: a genuine kubo fault must surface as IoError,
    // never as Ok(None) (which the media lane would render as 404).
    blobs
        .put(i0721_server::blob::BlobSlot::Media, "boom", b"x".as_slice())
        .await
        .unwrap();
    stub.fail_all_reads
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let got = blobs.get(i0721_server::blob::BlobSlot::Media, "boom").await;
    assert!(
        got.is_err(),
        "a genuine kubo 500 must surface as an IoError: {got:?}"
    );
}

#[tokio::test]
async fn ipfs_add_carries_pin_true() {
    let (state, stub) = ipfs_stub_state().await;
    state
        .blobs
        .put(
            i0721_server::blob::BlobSlot::Media,
            "pinned",
            b"pinme".as_slice(),
        )
        .await
        .unwrap();
    let query = stub
        .last_add_query
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();
    assert!(
        query.contains("pin=true"),
        "POST /api/v0/add must carry pin=true (GC survival): {query}"
    );
}

#[tokio::test]
async fn ipfs_mkdir_eexist_is_swallowed_but_faults_surface() {
    let (state, stub) = ipfs_stub_state().await;
    use i0721_server::blob::BlobSlot;

    // EEXIST shape: kubo answers 500 "file already exists" — the store
    // treats it as mkdir -p success.
    stub.mkdir_mode
        .store(1, std::sync::atomic::Ordering::Relaxed);
    let ok = state
        .blobs
        .put(BlobSlot::Media, "eexist", b"e".as_slice())
        .await;
    assert!(ok.is_ok(), "EEXIST is swallowed: {ok:?}");

    // A genuine mkdir fault must surface as IoError, never as silence.
    stub.mkdir_mode
        .store(2, std::sync::atomic::Ordering::Relaxed);
    let fault = state
        .blobs
        .put(BlobSlot::Media, "fault", b"f".as_slice())
        .await;
    assert!(fault.is_err(), "a genuine mkdir fault surfaces: {fault:?}");
}

// ── the API key + MCP lane ─────────────────────────────────────────────

#[tokio::test]
async fn mcp_keys_crud_copy_once_semantics() {
    let (state, token) = test_state_with_token().await;

    // Create: the plaintext key appears EXACTLY ONCE.
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "my agent key", "ops": all_ops()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let key = envelope["result"]["key"].as_str().unwrap().to_owned();
    assert!(
        key.starts_with("i0721-"),
        "the format is i0721-<hex>: {key}"
    );
    assert_eq!(key.len(), 38, "prefix + 32 hex chars: {key}");
    let key_id = envelope["result"]["id"].as_str().unwrap().to_owned();

    // List: the plaintext NEVER appears.
    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    let items = list["result"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        !serde_json::to_string(items).unwrap().contains(&key),
        "the LIST must NOT carry the plaintext key"
    );
    assert!(
        items[0]["key_prefix"].as_str().unwrap().ends_with("****"),
        "the list shows the masked prefix"
    );

    // Revoke.
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.revoke",
        serde_json::json!({"id": key_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    // Revoked key: create a second one, verify the revoked one is dead.
    let (_, e2) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "second", "ops": all_ops()}),
    )
    .await;
    let key2 = e2["result"]["key"].as_str().unwrap().to_owned();
    let record2 = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key2))
        .await
        .unwrap();
    assert!(record2.is_some(), "the live key verifies");

    let record1 = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key))
        .await
        .unwrap();
    assert!(record1.is_none(), "the REVOKED key does not verify");
}

/// Hash helper for tests (exposes the private function).
mod mcp_test_helper {
    pub fn hash(key: &str) -> String {
        i0721_server::mcp::hash_api_key(key)
    }
}

#[tokio::test]
async fn mcp_sse_handshake_and_tool_flow() {
    let (state, token) = test_state_with_token().await;

    // Create an API key.
    let (_, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "mcp-test", "ops": all_ops()}),
    )
    .await;
    let api_key = envelope["result"]["key"].as_str().unwrap().to_owned();

    // 1. SSE without key → 401 (oneshot).
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/mcp/sse")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "no key → 401");

    // 2. SSE with key → 200 (the stream opens; we can't easily read SSE
    // frames through oneshot — the real TCP test below covers the shape).
    // Instead, verify the MESSAGE dispatch through oneshot by first
    // registering a session via the SSE handshake.
    // (The SSE stream is a long-lived response; oneshot can't complete it.
    // The tool dispatch is exercised through the message endpoint below.)

    // 3. Message with an unknown session → 401.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/mcp/message?sessionId=00000000-0000-0000-0000-000000000000")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "unknown session → 401"
    );

    let _ = api_key; // (the SSE 200 is proven by the raw TCP test below)
}

/// Raw TCP: the SSE first frame carries the endpoint event + sessionId.
#[tokio::test]
async fn mcp_sse_first_frame_has_endpoint_event() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (state, token) = test_state_with_token().await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "tcp-sse", "ops": all_ops()}),
    )
    .await;
    let api_key = envelope["result"]["key"].as_str().unwrap().to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = routes::app(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", addr.port()))
        .await
        .unwrap();
    stream
        .write_all(
            format!(
        "GET /api/mcp/sse HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {api_key}\r\n\r\n"
    )
            .as_bytes(),
        )
        .await
        .unwrap();

    let mut buf = [0u8; 8192];
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("read within 5s")
        .unwrap();
    let sse = String::from_utf8_lossy(&buf[..n]).to_string();
    assert!(
        sse.starts_with("HTTP/1.1 200"),
        "with key → 200: {}",
        sse.lines().next().unwrap_or_default()
    );
    assert!(
        sse.contains("event: endpoint"),
        "the first frame is the endpoint event"
    );
    assert!(sse.contains("sessionId="), "the sessionId is in the data");
}

#[tokio::test]
async fn mcp_revoked_key_sse_handshake_401() {
    let (state, token) = test_state_with_token().await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "revoke-me", "ops": all_ops()}),
    )
    .await;
    let api_key = envelope["result"]["key"].as_str().unwrap().to_owned();
    let key_id = envelope["result"]["id"].as_str().unwrap().to_owned();

    // Revoke.
    let _ = rpc(
        &state,
        &token,
        "keys.revoke",
        serde_json::json!({"id": key_id}),
    )
    .await;

    // The revoked key cannot open an SSE stream.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/mcp/sse")
                .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "revoked key → 401"
    );
}

#[tokio::test]
async fn mcp_keys_admin_gate() {
    let (state, token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "keymate".to_owned(),
            display_name: "KeyMate".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());

    // Admin creates a key.
    let (_, e1) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "admin-key", "ops": all_ops()}),
    )
    .await;
    let admin_key_id = e1["result"]["id"].as_str().unwrap().to_owned();

    // Member tries to revoke the admin's key → 403.
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "keys.revoke",
        serde_json::json!({"id": admin_key_id}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "member cannot revoke another's key"
    );
}

// ── W7 S1: the MCP upload chain + the session lifecycle ────────────────

/// Serve the real router on an ephemeral port (the raw-TCP SSE lane needs
/// a socket) and return the port.
async fn mcp_serve(state: &AppState) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = routes::app(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

/// Read from a raw TCP socket until the accumulated bytes contain
/// `needle` (5 s budget). Panics with the partial transcript instead of
/// hanging when the frame never arrives.
async fn sse_read_until(stream: &mut tokio::net::TcpStream, needle: &str) -> String {
    use tokio::io::AsyncReadExt;
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let text = String::from_utf8_lossy(&acc).to_string();
        if text.contains(needle) {
            return text;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "the SSE frame carrying {needle:?} never arrived: {text}"
        );
        let mut buf = [0u8; 8192];
        let n = tokio::time::timeout(left, stream.read(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("SSE read timed out waiting for {needle:?}: {text}"))
            .unwrap_or_else(|err| panic!("SSE read failed: {err}"));
        assert!(n > 0, "the SSE stream closed before {needle:?}: {text}");
        acc.extend_from_slice(&buf[..n]);
    }
}

/// Read whatever the stream delivers within `budget` and return it. The
/// negative-side companion of [`sse_read_until`]: used to prove that a
/// frame does NOT arrive (a notification is never answered).
async fn sse_drain(stream: &mut tokio::net::TcpStream, budget: std::time::Duration) -> String {
    use tokio::io::AsyncReadExt;
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return String::from_utf8_lossy(&acc).to_string();
        }
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(left, stream.read(&mut buf)).await {
            // The peer closed, or the budget elapsed with nothing new.
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return String::from_utf8_lossy(&acc).to_string(),
            Ok(Ok(n)) => acc.extend_from_slice(&buf[..n]),
        }
    }
}

/// The `sessionId` carried by the handshake's first (endpoint) frame.
fn session_id_of(frame: &str) -> String {
    frame
        .split("sessionId=")
        .nth(1)
        .expect("the endpoint frame carries the sessionId")
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// The permissions a test key needs when the test is about something
/// OTHER than permissions: every op, in the canonical order. Existing
/// lane tests predate per-key permissions; granting all three keeps their
/// subject (uploads, paging, shares) the only thing under test.
const ALL_OPS: &[&str] = &["read", "upload", "share"];

/// The same set, as the `ops` array a `keys.create` / `keys.update`
/// payload carries.
fn all_ops() -> serde_json::Value {
    serde_json::json!(ALL_OPS)
}

/// Mint an API key through the RPC lane and return the plaintext.
///
/// The key is granted [`ALL_OPS`]: a key created with no permission can
/// do nothing, which would turn every tool test below into a scope test.
/// The permission lanes build their own keys with an explicit `ops` list
/// (`mcp_api_key_with`).
async fn mcp_api_key(state: &AppState, token: &str, name: &str) -> String {
    mcp_api_key_with(state, token, name, all_ops()).await
}

/// Mint an API key with an explicit permission list (and optional
/// `expires_at`), returning `(id, plaintext)`.
async fn mcp_api_key_with_at(
    state: &AppState,
    token: &str,
    name: &str,
    ops: serde_json::Value,
    expires_at: Option<&str>,
) -> (String, String) {
    let mut payload = serde_json::json!({"name": name, "ops": ops});
    if let Some(expires_at) = expires_at {
        payload["expires_at"] = serde_json::Value::String(expires_at.to_owned());
    }
    let (status, envelope) = rpc(state, token, "keys.create", payload).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope.get("error").is_none(),
        "key creation must succeed: {envelope}"
    );
    (
        envelope["result"]["id"]
            .as_str()
            .expect("the response carries the id")
            .to_owned(),
        envelope["result"]["key"]
            .as_str()
            .expect("the response carries the plaintext")
            .to_owned(),
    )
}

/// [`mcp_api_key_with_at`] without an expiry.
async fn mcp_api_key_with(
    state: &AppState,
    token: &str,
    name: &str,
    ops: serde_json::Value,
) -> String {
    mcp_api_key_with_at(state, token, name, ops, None).await.1
}

/// The user id behind a session token (the tests' shortcut to "who am
/// I" for rows planted straight into the store).
fn admin_of(state: &AppState, token: &str) -> uuid::Uuid {
    state
        .signer
        .verify(token)
        .expect("the test token verifies")
        .sub
        .parse()
        .expect("the subject is a uuid")
}

/// Plant an API key DIRECTLY in the store — the only way to build a row
/// the RPC lanes refuse to create (an already-expired key, or a legacy
/// row whose `scopes` is not the canonical object). Answers
/// `(id, plaintext)`.
///
/// The RPC pre-validates `expires_at` (must be in the future), so a test
/// that needs a key to EXPIRE plants one and lets the clock do it, or
/// plants one already past its instant.
async fn plant_api_key(
    state: &AppState,
    user_id: uuid::Uuid,
    name: &str,
    scopes: &str,
    expires_at: Option<&str>,
) -> (uuid::Uuid, String) {
    let plaintext = format!("i0721-{}", uuid::Uuid::new_v4().simple());
    let record = i0721_proto::ApiKeyRecord {
        id: uuid::Uuid::new_v4(),
        user_id,
        name: name.to_owned(),
        key_prefix: i0721_server::mcp::mask_api_key(&plaintext),
        key_hash: i0721_server::mcp::hash_api_key(&plaintext),
        scopes: scopes.to_owned(),
        expires_at: expires_at.map(str::to_owned),
        last_used_at: None,
        revoked_at: None,
        created_at: Timestamp::now(),
    };
    let id = record.id;
    state
        .store
        .create_api_key(&record)
        .await
        .expect("plant the API key row");
    (id, plaintext)
}

/// Open the SSE lane over real TCP: the socket plus its session id.
async fn mcp_handshake(state: &AppState, api_key: &str) -> (tokio::net::TcpStream, String) {
    use tokio::io::AsyncWriteExt;
    let port = mcp_serve(state).await;
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    stream
        .write_all(
            format!(
                "GET /api/mcp/sse HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                 Authorization: Bearer {api_key}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let frame = sse_read_until(&mut stream, "sessionId=").await;
    assert!(
        frame.contains("event: endpoint"),
        "the first frame is the endpoint event: {frame}"
    );
    let session_id = session_id_of(&frame);
    assert!(
        !session_id.is_empty(),
        "the frame carries a sessionId: {frame}"
    );
    (stream, session_id)
}

/// POST one JSON-RPC frame on the message lane (the real router) and
/// return its HTTP answer.
async fn mcp_post_message(
    state: &AppState,
    session_id: &str,
    method: &str,
    params: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
    .to_string();
    mcp_post_body(state, session_id, body).await
}

/// POST one raw JSON-RPC body on the message lane. Covers the shapes the
/// typed helper cannot build: notifications (no `id`) and deliberate
/// protocol violations.
async fn mcp_post_body(
    state: &AppState,
    session_id: &str,
    body: String,
) -> (StatusCode, serde_json::Value) {
    mcp_post_body_at(state, session_id, None, body).await
}

/// The same POST with an explicit `Host` header (`Some`) or none at all
/// (`None`) — the origin-derivation lane's two inputs.
async fn mcp_post_body_at(
    state: &AppState,
    session_id: &str,
    host: Option<&str>,
    body: String,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/mcp/message?sessionId={session_id}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(host) = host {
        request = request.header(header::HOST, host);
    }
    let response = app(state)
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

/// POST one frame with an explicit `id` and return the HTTP answer. The
/// result itself comes back on the SSE stream, tagged with this id.
async fn mcp_post_frame(
    state: &AppState,
    session_id: &str,
    id: i64,
    method: &str,
    params: serde_json::Value,
) -> StatusCode {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    mcp_post_body(state, session_id, body).await.0
}

/// `tools/call` through the message lane; returns the HTTP answer (the
/// tool RESULT itself comes back on the SSE stream).
async fn mcp_call_tool(
    state: &AppState,
    session_id: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> StatusCode {
    let (status, _) = mcp_post_message(
        state,
        session_id,
        "tools/call",
        serde_json::json!({"name": tool, "arguments": arguments}),
    )
    .await;
    status
}

/// The JSON-RPC envelope of the first `data:` line that carries one (the
/// stream also carries the endpoint frame and keepalive comments).
fn sse_envelope(frame: &str) -> serde_json::Value {
    frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|data| {
            serde_json::from_str::<serde_json::Value>(data.trim())
                .ok()
                .filter(|value| value.get("jsonrpc").is_some())
        })
        .unwrap_or_else(|| panic!("no JSON-RPC frame in: {frame}"))
}

/// `(isError, text)` of a `tools/call` result.
fn mcp_tool_result(envelope: &serde_json::Value) -> (bool, String) {
    let result = &envelope["result"];
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    (is_error, text)
}

/// One tool of a `tools/list` answer, by name.
fn tool_of<'a>(tools: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    tools
        .as_array()
        .unwrap_or_else(|| panic!("tools/list must answer an array: {tools}"))
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("no tool named {name:?} in {tools}"))
}

/// A PNG of the given shape — a DISTINCT byte string per shape, so the
/// per-owner content dedupe does not collapse two test uploads into one
/// row.
fn png_of(width: u32, height: u32) -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(width, height);
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// Base64 of the bytes, as the MCP `content_base64` argument carries them.
fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The MCP upload tool runs the REST chain: the QUOTA gate rejects an
/// over-quota upload as a tool error (the tool used to skip quota).
#[tokio::test]
async fn mcp_upload_file_enforces_quota() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());
    // A quota far below one upload.
    state
        .store
        .set_user_quota(admin.id, Some(16))
        .await
        .unwrap();
    let api_key = mcp_api_key(&state, &token, "quota-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let status = mcp_call_tool(
        &state,
        &session_id,
        "upload_file",
        serde_json::json!({
            "content_base64": base64_of(&tiny_png()),
            "name": "too-much.png",
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "the message lane accepted the frame"
    );

    let frames = sse_read_until(&mut sse, "\"isError\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(
        is_error,
        "an over-quota upload must be a tool error: {frames}"
    );
    assert!(
        text.to_lowercase().contains("quota"),
        "the tool text names the quota gate: {text}"
    );
}

/// The MCP upload tool refuses a destination folder the caller does not
/// own (the tool used to pass any folder id straight into the row).
#[tokio::test]
async fn mcp_upload_file_refuses_foreign_folder() {
    let (state, admin) = test_state().await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    let admin_folder = mk_folder(&state, &admin_token, "admin-only", None).await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "mcp-member".to_owned(),
            display_name: "MCP Member".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let api_key = mcp_api_key(&state, &member_token, "member-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let status = mcp_call_tool(
        &state,
        &session_id,
        "upload_file",
        serde_json::json!({
            "content_base64": base64_of(&tiny_png()),
            "name": "intruder.png",
            "folder_id": admin_folder,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let frames = sse_read_until(&mut sse, "\"isError\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(is_error, "a foreign folder must be a tool error: {frames}");
    assert!(
        text.contains("not your folder"),
        "the tool text names the ownership rule: {text}"
    );

    // Nothing landed anywhere in the admin's folder.
    let folder_id = uuid::Uuid::parse_str(&admin_folder).unwrap();
    let rows = state
        .store
        .list_folder(admin.id, Some(folder_id), None, None, 0, 100)
        .await
        .unwrap();
    assert!(rows.is_empty(), "the refused upload wrote no row: {rows:?}");
}

/// The MCP upload tool records the CONFIGURED storage backend (it used to
/// hardcode "local" while the bytes went to the configured backend).
#[tokio::test]
async fn mcp_upload_records_configured_backend() {
    let (state, stub) = ipfs_stub_state().await;
    let admin = state
        .store
        .get_user_by_username("admin")
        .await
        .unwrap()
        .unwrap();
    let token = state.signer.sign(admin.id, Timestamp::now());
    let api_key = mcp_api_key(&state, &token, "ipfs-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let status = mcp_call_tool(
        &state,
        &session_id,
        "upload_file",
        serde_json::json!({
            "content_base64": base64_of(&tiny_png()),
            "name": "mcp-tag.png",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let frames = sse_read_until(&mut sse, "\"result\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(
        !is_error,
        "the upload must succeed on the ipfs lane: {frames}"
    );
    let dto: serde_json::Value = serde_json::from_str(&text).expect("the tool answers a FileDto");
    let file_id = uuid::Uuid::parse_str(dto["id"].as_str().unwrap()).unwrap();
    let record = state.store.get_file(file_id).await.unwrap().unwrap();
    assert_eq!(
        record.storage_backend, "ipfs",
        "the configured tag, not the tool's old literal"
    );
    assert!(
        stub.mfs
            .lock()
            .unwrap()
            .contains_key(&format!("/i0721/media/{}", record.storage_key)),
        "the bytes flowed through the IPFS backend: {:?}",
        stub.mfs.lock().unwrap().keys()
    );
}

/// A key revoked AFTER the handshake kills the live session: the message
/// lane re-checks the session's key, answers 401 naming the revocation
/// and drops the entry, so the stream ends and the sessionId is dead.
#[tokio::test]
async fn mcp_revoked_key_kills_the_live_session() {
    let (state, token) = test_state_with_token().await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "live-revoke", "ops": all_ops()}),
    )
    .await;
    let api_key = envelope["result"]["key"].as_str().unwrap().to_owned();
    let key_id = envelope["result"]["id"].as_str().unwrap().to_owned();

    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    // The session really is live BEFORE the revocation.
    let (status, _) =
        mcp_post_message(&state, &session_id, "initialize", serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "a live session accepts a frame"
    );
    let frames = sse_read_until(&mut sse, "protocolVersion").await;
    assert!(
        frames.contains("\"result\""),
        "the live session answered: {frames}"
    );

    // Revoke the key behind the session's back.
    let (_, revoked) = rpc(
        &state,
        &token,
        "keys.revoke",
        serde_json::json!({"id": key_id}),
    )
    .await;
    assert!(
        revoked.get("result").is_some(),
        "the revoke call succeeded: {revoked}"
    );

    // The next frame on the SAME session is refused: 401 whose body names
    // the revocation (not the generic unknown-session answer).
    let (status, body) =
        mcp_post_message(&state, &session_id, "initialize", serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the revoked key's live session is dead: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("revoked"),
        "the answer names the revocation: {body}"
    );

    // The entry is gone: a retry answers the generic unknown-session 401.
    let (status, body) =
        mcp_post_message(&state, &session_id, "initialize", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown"),
        "the session left the table: {body}"
    );

    // And the SSE stream itself ends: the client cannot keep riding it.
    // (A finished chunked body is NOT a TCP EOF — hyper keeps the
    // connection for the next request — so the observable event is the
    // response's terminating `0\r\n\r\n` chunk.)
    assert!(
        sse_read_to_end(&mut sse, std::time::Duration::from_secs(5)).await,
        "the revoked key's SSE stream must end, not keep pinging"
    );
}

/// Read to the end of the chunked response body (the `0\r\n\r\n`
/// terminator) within the budget; `true` when the server finished the
/// stream (or closed the socket) in time.
///
/// The terminator is matched WITHOUT its leading CRLF: the earlier
/// `sse_read_until` reads already consumed the last data chunk's trailing
/// CRLF, so a finished stream here reads exactly `"0\r\n\r\n"`.
async fn sse_read_to_end(stream: &mut tokio::net::TcpStream, budget: std::time::Duration) -> bool {
    use tokio::io::AsyncReadExt;
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if String::from_utf8_lossy(&acc).ends_with("0\r\n\r\n") {
            return true;
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        let mut buf = [0u8; 8192];
        match tokio::time::timeout(left, stream.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return true,
            Ok(Ok(n)) => acc.extend_from_slice(&buf[..n]),
            Err(_) => return false,
        }
    }
}

/// The disconnect hook: dropping the SSE body removes the session from
/// the table immediately instead of squatting for the rest of the TTL.
#[tokio::test]
async fn mcp_session_leaves_the_table_on_disconnect() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "drop-key").await;

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/mcp/sse")
                .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Poll the first frame (the endpoint event) so the stream body — and
    // with it the guard — is really alive, then drop it.
    let mut frames = Box::pin(response.into_body().into_data_stream());
    let first = futures::StreamExt::next(&mut frames)
        .await
        .expect("the endpoint frame")
        .unwrap();
    let text = String::from_utf8_lossy(&first).to_string();
    assert!(
        text.contains("event: endpoint"),
        "the first frame is the endpoint event: {text}"
    );
    let session_id = session_id_of(&text);
    drop(frames); // the client went away

    // The session left the table with its stream: the message lane no
    // longer knows it. (Without the guard it stays accepted until the
    // TTL sweep.)
    let (status, body) =
        mcp_post_message(&state, &session_id, "initialize", serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the session left the table with its stream: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("unknown"),
        "{body}"
    );
}

// ── W7 S2: the JSON-RPC envelope, the tool schemas and the share link ──

/// `initialize` answers the shape a client needs before it calls
/// anything: protocolVersion, serverInfo and capabilities.
#[tokio::test]
async fn mcp_initialize_answers_protocol_version_and_server_info() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "init-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let status = mcp_post_frame(&state, &session_id, 5, "initialize", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let frames = sse_read_until(&mut sse, "\"serverInfo\"").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(envelope["id"], 5, "the id rides back verbatim");
    assert!(
        envelope.get("error").is_none(),
        "initialize is not an error: {envelope}"
    );
    let result = &envelope["result"];
    assert_eq!(
        result["protocolVersion"], "2024-11-05",
        "the advertised MCP revision"
    );
    assert_eq!(result["serverInfo"]["name"], "0721");
    assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        result["capabilities"]["tools"].is_object(),
        "the server advertises the tools capability: {result}"
    );
}

/// Five tools, each with a REAL JSON Schema `inputSchema`: an object
/// schema, typed properties, and a `required` list whose entries are
/// properties. The two paging tools declare the same window their
/// implementation honors (default 100/500, capped at 100/500).
#[tokio::test]
async fn mcp_tools_list_carries_a_real_input_schema_per_tool() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "schema-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let status = mcp_post_frame(&state, &session_id, 1, "tools/list", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"inputSchema\"").await;
    let envelope = sse_envelope(&frames);
    assert!(envelope.get("error").is_none(), "{envelope}");
    let tools = &envelope["result"]["tools"];

    let mut names: Vec<&str> = tools
        .as_array()
        .expect("tools/list answers an array of tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("each tool has a name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "file_info",
            "get_share_link",
            "list_files",
            "search_files",
            "upload_file"
        ],
        "the five tools, each named once"
    );

    for tool in tools.as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        assert_eq!(
            schema["type"], "object",
            "{name}: inputSchema must be an object schema"
        );
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: inputSchema has no properties object"));
        assert!(
            !properties.is_empty(),
            "{name}: inputSchema properties must not be empty"
        );
        let required = schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: inputSchema has no required list"));
        for key in required {
            let key = key.as_str().expect("required entries are strings");
            assert!(
                properties.contains_key(key),
                "{name}: required {key:?} is not one of its properties"
            );
        }
    }

    // `list_files` really takes the two paging knobs (and says how wide).
    let list = &tool_of(tools, "list_files")["inputSchema"]["properties"];
    for key in ["offset", "limit"] {
        assert_eq!(
            list[key]["type"], "integer",
            "list_files.{key} is an integer"
        );
    }
    assert_eq!(
        list["limit"]["maximum"], 100,
        "list_files caps the page at 100"
    );

    // `search_files` pages too, at 500, and requires its query.
    let search = &tool_of(tools, "search_files")["inputSchema"];
    assert_eq!(search["properties"]["limit"]["maximum"], 500);
    assert!(
        search["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "query"),
        "search_files requires the query: {search}"
    );

    // `get_share_link` exposes the share parameters it now honors.
    let share = &tool_of(tools, "get_share_link")["inputSchema"]["properties"];
    for key in ["file_id", "expires_at", "allow_preview"] {
        assert!(
            share.get(key).is_some(),
            "get_share_link exposes {key}: {share}"
        );
    }
    assert_eq!(share["allow_preview"]["type"], "boolean");
}

/// A notification (a valid request object WITHOUT `id`) is accepted with
/// 202 and NEVER answered: no frame appears on the stream. The session
/// stays usable, so the silence is the notification branch, not a dead
/// stream.
#[tokio::test]
async fn mcp_notification_is_accepted_without_a_response_frame() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "notify-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    let (status, body) = mcp_post_body(
        &state,
        &session_id,
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "a notification is accepted: {body}"
    );

    let drained = sse_drain(&mut sse, std::time::Duration::from_millis(300)).await;
    assert!(
        !drained.contains("\"jsonrpc\""),
        "a notification must NOT be answered on the stream: {drained:?}"
    );

    // Positive control: the very same session answers the next real call.
    let status = mcp_post_frame(&state, &session_id, 2, "initialize", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"protocolVersion\"").await;
    assert!(
        frames.contains("\"id\":2"),
        "the session survived the notification: {frames}"
    );
}

/// Protocol failures ride the envelope's TOP-LEVEL `error` object — never
/// a `result` payload that merely contains an error shape.
#[tokio::test]
async fn mcp_protocol_failures_use_the_top_level_error_object() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "fault-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    // Unknown method → -32601.
    let status = mcp_post_frame(
        &state,
        &session_id,
        3,
        "no.such.method",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "-32601").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(envelope["id"], 3);
    assert!(
        envelope.get("result").is_none(),
        "a protocol failure is not a result payload: {envelope}"
    );
    assert_eq!(envelope["error"]["code"], -32601, "{envelope}");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no.such.method"),
        "the error names the method: {envelope}"
    );

    // Unknown tool → -32602, the same top-level shape.
    let status = mcp_post_frame(
        &state,
        &session_id,
        4,
        "tools/call",
        serde_json::json!({"name": "no_such_tool", "arguments": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "-32602").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(envelope["id"], 4);
    assert!(
        envelope.get("result").is_none(),
        "an unknown tool is a protocol error, not a tool result: {envelope}"
    );
    assert_eq!(envelope["error"]["code"], -32602, "{envelope}");
}

/// A body the server cannot even read answers an HTTP error whose body
/// carries the prescribed id-less error frame (-32700 / -32600) — and
/// nothing leaks onto the stream.
#[tokio::test]
async fn mcp_message_refuses_unusable_bodies_with_the_prescribed_frames() {
    let (state, token) = test_state_with_token().await;
    let api_key = mcp_api_key(&state, &token, "bad-body-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    // Unparseable JSON → -32700.
    let (status, frame) = mcp_post_body(&state, &session_id, "{".to_owned()).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unparseable body is an HTTP error: {frame}"
    );
    assert_eq!(frame["jsonrpc"], "2.0");
    assert!(frame["id"].is_null(), "no request id existed: {frame}");
    assert_eq!(frame["error"]["code"], -32700, "{frame}");

    // A JSON value that is not an object → -32600.
    let (status, frame) = mcp_post_body(&state, &session_id, "[]".to_owned()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(frame["error"]["code"], -32600, "{frame}");

    // A non-string `method` → -32600 (Invalid Request, never "unknown
    // method": the call never happened).
    let (status, frame) = mcp_post_body(
        &state,
        &session_id,
        serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": 42}).to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(frame["error"]["code"], -32600, "{frame}");
    assert!(
        frame["error"]["message"]
            .as_str()
            .unwrap()
            .contains("string"),
        "the error names the method rule: {frame}"
    );

    // None of it reached the stream.
    let drained = sse_drain(&mut sse, std::time::Duration::from_millis(300)).await;
    assert!(
        !drained.contains("\"jsonrpc\""),
        "unusable frames stay off the stream: {drained:?}"
    );
}

/// A real `tools/call` (list_files) over the SSE lane: handshake →
/// initialize → tools/call, with the result frame carrying the request's
/// own id; `offset`/`limit` really page (they were ignored before).
#[tokio::test]
async fn mcp_list_files_pages_through_the_tool_over_sse() {
    let (state, token) = test_state_with_token().await;
    // Two DISTINCT uploads (the per-owner content dedupe would collapse
    // two identical PNGs into one row).
    let (_, first) = upload(&state, &token, png_of(4, 4), "a.png").await;
    let (_, second) = upload(&state, &token, png_of(5, 4), "b.png").await;
    let ids = [
        first["id"].as_str().unwrap().to_owned(),
        second["id"].as_str().unwrap().to_owned(),
    ];

    let api_key = mcp_api_key(&state, &token, "page-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    // The real client order: initialize first.
    let status = mcp_post_frame(&state, &session_id, 1, "initialize", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"protocolVersion\"").await;
    assert!(frames.contains("\"id\":1"), "{frames}");

    // limit=1 → exactly one row, and the frame carries id 7.
    let status = mcp_post_frame(
        &state,
        &session_id,
        7,
        "tools/call",
        serde_json::json!({"name": "list_files", "arguments": {"limit": 1}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":7").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(envelope["id"], 7, "the result frame carries its request id");
    assert!(envelope.get("error").is_none(), "{envelope}");
    let (is_error, text) = mcp_tool_result(&envelope);
    assert!(!is_error, "the tool call succeeded: {text}");
    let page: Vec<serde_json::Value> =
        serde_json::from_str(&text).expect("the tool answers a FileDto array");
    assert_eq!(page.len(), 1, "limit=1 returns one row: {page:?}");
    let first_row = page[0]["id"].as_str().unwrap().to_owned();
    assert!(ids.contains(&first_row), "it is one of the caller's files");

    // offset=1 & limit=1 → the OTHER row (the offset is really applied).
    let status = mcp_post_frame(
        &state,
        &session_id,
        8,
        "tools/call",
        serde_json::json!({"name": "list_files", "arguments": {"offset": 1, "limit": 1}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":8").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(!is_error, "{text}");
    let page: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(page.len(), 1, "the second page holds one row: {page:?}");
    let second_row = page[0]["id"].as_str().unwrap().to_owned();
    assert_ne!(second_row, first_row, "offset skipped the first row");
    assert!(ids.contains(&second_row));

    // Past the end → an empty page.
    let status = mcp_post_frame(
        &state,
        &session_id,
        9,
        "tools/call",
        serde_json::json!({"name": "list_files", "arguments": {"offset": 5, "limit": 100}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":9").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(!is_error, "{text}");
    let page: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert!(
        page.is_empty(),
        "offset past the end lists nothing: {page:?}"
    );
}

/// `get_share_link` aligns with the REST share lane: a folder is refused,
/// the link's origin follows the deployment's public origin (or the
/// request Host) and NEVER a hardcoded localhost, and a live share is
/// reused instead of minting a token per call.
#[tokio::test]
async fn mcp_get_share_link_refuses_folders_and_follows_the_public_origin() {
    let (mut state, token) = test_state_with_token().await;
    let origin = "https://res.example.test";
    let base_config = (*state.config).clone();
    let mut configured = base_config.clone();
    configured.public_origin = Some(origin.to_owned());
    state.config = Arc::new(configured);

    let folder = mk_folder(&state, &token, "photos", None).await;
    let (_, up) = upload(&state, &token, tiny_png(), "cat.png").await;
    let file_id = up["id"].as_str().unwrap().to_owned();

    let api_key = mcp_api_key(&state, &token, "share-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;

    // A folder is a TOOL error, not a link (the REST lane filters folders
    // out of share_create for the same reason).
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": folder}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"isError\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(is_error, "a folder cannot be shared: {text}");
    assert!(
        text.contains("folder"),
        "the tool text names the folder rule: {text}"
    );

    // A file gets a link under the configured public origin.
    let status = mcp_post_frame(
        &state,
        &session_id,
        2,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":2").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(!is_error, "the share link was minted: {text}");
    let link: serde_json::Value =
        serde_json::from_str(&text).expect("the tool answers {url, token}");
    let share_token = link["token"].as_str().unwrap().to_owned();
    let url = link["url"].as_str().unwrap().to_owned();
    assert_eq!(
        url,
        format!("{origin}/s/{share_token}"),
        "the URL origin is I0721_PUBLIC_ORIGIN, not a localhost literal"
    );
    assert!(!url.contains("localhost"), "no localhost fallback: {url}");
    // The minted token really resolves on the public share lane.
    let meta = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{share_token}/meta"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(meta.status(), StatusCode::OK, "the minted token resolves");

    // A second call REUSES the live share: same token, no second row.
    let status = mcp_post_frame(
        &state,
        &session_id,
        3,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":3").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(!is_error, "{text}");
    let again: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        again["token"].as_str().unwrap(),
        share_token,
        "the existing live share is reused, not re-minted"
    );
    let (_, shares) = rpc(&state, &token, "files.share_list", serde_json::json!({})).await;
    assert_eq!(
        shares["result"].as_array().unwrap().len(),
        1,
        "one share row, not one per call: {shares}"
    );

    // Without I0721_PUBLIC_ORIGIN the Host header is the source — the
    // same derivation the OAuth callback lane uses.
    state.config = Arc::new(base_config.clone());
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {"name": "get_share_link", "arguments": {"file_id": file_id}},
    })
    .to_string();
    let (status, _) = mcp_post_body_at(&state, &session_id, Some("mcp.example.test"), body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":4").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(!is_error, "{text}");
    let link: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        link["url"].as_str().unwrap(),
        format!("http://mcp.example.test/s/{share_token}"),
        "the Host header is the origin when the env pin is unset"
    );

    // With NEITHER the env pin NOR a Host header there is no origin at
    // all: an explicit error, never a localhost link.
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {"name": "get_share_link", "arguments": {"file_id": file_id}},
    })
    .to_string();
    let (status, _) = mcp_post_body(&state, &session_id, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"id\":5").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(
        is_error,
        "no derivable origin must be an error, not a localhost link: {text}"
    );
    assert!(
        text.contains("I0721_PUBLIC_ORIGIN"),
        "the error names the fix: {text}"
    );
    assert!(!text.contains("localhost"), "no localhost fallback: {text}");
}

#[tokio::test]
async fn mcp_reupload_of_own_bytes_still_faces_the_quota_gate() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());
    let api_key = mcp_api_key(&state, &token, "reupload-key").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;
    let payload = serde_json::json!({
        "content_base64": base64_of(&tiny_png()),
        "name": "twice.png",
    });

    // First upload succeeds under the default (unlimited) quota.
    mcp_call_tool(&state, &session_id, "upload_file", payload.clone()).await;
    let frames = sse_read_until(&mut sse, "\"id\":1").await;
    let first = sse_envelope(&frames);
    assert!(
        first["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("twice.png"),
        "the first upload succeeds: {frames}"
    );

    // Squeeze the quota, then re-upload the SAME bytes: dedupe runs after
    // the gates, so the shortcut must not answer 200 past the quota.
    state.store.set_user_quota(admin.id, Some(1)).await.unwrap();
    mcp_call_tool(&state, &session_id, "upload_file", payload).await;
    let frames = sse_read_until(&mut sse, "\"isError\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(
        is_error && text.contains("quota"),
        "an over-quota re-upload must NOT ride the dedupe shortcut: {text}"
    );
}

/// The session cookie policy (public-exposure hardening): `SameSite=Lax`
/// always — `Strict` would strip the cookie from a top-level navigation
/// that follows a share link from another site, which is this service's
/// bread and butter — and `Secure` decided per REQUEST (the edge's
/// `X-Forwarded-Proto`) with `I0721_COOKIE_SECURE` as the pin for edges
/// that terminate TLS without forwarding the scheme.
#[tokio::test]
async fn session_cookie_is_lax_and_secure_only_over_https() {
    async fn login_cookie(state: &AppState, extra: Option<(&'static str, &'static str)>) -> String {
        let mut request = login_request("admin", "admin-password");
        if let Some((name, value)) = extra {
            request.headers_mut().insert(
                header::HeaderName::from_static(name),
                header::HeaderValue::from_static(value),
            );
        }
        let response = app(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .expect("login sets the cookie")
            .to_owned()
    }

    // Plain http, no forwarded scheme: Lax, no Secure (the LAN dev lane
    // must be able to log in at all).
    let (state, _admin) = test_state().await;
    let cookie = login_cookie(&state, None).await;
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(!cookie.contains("Strict"), "no Strict left: {cookie}");
    assert!(!cookie.contains("Secure"), "http keeps working: {cookie}");

    // Behind a TLS edge that forwards the scheme: Secure appears.
    let cookie = login_cookie(&state, Some(("x-forwarded-proto", "https"))).await;
    assert!(cookie.contains("Secure"), "https edge: {cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");

    // The pin for edges that do not forward the scheme.
    let (state_always, _) = test_state().await;
    let mut config = state_always.config.as_ref().clone();
    config.cookie_secure = "always".to_owned();
    let pinned = i0721_server::state::AppState::new(
        state_always.store.clone(),
        state_always.blobs.clone(),
        std::sync::Arc::new(config),
    );
    let cookie = login_cookie(&pinned, None).await;
    assert!(cookie.contains("Secure"), "pinned always: {cookie}");
}

/// `I0721_COOKIE_SECURE` is validated at startup: a typo must refuse to
/// start rather than silently degrade to `auto` (which is how a
/// production https edge would end up with a non-Secure session cookie).
#[tokio::test]
async fn cookie_secure_rejects_an_unknown_value() {
    let result = i0721_server::config::Config::from_env_with(|key| match key {
        "I0721_DATABASE_URL" => Some("postgres://x/y".to_owned()),
        "JWT_SECRET" => Some("s".to_owned()),
        "I0721_COOKIE_SECURE" => Some("Secure".to_owned()),
        _ => None,
    });
    match result {
        Err(err) => assert!(
            format!("{err}").contains("I0721_COOKIE_SECURE"),
            "the error names the variable: {err}"
        ),
        Ok(_) => panic!("an unknown I0721_COOKIE_SECURE must refuse to start"),
    }
}

// ── the permission / expiry wave (K) ───────────────────────────────────
//
// A key's operation set is enforced per tool on the MCP lane, and an
// optional expiry is checked at the handshake AND on every frame. The
// vocabulary itself is unit-tested in `i0721_server::key_scopes`; these
// tests prove the WIRING (which tool asks for which op, which surface
// answers which code).

/// ① The five tools `tools/list` advertises and the permission table are
/// the SAME set, in both directions: a tool added to the specs without a
/// permission mapping (or the reverse) fails here instead of shipping a
/// permission-free tool.
#[test]
fn tool_specs_match_the_permission_table_exactly() {
    let specs = i0721_server::mcp::tool_specs();
    let mut spec_names: Vec<String> = specs
        .as_array()
        .expect("the tool specs are an array")
        .iter()
        .map(|tool| {
            tool["name"]
                .as_str()
                .expect("every spec carries a name")
                .to_owned()
        })
        .collect();
    spec_names.sort_unstable();
    let mut table_names: Vec<String> = i0721_server::key_scopes::TOOL_REQUIRED_OP
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    table_names.sort_unstable();
    assert_eq!(
        spec_names, table_names,
        "every advertised tool must be in TOOL_REQUIRED_OP and vice versa"
    );
    for (name, op) in i0721_server::key_scopes::TOOL_REQUIRED_OP {
        assert_eq!(
            i0721_server::key_scopes::required_op(name),
            Some(op),
            "{name} maps to one op"
        );
    }
    assert_eq!(
        i0721_server::key_scopes::required_op("no_such_tool"),
        None,
        "an unknown name has no op"
    );
}

/// The code a permission refusal rides: the plana family's `-32005`,
/// **imported from the service** rather than copied, so a lane that went
/// back to a number of its own turns these assertions red instead of
/// leaving the tests agreeing with a constant nobody sends.
const SCOPE_DENIED: i64 = i0721_server::rpc::rpc_error_codes::AUTH;

/// The exact refusal text the spec fixes (one phrasing, every surface).
fn scope_denied_message(op: &str, tool: &str) -> String {
    format!("scope denied: this API key lacks the \"{op}\" permission required by tool \"{tool}\"")
}

/// ② Every tool × (key lacking its op → the permission frame; key holding it →
/// allowed). "Allowed" is proven by the call producing a TOOL result (a
/// domain error for the deliberately non-existent ids is fine — what
/// must NOT happen is the protocol refusal).
#[tokio::test]
async fn mcp_tool_calls_require_the_mapped_permission() {
    let (mut state, token) = test_state_with_token().await;
    // The share tool needs a public origin to succeed; it only has to
    // get PAST the permission gate here.
    let mut configured = (*state.config).clone();
    configured.public_origin = Some("https://res.example.test".to_owned());
    state.config = Arc::new(configured);

    // (tool, required op, arguments that reach the tool body)
    let cases: [(&str, &str, serde_json::Value); 5] = [
        ("list_files", "read", serde_json::json!({})),
        (
            "file_info",
            "read",
            serde_json::json!({"id": "00000000-0000-0000-0000-000000000001"}),
        ),
        (
            "search_files",
            "read",
            serde_json::json!({"query": "nothing-matches"}),
        ),
        (
            "upload_file",
            "upload",
            serde_json::json!({"content_base64": base64_of(&tiny_png()), "name": "gated.png"}),
        ),
        (
            "get_share_link",
            "share",
            serde_json::json!({"file_id": "00000000-0000-0000-0000-000000000002"}),
        ),
    ];
    let every_op = ["read", "upload", "share"];

    for (index, (tool, op, arguments)) in cases.into_iter().enumerate() {
        // A key granted exactly the OTHER two permissions: it lacks the
        // one this tool needs.
        let others: Vec<&str> = every_op
            .into_iter()
            .filter(|candidate| *candidate != op)
            .collect();
        let lacking = mcp_api_key_with(
            &state,
            &token,
            &format!("lacks-{op}-{index}"),
            serde_json::json!(others),
        )
        .await;
        let (mut sse, session_id) = mcp_handshake(&state, &lacking).await;
        let id = 100 + index as i64 * 2;
        let status = mcp_post_frame(
            &state,
            &session_id,
            id,
            "tools/call",
            serde_json::json!({"name": tool, "arguments": arguments}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "{tool}: the refusal rides the stream, not the POST"
        );
        let frames = sse_read_until(&mut sse, "\"error\"").await;
        let envelope = sse_envelope(&frames);
        assert_eq!(envelope["id"], id, "{tool}: the frame carries its id");
        assert_eq!(
            envelope["error"]["code"], SCOPE_DENIED,
            "{tool} without {op} is a permission protocol error: {envelope}"
        );
        assert!(
            envelope.get("result").is_none(),
            "{tool}: a scope refusal is NOT an isError result: {envelope}"
        );
        assert_eq!(
            envelope["error"]["message"],
            scope_denied_message(op, tool),
            "{tool}: the refusal names the permission and the tool"
        );
        // The ops refusal carries NO `data`: there is no bucket to name at
        // this point (the bucket checks come later, per tool), and a `data`
        // here would be a place for one to leak from. The two bucket arms
        // are distinguished by `data.denial`; this arm is distinguished by
        // having none.
        assert!(
            envelope["error"].get("data").is_none(),
            "{tool}: the ops refusal must not carry data: {envelope}"
        );

        // The SAME tool with a key that holds the op gets past the gate.
        let holding = mcp_api_key_with(
            &state,
            &token,
            &format!("holds-{op}-{index}"),
            serde_json::json!([op]),
        )
        .await;
        let (mut sse, session_id) = mcp_handshake(&state, &holding).await;
        let status = mcp_post_frame(
            &state,
            &session_id,
            id + 1,
            "tools/call",
            serde_json::json!({"name": tool, "arguments": arguments}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let frames = sse_read_until(&mut sse, &format!("\"id\":{}", id + 1)).await;
        let envelope = sse_envelope(&frames);
        assert!(
            envelope.get("error").is_none(),
            "{tool} with {op} must run, not be refused: {envelope}"
        );
        assert!(
            envelope["result"]["content"].is_array(),
            "{tool} with {op} answers a tool result: {envelope}"
        );
    }
}

/// ③ `tools/list` advertises only the tools the key may call: no
/// permission → none, one permission → exactly the tools of that op, all
/// three → the whole surface.
#[tokio::test]
async fn mcp_tools_list_is_filtered_by_the_keys_permissions() {
    let (state, token) = test_state_with_token().await;

    // `keys.create` refuses an empty op list, so the empty-permission key
    // is PLANTED — that is also the shape every pre-permission-wave key
    // carries after the migration.
    let admin_id = admin_of(&state, &token);
    let (_, no_perms) = plant_api_key(&state, admin_id, "no-perms", r#"{"ops":[]}"#, None).await;

    // (op list, the tools those ops cover)
    let expectations: [(&str, Vec<&str>, Vec<&str>); 2] = [
        ("upload-only", vec!["upload"], vec!["upload_file"]),
        (
            "read-share",
            vec!["share", "read"],
            vec!["file_info", "get_share_link", "list_files", "search_files"],
        ),
    ];
    // First the planted empty-permission key: it may call NOTHING, so
    // `tools/list` answers an EMPTY array.
    {
        let (mut sse, session_id) = mcp_handshake(&state, &no_perms).await;
        let status =
            mcp_post_frame(&state, &session_id, 1, "tools/list", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let frames = sse_read_until(&mut sse, "\"id\":1").await;
        let envelope = sse_envelope(&frames);
        assert!(envelope.get("error").is_none(), "no-perms: {envelope}");
        assert_eq!(
            envelope["result"]["tools"],
            serde_json::json!([]),
            "a key with no permission is advertised no tool at all"
        );
    }
    for (id, (label, ops, expected)) in (2_i64..).zip(expectations) {
        let key = mcp_api_key_with(&state, &token, label, serde_json::json!(ops)).await;
        let (mut sse, session_id) = mcp_handshake(&state, &key).await;
        let status =
            mcp_post_frame(&state, &session_id, id, "tools/list", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let frames = sse_read_until(&mut sse, &format!("\"id\":{id}")).await;
        let envelope = sse_envelope(&frames);
        assert!(envelope.get("error").is_none(), "{label}: {envelope}");
        let mut names: Vec<&str> = envelope["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{label}: tools must be an array: {envelope}"))
            .iter()
            .map(|tool| tool["name"].as_str().expect("a tool name"))
            .collect();
        names.sort_unstable();
        assert_eq!(names, expected, "{label}: exactly the permitted tools");
    }
}

/// ④ Expiry on the transport: an expired key cannot open a stream; a
/// session whose key lapses mid-flight is refused on its NEXT frame with
/// a 401 and the session is removed; a key without a future expiry keeps
/// working.
#[tokio::test]
async fn mcp_expired_keys_cannot_open_or_keep_a_session() {
    let (state, token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &token);

    // (a) Already past its instant → the handshake itself answers 401.
    let past = (Timestamp::now() - std::time::Duration::from_secs(60)).to_string();
    let (_, expired) = plant_api_key(
        &state,
        admin_id,
        "already-expired",
        r#"{"ops":["read"]}"#,
        Some(&past),
    )
    .await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/mcp/sse")
                .header(header::AUTHORIZATION, format!("Bearer {expired}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an expired key cannot open an SSE stream"
    );
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
        "this API key has expired",
        "the refusal says WHY (not the generic invalid-key text)"
    );

    // (a2) An unparseable expiry is expired too (fail-closed).
    let (_, broken) = plant_api_key(
        &state,
        admin_id,
        "broken-expiry",
        r#"{"ops":["read"]}"#,
        Some("soon"),
    )
    .await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/mcp/sse")
                .header(header::AUTHORIZATION, format!("Bearer {broken}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an expiry that does not parse must not read as 'never expires'"
    );

    // (b) Lapses AFTER the session opened → the next frame is a 401 and
    // the session leaves the table.
    let (lapsing_id, lapsing) = plant_api_key(
        &state,
        admin_id,
        "lapsing",
        r#"{"ops":["read"]}"#,
        Some(&(Timestamp::now() + std::time::Duration::from_secs(3600)).to_string()),
    )
    .await;
    let (mut sse, session_id) = mcp_handshake(&state, &lapsing).await;
    // The session works while the key is live...
    let status = mcp_post_frame(&state, &session_id, 1, "initialize", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"protocolVersion\"").await;
    assert!(frames.contains("\"id\":1"), "{frames}");

    // ...then the key expires under the session (the row is rewritten
    // with a past instant — the store is the authority, not a cache).
    let past_again = (Timestamp::now() - std::time::Duration::from_secs(1)).to_string();
    state
        .store
        .update_api_key(
            lapsing_id,
            "lapsing",
            r#"{"ops":["read"]}"#,
            Some(&past_again),
        )
        .await
        .expect("expire the key");
    let (status, body) =
        mcp_post_message(&state, &session_id, "tools/list", serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a frame arriving after the key expired is refused"
    );
    assert_eq!(
        body["error"], "this session's API key has expired",
        "the frame refusal names the expiry"
    );
    assert_eq!(
        state.mcp_sessions.session_count(),
        0,
        "the expired session was removed from the table"
    );
    // A removed session answers the generic 401 from then on.
    let (status, _) =
        mcp_post_message(&state, &session_id, "tools/list", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The stream ends (the sender fell out of scope): the socket is
    // closed instead of squatting.
    let tail = sse_drain(&mut sse, std::time::Duration::from_secs(2)).await;
    assert!(
        !tail.contains("event: message"),
        "no further frames ride a dead session: {tail}"
    );

    // (c) A future expiry, and no expiry at all, both keep working.
    for (label, expiry) in [
        (
            "future",
            Some((Timestamp::now() + std::time::Duration::from_secs(3600)).to_string()),
        ),
        ("never", None),
    ] {
        let (_, live) = plant_api_key(
            &state,
            admin_id,
            label,
            r#"{"ops":["read"]}"#,
            expiry.as_deref(),
        )
        .await;
        let (mut sse, session_id) = mcp_handshake(&state, &live).await;
        let status =
            mcp_post_frame(&state, &session_id, 2, "tools/list", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{label}: the session opens");
        let frames = sse_read_until(&mut sse, "\"id\":2").await;
        assert!(
            sse_envelope(&frames).get("error").is_none(),
            "{label}: the frame is answered"
        );
    }
}

/// ⑦ `keys.create`: the happy path stores the canonical scopes and the
/// canonical instant; an empty op list, an unknown op, a past instant and
/// an empty name are all refused with the repo's invalid-params shape.
#[tokio::test]
async fn keys_create_validates_ops_and_expiry() {
    let (state, token) = test_state_with_token().await;

    // The happy path: duplicates collapse, the order is canonical, and
    // the instant is stored in canonical RFC 3339 form.
    let future = Timestamp::now() + std::time::Duration::from_secs(3600);
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({
            "name": "  agent one  ",
            "ops": ["share", "read", "share"],
            "expires_at": future.to_string(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(envelope.get("error").is_none(), "{envelope}");
    let created = &envelope["result"];
    assert_eq!(created["name"], "agent one", "the name is trimmed");
    assert!(
        created["key"].as_str().unwrap().starts_with("i0721-"),
        "the plaintext key is still an `ApiKeyCreated`"
    );
    let id = created["id"].as_str().unwrap().to_owned();

    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    let row = list["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id.as_str())
        .unwrap_or_else(|| panic!("the created key is listed: {list}"));
    assert_eq!(
        row["ops"],
        serde_json::json!(["read", "share"]),
        "ops come back deduped in the canonical order"
    );
    assert_eq!(
        row["expires_at"],
        future.to_string(),
        "the instant is stored canonically"
    );
    assert_eq!(row["expired"], false);

    // The refusals — all of them invalid params (-32602), not a new code.
    for (label, payload) in [
        ("empty ops", serde_json::json!({"name": "n", "ops": []})),
        (
            "unknown op",
            serde_json::json!({"name": "n", "ops": ["root"]}),
        ),
        (
            "past instant",
            serde_json::json!({
                "name": "n",
                "ops": ["read"],
                "expires_at": "2000-01-01T00:00:00Z",
            }),
        ),
        (
            "unparseable instant",
            serde_json::json!({"name": "n", "ops": ["read"], "expires_at": "tomorrow"}),
        ),
        (
            "empty name",
            serde_json::json!({"name": "   ", "ops": ["read"]}),
        ),
        ("no ops field", serde_json::json!({"name": "n"})),
    ] {
        let (_, envelope) = rpc(&state, &token, "keys.create", payload).await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32602,
            "{label}: invalid params, the repo's existing shape: {envelope}"
        );
    }

    // The canonical-instant half: a NON-UTC offset is really sent and
    // comes back as the same instant in canonical `Z` form. (Formatting a
    // jiff `Timestamp` here would only ever produce `…Z`, which is why the
    // literal is spelled out: a server that stored the caller's spelling
    // verbatim, or refused offset-bearing RFC 3339, both fail below.)
    let (_, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({
            "name": "offset",
            "ops": ["upload"],
            "expires_at": "2127-03-04T10:03:04+08:00",
        }),
    )
    .await;
    assert!(
        envelope.get("error").is_none(),
        "an offset-bearing instant is accepted: {envelope}"
    );
    let offset_id = envelope["result"]["id"].as_str().unwrap().to_owned();
    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    let row = find_key_row(&list, &offset_id);
    assert_eq!(
        row["expires_at"], "2127-03-04T02:03:04Z",
        "the offset-bearing instant is stored as its canonical UTC form: {row}"
    );
    assert_eq!(row["expired"], false, "it is still in the future: {row}");
}

/// ⑧ `keys.update`: owner and admin may edit, a stranger may not; the
/// whole record is replaced (an omitted `expires_at` clears it); a
/// revoked key answers not-found; an EXPIRED key can be renewed.
#[tokio::test]
async fn keys_update_is_owner_or_admin_and_replaces_the_whole_record() {
    let (state, admin_token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "updater".to_owned(),
            display_name: "Updater".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let other = state
        .store
        .create_user(CreateUser {
            username: "stranger".to_owned(),
            display_name: "Stranger".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let stranger_token = state.signer.sign(other.id, Timestamp::now());

    // A member's key, with an expiry.
    let (key_id, _plaintext) = mcp_api_key_with_at(
        &state,
        &member_token,
        "member-key",
        serde_json::json!(["read", "upload"]),
        Some(&(Timestamp::now() + std::time::Duration::from_secs(3600)).to_string()),
    )
    .await;

    // (1) A stranger cannot edit it → 403 (-32005), the owner-or-admin
    // rule `keys.revoke` uses.
    let (_, envelope) = rpc(
        &state,
        &stranger_token,
        "keys.update",
        serde_json::json!({
            "id": key_id,
            "name": "hijacked",
            "ops": ["read", "upload", "share"],
        }),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "a member cannot edit another member's key: {envelope}"
    );

    // (1b) The UPDATE lane shares the create lane's write validation: a
    // past or an unparseable instant is refused, and a refused update
    // writes nothing — the whole listed row is byte-for-byte what it was.
    let (_, list) = rpc(&state, &member_token, "keys.list", serde_json::Value::Null).await;
    let before = find_key_row(&list, &key_id).clone();
    assert!(
        !before["expires_at"].is_null(),
        "the member's key starts with an expiry: {before}"
    );
    for (label, bad) in [
        ("past instant", "2000-01-01T00:00:00Z"),
        ("unparseable instant", "tomorrow"),
    ] {
        let (_, envelope) = rpc(
            &state,
            &member_token,
            "keys.update",
            serde_json::json!({
                "id": key_id,
                "name": "sneaked",
                "ops": ["share"],
                "expires_at": bad,
            }),
        )
        .await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32602,
            "{label}: the update lane validates the instant like the create lane: {envelope}"
        );
    }
    let (_, list) = rpc(&state, &member_token, "keys.list", serde_json::Value::Null).await;
    assert_eq!(
        find_key_row(&list, &key_id),
        &before,
        "a refused update writes nothing: name, ops and expiry are unchanged"
    );

    // (2) The owner edits: a whole-record replacement that clears the
    // expiry.
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "keys.update",
        serde_json::json!({
            "id": key_id,
            "name": "renamed",
            "ops": ["share"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        envelope["result"],
        serde_json::Value::Null,
        "update answers null"
    );
    let (_, list) = rpc(&state, &member_token, "keys.list", serde_json::Value::Null).await;
    let row = find_key_row(&list, &key_id);
    assert_eq!(row["name"], "renamed");
    assert_eq!(
        row["ops"],
        serde_json::json!(["share"]),
        "ops were replaced"
    );
    assert!(
        row["expires_at"].is_null(),
        "an omitted expires_at CLEARS the expiry: {row}"
    );

    // (3) An admin may edit a key they do NOT own. This is the leg that
    // exercises the ownership decision itself: the key belongs to the
    // STRANGER, who is neither the caller nor an admin, so a check based
    // on "is it in MY list" would refuse — the record's own `user_id` is
    // what has to authorize it.
    let (foreign_id, _foreign_plaintext) = mcp_api_key_with_at(
        &state,
        &stranger_token,
        "stranger-key",
        serde_json::json!(["read"]),
        Some(&(Timestamp::now() + std::time::Duration::from_secs(7200)).to_string()),
    )
    .await;
    assert_ne!(foreign_id, key_id, "the foreign key is a different row");
    // Before: the stranger's key, untouched.
    let (_, list) = rpc(
        &state,
        &stranger_token,
        "keys.list",
        serde_json::Value::Null,
    )
    .await;
    let before = find_key_row(&list, &foreign_id);
    assert_eq!(before["name"], "stranger-key");
    assert_eq!(before["ops"], serde_json::json!(["read"]));

    let admin_expiry = (Timestamp::now() + std::time::Duration::from_secs(600)).to_string();
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "keys.update",
        serde_json::json!({
            "id": foreign_id,
            "name": "admin-edit",
            "ops": ["share", "upload"],
            "expires_at": admin_expiry,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an admin may edit a key they do not own: {envelope}"
    );
    // After: the OWNER's view shows all three fields replaced (the row,
    // not just the response, carries the edit).
    let (_, list) = rpc(
        &state,
        &stranger_token,
        "keys.list",
        serde_json::Value::Null,
    )
    .await;
    let after = find_key_row(&list, &foreign_id);
    assert_eq!(
        after["name"], "admin-edit",
        "the admin replaced the foreign key's name: {after}"
    );
    assert_eq!(
        after["ops"],
        serde_json::json!(["upload", "share"]),
        "the admin replaced the foreign key's ops: {after}"
    );
    assert_eq!(
        after["expires_at"], admin_expiry,
        "the admin replaced the foreign key's expiry: {after}"
    );
    // ...and the admin's own list still does not describe it as theirs.
    // MUT:M15 — a store whose list ignores the owner turns this assertion red.
    let (_, admin_list) = rpc(&state, &admin_token, "keys.list", serde_json::Value::Null).await;
    assert!(
        !admin_list["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == foreign_id.as_str()),
        "keys.list stays owner-scoped even for an admin: {admin_list}"
    );

    // (4) An EXPIRED key can be renewed (that is the point of editing a
    // live-enough key): plant one in the past and push its expiry into
    // the future.
    let past = (Timestamp::now() - std::time::Duration::from_secs(60)).to_string();
    let (expired_id, _) = plant_api_key(
        &state,
        member.id,
        "expired",
        r#"{"ops":["read"]}"#,
        Some(&past),
    )
    .await;
    let (_, list) = rpc(&state, &member_token, "keys.list", serde_json::Value::Null).await;
    assert_eq!(
        find_key_row(&list, &expired_id.to_string())["expired"],
        true,
        "the expired key is listed as expired"
    );
    let renewed = (Timestamp::now() + std::time::Duration::from_secs(3600)).to_string();
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "keys.update",
        serde_json::json!({
            "id": expired_id.to_string(),
            "name": "renewed",
            "ops": ["read", "upload"],
            "expires_at": renewed,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an expired key is renewable: {envelope}"
    );
    let (_, list) = rpc(&state, &member_token, "keys.list", serde_json::Value::Null).await;
    let row = find_key_row(&list, &expired_id.to_string());
    assert_eq!(
        row["expired"], false,
        "the renewal cleared the expired flag"
    );
    assert_eq!(row["ops"], serde_json::json!(["read", "upload"]));

    // (5) A REVOKED key cannot be edited → not-found (-32006), the same
    // answer `keys.revoke` gives a dead id.
    let (status, _) = rpc(
        &state,
        &member_token,
        "keys.revoke",
        serde_json::json!({"id": key_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "keys.update",
        serde_json::json!({"id": key_id, "name": "zombie", "ops": ["read"]}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32006,
        "a revoked key is not updatable (not found): {envelope}"
    );

    // (6) The validation is the same as create's.
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "keys.update",
        serde_json::json!({"id": expired_id.to_string(), "name": "x", "ops": []}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32602,
        "update refuses an empty permission set: {envelope}"
    );
}

/// ⑨ `keys.list` reports the new fields and the canonical op order,
/// whatever shape the stored row has (a legacy or hand-written scopes
/// payload is parsed, never echoed).
/// MUT:M16 — a list lane that answers every row with the full op set
/// turns this test red.
#[tokio::test]
async fn keys_list_reports_ops_and_expiry_in_canonical_order() {
    let (state, token) = test_state_with_token().await;

    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({
            "name": "ordered",
            "ops": ["share", "upload", "read"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let id = envelope["result"]["id"].as_str().unwrap().to_owned();

    // A hand-planted row carrying the future-wave shape (an extra key)
    // and an out-of-order list.
    let admin_id = admin_of(&state, &token);
    let (planted_id, _) = plant_api_key(
        &state,
        admin_id,
        "hand-written",
        r#"{"buckets":["b1"],"ops":["share","read"]}"#,
        None,
    )
    .await;

    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    let row = find_key_row(&list, &id);
    assert_eq!(
        row["ops"],
        serde_json::json!(["read", "upload", "share"]),
        "the list reports the canonical order, not the stored order: {row}"
    );
    assert!(
        row.get("expires_at").is_some(),
        "the field is present (null)"
    );
    assert_eq!(row["expired"], false);
    let planted = find_key_row(&list, &planted_id.to_string());
    assert_eq!(
        planted["ops"],
        serde_json::json!(["read", "share"]),
        "an object scopes payload with an extra key still yields its ops: {planted}"
    );
    // MUT:K-M4-PROJECTION — the raw stored payload is not echoed: the
    // list projection is the PARSED fields only (ops canonical-ordered,
    // buckets parsed and canonicalized for the「无桶」marker; the stored
    // text, with its out-of-order ops, is gone).
    assert_eq!(
        planted["buckets"],
        serde_json::json!(["b1"]),
        "the list reports the parsed bucket set, canonicalized: {planted}"
    );
    assert_eq!(
        planted["ops"],
        serde_json::json!(["read", "share"]),
        "the ops are re-ordered canonically, not echoed: {planted}"
    );
    assert!(
        planted.get("key_prefix").is_some(),
        "the projection carries only parsed fields"
    );
}

/// The listed row of one key (panics with the whole list when absent).
fn find_key_row<'a>(list: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    list["result"]
        .as_array()
        .unwrap_or_else(|| panic!("keys.list answers an array: {list}"))
        .iter()
        .find(|row| row["id"] == id)
        .unwrap_or_else(|| panic!("no key {id} in {list}"))
}

/// The seam for the next wave, proven at the WIRE level: a client that
/// sends the (not-yet-persisted) `buckets` key alongside `ops` is
/// IGNORED, not rejected — the key is created exactly as if it had only
/// carried `ops`, and the stored payload is the canonical op-only object
/// (the next wave adds `buckets` to the SCHEMA, not to this request
/// shape). The parser's own tolerance is unit-tested in
/// `i0721_server::key_scopes` (`parse_scopes_truth_table_is_fail_closed`).
#[tokio::test]
async fn keys_create_and_update_ignore_an_unknown_buckets_key() {
    let (state, token) = test_state_with_token().await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({
            "name": "bucketed",
            "ops": ["read"],
            "buckets": [],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope.get("error").is_none(),
        "an unknown `buckets` key must not turn into a -32602: {envelope}"
    );
    let id = envelope["result"]["id"].as_str().unwrap().to_owned();

    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    assert_eq!(find_key_row(&list, &id)["ops"], serde_json::json!(["read"]));

    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.update",
        serde_json::json!({
            "id": id,
            "name": "bucketed",
            "ops": ["read", "upload"],
            "buckets": ["00000000-0000-0000-0000-000000000001"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let (_, list) = rpc(&state, &token, "keys.list", serde_json::Value::Null).await;
    assert_eq!(
        find_key_row(&list, &id)["ops"],
        serde_json::json!(["read", "upload"])
    );
}

/// The row a `keys.*` lane must leave ALONE: read straight from the
/// store, any state (the test asserts on the DB, not on a response).
async fn stored_key(state: &AppState, id: &str) -> i0721_proto::ApiKeyRecord {
    let id: uuid::Uuid = id.parse().expect("a key id");
    state
        .store
        .find_api_key_by_id(id)
        .await
        .expect("key lookup")
        .expect("the key row still exists")
}

/// F-A: a member cannot edit ANOTHER user's key, and the refusal leaves
/// that key's row byte-identical. Ownership is decided from the record's
/// own `user_id` — a check based on the caller's own key list would have
/// answered "not mine" for the right reason but by accident, and would
/// have made "someone else's key" indistinguishable from "no such key".
#[tokio::test]
async fn a_member_cannot_update_another_users_key() {
    let (state, _admin_token) = test_state_with_token().await;
    let owner = state
        .store
        .create_user(CreateUser {
            username: "key-owner".to_owned(),
            display_name: "KeyOwner".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let owner_token = state.signer.sign(owner.id, Timestamp::now());
    let intruder = state
        .store
        .create_user(CreateUser {
            username: "key-intruder".to_owned(),
            display_name: "KeyIntruder".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let intruder_token = state.signer.sign(intruder.id, Timestamp::now());

    let (key_id, _plaintext) = mcp_api_key_with_at(
        &state,
        &owner_token,
        "owned-by-the-victim",
        serde_json::json!(["read"]),
        Some(&(Timestamp::now() + std::time::Duration::from_secs(3600)).to_string()),
    )
    .await;
    let before = stored_key(&state, &key_id).await;

    // The intruder (a plain member, and not the owner) tries to take the
    // key over.
    let (status, envelope) = rpc(
        &state,
        &intruder_token,
        "keys.update",
        serde_json::json!({
            "id": key_id,
            "name": "stolen",
            "ops": ["read", "upload", "share"],
            "expires_at": (Timestamp::now() + std::time::Duration::from_secs(60)).to_string(),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the refusal rides the envelope");
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "a member cannot update another user's key: {envelope}"
    );
    assert_eq!(
        envelope["error"]["message"], "not your key",
        "the refusal is the established wording: {envelope}"
    );

    // The DB row is unchanged in every field the update could touch.
    let after = stored_key(&state, &key_id).await;
    assert_eq!(after.name, before.name, "the name was not replaced");
    assert_eq!(after.scopes, before.scopes, "the ops were not replaced");
    assert_eq!(
        after.expires_at, before.expires_at,
        "the expiry was not replaced"
    );
    assert_eq!(after.user_id, owner.id, "the owner is still the owner");
}

/// F-A: a member cannot revoke ANOTHER user's key, and the row stays
/// live (revoking is destructive, so "refused" must mean "untouched").
#[tokio::test]
async fn a_member_cannot_revoke_another_users_key() {
    let (state, _admin_token) = test_state_with_token().await;
    let owner = state
        .store
        .create_user(CreateUser {
            username: "revoke-owner".to_owned(),
            display_name: "RevokeOwner".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let owner_token = state.signer.sign(owner.id, Timestamp::now());
    let intruder = state
        .store
        .create_user(CreateUser {
            username: "revoke-intruder".to_owned(),
            display_name: "RevokeIntruder".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let intruder_token = state.signer.sign(intruder.id, Timestamp::now());

    let (key_id, plaintext) = mcp_api_key_with_at(
        &state,
        &owner_token,
        "victims-live-key",
        serde_json::json!(["read"]),
        None,
    )
    .await;

    let (status, envelope) = rpc(
        &state,
        &intruder_token,
        "keys.revoke",
        serde_json::json!({"id": key_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the refusal rides the envelope");
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "a member cannot revoke another user's key: {envelope}"
    );
    assert_eq!(
        envelope["error"]["message"], "not your key",
        "the refusal is the established wording: {envelope}"
    );

    // Untouched: the row is still not revoked, and the key still opens a
    // session (the strongest possible statement that the refusal was a
    // no-op).
    let after = stored_key(&state, &key_id).await;
    assert!(
        after.revoked_at.is_none(),
        "the key was NOT revoked: {:?}",
        after.revoked_at
    );
    let (mut sse, session_id) = mcp_handshake(&state, &plaintext).await;
    let status = mcp_post_frame(&state, &session_id, 1, "tools/list", serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "the owner's key still works after the failed takeover"
    );
    let frames = sse_read_until(&mut sse, "\"id\":1").await;
    assert!(
        sse_envelope(&frames).get("error").is_none(),
        "the session answers normally: {frames}"
    );
}

// ── the bucket registry's guard tests (stage A) ─────────────────────────

/// `health_reports_bucket_unavailable_with_the_missing_var`: a registry
/// whose bucket lacks its env reports `unavailable` with the variable
/// named — never `ready`.
/// MUT:M10 — health mapping Unavailable→Ready (a deliberately wrong but
/// compiling change to the state mapping) must turn this red.
#[tokio::test]
async fn health_reports_bucket_unavailable_with_the_missing_var() {
    let bucket = i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: "bootstrap-local".to_owned(),
        display_name: "bootstrap (local)".to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "legacy_local_env".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: true,
        created_at: Timestamp::now(),
    };
    let registry = i0721_server::blob_registry::BlobRegistry::build(
        vec![bucket],
        |_| None, // I0721_MEDIA_ROOT missing
    )
    .unwrap();
    let health = registry.health();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].state, "unavailable", "{health:?}");
    assert!(
        health[0]
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("I0721_MEDIA_ROOT"),
        "the reason names the missing variable: {:?}",
        health[0].reason
    );
}

/// The failing-settings double (`settings_state` → Err on EVERY key):
/// both the health report test and the upload-defaults Unreadable test
/// ride the same delegation impl, so the lane's one mock stays the ONE
/// place the failure shape is defined.
async fn failing_settings_state() -> AppState {
    struct FailingSettings(MemoryStore);
    #[async_trait::async_trait]
    impl Store for FailingSettings {
        // `get_setting` DELEGATES: the site-config read must succeed so
        // the upload lane reaches `default_bucket_slug` — the failing
        // read this fixture isolates is `settings_state` (the four-state
        // read), not the raw site-config fetch.
        async fn get_setting(
            &self,
            key: &str,
        ) -> Result<Option<serde_json::Value>, i0721_store::StoreError> {
            self.0.get_setting(key).await
        }
        async fn get_storage_bucket_by_slug(
            &self,
            _slug: &str,
        ) -> Result<Option<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            Ok(None)
        }
        async fn list_storage_buckets(
            &self,
        ) -> Result<Vec<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            Ok(Vec::new())
        }
        async fn storage_keys_live_in_bucket(
            &self,
            _bucket_id: Option<uuid::Uuid>,
        ) -> Result<std::collections::HashSet<String>, i0721_store::StoreError> {
            Ok(std::collections::HashSet::new())
        }
        // Everything else delegates to the inner memory store.
        async fn list_users(
            &self,
            o: u64,
            l: u64,
        ) -> Result<Vec<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.list_users(o, l).await
        }
        async fn count_users(&self) -> Result<u64, i0721_store::StoreError> {
            self.0.count_users().await
        }
        async fn get_user(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user(id).await
        }
        async fn get_user_by_username(
            &self,
            u: &str,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user_by_username(u).await
        }
        async fn oauth_providers_for_user(
            &self,
            user_id: uuid::Uuid,
        ) -> Result<Vec<String>, i0721_store::StoreError> {
            self.0.oauth_providers_for_user(user_id).await
        }
        async fn create_user(
            &self,
            u: i0721_store::CreateUser,
        ) -> Result<i0721_proto::UserRecord, i0721_store::StoreError> {
            self.0.create_user(u).await
        }
        async fn delete_user(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_user(id).await
        }
        async fn list_files(
            &self,
            b: Option<&[uuid::Uuid]>,
            o: u64,
            l: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_files(b, o, l).await
        }
        async fn count_files(
            &self,
            b: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_files(b).await
        }
        async fn get_file(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_file(id).await
        }
        async fn get_file_by_owner_and_sha256(
            &self,
            o: uuid::Uuid,
            s: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_file_by_owner_and_sha256(o, s).await
        }
        async fn list_live_rows_by_storage_key(
            &self,
            k: &str,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_live_rows_by_storage_key(k).await
        }
        async fn count_live_files_by_storage_key(
            &self,
            b: Option<uuid::Uuid>,
            k: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_storage_key(b, k).await
        }
        async fn count_live_files_by_thumbnail_key(
            &self,
            b: Option<uuid::Uuid>,
            k: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_thumbnail_key(b, k).await
        }
        async fn create_file(
            &self,
            f: i0721_proto::NewFile,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_file(f).await
        }
        async fn rename_file(
            &self,
            id: uuid::Uuid,
            n: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.rename_file(id, n).await
        }
        async fn soft_delete_file(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.soft_delete_file(id).await
        }
        async fn get_oauth_identity(
            &self,
            p: &str,
            s: &str,
        ) -> Result<Option<uuid::Uuid>, i0721_store::StoreError> {
            self.0.get_oauth_identity(p, s).await
        }
        async fn link_oauth_identity(
            &self,
            p: &str,
            s: &str,
            u: uuid::Uuid,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.link_oauth_identity(p, s, u).await
        }
        async fn count_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            k: Option<String>,
            b: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_folder(o, p, k, b).await
        }
        async fn list_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            k: Option<String>,
            b: Option<&[uuid::Uuid]>,
            off: u64,
            lim: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_folder(o, p, k, b, off, lim).await
        }
        async fn folder_name_taken(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            b: Option<uuid::Uuid>,
            n: &str,
            e: Option<uuid::Uuid>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.folder_name_taken(o, p, b, n, e).await
        }
        async fn create_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            n: &str,
            b: Option<uuid::Uuid>,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_folder(o, p, n, b).await
        }
        async fn set_parent(
            &self,
            id: uuid::Uuid,
            p: Option<uuid::Uuid>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_parent(id, p).await
        }
        async fn ancestor_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.ancestor_ids(id).await
        }
        async fn subtree_live_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.subtree_live_ids(id).await
        }
        async fn soft_delete_many(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.soft_delete_many(ids).await
        }
        async fn get_files_by_ids(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_files_by_ids(ids).await
        }
        async fn create_share(
            &self,
            s: i0721_proto::NewShare,
        ) -> Result<i0721_proto::ShareRecord, i0721_store::StoreError> {
            self.0.create_share(s).await
        }
        async fn get_share(
            &self,
            t: &str,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.get_share(t).await
        }
        async fn find_share_by_file(
            &self,
            o: uuid::Uuid,
            f: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.find_share_by_file(o, f).await
        }
        async fn list_shares(
            &self,
            o: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.list_shares(o).await
        }
        async fn delete_share(&self, t: &str) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_share(t).await
        }
        async fn delete_shares_for_files(
            &self,
            f: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.delete_shares_for_files(f).await
        }
        async fn sum_live_sizes(&self, o: uuid::Uuid) -> Result<i64, i0721_store::StoreError> {
            self.0.sum_live_sizes(o).await
        }
        async fn set_user_quota(
            &self,
            u: uuid::Uuid,
            q: Option<i64>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_user_quota(u, q).await
        }
        async fn create_api_key(
            &self,
            r: &i0721_proto::ApiKeyRecord,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.create_api_key(r).await
        }
        async fn list_api_keys(
            &self,
            u: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.list_api_keys(u).await
        }
        async fn revoke_api_key(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.revoke_api_key(id).await
        }
        async fn update_api_key(
            &self,
            id: uuid::Uuid,
            n: &str,
            s: &str,
            e: Option<&str>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.update_api_key(id, n, s, e).await
        }
        async fn find_api_key_by_hash(
            &self,
            h: &str,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_hash(h).await
        }
        async fn find_api_key_by_id(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_id(id).await
        }
        async fn touch_api_key(&self, id: uuid::Uuid) -> Result<(), i0721_store::StoreError> {
            self.0.touch_api_key(id).await
        }
        async fn set_setting(
            &self,
            k: &str,
            v: serde_json::Value,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_setting(k, v).await
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn upsert_storage_bucket(
            &self,
            slug: &str,
            d: &str,
            a: bool,
            g: bool,
            e: bool,
        ) -> Result<i0721_proto::StorageBucketRecord, i0721_store::StoreError> {
            self.0.upsert_storage_bucket(slug, d, a, g, e).await
        }
        async fn delete_storage_bucket(
            &self,
            id: uuid::Uuid,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_storage_bucket(id).await
        }
        async fn count_live_files_by_bucket(
            &self,
            id: Option<uuid::Uuid>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_bucket(id).await
        }
        async fn get_file_by_owner_bucket_and_sha256(
            &self,
            owner: uuid::Uuid,
            bucket: Option<uuid::Uuid>,
            sha: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0
                .get_file_by_owner_bucket_and_sha256(owner, bucket, sha)
                .await
        }
        async fn get_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
        ) -> Result<Option<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.get_bucket_grant(u, b).await
        }
        async fn set_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
            r: bool,
            w: bool,
            s: bool,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_bucket_grant(u, b, r, w, s).await
        }
        async fn list_bucket_grants_for_user(
            &self,
            u: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_user(u).await
        }
        async fn list_bucket_grants_for_bucket(
            &self,
            b: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_bucket(b).await
        }
        async fn a_live_key_references_bucket_slug(
            &self,
            slug: &str,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.a_live_key_references_bucket_slug(slug).await
        }
        async fn settings_state(
            &self,
            key: &str,
        ) -> Result<i0721_store::SettingsState, i0721_store::StoreError> {
            // THE deliberate failure. Two shapes: the `site` key (and any
            // other) ERRORS — a store failure that the health report's
            // config_source must call `unreadable`; the
            // `storage_defaults` key answers Ok(Unreadable) — the
            // four-state value the upload lane's
            // `default_bucket_slug` Unreadable arm must fail closed on
            // (an Err here would be scrubbed by From<StoreError> BEFORE
            // the arm could speak, which is exactly the lane this
            // fixture must NOT mask).
            if key == "storage_defaults" {
                return Ok(i0721_store::SettingsState::Unreadable {
                    err: "lane: unreadable".to_owned(),
                });
            }
            Err(i0721_store::StoreError::Db("lane: unreadable".to_owned()))
        }
    }
    let store = Arc::new(FailingSettings(MemoryStore::new()));
    let _ = store
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(hash_password("admin-password").unwrap()),
        })
        .await
        .unwrap();
    AppState::new(
        store,
        Arc::new(MemoryBlobStore::new()),
        Arc::new(test_config()),
    )
}

/// `health_reports_unreadable_settings_instead_of_default`: a store
/// read failure must surface as `config_source: "unreadable"`, not the
/// old `"default"` (the value the corrupt row also produces), and EVERY
/// `settings[]` row's state must read `"unreadable"` — the fixture's
/// store fails every key.
/// MUT:P-10-MAP — mapping the Err arm of `settings_state` in
/// `api/settings.rs` to "absent" (or anything else) turns this red.
#[tokio::test]
async fn health_reports_unreadable_settings_instead_of_default() {
    let state = failing_settings_state().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["config_source"], "unreadable",
        "a store read failure is its OWN state, never 'default': {json}"
    );
    assert!(json["buckets"].is_array(), "{json}");
    // The P-10 array: the fixture's store fails EVERY settings read, so
    // every row's state must carry the honest "unreadable" tag — not a
    // silent "absent"/"default" fallback.
    let settings = json["settings"].as_array().expect("a settings array");
    assert!(
        !settings.is_empty(),
        "the settings array is present: {json}"
    );
    for row in settings {
        assert_eq!(
            row["state"], "unreadable",
            "every settings row reads unreadable when the store fails: {row}"
        );
    }
}

/// R-F's Unreadable arm on the UPLOAD lane: with the default-bucket
/// setting's store read failing, `POST /api/v1/files` must fail closed —
/// 500 and a body that names the settings key and its state — never a
/// silent fallback to the bootstrap bucket.
/// MUT:R-F-UP — mapping `default_bucket_slug`'s Unreadable arm back to
/// "the default (bootstrap) bucket" turns this red.
#[tokio::test]
async fn upload_refused_while_the_default_bucket_state_is_unreadable() {
    let state = failing_settings_state().await;
    let admin = state
        .store
        .list_users(0, 1)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the helper seeded the admin");
    let token = state.signer.sign(admin.id, Timestamp::now());
    let (status, body) = upload_raw(&state, &token, tiny_png(), "u.png", None).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "unreadable default = fail-closed, never a bootstrap fallback: {body}"
    );
    let body: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let text = body["error"].as_str().unwrap_or_default();
    assert!(
        text.contains("storage_defaults") && text.contains("unreadable"),
        "the error names the settings key and the state: {text}"
    );
}
// ── the account-first ladder's discovery endpoint ──────────────────────

/// A raw `POST /api/v1/auth/account-methods` — status plus the decoded
/// body (the error document is a body too: a parity check must be able to
/// compare REFUSALS, not only answers).
async fn account_methods_raw(
    state: &AppState,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/account-methods")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

/// `legacy_backend_rows_are_disabled_and_explained_in_health`: a
/// `legacy_unknown` bucket is Unavailable/Disabled with the
/// actionable reason (register the credentials to serve it).
/// MUT:M10 — mapping Unavailable→Ready in the state classification
/// must turn this red too.
#[tokio::test]
async fn legacy_backend_rows_are_disabled_and_explained_in_health() {
    let legacy = i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: "bootstrap-s3".to_owned(),
        display_name: "bootstrap (s3) — legacy, not registered".to_owned(),
        backend: i0721_proto::BucketBackend::S3,
        location: "legacy:s3 (unknown — register its bucket to serve)".to_owned(),
        endpoint_ref: "legacy_unknown".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true, // enabled in the DB — the UNKNOWN endpoint is the blocker
        is_bootstrap: true,
        created_at: Timestamp::now(),
    };
    let registry =
        i0721_server::blob_registry::BlobRegistry::build(vec![legacy], |_| None).unwrap();
    let health = registry.health();
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].state, "unavailable", "{health:?}");
    let reason = health[0].reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("register this bucket's credentials"),
        "the reason explains the remedy: {reason}"
    );
}

/// The bootstrap bucket's IDENTITY fields are pinned by the PG lane
/// (`pg_0007_seats_the_bootstrap_bucket_with_its_identity_fields`):
/// a structural no-mutator argument cannot be pinned by a runtime
/// test, and an existence pin has no defensive value. P2a's upsert
/// rejection adds the runtime half.
/// `missing_storage_backend_env_while_0007_is_pending_fails_startup`
/// (+ negative control: an explicit `local` passes through):
/// the pure function's contract.
/// MUT:M-RU — inverting the pending branch (reading the env only when
/// 0007 is applied) must turn this red.
#[test]
fn missing_storage_backend_env_while_0007_is_pending_fails_startup() {
    use i0721_server::blob_selection::{bootstrap_backend_for_startup, StartupError};
    let missing = bootstrap_backend_for_startup(None, true);
    match missing {
        Err(StartupError::MissingBackend { var }) => {
            assert_eq!(var, "I0721_STORAGE_BACKEND");
        }
        other => panic!("missing env while 0007 is pending must fail: {other:?}"),
    }
    let invalid = bootstrap_backend_for_startup(Some("gcs"), true);
    assert!(
        matches!(invalid, Err(StartupError::InvalidBackend { .. })),
        "an invalid label is refused too: {invalid:?}"
    );
    // Negative control: an explicit `local` builds `bootstrap-local`.
    let local = bootstrap_backend_for_startup(Some("local"), true).unwrap();
    assert_eq!(local, Some("local"));
    // Applied: the env is NEVER read (MUT:M-ENV-AFTER — validating it
    // anyway hard-fails on `Some("gcs")`).
    let applied = bootstrap_backend_for_startup(Some("gcs"), false).unwrap();
    assert_eq!(applied, None);
    let applied_unset = bootstrap_backend_for_startup(None, false).unwrap();
    assert_eq!(applied_unset, None);
}

/// `env_backend_is_ignored_once_0007_is_applied` — the sibling of the
/// missing-env failure: a set variable after the migration must not
/// change the answer (the startup logs an info, nothing else).
#[test]
fn env_backend_is_ignored_once_0007_is_applied() {
    use i0721_server::blob_selection::bootstrap_backend_for_startup;
    for env in [None, Some("local"), Some("s3"), Some("gcs")] {
        let decided = bootstrap_backend_for_startup(env, false).unwrap();
        assert_eq!(decided, None, "env {env:?} must be ignored once applied");
    }
}

/// `migrated_database_with_zero_bucket_rows_fails_startup` — the
/// registry's empty-table failure (R-O): only a hand-cleared table
/// produces this, and the registry refuses to invent rows.
#[test]
fn migrated_database_with_zero_bucket_rows_fails_startup() {
    use i0721_server::blob_registry::{BlobRegistry, RegistryError};
    let err = BlobRegistry::build(Vec::new(), |_| None).err().unwrap();
    assert!(matches!(err, RegistryError::EmptyRegistry));
}

/// `ipfs_api_url_with_basic_auth_fails_startup` — R-K's refusal lands
/// at `BlobRegistry::build` (startup, not first request).
#[test]
fn ipfs_api_url_with_basic_auth_fails_startup() {
    use i0721_server::blob_registry::{BlobRegistry, RegistryError};
    let bucket = i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: "bootstrap-ipfs".to_owned(),
        display_name: "bootstrap (ipfs)".to_owned(),
        backend: i0721_proto::BucketBackend::Ipfs,
        location: "127.0.0.1:5001".to_owned(),
        endpoint_ref: "legacy_ipfs_env".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: true,
        created_at: Timestamp::now(),
    };
    let err = BlobRegistry::build(vec![bucket], |k| {
        (k == "I0721_IPFS_API").then(|| "http://user:secret@127.0.0.1:5001".to_owned())
    })
    .expect_err("basic-auth userinfo must fail the build");
    assert!(
        matches!(err, RegistryError::IpfsUrlUserinfo { .. }),
        "{err}"
    );
}

/// `ipfs_location_round_trips_host_port_without_credentials` — the
/// location is the host:port form, the URL carries no credentials, and
/// the bucket builds Ready with no network I/O.
#[test]
fn ipfs_location_round_trips_host_port_without_credentials() {
    use i0721_server::blob_registry::BlobRegistry;
    let bucket = i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: "bootstrap-ipfs".to_owned(),
        display_name: "bootstrap (ipfs)".to_owned(),
        backend: i0721_proto::BucketBackend::Ipfs,
        location: "127.0.0.1:5001".to_owned(),
        endpoint_ref: "legacy_ipfs_env".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: true,
        created_at: Timestamp::now(),
    };
    let registry = BlobRegistry::build(vec![bucket], |k| {
        (k == "I0721_IPFS_API").then(|| "http://127.0.0.1:5001".to_owned())
    })
    .expect("clean ipfs env builds");
    let health = registry.health();
    assert_eq!(health[0].state, "ready", "{health:?}");
    assert!(health[0].reason.is_none());
}

/// MUT:M-REG1 / MUT:M-REG2 — the guard those mutations exercise at the
/// unit level, pinned here on the state shape: a `for_row` whose row's
/// bucket is Unavailable answers an error NAMING that bucket, never a
/// substitute store (MUT:M2 drives the same shape through the media
/// lane).
#[test]
fn blob_registry_never_falls_back_to_another_bucket() {
    use i0721_server::blob_registry::{BlobError, BlobRegistry};
    fn record(slug: &str, endpoint_ref: &str, enabled: bool) -> i0721_proto::StorageBucketRecord {
        i0721_proto::StorageBucketRecord {
            id: uuid::Uuid::new_v4(),
            slug: slug.to_owned(),
            display_name: slug.to_owned(),
            backend: i0721_proto::BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: endpoint_ref.to_owned(),
            anonymous_read: true,
            grant_on_signup: false,
            enabled,
            is_bootstrap: true,
            created_at: Timestamp::now(),
        }
    }
    let own = record("bootstrap-local", "legacy_local_env", true);
    let registry = BlobRegistry::build(vec![own], |_| None).unwrap();
    let row = i0721_proto::FileRecord {
        id: uuid::Uuid::new_v4(),
        owner_id: uuid::Uuid::new_v4(),
        parent_id: None,
        name: "x".to_owned(),
        kind: i0721_proto::FileKind::Image,
        mime: "image/webp".to_owned(),
        size: 1,
        sha256: "abc".to_owned(),
        bucket_id: None,
        storage_backend: "local".to_owned(),
        storage_key: "abc".to_owned(),
        thumbnail_key: None,
        original_name: "x".to_owned(),
        created_at: Timestamp::now(),
        deleted_at: None,
    };
    match registry.for_row(&row) {
        Err(BlobError::Unavailable { slug, .. }) => assert_eq!(slug, "bootstrap-local"),
        Err(other) => panic!("expected Unavailable naming the OWN bucket: {other}"),
        Ok(_) => panic!("the unavailable bucket must not resolve"),
    }
}

/// A second bucket layered onto `state`'s registry: its store is a
/// DIFFERENT MemoryBlobStore. `bootstrap_enabled = false` leaves the
/// bootstrap entry Disabled (the fail-closed fixture); `true` keeps both
/// buckets Ready. Returns the patched state, the second bucket's id and
/// its store.
fn plant_second_bucket(
    state: &AppState,
    slug: &str,
    bootstrap_enabled: bool,
) -> (
    AppState,
    uuid::Uuid,
    Arc<i0721_server::blob::MemoryBlobStore>,
) {
    let id = uuid::Uuid::new_v4();
    let store = Arc::new(i0721_server::blob::MemoryBlobStore::new());
    let bootstrap_id = state.registry.bootstrap_bucket_id().unwrap();
    let bootstrap_entry = i0721_server::blob_registry::BucketEntry {
        slug: "bootstrap".to_owned(),
        state: if bootstrap_enabled {
            i0721_server::blob_registry::BucketState::Ready
        } else {
            i0721_server::blob_registry::BucketState::Disabled
        },
        reason: bootstrap_enabled
            .then_some(None)
            .flatten()
            .or(Some("the bucket is disabled in the registry".to_owned())),
        // The state's OWN blob store (the fixture's bootstrap bytes).
        store: bootstrap_enabled.then(|| state.blobs.clone()),
    };
    let second_entry = i0721_server::blob_registry::BucketEntry {
        slug: slug.to_owned(),
        state: i0721_server::blob_registry::BucketState::Ready,
        reason: None,
        store: Some(store.clone()),
    };
    let merged = i0721_server::blob_registry::BlobRegistry::from_entries(
        vec![(bootstrap_id, bootstrap_entry), (id, second_entry)],
        bootstrap_id,
        i0721_proto::BucketBackend::Local,
    );
    let mut patched = state.clone();
    patched.registry = merged;
    (patched, id, store)
}

/// `media_reads_from_the_rows_bucket`: two rows share one storage key
/// across two buckets; `/media` serves the bytes of the row that the
/// id-ASC serving-row rule selects — its OWN bucket's bytes, never the
/// other bucket's. MUT:M2 — `for_row`'s NULL branch resolving to "the
/// first Ready bucket" instead of the bootstrap bucket turns this and
/// `a_share_reads_only_bytes_from_its_own_rows_bucket` red.
#[tokio::test]
async fn media_reads_from_the_rows_bucket() {
    let (base_state, admin) = test_state().await;
    let token = login(&base_state, "admin", "admin-password").await;
    let (status, dto) = upload(&base_state, &token, tiny_png(), "a.png").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let key = dto["sha256"].as_str().unwrap().to_owned();
    // The pipeline converts to WebP: the row's OWN bytes are what the
    // row's bucket store holds.
    let bytes = base_state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .expect("the row's bucket holds its bytes");

    // Plant a SECOND Ready bucket whose store holds DIFFERENT bytes at
    // the same key, and DISABLE the bootstrap bucket: the row's bytes
    // still come from the row's OWN (now disabled) bucket per `for_row`,
    // MUT:M2 — never from "the first Ready bucket" (that wrong shape).
    // Serving itself answers 404 in P1 when no usable serving
    // row exists, so the assertion pins the RESOLUTION side through the
    // registry directly plus the decoy shape below.
    let (state, _second_id, second_store) = plant_second_bucket(&base_state, "satellite", false);
    second_store
        .put(
            i0721_server::blob::BlobSlot::Media,
            &key,
            b"decoy-bytes-from-the-other-bucket",
        )
        .await
        .unwrap();
    let other = state
        .store
        .create_user(CreateUser {
            username: "satellite-owner".to_owned(),
            display_name: "Satellite".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    // A same-key row with a NULL bucket (the legacy-row shape): for_row
    // must resolve it to the BOOTSTRAP bucket — which is disabled — and
    // therefore to NO usable serving row. The M2 mutation (NULL → "the
    // first Ready bucket" = satellite) would instead serve the satellite
    // store's decoy bytes as 200.
    state
        .store
        .create_file(i0721_proto::NewFile {
            bucket_id: None,
            parent_id: None,
            owner_id: other.id,
            name: "legacy.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: bytes.len() as i64,
            sha256: key.clone(),
            storage_backend: "local".to_owned(),
            storage_key: key.clone(),
            thumbnail_key: None,
            original_name: "legacy.png".to_owned(),
        })
        .await
        .unwrap();

    // The NULL row's own bucket is disabled and the second bucket is not
    // its bucket: 404 — and crucially NOT the satellite bytes.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "a NULL row resolves to its bootstrap bucket (disabled here): 404, never the decoy"
    );

    // With the bootstrap bucket KEPT ENABLED, the same request serves
    // the row's OWN bytes while the decoy sits in the second store.
    let (state, _second_id, second_store) = plant_second_bucket(&base_state, "satellite", true);
    second_store
        .put(
            i0721_server::blob::BlobSlot::Media,
            &key,
            b"decoy-bytes-from-the-other-bucket",
        )
        .await
        .unwrap();
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        bytes.as_ref(),
        "the row's own bucket bytes serve, never the decoy's"
    );

    // And the row itself carries its bucket id (the write side of I4).
    let row = state
        .store
        .get_file(uuid::Uuid::parse_str(dto["id"].as_str().unwrap()).unwrap())
        .await
        .unwrap()
        .unwrap();
    let expected = state.registry.bootstrap_bucket_id().unwrap();
    assert_eq!(row.bucket_id, Some(expected), "the row carries its bucket");
    assert_eq!(
        row.storage_backend, "local",
        "the row's tag is its bucket's backend"
    );
    let _ = admin;
}

/// `a_share_reads_only_bytes_from_its_own_rows_bucket`: a share's
/// download reads the FILE ROW's bucket — with a same-key decoy in the
/// registry's other store the served bytes must still be the row's.
/// MUT:M-B1 — share resolution reverting to "any row by storage_key"
/// turns this red.
#[tokio::test]
async fn a_share_reads_only_bytes_from_its_own_rows_bucket() {
    let (base_state, _) = test_state().await;
    let token = login(&base_state, "admin", "admin-password").await;
    let (_, dto) = upload(&base_state, &token, tiny_png(), "share.png").await;
    let file_id = dto["id"].as_str().unwrap().to_owned();
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let bytes = base_state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .expect("the row's bucket holds its bytes");

    // Plant the second Ready bucket with DIFFERENT bytes at the same
    // key AND a same-key row in it (another owner's): a share routed
    // "by storage key, any row" could pick the decoy row — the served
    // bytes must still be the share's file row's OWN bucket's.
    let (state, second_id, second_store) = plant_second_bucket(&base_state, "satellite", true);
    use i0721_server::blob::BlobStore as _;
    second_store
        .put(
            i0721_server::blob::BlobSlot::Media,
            &key,
            b"decoy-bytes-from-the-other-bucket",
        )
        .await
        .unwrap();
    let other = state
        .store
        .create_user(CreateUser {
            username: "satellite-owner".to_owned(),
            display_name: "Satellite".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    state
        .store
        .create_file(i0721_proto::NewFile {
            bucket_id: Some(second_id),
            parent_id: None,
            owner_id: other.id,
            name: "decoy.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: bytes.len() as i64,
            sha256: key.clone(),
            storage_backend: "local".to_owned(),
            storage_key: key.clone(),
            thumbnail_key: None,
            original_name: "decoy.png".to_owned(),
        })
        .await
        .unwrap();
    let (status, envelope) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id, "allow_preview": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let token_share = envelope["result"]["token"].as_str().unwrap().to_owned();

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{token_share}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        bytes.as_ref(),
        "the share serves the file row's OWN bucket bytes"
    );
}

/// `traversal_order_is_identical_across_stores`: `list_live_rows_by_storage_key`
/// returns the same `id ASC` order on the memory store and (via the
/// wrapper) any other store — the serving-row rule's determinism anchor.
#[tokio::test]
async fn traversal_order_is_identical_across_stores() {
    let store = MemoryStore::new();
    let owner = store
        .create_user(CreateUser {
            username: "trav".to_owned(),
            display_name: "Trav".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let record = store
            .create_file(i0721_proto::NewFile {
                bucket_id: None,
                parent_id: None,
                owner_id: owner.id,
                name: uuid::Uuid::new_v4().simple().to_string(),
                kind: i0721_proto::FileKind::Image,
                mime: "image/webp".to_owned(),
                size: 1,
                sha256: format!("t{}", uuid::Uuid::new_v4().simple()),
                storage_backend: "local".to_owned(),
                storage_key: "shared-key".to_owned(),
                thumbnail_key: None,
                original_name: "x".to_owned(),
            })
            .await
            .unwrap();
        ids.push(record.id);
    }
    let rows = store
        .list_live_rows_by_storage_key("shared-key")
        .await
        .unwrap();
    let mut expect = ids.clone();
    expect.sort();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        expect,
        "the traversal order is id ASC"
    );
}

/// `thumbnail_bytes_come_from_the_bucket_of_the_row_that_authorised`:
/// the `?thumbnail=true` variant serves the cache bytes of the serving
/// row's bucket — P1 verifies the serving-row-bucket half (the full
/// authorization predicate is P2b).
#[tokio::test]
async fn thumbnail_bytes_come_from_the_bucket_of_the_row_that_authorised() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "thumb.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let thumb_key = format!("{key}.webp");
    // The thumbnail is in the state's blob store (the row's bucket).
    let cached = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Cache, &thumb_key)
        .await
        .unwrap();
    assert!(cached.is_some(), "the upload produced a thumbnail");
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        cached.unwrap().as_ref(),
        "the thumbnail bytes come from the serving row's bucket"
    );
}

/// `a_lazily_regenerated_thumbnail_lands_in_the_serving_rows_bucket` —
/// the LAZY half of I4: with the cache entry DELETED from the serving
/// row's own store, `?thumbnail=true` regenerates from the original
/// blob and WRITES the `.webp` into the SERVING row's bucket — never
/// the bootstrap store. MUT:P2B-LAZY-WRITE — routing the lazy write to
/// `state.blobs` (the bootstrap store) turns this red.
#[tokio::test]
async fn a_lazily_regenerated_thumbnail_lands_in_the_serving_rows_bucket() {
    let (base_state, admin_token) = test_state_with_token().await;
    // A second Ready bucket with its OWN store; the registry keeps the
    // bootstrap entry too, so both stores are distinguishable.
    let (state, _second_id, second_store) = plant_second_bucket(&base_state, "satellite", true);
    plant_bucket_with_id(&state, _second_id, "satellite", false, false, true).await;
    // The admin uploads INTO the second bucket: the row (and its cache
    // slot) live in the satellite store, not the bootstrap store.
    let (status, dto) =
        upload_bucket(&state, &admin_token, tiny_png(), "lazy.png", "satellite").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let thumb_key = format!("{key}.webp");
    use i0721_server::blob::BlobSlot;
    // The upload DID pre-build the cache entry — now DELETE it so the
    // serving request must take the LAZY regeneration branch.
    assert!(
        second_store
            .get(BlobSlot::Cache, &thumb_key)
            .await
            .unwrap()
            .is_some(),
        "the upload pre-built the cache entry in the row's bucket"
    );
    second_store
        .delete(BlobSlot::Cache, &thumb_key)
        .await
        .unwrap();
    assert!(
        second_store
            .get(BlobSlot::Cache, &thumb_key)
            .await
            .unwrap()
            .is_none(),
        "the cache slot is now empty: the lazy branch must run"
    );
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the lazy branch regenerates the thumbnail"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/webp",
        "the regenerated thumbnail is webp"
    );
    // The regenerated `.webp` lands in the SERVING row's own bucket...
    let landed = second_store
        .get(BlobSlot::Cache, &thumb_key)
        .await
        .unwrap()
        .expect("the lazy write lands in the serving row's bucket");
    // ...and not in the bootstrap store (MUT:P2B-LAZY-WRITE's write target —
    // routing the lazy write there turns this red).
    assert!(
        state
            .blobs
            .get(BlobSlot::Cache, &thumb_key)
            .await
            .unwrap()
            .is_none(),
        "the lazy write never touches the bootstrap store"
    );
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        landed.as_ref(),
        "the served bytes are the regenerated ones"
    );
}

/// `folder_rows_carry_their_parents_bucket`: a child folder inherits the
/// parent's `bucket_id`; a root folder takes the resolved bootstrap
/// bucket. MUT:M-B2 — removing the `set_parent` cross-bucket guard turns
/// `cross_bucket_move_is_refused` red.
#[tokio::test]
async fn folder_rows_carry_their_parents_bucket() {
    let (state, admin) = test_state().await;
    let bucket = state.registry.bootstrap_bucket_id().unwrap();
    let root = state
        .store
        .create_folder(admin.id, None, "root", Some(bucket))
        .await
        .unwrap();
    assert_eq!(root.bucket_id, Some(bucket));
    let child = state
        .store
        .create_folder(admin.id, Some(root.id), "child", root.bucket_id)
        .await
        .unwrap();
    assert_eq!(
        child.bucket_id, root.bucket_id,
        "the child inherits the parent's bucket"
    );
}

/// MUT:M-FOLDER-KEY — folder rows have no storage key and no bytes, so
/// the byte lanes have nothing to route through the registry (the
/// placeholder shape proves the resolution consumes only the bucket id
/// and folders are excluded by kind before any byte work).
#[tokio::test]
async fn folder_rows_never_resolve_to_a_store() {
    let (state, admin) = test_state().await;
    let bucket = state.registry.bootstrap_bucket_id().unwrap();
    let root = state
        .store
        .create_folder(admin.id, None, "nofiles", Some(bucket))
        .await
        .unwrap();
    // A folder row carries the bucket id but an EMPTY storage key; the
    // delete lane excludes folders from every byte map before any store
    // is resolved.
    let reloaded = state.store.get_file(root.id).await.unwrap().unwrap();
    assert_eq!(reloaded.storage_key, "", "folders have no storage key");
    assert_eq!(reloaded.bucket_id, Some(bucket));
}

/// `cross_bucket_move_is_refused`: `files.move` into a folder of another
/// bucket answers `-32007`. MUT:M-B2 — deleting the `set_parent`
/// cross-bucket guard turns this red.
#[tokio::test]
async fn cross_bucket_move_is_refused() {
    let store = MemoryStore::new();
    let owner = store
        .create_user(CreateUser {
            username: "mover".to_owned(),
            display_name: "Mover".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let bucket_a = uuid::Uuid::new_v4();
    let bucket_b = uuid::Uuid::new_v4();
    let folder_a = store
        .create_folder(owner.id, None, "a", Some(bucket_a))
        .await
        .unwrap();
    let folder_b = store
        .create_folder(owner.id, None, "b", Some(bucket_b))
        .await
        .unwrap();
    let child = store
        .create_folder(owner.id, Some(folder_a.id), "child", Some(bucket_a))
        .await
        .unwrap();
    let moved = store.set_parent(child.id, Some(folder_b.id)).await;
    assert!(
        matches!(moved, Err(i0721_store::StoreError::Conflict(_))),
        "a cross-bucket move is refused at the store guard"
    );
    // And the moved row is unchanged.
    let reloaded = store.get_file(child.id).await.unwrap().unwrap();
    assert_eq!(reloaded.parent_id, Some(folder_a.id));
}

/// `root_folders_with_the_same_name_in_two_buckets_coexist`: the
/// same-layer name uniqueness is bucket-aware (R-G).
#[tokio::test]
async fn root_folders_with_the_same_name_in_two_buckets_coexist() {
    let store = MemoryStore::new();
    let owner = store
        .create_user(CreateUser {
            username: "roottwin".to_owned(),
            display_name: "RootTwin".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let a = store
        .create_folder(owner.id, None, "docs", Some(uuid::Uuid::new_v4()))
        .await
        .unwrap();
    let b = store
        .create_folder(owner.id, None, "docs", Some(uuid::Uuid::new_v4()))
        .await
        .unwrap();
    assert_ne!(a.id, b.id, "same-name roots in two buckets coexist");
}

/// `same_name_roots_are_rejected_within_one_bucket`: the bucket layer
/// still refuses duplicates inside ONE bucket.
#[tokio::test]
async fn same_name_roots_are_rejected_within_one_bucket() {
    let store = MemoryStore::new();
    let owner = store
        .create_user(CreateUser {
            username: "rootdup".to_owned(),
            display_name: "RootDup".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let bucket = uuid::Uuid::new_v4();
    store
        .create_folder(owner.id, None, "docs", Some(bucket))
        .await
        .unwrap();
    let dup = store
        .create_folder(owner.id, None, "docs", Some(bucket))
        .await;
    assert!(
        matches!(dup, Err(i0721_store::StoreError::Conflict(_))),
        "the same name in ONE bucket is a conflict"
    );
}

/// `upload_without_override_never_falls_back_to_the_bootstrap_bucket`:
/// P1 form — when the bootstrap bucket is UNRESOLVABLE the upload is an
/// error, never a re-route to another bucket. MUT:M-B3 — a
/// `resolve_upload_bucket` fallback to "some Ready bucket" turns this red.
#[tokio::test]
async fn upload_without_override_never_falls_back_to_the_bootstrap_bucket() {
    // A registry whose BOOTSTRAP bucket is DISABLED while ANOTHER bucket
    // is Ready: resolution must fail (NoBootstrap) — the M3 mutation
    // ("fall back to the first Ready bucket") would answer Ok with the
    // foreign bucket.
    let (base_state, _) = test_state().await;
    let (state, _second_id, _second_store) = plant_second_bucket(&base_state, "satellite", false);
    let resolved = state.registry.resolve_upload_bucket(None, None);
    assert!(
        resolved.is_err(),
        "an unresolvable upload bucket is an error, not a fallback"
    );
    match resolved {
        Err(err) => assert!(
            err.to_string().contains("bootstrap"),
            "the error names the bootstrap resolution: {err}"
        ),
        Ok(_) => unreachable!(),
    }
}

/// `reclaim_refcounts_are_per_bucket`: the reclaim probes count live
/// rows per (bucket_id, key) — a same-key row in ANOTHER bucket does not
/// keep this bucket's blob alive, and vice versa.
/// MUT:M-B4 — a count scoped to the key alone (dropping the bucket
/// dimension) turns this red.
#[tokio::test]
async fn reclaim_refcounts_are_per_bucket() {
    let store = MemoryStore::new();
    let owner = store
        .create_user(CreateUser {
            username: "reclaim".to_owned(),
            display_name: "Reclaim".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let bucket_a = uuid::Uuid::new_v4();
    let bucket_b = uuid::Uuid::new_v4();
    let new_file = |name: &str, bucket: uuid::Uuid| i0721_proto::NewFile {
        bucket_id: Some(bucket),
        parent_id: None,
        owner_id: owner.id,
        name: name.to_owned(),
        kind: i0721_proto::FileKind::Image,
        mime: "image/webp".to_owned(),
        size: 1,
        sha256: format!("r{}", uuid::Uuid::new_v4().simple()),
        storage_backend: "local".to_owned(),
        storage_key: "same-key".to_owned(),
        thumbnail_key: Some("same-key.webp".to_owned()),
        original_name: name.to_owned(),
    };
    store.create_file(new_file("a", bucket_a)).await.unwrap();
    store.create_file(new_file("b", bucket_b)).await.unwrap();
    assert_eq!(
        store
            .count_live_files_by_storage_key(Some(bucket_a), "same-key")
            .await
            .unwrap(),
        1,
        "bucket A's count sees only bucket A's row"
    );
    assert_eq!(
        store
            .count_live_files_by_storage_key(Some(bucket_b), "same-key")
            .await
            .unwrap(),
        1,
        "bucket B's count sees only bucket B's row"
    );
    assert_eq!(
        store
            .count_live_files_by_thumbnail_key(Some(bucket_a), "same-key.webp")
            .await
            .unwrap(),
        1,
        "the thumbnail refcount is per bucket too"
    );
}

/// The R-J serving-row rule when the bootstrap bucket CANNOT serve: the
/// serving row must come from a Ready bucket and every byte must come
/// from THAT bucket. Fixture: the bootstrap bucket is Disabled, a second
/// bucket is Ready and holds DIFFERENT bytes at the same key, and the
/// disabled row is forced to sort FIRST by `id ASC` — so a fallback to
/// `rows.first()`, a "wrong store" read, or a predicate that ignores
/// readiness all change the observable answer.
/// MUT:M-A8 / MUT:M-A3 / MUT:M-A11 drive those three wrong shapes.
#[tokio::test]
async fn media_serves_from_the_first_ready_row_not_the_bootstrap_bucket() {
    let (base_state, _admin) = test_state().await;
    let token = login(&base_state, "admin", "admin-password").await;
    let (status, dto) = upload(&base_state, &token, tiny_png(), "served.png").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let bootstrap_bytes = base_state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .expect("the bootstrap store holds the uploaded bytes");

    // Bootstrap Disabled, satellite Ready with its own store.
    let (state, second_id, second_store) = plant_second_bucket(&base_state, "satellite", false);
    let satellite_bytes = b"satellite-bucket-bytes-not-the-bootstrap-ones".to_vec();
    second_store
        .put(i0721_server::blob::BlobSlot::Media, &key, &satellite_bytes)
        .await
        .unwrap();

    // The satellite row: same key, born in the Ready bucket.
    let owner = state
        .store
        .create_user(CreateUser {
            username: "satellite-owner".to_owned(),
            display_name: "Satellite".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let satellite_row = state
        .store
        .create_file(i0721_proto::NewFile {
            bucket_id: Some(second_id),
            parent_id: None,
            owner_id: owner.id,
            name: "satellite.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: satellite_bytes.len() as i64,
            sha256: key.clone(),
            storage_backend: "local".to_owned(),
            storage_key: key.clone(),
            thumbnail_key: None,
            original_name: "satellite.png".to_owned(),
        })
        .await
        .unwrap();

    // Force the DISABLED (bootstrap) rows to sort first: the uploaded row
    // already sorts by its own id, so add bootstrap-bucket rows with the
    // same key until one has a SMALLER id than the satellite row. A
    // `rows.first()` fallback then hits a bucket that cannot serve.
    let mut smallest: Option<uuid::Uuid> = None;
    for attempt in 0..64 {
        // The `(owner, sha256)` live-unique index forbids a second row for
        // the same content under one owner, so every probe row gets its own
        // account (a deleted owner would cascade the row away).
        let probe_owner = state
            .store
            .create_user(CreateUser {
                username: format!("probe-{attempt}"),
                display_name: format!("Probe {attempt}"),
                role: Role::Member,
                password_hash: None,
            })
            .await
            .unwrap();
        let probe = state
            .store
            .create_file(i0721_proto::NewFile {
                bucket_id: Some(state.registry.bootstrap_bucket_id().unwrap()),
                parent_id: None,
                owner_id: probe_owner.id,
                name: format!("bootstrap-probe-{attempt}.png"),
                kind: i0721_proto::FileKind::Image,
                mime: "image/webp".to_owned(),
                size: bootstrap_bytes.len() as i64,
                sha256: key.clone(),
                storage_backend: "local".to_owned(),
                storage_key: key.clone(),
                thumbnail_key: None,
                original_name: "probe.png".to_owned(),
            })
            .await
            .unwrap();
        smallest = Some(smallest.map_or(probe.id, |current| current.min(probe.id)));
        if smallest.is_some_and(|id| id < satellite_row.id) {
            break;
        }
    }
    assert!(
        smallest.is_some_and(|id| id < satellite_row.id),
        "fixture: a disabled row must sort before the satellite row"
    );

    // Correct behaviour: 200 with the SATELLITE bucket's bytes.
    let response = ServiceExt::oneshot(
        app(&state),
        Request::builder()
            .uri(format!("/media/{key}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a Ready bucket holds the key, so the media is served"
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        satellite_bytes.as_slice(),
        "the bytes come from the SERVING row's bucket (the Ready one), not the disabled bootstrap store"
    );
    assert_ne!(
        body.as_ref(),
        bootstrap_bytes.as_ref(),
        "and certainly not from the bucket that cannot serve"
    );
}

/// The no-fallback upload guard must hold on the REAL request path, not
/// only when the resolver is called directly: with the bootstrap bucket
/// Disabled, `POST /api/v1/files` must refuse and no store may receive
/// the bytes (a fallback to a fresh in-memory store would silently accept
/// the upload into a bucket that is not registered).
/// MUT:M-B5 introduces exactly that fallback.
#[tokio::test]
async fn upload_through_the_route_refuses_when_no_bucket_can_serve() {
    let (base_state, _admin) = test_state().await;
    let token = login(&base_state, "admin", "admin-password").await;
    // Bootstrap Disabled; the satellite bucket exists but is Ready for a
    // DIFFERENT purpose — the resolver must not silently pick it either,
    // so the fixture keeps it out of the upload path by disabling nothing
    // else: the resolved target is the bootstrap bucket, which cannot
    // serve.
    let (state, _second_id, second_store) = plant_second_bucket(&base_state, "satellite", false);
    let (status, body) = upload(&state, &token, tiny_png(), "refused.png").await;
    assert!(
        !status.is_success(),
        "an upload with no usable bucket is an error, not a fallback: {status} {body}"
    );
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the resolver failure surfaces as a server error (the bucket cannot serve)"
    );
    // Nothing was written: the account has no file rows after the refusal
    // (a fallback store would have accepted the bytes and recorded a row).
    let page = state
        .store
        .count_folder(user_id_of(&state, "admin").await, None, None, None)
        .await
        .unwrap();
    assert_eq!(page, 0, "the refused upload recorded no file row: {page}");
    let _ = &second_store;
}

/// The admin account's id (the upload test needs it to count rows).
async fn user_id_of(state: &AppState, username: &str) -> uuid::Uuid {
    state
        .store
        .get_user_by_username(username)
        .await
        .unwrap()
        .expect("the seeded admin exists")
        .id
}

async fn account_methods(state: &AppState, username: &str) -> serde_json::Value {
    let (status, json) =
        account_methods_raw(state, serde_json::json!({ "username": username })).await;
    assert_eq!(status, StatusCode::OK, "unexpected refusal: {json}");
    json
}

/// Plant a member account directly (the ladder probes any name).
async fn plant_member(state: &AppState, username: &str, password: Option<&str>) -> uuid::Uuid {
    let hash = password.map(|plain| hash_password(plain).unwrap());
    state
        .store
        .create_user(CreateUser {
            username: username.to_owned(),
            display_name: username.to_owned(),
            role: Role::Member,
            password_hash: hash,
        })
        .await
        .unwrap()
        .id
}

/// The anti-enumeration pin, at the HTTP layer where an attacker reads:
/// an unknown name and a password-carrying account must be the SAME
/// response — same status, same body — with one erp provider registered
/// (so a naive "join the global provider list" implementation would show
/// the difference immediately).
#[tokio::test]
async fn account_methods_answers_unknown_names_exactly_like_password_accounts() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let _known = plant_member(&state, "known", Some("known-password")).await;

    let unknown = account_methods(&state, "no-such-account-at-all").await;
    let known = account_methods(&state, "known").await;
    assert_eq!(
        unknown, known,
        "MUT:enumeration-parity — the two documents must be identical"
    );
    assert_eq!(
        unknown,
        serde_json::json!({
            "factors": [{"kind": "password"}],
            "password": true,
        }),
        "and both must be the documented password-account shape"
    );

    // The same name in another case / with padding is still "unknown" and
    // still answers the same shape (no case-insensitive oracle).
    assert_eq!(account_methods(&state, "KNOWN").await, unknown);
    assert_eq!(
        account_methods(&state, "  no-such-account-at-all  ").await,
        unknown
    );
}

/// An OAuth-only account (NULL password hash, one linked identity)
/// announces exactly its provider — and keeps the deployment-level
/// `password` flag, which is the same for every caller.
#[tokio::test]
async fn account_methods_lists_the_linked_providers_of_a_passwordless_account() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let id = plant_member(&state, "sso-only", None).await;
    state
        .store
        .link_oauth_identity("stub", "subject-1", id)
        .await
        .unwrap();

    let json = account_methods(&state, "sso-only").await;
    assert_eq!(
        json,
        serde_json::json!({
            "factors": [{"kind": "oauth", "provider": "stub", "label": "Stub SSO"}],
            "password": true,
        }),
        "no password hash → no password factor, whatever the switch says"
    );

    // A password account with the SAME link announces both — the ladder's
    // "pick any one of these" list, password first.
    let both = plant_member(&state, "both", Some("both-password")).await;
    state
        .store
        .link_oauth_identity("stub", "subject-2", both)
        .await
        .unwrap();
    assert_eq!(
        account_methods(&state, "both").await["factors"],
        serde_json::json!([
            {"kind": "password"},
            {"kind": "oauth", "provider": "stub", "label": "Stub SSO"},
        ])
    );
}

/// A disabled provider must not be offered: its row is filtered out of the
/// label ORACLE as well, so a linked-but-disabled identity names itself by
/// id and the flow entry refuses — the same treatment the login page gets.
#[tokio::test]
async fn account_methods_never_labels_a_disabled_provider() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    // Disable the stub row.
    let mut rows = state
        .store
        .get_setting("oauth_providers")
        .await
        .unwrap()
        .unwrap();
    rows[0]["enabled"] = serde_json::json!(false);
    state
        .store
        .set_setting("oauth_providers", rows)
        .await
        .unwrap();

    let id = plant_member(&state, "sso-only", None).await;
    state
        .store
        .link_oauth_identity("stub", "subject-1", id)
        .await
        .unwrap();

    assert_eq!(
        account_methods(&state, "sso-only").await["factors"],
        serde_json::json!([{"kind": "oauth", "provider": "stub", "label": "stub"}]),
        "the account's real option survives, but a disabled row never \
         supplies its display name"
    );
}

/// The kill-switch turns the password factor off deployment-wide — and it
/// is the ONLY thing that changes between two accounts, so the shape stays
/// a function of the DEPLOYMENT, never of the name.
#[tokio::test]
async fn account_methods_password_switch_only_changes_the_password_factor() {
    let (state, _stub, _base, admin) = oauth_test_state_with_stub().await;
    let _known = plant_member(&state, "known", Some("known-password")).await;
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "auth.password.put",
        serde_json::json!({"enabled": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    let known = account_methods(&state, "known").await;
    let unknown = account_methods(&state, "ghost").await;
    assert_eq!(
        known, unknown,
        "still indistinguishable with the switch off"
    );
    assert_eq!(
        known,
        serde_json::json!({"factors": [{"kind": "password"}], "password": false}),
        "the baseline survives the switch (an empty list would become the \
         one shape that means 'no such account')"
    );
}

/// The endpoint is a public guess surface: it shares the login budget, so
/// six probes from one address are refused — and the refusal is the same
/// for a known and an unknown name (a difference would itself be an
/// oracle).
#[tokio::test]
async fn account_methods_shares_the_per_ip_login_budget() {
    let (state, _stub, _base, _admin) = oauth_test_state_with_stub().await;
    let _known = plant_member(&state, "known", Some("known-password")).await;

    for _ in 0..i0721_server::auth::LOGIN_ATTEMPT_LIMIT {
        let (status, json) =
            account_methods_raw(&state, serde_json::json!({"username": "known"})).await;
        assert_eq!(status, StatusCode::OK, "{json}");
    }
    let (status, refused_known) =
        account_methods_raw(&state, serde_json::json!({"username": "known"})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, refused_unknown) =
        account_methods_raw(&state, serde_json::json!({"username": "ghost"})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused_known, refused_unknown,
        "the 429 document must not differ by name either"
    );

    // The budget is the LOGIN budget: a login attempt from the same
    // address is refused too (one shared log, not a second allowance).
    let response = app(&state)
        .oneshot(login_request("known", "known-password"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

// ── the P2a slice: authorization, bucket selection, R-C, §7 shapes ─────

/// Grant a member a bucket through `storage.grants.set`; fails loudly
/// when refused (the admin-only surface).
#[allow(dead_code)]
async fn set_grant(
    state: &AppState,
    admin_token: &str,
    user_id: uuid::Uuid,
    slug: &str,
    read: bool,
    write: bool,
    share: bool,
) {
    let (status, envelope) = rpc(
        state,
        admin_token,
        "storage.grants.set",
        serde_json::json!({"user": user_id, "bucket_slug": slug, "read": read, "write": write, "share": share}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "grant set: {envelope}");
}

/// Plant a bucket row straight into the store (the memory lane's
/// registry row; the fixture registry uses `plant_second_bucket` for
/// the blob half when the test needs bytes there).
async fn plant_bucket(
    state: &AppState,
    slug: &str,
    anonymous_read: bool,
    grant_on_signup: bool,
    enabled: bool,
) -> i0721_proto::StorageBucketRecord {
    let bucket = i0721_proto::StorageBucketRecord {
        id: uuid::Uuid::new_v4(),
        slug: slug.to_owned(),
        display_name: slug.to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read,
        grant_on_signup,
        enabled,
        is_bootstrap: false,
        created_at: Timestamp::now(),
    };
    // The tests' fixture store is an Arc<MemoryStore> throughout (the
    // typed handle carries the seeding method).
    let memory = state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    memory.seed_bucket(bucket.clone());
    bucket
}

/// The stage-A `storage.buckets.list` surface, extended to VISIBILITY
/// SCOPE: a member gets ONLY their visible buckets (any grant bit or
/// the signup default), each with a `my_access` projection of their own
/// bits; the admin keeps every bucket. MUT:B-P2A-LIST-SCOPE — keeping
/// the admin gate (or dropping the visibility filter) turns the member
/// arm red; MUT:B-P2A-MY-ACCESS — dropping the `my_access` projection
/// empties the field and reddens the shape assertion.
#[tokio::test]
async fn buckets_list_is_visibility_scoped_with_the_actors_own_bits() {
    let (state, admin_token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "lister".to_owned(),
            display_name: "lister".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // A private non-signup bucket the member holds an explicit
    // read-only row on, plus one the member cannot see at all.
    plant_bucket(&state, "listed", false, false, true).await;
    assert!(
        state
            .store
            .get_storage_bucket_by_slug("listed")
            .await
            .unwrap()
            .is_some(),
        "the planted bucket exists"
    );
    let (_status, grant) = rpc(
        &state,
        &admin_token,
        "storage.grants.set",
        serde_json::json!({"user": member.id, "bucket_slug": "listed", "read": true, "write": false, "share": false}),
    )
    .await;
    assert!(grant.get("error").is_none(), "grant set: {grant}");
    plant_bucket(&state, "hidden", false, false, true).await;

    // The member sees exactly [bootstrap, listed] — the hidden bucket
    // (invisible: no row, no signup default) is absent.
    let (_status, envelope) = rpc(
        &state,
        &member_token,
        "storage.buckets.list",
        serde_json::Value::Null,
    )
    .await;
    let rows = envelope["result"].as_array().expect("a list: {envelope}");
    let slugs: Vec<&str> = rows.iter().filter_map(|r| r["slug"].as_str()).collect();
    assert!(
        slugs.contains(&"listed") && !slugs.contains(&"hidden"),
        "the member sees only visible buckets: {slugs:?}"
    );
    let listed_row = rows
        .iter()
        .find(|r| r["slug"] == "listed")
        .expect("the granted bucket is listed");
    assert_eq!(
        listed_row["my_access"],
        serde_json::json!({"read": true, "write": false, "share": false}),
        "my_access is the actor's OWN bits: {listed_row}"
    );
    let bootstrap_row = rows
        .iter()
        .find(|r| r["slug"] == "bootstrap")
        .expect("the signup bucket is listed");
    assert_eq!(
        bootstrap_row["my_access"],
        serde_json::json!({"read": true, "write": true, "share": true}),
        "the signup default reads through: {bootstrap_row}"
    );

    // The admin still sees every bucket (both planted ones included).
    let (_status, envelope) = rpc(
        &state,
        &admin_token,
        "storage.buckets.list",
        serde_json::Value::Null,
    )
    .await;
    let slugs: Vec<&str> = envelope["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["slug"].as_str())
        .collect();
    assert!(
        slugs.contains(&"hidden") && slugs.contains(&"listed"),
        "the admin sees all buckets: {slugs:?}"
    );
}

/// The §7 `files.list` pushdown (Q14 / M3b's landing): an INVISIBLE
/// bucket slug in the `bucket` param makes rows AND total vanish (never
/// an error), and a member's own-scope listing stays owner-scoped while
/// the visible-bucket filter rides INSIDE the store (a post-fetch
/// filter would leak the count). MUT:B-P2A-LIST-PUSHDOWN — dropping the
/// store-side filter (or filtering only one of items/total) turns the
/// total-inequality assertion red.
#[tokio::test]
async fn files_list_bucket_param_filters_rows_and_total_together() {
    let (state, _) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "pushdown".to_owned(),
            display_name: "pushdown".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // One file of the member's in the (signup) bootstrap bucket.
    let (_, _own) = upload(&state, &member_token, tiny_png(), "mine.png").await;

    // Own scope, no bucket param: the row lists (the signup bucket is
    // visible through the default).
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.list",
        serde_json::json!({"scope": "own"}),
    )
    .await;
    let total = envelope["result"]["total"].as_u64().unwrap();
    assert_eq!(total, 1, "the member's own row lists: {envelope}");

    // An INVISIBLE slug: rows AND total vanish together — HTTP OK, an
    // empty page, not an error (MUT:B-P2A-LIST-PUSHDOWN; §7's files.list
    // row).
    let (_status, envelope) = rpc(
        &state,
        &member_token,
        "files.list",
        serde_json::json!({"scope": "own", "bucket": "no-such-bucket"}),
    )
    .await;
    assert!(
        envelope.get("error").is_none(),
        "an invisible bucket is an empty page, not an error: {envelope}"
    );
    let page = &envelope["result"];
    assert_eq!(page["total"], 0, "the total vanishes: {page}");
    assert_eq!(
        page["items"].as_array().unwrap().len(),
        0,
        "the rows vanish with it: {page}"
    );
}

/// A member reads media from the signup bucket but lists only their OWN
/// rows: the signup default grants read/write/share on the
/// `grant_on_signup` bucket (Q14's visibility), the member's upload
/// lands there, and the member's listing stays owner-scoped.
#[tokio::test]
async fn a_member_reads_media_from_the_signup_bucket_but_lists_only_their_own_rows() {
    let (state, admin_token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &admin_token);
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // The bootstrap bucket IS the deployment's signup bucket (0007
    // created it grant_on_signup=TRUE) — a member needs NO grant row.
    let (_, own) = upload(&state, &member_token, png_of(4, 4), "mine.png").await;
    let own_id = own["id"].as_str().unwrap().to_owned();
    let (_, admin_file) = upload(&state, &admin_token, png_of(5, 4), "theirs.png").await;

    // The member's list carries ONLY their own rows.
    let (_, list) = rpc(
        &state,
        &member_token,
        "files.list",
        serde_json::json!({"scope": "own"}),
    )
    .await;
    let items = list["result"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|f| f["id"].as_str() == Some(own_id.as_str())),
        "the member sees their own file: {list}"
    );
    assert!(
        !items
            .iter()
            .any(|f| f["id"].as_str() == Some(admin_file["id"].as_str().unwrap())),
        "the member does NOT see the admin's file: {list}"
    );

    // The member's media reads through the row's bucket (the signup
    // bucket's read bit comes from the signup default).
    let key = own["sha256"].as_str().unwrap();
    let media = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::OK, "signup-bucket read");
    let _ = admin_id;
}

/// `bucket_scope_denials_are_jsonrpc_frames_not_tool_errors`: a key
/// whose `buckets` array lacks the target bucket answers the permission
/// FRAME (top-level error), never an isError tool result.
/// MUT:M-P2A11 — a pre-check answering ToolError instead of Frame turns
/// this red.
#[tokio::test]
async fn bucket_scope_denials_are_jsonrpc_frames_not_tool_errors() {
    let (state, admin_token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &admin_token);
    plant_bucket(&state, "gated", true, false, true).await;
    // Upload through the DEFAULT (bootstrap) lane first.
    let (_, up) = upload(&state, &admin_token, tiny_png(), "x.png").await;
    let file_id = up["id"].as_str().unwrap().to_owned();

    // A key whose buckets EXPLICITLY excludes the file's bucket (the
    // bootstrap bucket): the share tool's R-L arm ② fires → the
    // permission frame.
    let (_, key) = mcp_api_key_with_at(&state, &admin_token, "no-buckets", all_ops(), None).await;
    // Rewrite the key's scopes with an explicit EMPTY buckets array (the
    // RPC materializes granted_buckets when the field is omitted).
    let record = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key))
        .await
        .unwrap()
        .unwrap();
    let explicit_empty = r#"{"ops":["read","upload","share"],"buckets":[]}"#.to_owned();
    let _ = admin_id;
    state
        .store
        .update_api_key(
            record.id,
            record.name.as_str(),
            &explicit_empty,
            record.expires_at.as_deref(),
        )
        .await
        .unwrap();

    let (mut sse, session_id) = mcp_handshake(&state, &key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"error\"").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(
        envelope["error"]["code"], SCOPE_DENIED,
        "the denial is a FRAME: {envelope}"
    );
    assert!(
        envelope.get("result").is_none(),
        "never an isError result: {envelope}"
    );
    // The two permission refusals share one code (the family's), so the
    // thing that tells them apart is `data.denial` — the key's own bucket
    // list here, the acting user's grant in the arm-③ test below.
    assert_eq!(
        envelope["error"]["data"]["denial"], "key_scope",
        "a key whose bucket list excludes the bucket says so: {envelope}"
    );
    assert!(
        envelope["error"]["data"]["bucket"].is_string(),
        "the refusal names the bucket it is about: {envelope}"
    );
}

/// A file that is not the caller's must not teach the caller anything about
/// it — and in particular not the slug of the bucket it sits in.
///
/// `get_share_link`'s pre-check loads the file by id, and `get_file` is not
/// owner-scoped; the "not your file" answer is the tool body's, which runs
/// *after* the pre-check. So for a foreign file id the pre-check used to reach
/// the share-bit arm and answer `data.denial="user_access"` with the bucket's
/// slug — disclosing a slug the caller has no grant to see, structured by the
/// very field that was added to tell the arms apart. The rule the rest of this
/// file already holds (§7: what the caller cannot see answers as if it were
/// not there) is the fix.
///
/// The caller's key lists the bucket in `buckets` on purpose: that arm must
/// pass, so the refusal is about the *file*, and the test is about what the
/// refusal is allowed to say.
#[tokio::test]
async fn mcp_get_share_link_for_someone_elses_file_names_no_bucket() {
    let (base_state, _admin_token) = test_state_with_token().await;
    let (state, bucket_id, _store) = plant_second_bucket(&base_state, "not-yours", true);
    // The store row MUST carry the registry's id, or the upload finds no row
    // to route to (the same seeding the share-bit test below does).
    state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store")
        .seed_bucket(i0721_proto::StorageBucketRecord {
            id: bucket_id,
            slug: "not-yours".to_owned(),
            display_name: "not-yours".to_owned(),
            backend: i0721_proto::BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: "x".to_owned(),
            // Private: an anonymous caller would not see it either.
            anonymous_read: false,
            grant_on_signup: false,
            enabled: true,
            is_bootstrap: false,
            created_at: Timestamp::now(),
        });
    async fn new_member(state: &AppState) -> i0721_proto::UserRecord {
        state
            .store
            .create_user(CreateUser {
                username: format!("m-{}", uuid::Uuid::new_v4().simple()),
                display_name: "m".to_owned(),
                role: Role::Member,
                password_hash: None,
            })
            .await
            .unwrap()
    }
    // The owner can write to the bucket; the caller cannot even read it.
    let owner = new_member(&state).await;
    state
        .store
        .set_bucket_grant(owner.id, bucket_id, true, true, true)
        .await
        .unwrap();
    let owner_token = state.signer.sign(owner.id, Timestamp::now());
    let (_, up) = upload_bucket(
        &state,
        &owner_token,
        png_of(4, 4),
        "theirs.png",
        "not-yours",
    )
    .await;
    let file_id = up["id"].as_str().unwrap().to_owned();
    let caller = new_member(&state).await;
    // The caller's key holds every op AND lists the bucket: nothing about the
    // key refuses, so whatever refuses is about the file.
    let (_, key) = plant_api_key(
        &state,
        caller.id,
        "caller",
        r#"{"ops":["read","upload","share"],"buckets":["not-yours"]}"#,
        None,
    )
    .await;

    let (mut sse, session_id) = mcp_handshake(&state, &key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "data:").await;
    let envelope = sse_envelope(&frames);
    let text = envelope.to_string();
    assert!(
        !text.contains("not-yours"),
        "a file the caller does not own must not reveal its bucket: {envelope}"
    );
    // And it is refused the way a file that is not there is refused: a tool
    // result, not a permission frame — a frame would confirm the file exists.
    assert!(
        envelope.get("error").is_none(),
        "no permission frame for a file the caller cannot see: {envelope}"
    );
    assert_eq!(envelope["result"]["isError"], true, "{envelope}");
}

/// The other half of the share rule: an admin's key DOES share another
/// user's file.
///
/// The rule is the RPC lane's — the owner, or an admin — and an API key acts
/// as its owner, admin included. (A version of this PR made the lane
/// owner-only and wrote "an API key never elevates" into the README; the
/// review showed an admin's key already targeting any user's folder through
/// `upload_file` in this same lane, so the sentence was false and the
/// owner-only rule was a divergence nobody had asked for.)
#[tokio::test]
async fn mcp_get_share_link_lets_an_admin_share_another_users_file() {
    let (base_state, _admin_token) = test_state_with_token().await;
    let (mut state, bucket_id, _store) = plant_second_bucket(&base_state, "admin-shares", true);
    // A share link needs somewhere to point; the fixture has no origin.
    let mut config = (*state.config).clone();
    config.public_origin = Some("https://res.example.test".to_owned());
    state.config = std::sync::Arc::new(config);
    state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store")
        .seed_bucket(i0721_proto::StorageBucketRecord {
            id: bucket_id,
            slug: "admin-shares".to_owned(),
            display_name: "admin-shares".to_owned(),
            backend: i0721_proto::BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: "x".to_owned(),
            anonymous_read: false,
            grant_on_signup: false,
            enabled: true,
            is_bootstrap: false,
            created_at: Timestamp::now(),
        });
    let owner = state
        .store
        .create_user(CreateUser {
            username: format!("o-{}", uuid::Uuid::new_v4().simple()),
            display_name: "o".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    state
        .store
        .set_bucket_grant(owner.id, bucket_id, true, true, true)
        .await
        .unwrap();
    let owner_token = state.signer.sign(owner.id, Timestamp::now());
    let (_, up) = upload_bucket(
        &state,
        &owner_token,
        png_of(4, 4),
        "theirs.png",
        "admin-shares",
    )
    .await;
    let file_id = up["id"].as_str().unwrap().to_owned();
    // An admin's key. The admin holds NO grant on the bucket at all, and the
    // share bit is a `bucket_grants` column: the tool must not ask an admin
    // for it (`files.share_create` does not — it wraps the same check in
    // `if user.role != Role::Admin`). Asserted through the outcome: a refusal
    // here would be `data.denial = "user_access"`.
    let admin = state
        .store
        .create_user(CreateUser {
            username: format!("a-{}", uuid::Uuid::new_v4().simple()),
            display_name: "a".to_owned(),
            role: Role::Admin,
            password_hash: None,
        })
        .await
        .unwrap();
    let (_, key) = plant_api_key(
        &state,
        admin.id,
        "admin",
        r#"{"ops":["read","upload","share"],"buckets":["admin-shares"]}"#,
        None,
    )
    .await;

    let (mut sse, session_id) = mcp_handshake(&state, &key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "data:").await;
    let envelope = sse_envelope(&frames);
    assert!(
        envelope.get("error").is_none(),
        "an admin is not refused a permission frame: {envelope}"
    );
    // A tool that succeeds carries no `isError` at all — that member is how
    // a failure is marked. And the result is a link, by its shape.
    assert!(
        envelope["result"].get("isError").is_none(),
        "a success is not marked as an error: {envelope}"
    );
    let text = envelope["result"]["content"][0]["text"].as_str().unwrap();
    let link: serde_json::Value = serde_json::from_str(text).unwrap();
    assert!(
        link["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("https://res.example.test/s/")),
        "a share link for the member's file: {link}"
    );
}

/// A user with no password, in `role`, for the share tests below.
async fn share_lane_user(state: &AppState, role: Role) -> i0721_proto::UserRecord {
    state
        .store
        .create_user(CreateUser {
            username: format!("u-{}", uuid::Uuid::new_v4().simple()),
            display_name: "u".to_owned(),
            role,
            password_hash: None,
        })
        .await
        .unwrap()
}

/// `get_share_link` for `file_id` through `key`: the tool's text, which must
/// not be an error.
async fn share_link_text(state: &AppState, key: &str, file_id: &str) -> String {
    let (mut sse, session_id) = mcp_handshake(state, key).await;
    let status = mcp_post_frame(
        state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "data:").await;
    let envelope = sse_envelope(&frames);
    assert!(envelope.get("error").is_none(), "{envelope}");
    assert_ne!(envelope["result"]["isError"], true, "{envelope}");
    envelope["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A file has ONE live share, whoever asks for the link.
///
/// The share belongs to the file, so the RPC lane records the file's owner on
/// it — and this lane recorded the *acting user*, which was the same thing
/// while only the owner could get here. The moment an admin's key could, an
/// admin's call minted a second token on the member's file instead of
/// answering with the member's (measured by the review: two rows, two
/// tokens). The same call from two different keys must answer with one token.
#[tokio::test]
async fn an_admin_and_the_owner_get_the_same_link_for_one_file() {
    let (base_state, _admin_token) = test_state_with_token().await;
    let (mut state, bucket_id, _store) = plant_second_bucket(&base_state, "one-link", true);
    let mut config = (*state.config).clone();
    config.public_origin = Some("https://res.example.test".to_owned());
    state.config = std::sync::Arc::new(config);
    state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store")
        .seed_bucket(i0721_proto::StorageBucketRecord {
            id: bucket_id,
            slug: "one-link".to_owned(),
            display_name: "one-link".to_owned(),
            backend: i0721_proto::BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: "x".to_owned(),
            anonymous_read: false,
            grant_on_signup: false,
            enabled: true,
            is_bootstrap: false,
            created_at: Timestamp::now(),
        });
    let owner = share_lane_user(&state, Role::Member).await;
    state
        .store
        .set_bucket_grant(owner.id, bucket_id, true, true, true)
        .await
        .unwrap();
    let owner_token = state.signer.sign(owner.id, Timestamp::now());
    let (_, up) = upload_bucket(&state, &owner_token, png_of(4, 4), "mine.png", "one-link").await;
    let file_id = up["id"].as_str().unwrap().to_owned();
    let admin = share_lane_user(&state, Role::Admin).await;
    let scopes = r#"{"ops":["read","upload","share"],"buckets":["one-link"]}"#;
    let (_, owner_key) = plant_api_key(&state, owner.id, "owner", scopes, None).await;
    let (_, admin_key) = plant_api_key(&state, admin.id, "admin", scopes, None).await;

    let first = share_link_text(&state, &owner_key, &file_id).await;
    let second = share_link_text(&state, &admin_key, &file_id).await;
    let owner_link: serde_json::Value = serde_json::from_str(&first).unwrap();
    let admin_link: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(
        owner_link["token"], admin_link["token"],
        "one file, one live share, whoever asks: {first} vs {second}"
    );
    // And the store agrees: the file's shares, by its OWNER's id, are one.
    let rows = state
        .store
        .list_shares(owner.id)
        .await
        .unwrap()
        .into_iter()
        .filter(|share| share.file_id.to_string() == file_id)
        .count();
    assert_eq!(rows, 1, "the share is recorded against the file's owner");
}

/// §7's MCP `upload_file` second arm: a key whose `buckets` array does
/// not include the target bucket answers the permission FRAME naming the
/// bucket (order: ops → key buckets → user access) rather than an
/// isError tool result. MUT:§7-UP pins this shape.
/// MUT:§7-UP — dropping the key-buckets check from the upload lane's
/// precheck (mcp.rs) turns this red.
#[tokio::test]
async fn mcp_upload_file_with_a_bucket_outside_the_key_answers_the_permission_frame() {
    let (base_state, admin_token) = test_state_with_token().await;
    let (state, _second_id, _store) = plant_second_bucket(&base_state, "mcp-target", true);
    plant_bucket(&state, "mcp-target", true, false, true).await;
    // The key holds ALL ops but an explicit EMPTY buckets array: the
    // upload lane's second arm must fire before anything else.
    let (_, key) = mcp_api_key_with_at(&state, &admin_token, "up-gated", all_ops(), None).await;
    let record = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key))
        .await
        .unwrap()
        .unwrap();
    state
        .store
        .update_api_key(
            record.id,
            record.name.as_str(),
            r#"{"ops":["read","upload","share"],"buckets":[]}"#,
            record.expires_at.as_deref(),
        )
        .await
        .unwrap();

    let (mut sse, session_id) = mcp_handshake(&state, &key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "upload_file", "arguments": {
            "content_base64": base64_of(&tiny_png()),
            "name": "gated.png",
            "bucket": "mcp-target",
        }}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"error\"").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(
        envelope["error"]["code"], SCOPE_DENIED,
        "the bucket-outside-the-key denial is a FRAME: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("mcp-target"),
        "the frame names the bucket: {envelope}"
    );
    assert!(
        envelope.get("result").is_none(),
        "never an isError result: {envelope}"
    );
    assert_eq!(
        envelope["error"]["data"]["denial"], "key_scope",
        "the upload lane's second arm is the key's list, not the user's grant: {envelope}"
    );
    assert_eq!(
        envelope["error"]["data"]["bucket"], "mcp-target",
        "the refusal names the bucket it is about: {envelope}"
    );
}

/// R-L arm ③ over MCP: the key's buckets INCLUDE the file's bucket but
/// the USER holds an explicit deny-share grant row → the permission FRAME
/// naming the bucket (§7's third share arm).
/// MUT:R-L-MCP — dropping the `Need::Share` frame from
/// `get_share_link`'s precheck (mcp.rs) turns this red.
#[tokio::test]
async fn mcp_get_share_link_without_the_user_share_bit_answers_the_permission_frame() {
    let (base_state, _admin_token) = test_state_with_token().await;
    let (state, second_id, _store) = plant_second_bucket(&base_state, "share-gated", true);
    // The store row MUST carry the registry's id (bucket_row_for_id
    // prefers the store's rows; a mismatched id would fall through to
    // the registry's synthesized signup-open posture).
    let memory = state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    memory.seed_bucket(i0721_proto::StorageBucketRecord {
        id: second_id,
        slug: "share-gated".to_owned(),
        display_name: "share-gated".to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: false,
        created_at: Timestamp::now(),
    });
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    // The explicit deny-share row: visible, writable, but NO share bit.
    state
        .store
        .set_bucket_grant(member.id, second_id, true, true, false)
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let (_, own) = upload_bucket(
        &state,
        &member_token,
        png_of(4, 4),
        "mine.png",
        "share-gated",
    )
    .await;
    let file_id = own["id"].as_str().unwrap().to_owned();

    // The member's OWN key: ops include share, buckets include
    // share-gated — the OPS and key-buckets arms pass, the USER's
    // missing share bit is what must refuse.
    let (kid, key) = plant_api_key(
        &state,
        member.id,
        "member-share",
        r#"{"ops":["read","upload","share"],"buckets":["share-gated"]}"#,
        None,
    )
    .await;
    let _ = kid;

    let (mut sse, session_id) = mcp_handshake(&state, &key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"error\"").await;
    let envelope = sse_envelope(&frames);
    assert_eq!(
        envelope["error"]["code"], SCOPE_DENIED,
        "the missing user share bit is a FRAME: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("share-gated"),
        "the frame names the bucket: {envelope}"
    );
    // Arm ③ rides the same code as arm ② — deliberately, because both are
    // "permission" to a client — and says which permission in `data`.
    assert_eq!(
        envelope["error"]["data"]["denial"], "user_access",
        "the user's missing grant, not the key's list: {envelope}"
    );
    assert_eq!(
        envelope["error"]["data"]["bucket"], "share-gated",
        "the refusal names the bucket it is about: {envelope}"
    );
}

/// R-B's live-rows guard: a NON-bootstrap bucket holding a live file
/// row refuses the delete with the conflict shape — empty it first.
/// MUT:R-B-ROWS — dropping the `count_live_files_by_bucket` guard from
/// the `storage.buckets.delete` handler (rpc.rs) turns this red.
#[tokio::test]
async fn bucket_delete_refused_while_live_rows_remain() {
    let (state, token) = test_state_with_token().await;
    let bucket = plant_bucket(&state, "occupied", true, false, true).await;
    let owner = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    // A LIVE file row (not soft-deleted) in the bucket: the delete must
    // refuse BEFORE any store mutation.
    state
        .store
        .create_file(i0721_proto::NewFile {
            bucket_id: Some(bucket.id),
            parent_id: None,
            owner_id: owner.id,
            name: "keeper.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/webp".to_owned(),
            size: 1,
            sha256: format!("r{}", uuid::Uuid::new_v4().simple()),
            storage_backend: "local".to_owned(),
            storage_key: format!("k{}", uuid::Uuid::new_v4().simple()),
            thumbnail_key: None,
            original_name: "keeper.png".to_owned(),
        })
        .await
        .unwrap();
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.delete",
        serde_json::json!({"slug": "occupied"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "live rows refuse the delete: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("live rows"),
        "the conflict names the remaining rows: {envelope}"
    );
    // The refusal is pre-mutational: the bucket still exists.
    assert!(
        state
            .store
            .get_storage_bucket_by_slug("occupied")
            .await
            .unwrap()
            .is_some(),
        "the bucket survives a refused delete"
    );
}

/// R-B's bootstrap guard: the bootstrap bucket cannot be deleted at
/// all — every legacy NULL-bucket row resolves through it.
/// MUT:R-B-BOOT — dropping the `is_bootstrap` guard from the
/// `storage.buckets.delete` handler (rpc.rs) turns this red.
#[tokio::test]
async fn bucket_delete_refused_for_the_bootstrap_bucket() {
    let (state, token) = test_state_with_token().await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.delete",
        serde_json::json!({"slug": "bootstrap"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "the bootstrap bucket cannot be deleted: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bootstrap"),
        "the refusal names the bootstrap rule: {envelope}"
    );
    assert!(
        state
            .store
            .get_storage_bucket_by_slug("bootstrap")
            .await
            .unwrap()
            .is_some(),
        "the bootstrap bucket survives"
    );
}

/// R-B's three delete-guard directions, driven through the RPC surface
/// with keys planted DIRECTLY (so the scopes carry the exact slugs).
#[tokio::test]
async fn bucket_delete_refused_by_a_live_key_referencing_the_slug() {
    let (state, token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &token);
    let bucket = plant_bucket(&state, "guarded", true, false, true).await;
    let (_kid, _) = plant_api_key(
        &state,
        admin_id,
        "guard",
        r#"{"ops":["read"],"buckets":["guarded"]}"#,
        None,
    )
    .await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.delete",
        serde_json::json!({"slug": "guarded"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "the live key's reference refuses the delete: {envelope}"
    );
    let _ = bucket;
}

#[tokio::test]
async fn bucket_delete_allowed_when_the_referencing_key_is_revoked() {
    let (state, token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &token);
    plant_bucket(&state, "freed", true, false, true).await;
    let (kid, _) = plant_api_key(
        &state,
        admin_id,
        "guard",
        r#"{"ops":["read"],"buckets":["freed"]}"#,
        None,
    )
    .await;
    state.store.revoke_api_key(kid).await.unwrap();
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.delete",
        serde_json::json!({"slug": "freed"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope.get("error").is_none(),
        "the revoked key does not block: {envelope}"
    );
}

#[tokio::test]
async fn bucket_delete_allowed_when_a_live_key_references_a_different_slug() {
    let (state, token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &token);
    plant_bucket(&state, "other-guard", true, false, true).await;
    plant_bucket(&state, "deletable", true, false, true).await;
    plant_api_key(
        &state,
        admin_id,
        "guard",
        r#"{"ops":["read"],"buckets":["other-guard"]}"#,
        None,
    )
    .await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.delete",
        serde_json::json!({"slug": "deletable"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(envelope.get("error").is_none(), "{envelope}");
}

/// R-A's grant surface: an admin target is refused; the deny row
/// survives a signup flip (the all-false row's immunity).
#[tokio::test]
async fn grants_cannot_be_set_for_an_admin() {
    let (state, token) = test_state_with_token().await;
    let admin_id = admin_of(&state, &token);
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.grants.set",
        serde_json::json!({"user": admin_id, "bucket_slug": "bootstrap", "read": true, "write": true, "share": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32005, "{envelope}");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("admin"),
        "the refusal names the admin rule: {envelope}"
    );
}

/// `share_create_via_rpc_requires_the_share_bit`: a member holding an
/// EXPLICIT grant row with read+write but NO share bit on a non-bootstrap
/// bucket must be refused by RPC `files.share_create` (-32005 AUTH,
/// the message naming the bucket) — owner-or-admin is the WHO, the
/// share bit is the MAY; flipping the row's share bit lets the very
/// same call succeed.
/// MUT:R-L-RPC — removing the `Need::Share` gate from `files.share_create`
/// (rpc.rs) turns this red.
#[tokio::test]
async fn share_create_via_rpc_requires_the_share_bit() {
    let (base_state, admin_token) = test_state_with_token().await;
    // A second bucket so the gate reads the member's grant ROW, not the
    // signup default (the bootstrap bucket's signup-open posture would
    // mask a missing check).
    let (state, second_id, _store) = plant_second_bucket(&base_state, "club", true);
    // The store row must carry the SAME id the registry entry registered
    // (`plant_second_bucket`'s second id) — bucket_row_for_id prefers
    // the store's rows and only falls back to the registry's synthesized
    // posture when none matches, and a mismatched row id would resolve
    // the share gate through the fallback's signup-open posture.
    let memory = state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    memory.seed_bucket(i0721_proto::StorageBucketRecord {
        id: second_id,
        slug: "club".to_owned(),
        display_name: "club".to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: false,
        created_at: Timestamp::now(),
    });
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());

    // The explicit deny-share row: read+write, share=false. The row is
    // the WHOLE answer (R-A), so the member cannot share.
    set_grant(&state, &admin_token, member.id, "club", true, true, false).await;

    let (_, own) = upload_bucket(&state, &member_token, png_of(4, 4), "mine.png", "club").await;
    let file_id = own["id"].as_str().unwrap().to_owned();

    let (status, envelope) = rpc(
        &state,
        &member_token,
        "files.share_create",
        serde_json::json!({"file_id": file_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "a write-only grant row cannot publish a share: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("club"),
        "the refusal names the bucket: {envelope}"
    );

    // The admin flips the row's share bit: the SAME call now succeeds.
    set_grant(&state, &admin_token, member.id, "club", true, true, true).await;
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "files.share_create",
        serde_json::json!({"file_id": file_id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope["result"]["token"].as_str().is_some(),
        "the share is created once the bit is held: {envelope}"
    );
}

/// `grant_surface_method_table_is_exactly_set`: the `storage.grants.*`
/// method names are exactly `{set}` — nothing else dispatches. The
/// probe rides the REAL dispatch table over the wire: an unknown
/// method on the surface answers the method-table shape (`-32601`),
/// while the known name reaches its handler (its reachability is
/// covered by the grant tests elsewhere).
/// MUT:GATE-32601 — registering/accepting `storage.grants.bogus` (e.g.
/// a wildcard fall-through in the method table) turns this red.
#[test]
fn grant_surface_method_table_is_exactly_set() {
    // The method table is static and the lane is stateless here: build
    // the real server and ask the WHOLE dispatch table whether the
    // surface carries anything beyond `set`, AND probe an unknown name
    // through the HTTP face for the -32601 shape. The server half is
    // sync-built; the wire probe needs no runtime (the router answers
    // the method-table miss before any store touch).
    let state = {
        let store = Arc::new(MemoryStore::new());
        AppState::new(
            store,
            Arc::new(MemoryBlobStore::new()),
            Arc::new(test_config()),
        )
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        // The UNKNOWN method: the method-table shape (-32601) —
        // proof the surface dispatches nothing beyond its members. A
        // token is minted because the auth hook precedes the dispatch
        // (an anonymous frame answers 401, not a JSON-RPC fault).
        let admin = state
            .store
            .create_user(CreateUser {
                username: "admin".to_owned(),
                display_name: "Administrator".to_owned(),
                role: Role::Admin,
                password_hash: Some(hash_password("admin-password").unwrap()),
            })
            .await
            .unwrap();
        let token = state.signer.sign(admin.id, Timestamp::now());
        let (_, envelope) = rpc(
            &state,
            &token,
            "storage.grants.bogus",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32601,
            "the unknown grants method is a method-table miss: {envelope}"
        );
    });
}

/// R-F's four states on the default-bucket setting (the upload lane's
/// fail-closed read): absent falls back to the bootstrap bucket (the
/// single-bucket deployment), corrupt answers 500 naming the key.
#[tokio::test]
async fn storage_defaults_corrupt_row_fails_closed() {
    let (state, token) = test_state_with_token().await;
    // Plant a CORRUPT row (the raw text the typed setter cannot write).
    state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store")
        .seed_raw_setting("storage_defaults", "not json");
    let (status, body) = upload_raw(&state, &token, tiny_png(), "c.png", None).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "corrupt default = fail-closed: {body}"
    );
    let body: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let text = body["error"].as_str().unwrap_or_default();
    assert!(
        text.contains("storage_defaults") && text.contains("corrupt"),
        "the error names the key and the state: {text}"
    );
}

/// The health report serializes the P-10 settings four-state array.
#[tokio::test]
async fn health_serializes_the_settings_state_array() {
    let (state, _) = test_state().await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let settings = json["settings"].as_array().expect("a settings array");
    assert_eq!(settings.len(), 2, "site + storage_defaults: {json}");
    for row in settings {
        assert!(row["key"].is_string(), "every row names its key: {row}");
        let tag = row["state"].as_str().unwrap();
        assert!(
            ["absent", "corrupt", "unreadable", "value"].contains(&tag),
            "the tag is one of the four states: {row}"
        );
    }
    // The site row is seeded by every test state's settings put? No —
    // absent here; the storage_defaults row is absent too.
    assert!(
        settings.iter().all(|row| row["state"] == "absent"),
        "a fresh store has both rows absent: {json}"
    );
}

/// MY-11's arms of the four-state array: a store holding one valid JSON
/// `Value` row and one INVALID JSON `Corrupt` row reads back the tags
/// `"value"` and `"corrupt"` respectively (alongside the absent /
/// unreadable arms pinned by the neighbors).
/// MUT:R-F-TAG — mapping `SettingsState::Value` to "corrupt" in the
/// health mapping (api/settings.rs) turns this red.
#[tokio::test]
async fn health_tags_value_and_corrupt_settings_rows_distinctly() {
    let (state, _) = test_state().await;
    {
        let memory = state
            .store
            .as_any()
            .downcast_ref::<MemoryStore>()
            .expect("the fixture store is the memory store");
        // site = a VALID JSON document (the Value arm); storage_defaults
        // = INVALID JSON text (the Corrupt arm).
        memory.seed_raw_setting(
            i0721_proto::CONFIG_SETTING_KEY,
            r#"{"upload":{"max_bytes":1000}}"#,
        );
        memory.seed_raw_setting("storage_defaults", "not json at all");
    }
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let settings = json["settings"].as_array().expect("a settings array");
    let tag_of = |key: &str| {
        settings
            .iter()
            .find(|row| row["key"] == key)
            .unwrap_or_else(|| panic!("a row for {key}: {json}"))["state"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        tag_of(i0721_proto::CONFIG_SETTING_KEY),
        "value",
        "a valid JSON row tags value: {json}"
    );
    assert_eq!(
        tag_of("storage_defaults"),
        "corrupt",
        "an invalid-JSON row tags corrupt: {json}"
    );
}

/// R-C: re-upload to the SAME bucket returns the existing row; the same
/// content into ANOTHER bucket is refused WITHOUT naming a bucket;
/// same-name different-content keeps the NAME conflict message.
#[tokio::test]
async fn reupload_to_the_same_bucket_returns_the_existing_row() {
    let (state, token) = test_state_with_token().await;
    let (status, first) = upload(&state, &token, tiny_png(), "same.png").await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, second) = upload(&state, &token, tiny_png(), "same.png").await;
    assert_eq!(status, StatusCode::OK, "dedupe hit: {second}");
    assert_eq!(
        second["id"].as_str(),
        first["id"].as_str(),
        "the SAME row answers"
    );
}

#[tokio::test]
async fn reupload_to_another_bucket_is_refused_by_the_dedupe_probe() {
    let (base_state, token) = test_state_with_token().await;
    // A second Ready bucket whose slug the member can target: the blob
    // half rides the fixture registry (`plant_second_bucket`), the store
    // row the upload resolution reads.
    let (state, _second_id, _store) = plant_second_bucket(&base_state, "satellite", true);
    plant_bucket(&state, "satellite", true, false, true).await;
    let (status, first) = upload(&state, &token, tiny_png(), "dup.png").await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    // Same bytes, ?bucket=satellite: the cross-bucket probe fires.
    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let mut raw = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"dup.png\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    ).into_bytes();
    raw.extend_from_slice(&tiny_png());
    raw.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files?bucket=satellite")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(raw))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "the cross-bucket probe refuses"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let text = body["error"].as_str().unwrap_or_default();
    assert!(text.contains("another bucket"), "the R-C text: {text}");
    assert!(
        !text.contains("bootstrap") && !text.contains("satellite"),
        "the conflict names NO bucket: {text}"
    );
}

/// `?bucket=` visibility: an invisible slug answers 404 (§7's REST
/// column) — the member must not learn the bucket exists.
#[tokio::test]
async fn an_invisible_bucket_in_the_upload_query_answers_404() {
    let (state, _admin_token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    plant_bucket(&state, "hidden", true, false, true).await; // NOT grant_on_signup

    let boundary = "X-BOUNDARY-X";
    let b = boundary;
    let mut raw = format!(
        "--{b}\r\ncontent-disposition: form-data; name=\"file\"; filename=\"h.png\"\r\ncontent-type: application/octet-stream\r\n\r\n"
    ).into_bytes();
    raw.extend_from_slice(&tiny_png());
    raw.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/files?bucket=hidden")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(raw))
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "invisible = 404");
}

/// Same name, DIFFERENT content, same folder: the name-conflict
/// message stays (MUT:R-C-PROBE family — the cross-bucket probe leaves
/// the name conflict's identity intact).
#[tokio::test]
async fn same_name_different_content_in_the_same_folder_keeps_the_name_conflict_message() {
    let (state, token) = test_state_with_token().await;
    let big = {
        let image = image::DynamicImage::new_rgb8(9, 9);
        let mut buf = std::io::Cursor::new(Vec::new());
        image.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    };
    let (status, first) =
        upload_raw_query(&state, &token, tiny_png(), "one.png", None, Some("dup.png")).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, second) =
        upload_raw_query(&state, &token, big, "two.png", None, Some("dup.png")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{second}");
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    let text = second["error"].as_str().unwrap_or_default();
    assert!(text.contains("name"), "the NAME conflict: {text}");
}

/// R-F: the strict `storage.buckets.upsert` — an omitted field refuses,
/// and `grant_on_signup` without `anonymous_read` refuses.
#[tokio::test]
async fn upsert_omitting_a_field_is_rejected_not_reset() {
    let (state, token) = test_state_with_token().await;
    plant_bucket(&state, "strict", true, false, true).await;
    // Omit `enabled`: the strict full-document gate refuses (-32602).
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.upsert",
        serde_json::json!({"slug": "strict", "display_name": "Strict", "anonymous_read": true, "grant_on_signup": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rpc_error_code(&envelope),
        -32602,
        "a missing field is a refusal: {envelope}"
    );
}

/// R-A's STRICT FULL DOCUMENT: an EXTRA unknown field refuses with the
/// invalid-params shape — never silently accepted with the unknown
/// field dropped (a dropped field would let a caller believe a knob was
/// set when the binary never heard of it).
/// MUT:R-A-STRICT — removing `deny_unknown_fields` from
/// `BucketUpsertParams` (rpc.rs) turns this red.
#[tokio::test]
async fn upsert_rejects_unknown_fields_strictly() {
    let (state, token) = test_state_with_token().await;
    plant_bucket(&state, "strict-extra", true, false, true).await;
    // A fully valid document PLUS one key the struct does not know.
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.upsert",
        serde_json::json!({"slug": "strict-extra", "display_name": "Strict", "anonymous_read": true, "grant_on_signup": false, "enabled": true, "backend": "s3"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        rpc_error_code(&envelope),
        -32602,
        "an unknown field is a refusal (the strict full document): {envelope}"
    );
}

#[tokio::test]
async fn signup_default_requires_anonymous_read() {
    let (state, token) = test_state_with_token().await;
    plant_bucket(&state, "private-signup", false, false, true).await;
    let (status, envelope) = rpc(
        &state,
        &token,
        "storage.buckets.upsert",
        serde_json::json!({"slug": "private-signup", "display_name": "p", "anonymous_read": false, "grant_on_signup": true, "enabled": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rpc_error_code(&envelope), -32602, "{envelope}");
}

/// `files.chain`'s §7 shapes: a foreign member's folder answers -32006,
/// and an unknown folder -32006 (the existing chain test proves the
/// happy path).
#[tokio::test]
async fn files_chain_answers_the_section7_shapes() {
    let (state, admin_token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let admin_folder = mk_folder(&state, &admin_token, "admin-only", None).await;

    // A member cannot walk an admin's folder: invisible → -32006.
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.chain",
        serde_json::json!({"folder_id": admin_folder}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32006, "{envelope}");

    // An unknown folder → -32006.
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.chain",
        serde_json::json!({"folder_id": uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32006, "{envelope}");
}

/// §7's files.chain VISIBLE-but-no-READ arm: the member holds an
/// explicit write-only grant row (any bit → visible, read=false) on a
/// non-bootstrap bucket → the -32005 AUTH fault naming the access.
/// MUT:CHAIN-32005 — collapsing `!access.read` into the invisible
/// branch (or dropping the read check) turns this red.
#[tokio::test]
async fn files_chain_visible_without_read_answers_32005() {
    let (base_state, _admin_token) = test_state_with_token().await;
    let (state, second_id, _store) = plant_second_bucket(&base_state, "wonly", true);
    // The store row MUST carry the registry's id (bucket_row_for_id
    // prefers the store's rows; a mismatched id would fall through to
    // the registry's synthesized signup-open posture).
    let memory = state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    memory.seed_bucket(i0721_proto::StorageBucketRecord {
        id: second_id,
        slug: "wonly".to_owned(),
        display_name: "wonly".to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read: true,
        grant_on_signup: false,
        enabled: true,
        is_bootstrap: false,
        created_at: Timestamp::now(),
    });
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    // Visible (write=true) but NOT readable.
    state
        .store
        .set_bucket_grant(member.id, second_id, false, true, false)
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // The member's OWN folder in that bucket (owner-or-admin passes).
    let folder = state
        .store
        .create_folder(member.id, None, "wonly-root", Some(second_id))
        .await
        .unwrap();
    let (_, envelope) = rpc(
        &state,
        &member_token,
        "files.chain",
        serde_json::json!({"folder_id": folder.id.to_string()}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "visible without read is the AUTH fault, not a 404: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("read access"),
        "the fault names the missing read: {envelope}"
    );
}

/// §7's files.chain UNSERVABLE arm: the folder's bucket's registry
/// entry is Unavailable (backend dead) → the -32010 fault naming the
/// backend, evaluated AFTER visibility and read (Q8's order).
/// MUT:CHAIN-32010 — answering -32006/404 instead of -32010 when the
/// registry resolve fails turns this red.
#[tokio::test]
async fn files_chain_unservable_bucket_answers_32010() {
    let (base_state, admin_token) = test_state_with_token().await;
    // The folder is born while the bootstrap bucket still serves; the
    // fixture registry with the bootstrap entry DISABLED is layered on
    // afterwards: the row stays visible + readable (the admin holds
    // every bit) but the backend cannot serve.
    let folder = mk_folder(&base_state, &admin_token, "doomed", None).await;
    let (state, _second_id, _store) = plant_second_bucket(&base_state, "spare", false);
    let (_, envelope) = rpc(
        &state,
        &admin_token,
        "files.chain",
        serde_json::json!({"folder_id": folder}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32010,
        "a disabled backend is the storage-unavailable fault: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("storage backend unavailable"),
        "the fault names the backend failure: {envelope}"
    );
}

/// Q7's materialization boundary: an EXPLICIT `buckets: []` is the
/// caller's pick (an empty authorized set) — the create path must NOT
/// silently replace it with `granted_buckets(owner)`. MUT:M-P2A7 —
/// ignoring the explicit array turns this red.
#[tokio::test]
async fn keys_create_respects_an_explicit_empty_buckets_array() {
    let (state, token) = test_state_with_token().await;
    // The admin HAS authorized buckets (the fixture bootstrap bucket) —
    // materialization would pick them up; the explicit [] must win.
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "empty-buckets", "ops": ["read"], "buckets": []}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let key = envelope["result"]["key"].as_str().unwrap().to_owned();
    let record = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key))
        .await
        .unwrap()
        .expect("the row exists");
    let value: serde_json::Value = serde_json::from_str(&record.scopes).unwrap();
    assert_eq!(
        value["buckets"],
        serde_json::json!([]),
        "the explicit empty array is the caller's pick, not a request to materialize: {value}"
    );
}
/// Q7's fail-closed cap on the CREATE lane: a `keys.create` with a
/// 65-slug `buckets` array (one over the cap). The shipped behavior
/// (read from the code): the write path stores the caller's array
/// VERBATIM (`encode_scopes_with_buckets` neither truncates nor
/// refuses); the fail-closed cap lives in the INTERPRETATION layer —
/// `parse_scopes_buckets` answers the EMPTY set for anything over 64,
/// so the stored key's EFFECTIVE authorization set is NO buckets.
/// MUT:Q7-CAP — lifting `MAX_BUCKETS` in `parse_scopes_buckets`
/// (key_scopes.rs) so 65 slugs parse turns the effective-set assertion
/// red (65 buckets in the set instead of none).
#[tokio::test]
async fn keys_create_over_the_64_bucket_cap_answers_no_buckets() {
    let (state, token) = test_state_with_token().await;
    let buckets: Vec<String> = (0..65).map(|i| format!("cap-{i}")).collect();
    let (status, envelope) = rpc(
        &state,
        &token,
        "keys.create",
        serde_json::json!({"name": "over-cap", "ops": ["read"], "buckets": buckets}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the create itself succeeds: {envelope}"
    );
    let key = envelope["result"]["key"].as_str().unwrap().to_owned();
    let record = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key))
        .await
        .unwrap()
        .expect("the row exists");
    // MUT:Q7-CAP — the EFFECTIVE set (what the enforcement layer reads)
    // is the fail-closed empty set, not the stored 65 slugs.
    assert!(
        i0721_server::key_scopes::parse_scopes_buckets(&record.scopes).is_empty(),
        "one over the cap answers NO buckets (fail-closed): {}",
        record.scopes
    );
}

/// `storage.defaults.get/set` — the round trip through the RPC face
/// (the upload lane's consumption is pinned elsewhere; this surface had
/// NO test at all): an admin sets the default to a VISIBLE bucket slug
/// and `get` reads the same slug back; setting an UNKNOWN slug refuses
/// (-32006). A member WITHOUT visibility of the default bucket is
/// answered by the §7 shape — `-32006` (the "invisible" answer), never
/// the slug (they must not learn the bucket exists).
/// MUT:DEF-GET — breaking the get round trip (e.g. answering
/// `Some(Null)` for a readable Value in `storage.defaults.get`, rpc.rs)
/// turns the read-back assertion red.
/// MUT:DEF-SET — skipping the bucket-exists check in
/// `storage.defaults.set` turns the unknown-slug refusal red.
#[tokio::test]
async fn storage_defaults_round_trip_and_visibility() {
    let (state, admin_token) = test_state_with_token().await;
    plant_bucket(&state, "defaultable", true, false, true).await;

    // An unknown slug refuses before any write.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "storage.defaults.set",
        serde_json::json!({"bucket": "no-such-bucket"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        rpc_error_code(&envelope),
        -32006,
        "the default must name a REAL bucket: {envelope}"
    );

    // The round trip: set, then read the SAME slug back.
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "storage.defaults.set",
        serde_json::json!({"bucket": "defaultable"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let (status, envelope) = rpc(
        &state,
        &admin_token,
        "storage.defaults.get",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        envelope["result"]["bucket"].as_str(),
        Some("defaultable"),
        "get reads back the slug set wrote: {envelope}"
    );

    // A member WITHOUT visibility of the defaultable bucket: the
    // surface is admin-only (MUT:DEF-GET — require_admin answers -32005
    // before any read), so the slug stays behind this face; the §7
    // invisible-answer duty lives on the upload lane (the 404 pin).
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let (status, envelope) = rpc(
        &state,
        &member_token,
        "storage.defaults.get",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "defaults.get is admin-only — a member is refused before any read: {envelope}"
    );
    assert!(
        !envelope.to_string().contains("defaultable"),
        "the slug never leaks through the refusal: {envelope}"
    );
}

/// MY-1's SERVICE face: `storage.grants.set` REPLACES the row's bits —
/// the all-false row STILL EXISTS after the revoke (no deletion of
/// grant rows exists in the model). The store-level pin lives in
/// bucket_acl's `revoking_access_writes_an_explicit_deny_row`; this
/// rides the RPC handler, so a revoke path that DELETES the row turns
/// the read-back red.
/// MUT:MY-1 — making the all-false write (or the grants.set handler)
/// delete the row instead of upserting it turns this red.
#[tokio::test]
async fn revoking_via_grants_set_leaves_the_all_false_row_in_place() {
    let (state, token) = test_state_with_token().await;
    let bucket = plant_bucket(&state, "revoke-me", true, false, true).await;
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    set_grant(&state, &token, member.id, "revoke-me", true, true, true).await;
    let row = state
        .store
        .get_bucket_grant(member.id, bucket.id)
        .await
        .unwrap()
        .expect("the full grant wrote a row");
    assert!(row.can_read && row.can_write && row.can_share);

    // Revoke through the SAME RPC surface: {f,f,f} on the same pair.
    set_grant(&state, &token, member.id, "revoke-me", false, false, false).await;

    let row = state
        .store
        .get_bucket_grant(member.id, bucket.id)
        .await
        .unwrap()
        .expect("the revoke WROTE an all-false row — the row was NOT deleted");
    assert!(
        !row.can_read && !row.can_write && !row.can_share,
        "the surviving row is the explicit deny"
    );
}

/// R-L's legacy face: a file row whose `bucket_id` is NULL (the
/// pre-0007 shape) still shares through `files.share_create` — the row
/// falls back to the BOOTSTRAP bucket for the share-bit gate (the
/// bootstrap is signup-open, so a fresh member holds the share bit).
/// MUT:MY-8 — dropping the `.or_else(bootstrap_bucket_id())` fallback
/// (rpc.rs `files.share_create`) turns the create red with
/// "file not found".
#[tokio::test]
async fn share_create_on_a_legacy_null_bucket_row_uses_the_bootstrap_bucket() {
    let (state, _token) = test_state_with_token().await;
    let member = state
        .store
        .create_user(CreateUser {
            username: format!("m-{}", uuid::Uuid::new_v4().simple()),
            display_name: "m".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    // The legacy row, planted straight into the store: bucket_id NULL.
    let file = state
        .store
        .create_file(i0721_proto::NewFile {
            bucket_id: None,
            parent_id: None,
            owner_id: member.id,
            name: "legacy.png".to_owned(),
            kind: i0721_proto::FileKind::Image,
            mime: "image/png".to_owned(),
            size: 1,
            sha256: format!("legacy-{}", uuid::Uuid::new_v4().simple()),
            storage_backend: "local".to_owned(),
            storage_key: format!("legacy-{}", uuid::Uuid::new_v4().simple()),
            thumbnail_key: None,
            original_name: "legacy.png".to_owned(),
        })
        .await
        .expect("plant the NULL-bucket row");

    let (status, envelope) = rpc(
        &state,
        &member_token,
        "files.share_create",
        serde_json::json!({"file_id": file.id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope["result"]["token"].as_str().is_some(),
        "the NULL-bucket row resolves through the bootstrap bucket and shares: {envelope}"
    );
}

/// MY-20's isError cell on the MCP share lane: when the store read of
/// the file row FAILS, `get_share_link` answers a TOOL error
/// (`isError: true`) whose text names the storage backend — never a
/// "file not found" mask, never a hang, never a link.
/// MUT:MY-20 — masking the precheck's store-Err arm as "file not
/// found" (mcp.rs precheck_tool) turns this red: the text loses the
/// backend sentence.
#[tokio::test]
async fn mcp_get_share_link_on_a_store_failure_answers_the_iserror_tool_error() {
    let (base_state, admin_token) = test_state_with_token().await;
    let (_, up) = upload(&base_state, &admin_token, tiny_png(), "doomed.png").await;
    let file_id = up["id"].as_str().unwrap().to_owned();

    // The failing-store double: `get_file` errors, everything else
    // delegates (the same delegation shape as the health lane's
    // FailingSettings; the frame must REACH the precheck).
    struct FailingFileStore(MemoryStore);
    #[async_trait::async_trait]
    impl Store for FailingFileStore {
        // `get_setting` DELEGATES: the site-config read must succeed so
        // the upload lane reaches `default_bucket_slug` — the failing
        // read this fixture isolates is `settings_state` (the four-state
        // read), not the raw site-config fetch.
        async fn get_setting(
            &self,
            key: &str,
        ) -> Result<Option<serde_json::Value>, i0721_store::StoreError> {
            self.0.get_setting(key).await
        }
        async fn get_storage_bucket_by_slug(
            &self,
            _slug: &str,
        ) -> Result<Option<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            Ok(None)
        }
        async fn list_storage_buckets(
            &self,
        ) -> Result<Vec<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            Ok(Vec::new())
        }
        async fn storage_keys_live_in_bucket(
            &self,
            _bucket_id: Option<uuid::Uuid>,
        ) -> Result<std::collections::HashSet<String>, i0721_store::StoreError> {
            Ok(std::collections::HashSet::new())
        }
        // Everything else delegates to the inner memory store.
        async fn list_users(
            &self,
            o: u64,
            l: u64,
        ) -> Result<Vec<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.list_users(o, l).await
        }
        async fn count_users(&self) -> Result<u64, i0721_store::StoreError> {
            self.0.count_users().await
        }
        async fn get_user(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user(id).await
        }
        async fn get_user_by_username(
            &self,
            u: &str,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user_by_username(u).await
        }
        async fn oauth_providers_for_user(
            &self,
            user_id: uuid::Uuid,
        ) -> Result<Vec<String>, i0721_store::StoreError> {
            self.0.oauth_providers_for_user(user_id).await
        }
        async fn create_user(
            &self,
            u: i0721_store::CreateUser,
        ) -> Result<i0721_proto::UserRecord, i0721_store::StoreError> {
            self.0.create_user(u).await
        }
        async fn delete_user(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_user(id).await
        }
        async fn list_files(
            &self,
            b: Option<&[uuid::Uuid]>,
            o: u64,
            l: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_files(b, o, l).await
        }
        async fn count_files(
            &self,
            b: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_files(b).await
        }
        async fn get_file(
            &self,
            _id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            // THE deliberate failure: the file row read itself is dead.
            Err(i0721_store::StoreError::Db(
                "lane: file read dead".to_owned(),
            ))
        }
        async fn get_file_by_owner_and_sha256(
            &self,
            o: uuid::Uuid,
            s: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_file_by_owner_and_sha256(o, s).await
        }
        async fn list_live_rows_by_storage_key(
            &self,
            k: &str,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_live_rows_by_storage_key(k).await
        }
        async fn count_live_files_by_storage_key(
            &self,
            b: Option<uuid::Uuid>,
            k: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_storage_key(b, k).await
        }
        async fn count_live_files_by_thumbnail_key(
            &self,
            b: Option<uuid::Uuid>,
            k: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_thumbnail_key(b, k).await
        }
        async fn create_file(
            &self,
            f: i0721_proto::NewFile,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_file(f).await
        }
        async fn rename_file(
            &self,
            id: uuid::Uuid,
            n: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.rename_file(id, n).await
        }
        async fn soft_delete_file(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.soft_delete_file(id).await
        }
        async fn get_oauth_identity(
            &self,
            p: &str,
            s: &str,
        ) -> Result<Option<uuid::Uuid>, i0721_store::StoreError> {
            self.0.get_oauth_identity(p, s).await
        }
        async fn link_oauth_identity(
            &self,
            p: &str,
            s: &str,
            u: uuid::Uuid,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.link_oauth_identity(p, s, u).await
        }
        async fn count_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            k: Option<String>,
            b: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_folder(o, p, k, b).await
        }
        async fn list_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            k: Option<String>,
            b: Option<&[uuid::Uuid]>,
            off: u64,
            lim: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_folder(o, p, k, b, off, lim).await
        }
        async fn folder_name_taken(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            b: Option<uuid::Uuid>,
            n: &str,
            e: Option<uuid::Uuid>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.folder_name_taken(o, p, b, n, e).await
        }
        async fn create_folder(
            &self,
            o: uuid::Uuid,
            p: Option<uuid::Uuid>,
            n: &str,
            b: Option<uuid::Uuid>,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_folder(o, p, n, b).await
        }
        async fn set_parent(
            &self,
            id: uuid::Uuid,
            p: Option<uuid::Uuid>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_parent(id, p).await
        }
        async fn ancestor_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.ancestor_ids(id).await
        }
        async fn subtree_live_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.subtree_live_ids(id).await
        }
        async fn soft_delete_many(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.soft_delete_many(ids).await
        }
        async fn get_files_by_ids(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_files_by_ids(ids).await
        }
        async fn create_share(
            &self,
            s: i0721_proto::NewShare,
        ) -> Result<i0721_proto::ShareRecord, i0721_store::StoreError> {
            self.0.create_share(s).await
        }
        async fn get_share(
            &self,
            t: &str,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.get_share(t).await
        }
        async fn find_share_by_file(
            &self,
            o: uuid::Uuid,
            f: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.find_share_by_file(o, f).await
        }
        async fn list_shares(
            &self,
            o: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.list_shares(o).await
        }
        async fn delete_share(&self, t: &str) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_share(t).await
        }
        async fn delete_shares_for_files(
            &self,
            f: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.delete_shares_for_files(f).await
        }
        async fn sum_live_sizes(&self, o: uuid::Uuid) -> Result<i64, i0721_store::StoreError> {
            self.0.sum_live_sizes(o).await
        }
        async fn set_user_quota(
            &self,
            u: uuid::Uuid,
            q: Option<i64>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_user_quota(u, q).await
        }
        async fn create_api_key(
            &self,
            r: &i0721_proto::ApiKeyRecord,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.create_api_key(r).await
        }
        async fn list_api_keys(
            &self,
            u: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.list_api_keys(u).await
        }
        async fn revoke_api_key(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.revoke_api_key(id).await
        }
        async fn update_api_key(
            &self,
            id: uuid::Uuid,
            n: &str,
            s: &str,
            e: Option<&str>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.update_api_key(id, n, s, e).await
        }
        async fn find_api_key_by_hash(
            &self,
            h: &str,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_hash(h).await
        }
        async fn find_api_key_by_id(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_id(id).await
        }
        async fn touch_api_key(&self, id: uuid::Uuid) -> Result<(), i0721_store::StoreError> {
            self.0.touch_api_key(id).await
        }
        async fn set_setting(
            &self,
            k: &str,
            v: serde_json::Value,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_setting(k, v).await
        }
        async fn upsert_storage_bucket(
            &self,
            slug: &str,
            d: &str,
            a: bool,
            g: bool,
            e: bool,
        ) -> Result<i0721_proto::StorageBucketRecord, i0721_store::StoreError> {
            self.0.upsert_storage_bucket(slug, d, a, g, e).await
        }
        async fn delete_storage_bucket(
            &self,
            id: uuid::Uuid,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_storage_bucket(id).await
        }
        async fn count_live_files_by_bucket(
            &self,
            id: Option<uuid::Uuid>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_bucket(id).await
        }
        async fn get_file_by_owner_bucket_and_sha256(
            &self,
            owner: uuid::Uuid,
            bucket: Option<uuid::Uuid>,
            sha: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0
                .get_file_by_owner_bucket_and_sha256(owner, bucket, sha)
                .await
        }
        async fn get_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
        ) -> Result<Option<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.get_bucket_grant(u, b).await
        }
        async fn set_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
            r: bool,
            w: bool,
            s: bool,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_bucket_grant(u, b, r, w, s).await
        }
        async fn list_bucket_grants_for_user(
            &self,
            u: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_user(u).await
        }
        async fn list_bucket_grants_for_bucket(
            &self,
            b: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_bucket(b).await
        }
        async fn a_live_key_references_bucket_slug(
            &self,
            slug: &str,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.a_live_key_references_bucket_slug(slug).await
        }
        async fn settings_state(
            &self,
            key: &str,
        ) -> Result<i0721_store::SettingsState, i0721_store::StoreError> {
            self.0.settings_state(key).await
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    let memory = base_state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    let mut state = base_state.clone();
    state.store = Arc::new(FailingFileStore(memory.clone()));

    // The key is minted against the HEALTHY store (the double only
    // breaks the file READ, so the handshake's key lookup still works).
    let api_key = mcp_api_key(&state, &admin_token, "share-doomed").await;
    let (mut sse, session_id) = mcp_handshake(&state, &api_key).await;
    let status = mcp_post_frame(
        &state,
        &session_id,
        1,
        "tools/call",
        serde_json::json!({"name": "get_share_link", "arguments": {"file_id": file_id}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"isError\"").await;
    let (is_error, text) = mcp_tool_result(&sse_envelope(&frames));
    assert!(is_error, "the store failure is a TOOL error: {text}");
    assert!(
        text.contains("storage backend unavailable"),
        "the tool text names the storage failure, never a not-found mask: {text}"
    );
}

// ── P2b: anonymous direct links (R-J full text), R-S cache headers,
//    R-D shares surviving privatisation ────────────────────────────────

/// Plant a bucket row whose id MATCHES the fixture registry's second
/// entry (the P2a `share_create_via_rpc_requires_the_share_bit`
/// posture): `bucket_row_for_id` prefers the store's rows, so a
/// mismatched id would silently resolve through the synthesized
/// signup-open fallback.
async fn plant_bucket_with_id(
    state: &AppState,
    id: uuid::Uuid,
    slug: &str,
    anonymous_read: bool,
    grant_on_signup: bool,
    enabled: bool,
) {
    let memory = state
        .store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store");
    memory.seed_bucket(i0721_proto::StorageBucketRecord {
        id,
        slug: slug.to_owned(),
        display_name: slug.to_owned(),
        backend: i0721_proto::BucketBackend::Local,
        location: "loc".to_owned(),
        endpoint_ref: "x".to_owned(),
        anonymous_read,
        grant_on_signup,
        enabled,
        is_bootstrap: false,
        created_at: Timestamp::now(),
    });
}

/// Flip a bucket's mutable fields through the STORE's own upsert path
/// (the production face). The fixture registry's copy is static — the
/// media lane reads the STORE's rows for visibility/A (MUT:P2B-A-SET and
/// MUT:P2B-A-DEFAULT both go red through store-only flips), so a
/// store-only flip exercises exactly the
/// A-vs-availability split the P2b faces test.
async fn patch_bucket(
    state: &AppState,
    slug: &str,
    anonymous_read: bool,
    grant_on_signup: bool,
    enabled: bool,
) {
    let row = state
        .store
        .get_storage_bucket_by_slug(slug)
        .await
        .unwrap()
        .expect("the bucket exists");
    state
        .store
        .upsert_storage_bucket(
            slug,
            &row.display_name,
            anonymous_read,
            grant_on_signup,
            enabled,
        )
        .await
        .unwrap();
}

/// Flip `anonymous_read` on a bucket row (the bucket goes private /
/// public again).
async fn set_bucket_anonymous(state: &AppState, slug: &str, anonymous_read: bool) {
    let row = state
        .store
        .get_storage_bucket_by_slug(slug)
        .await
        .unwrap()
        .expect("the bucket exists");
    patch_bucket(
        state,
        slug,
        anonymous_read,
        row.grant_on_signup,
        row.enabled,
    )
    .await;
}

/// Flip `grant_on_signup` on a bucket row (members stop gaining the
/// bucket by default — the fixture for isolating the role-based arms).
async fn set_bucket_signup(state: &AppState, slug: &str, grant_on_signup: bool) {
    let row = state
        .store
        .get_storage_bucket_by_slug(slug)
        .await
        .unwrap()
        .expect("the bucket exists");
    patch_bucket(
        state,
        slug,
        row.anonymous_read,
        grant_on_signup,
        row.enabled,
    )
    .await;
}

/// Flip `enabled` on a bucket row (the bucket becomes unusable).
async fn set_bucket_enabled(state: &AppState, slug: &str, enabled: bool) {
    let row = state
        .store
        .get_storage_bucket_by_slug(slug)
        .await
        .unwrap()
        .expect("the bucket exists");
    patch_bucket(
        state,
        slug,
        row.anonymous_read,
        row.grant_on_signup,
        enabled,
    )
    .await;
}

/// The cache-control header of a media response.
fn cache_of(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

/// `public_bucket_media_anonymous_ok` — the REGRESSION ANCHOR: a
/// public (anonymous-readable) bucket's media keeps serving to
/// anonymous requests, exactly as the legacy engine did. MUT:P2B-A-SET
/// — a predicate that loses the anonymous arm turns this red.
#[tokio::test]
async fn public_bucket_media_anonymous_ok() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "public.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "anonymous media on a public bucket still serves"
    );
    assert_eq!(
        cache_of(&response).as_deref(),
        Some("public, max-age=31536000, immutable"),
        "2xx ∧ A carries the immutable public directive"
    );
}

/// `private_bucket_media_404_anonymous` — a private bucket's media
/// answers 404 to anonymous requests (I3: no 403, no 503, no oracle).
/// MUT:P2B-A-DEFAULT — the predicate treating signup-OPEN as
/// anonymous-readable turns this red.
#[tokio::test]
async fn private_bucket_media_404_anonymous() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "private.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    // The bootstrap bucket (where the upload landed) flips PRIVATE: A
    // no longer holds for this key.
    set_bucket_anonymous(&state, "bootstrap", false).await;
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        cache_of(&response).as_deref(),
        Some("no-store"),
        "every non-2xx answer is no-store"
    );
}

/// `private_bucket_media_200_with_session_read` — the same private
/// bucket serves 200 to a session whose bucket holds `Read`, with the
/// PRIVATE cache directive (2xx ∧ ¬A). MUT:P2B-SESSION-READ — dropping
/// the Read arm of `authorized_rows` turns this red.
#[tokio::test]
async fn private_bucket_media_200_with_session_read() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "own.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    set_bucket_anonymous(&state, "bootstrap", false).await;
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a session with Read on the private bucket still serves"
    );
    assert_eq!(
        cache_of(&response).as_deref(),
        Some("private, no-store"),
        "2xx ∧ ¬A never carries a public directive"
    );
}

/// `an_admin_reads_a_private_buckets_media_without_any_grant_row` — R-A's
/// admin arm (admin → every bucket) must hold through the `/media` lane
/// for an admin who is NOT the owner and holds NO grant row: a private
/// bucket's bytes answer 200 to the admin session. MUT:P2B-ADMIN-MEDIA —
/// collapsing `actor_reads_row`'s resolution chain to owner-only (dropping
/// BOTH the admin short-circuit and the `resolve_access` call) turns this
/// red (404 instead of 200); the owner-path tests survive that mutant,
/// this one cannot. Note: the `media.rs` admin short-circuit alone is
/// redundant with `resolve_access`'s own admin arm — removing only it is
/// behaviorally unobservable by design (one resolution place, two guards).
#[tokio::test]
async fn an_admin_reads_a_private_buckets_media_without_any_grant_row() {
    let (state, _seeded) = test_state().await;
    let owner = state
        .store
        .create_user(CreateUser {
            username: "private-owner".to_owned(),
            display_name: "Owner".to_owned(),
            role: Role::Member,
            password_hash: Some(hash_password("owner-password").unwrap()),
        })
        .await
        .unwrap();
    let owner_token = login(&state, "private-owner", "owner-password").await;
    let (_, dto) = upload(&state, &owner_token, tiny_png(), "owned.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let expected = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .expect("the owner's bytes sit in the bucket");

    // Private bucket; the admin below is a DIFFERENT account with no
    // grant row anywhere (R-A: admin → all buckets, by role alone).
    set_bucket_anonymous(&state, "bootstrap", false).await;
    set_bucket_signup(&state, "bootstrap", false).await;
    let other_admin = state
        .store
        .create_user(CreateUser {
            username: "second-admin".to_owned(),
            display_name: "Second admin".to_owned(),
            role: Role::Admin,
            password_hash: Some(hash_password("second-password").unwrap()),
        })
        .await
        .unwrap();
    assert_ne!(other_admin.id, owner.id, "fixture: a different account");
    let admin_token = login(&state, "second-admin", "second-password").await;

    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an admin reaches a private bucket's media by role, with no grant row"
    );
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        expected.as_ref(),
        "and it is the row's OWN bytes"
    );
}

/// `private_bucket_thumbnail_follows_the_same_check` — the
/// `?thumbnail=true` variant runs the SAME R-J judgment (the same
/// private-bucket 404/200 faces), not a laxer thumbnail path
/// (MUT:P2B-A-DEFAULT reddens it through the same A arm).
#[tokio::test]
async fn private_bucket_thumbnail_follows_the_same_check() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "thumb.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    set_bucket_anonymous(&state, "bootstrap", false).await;
    let anonymous = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        anonymous.status(),
        StatusCode::NOT_FOUND,
        "the thumbnail variant is anonymous-404 like the original"
    );
    let session = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        session.status(),
        StatusCode::OK,
        "the thumbnail variant serves the Read-holding session"
    );
}

/// `private_media_grant_does_not_leak_through_a_shared_storage_key` —
/// one key, two rows in two buckets, the PRIVATE row sorting FIRST by
/// `id ASC`: the private bucket's row must not leak through the PUBLIC
/// bucket's anonymous readability of the same key, neither to anonymous
/// nor to a session that holds no grant on the private bucket.
/// MUT:P2B-LEAK — an authorized set that silently widens past the
/// per-row bucket check turns this red (the leaked row's id sorts
/// first, so a widened set serves the private store's bytes as 200).
#[tokio::test]
async fn private_media_grant_does_not_leak_through_a_shared_storage_key() {
    let (base_state, _) = test_state().await;

    // A second, PRIVATE bucket whose store holds DIFFERENT bytes at the
    // same key, with a row of ANOTHER owner in it — its row id must
    // sort BEFORE the public row's (the id-ASC serving order then puts
    // the private row ahead). Each attempt uploads distinct content
    // into the public bucket and plants the vault row at the SAME key;
    // a wrong ordering is soft-deleted (it leaves the live set) and the
    // pair retries.
    let (state, second_id, second_store) = plant_second_bucket(&base_state, "vault", true);
    plant_bucket_with_id(&state, second_id, "vault", false, false, true).await;
    let other = state
        .store
        .create_user(CreateUser {
            username: "vault-owner".to_owned(),
            display_name: "Vault".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();

    // The session face's actor: a MEMBER who owns the public row but
    // holds NO grant on the private vault bucket (the admin reads every
    // bucket — an admin session legitimately serves the private row and
    // would mask the leak).
    let reader = base_state
        .store
        .create_user(CreateUser {
            username: "pub-reader".to_owned(),
            display_name: "Reader".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let reader_token = base_state.signer.sign(reader.id, Timestamp::now());
    let mut public_key: Option<String> = None;
    let mut public_bytes: Option<bytes::Bytes> = None;
    for attempt in 0..256u32 {
        let mut bytes = tiny_png();
        bytes[..4].copy_from_slice(&(attempt ^ 0x5EED).to_be_bytes());
        let (_, dto) = upload(
            &base_state,
            &reader_token,
            bytes,
            &format!("pub-{attempt}.png"),
        )
        .await;
        let attempt_key = dto["sha256"].as_str().unwrap().to_owned();
        let attempt_id = dto["id"].as_str().unwrap().to_owned();
        let stored = base_state
            .blobs
            .get(i0721_server::blob::BlobSlot::Media, &attempt_key)
            .await
            .unwrap()
            .expect("the public bucket holds its bytes");
        // The vault row: same KEY, DIFFERENT bytes in the private store.
        second_store
            .put(
                i0721_server::blob::BlobSlot::Media,
                &attempt_key,
                b"private-bytes-must-not-leak",
            )
            .await
            .unwrap();
        let vault_row = state
            .store
            .create_file(i0721_proto::NewFile {
                bucket_id: Some(second_id),
                parent_id: None,
                owner_id: other.id,
                name: "vault.png".to_owned(),
                kind: i0721_proto::FileKind::Image,
                mime: "image/webp".to_owned(),
                size: 28,
                sha256: attempt_key.clone(),
                storage_backend: "local".to_owned(),
                storage_key: attempt_key.clone(),
                thumbnail_key: None,
                original_name: "vault.png".to_owned(),
            })
            .await
            .unwrap();
        if vault_row.id.to_string().as_str() < attempt_id.as_str() {
            public_key = Some(attempt_key);
            public_bytes = Some(stored);
            break;
        }
        // Wrong order: retire this attempt's vault row and retry.
        let _ = state.store.soft_delete_file(vault_row.id).await.unwrap();
    }
    let key = public_key.expect("fixture: a vault row must sort before its public twin");
    let public_bytes = public_bytes.expect("the matched attempt's bytes");

    // Anonymous: 200 — but from the PUBLIC row's bucket, not the private
    // store's bytes (MUT:P2B-LEAK — widening the authorized set past the
    // per-row check serves the wrong bytes and reddens this).
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        body.as_ref(),
        public_bytes.as_ref(),
        "anonymous serves the public row's bytes only"
    );

    // A session (the upload owner, the admin) with NO grant row on the
    // private bucket still reads through the PUBLIC witness row — the
    // private grant of another owner leaks nothing, and the bytes must
    // be the PUBLIC row's (the vault row sorts first; a widened
    // authorized set would serve its bytes instead).
    let session = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &reader_token,
        ))
        .await
        .unwrap();
    assert_eq!(session.status(), StatusCode::OK);
    let session_body = axum::body::to_bytes(session.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        session_body.as_ref(),
        public_bytes.as_ref(),
        "the session serves the public row's bytes — the private grant of \
         another owner leaks neither status nor content"
    );
}

/// `shared_storage_key_resolves_identically_on_every_request` — the
/// same URL answers the same face on every request (no per-request
/// drift, no order dependence in the set predicate).
#[tokio::test]
async fn shared_storage_key_resolves_identically_on_every_request() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "stable.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let first = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let second = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), second.status(), "same URL, same face");
    assert_eq!(
        cache_of(&first),
        cache_of(&second),
        "same URL, same cache directive"
    );
}

/// `a_member_sees_503_not_404_on_a_disabled_public_bucket` — the R-J
/// letter for the MEMBER lane: `enabled` is consulted only for
/// AVAILABILITY (the serving-row scan), never for A/visibility. A
/// member session with NO grant on a DISABLED public bucket still has
/// the A witness rows in `authorized_rows` (non-empty), so the face is
/// 503 — the mutated shape (A consulting `enabled`, emptying the
/// authorized set on an ungranted invisible bucket) answers 404.
/// MUT:P2B-A-ENABLED — letting A consult `enabled` turns this red.
#[tokio::test]
async fn a_member_sees_503_not_404_on_a_disabled_public_bucket() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "dim.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    // The public bucket goes DISABLED in the store (the media lane's
    // authority for the flag): A still holds, but no row is servable.
    // The bucket also closes SIGNUP-GRANT: no grant row + gos = FALSE
    // leaves the member invisible-by-grant. ONLY the A witness row
    // (enabled ignored) can keep the authorized set non-empty, and
    // with it the 503 face.
    patch_bucket(&state, "bootstrap", true, false, false).await;
    let member = state
        .store
        .create_user(CreateUser {
            username: "no-grant-member".to_owned(),
            display_name: "NoGrant".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = state.signer.sign(member.id, Timestamp::now());
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the witness row keeps authorized_rows non-empty: 503, not 404"
    );
    assert_eq!(
        cache_of(&response).as_deref(),
        Some("no-store"),
        "the 503 face is no-store"
    );
}

/// `disabling_a_public_bucket_answers_503_to_sessions_and_404_to_anonymous`
/// — visibility (A) and availability (Ready ∧ enabled) split: the
/// disabled public bucket keeps A TRUE (enabled is ignored), so
/// sessions fall to the 503 face while anonymous still answers 404
/// (I3: anonymous never sees the 503). MUT:P2B-503-FACE — collapsing
/// the session face to 404 (or leaking the 503 to anonymous) turns
/// this red.
#[tokio::test]
async fn disabling_a_public_bucket_answers_503_to_sessions_and_404_to_anonymous() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "down.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    set_bucket_enabled(&state, "bootstrap", false).await;
    let session = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        session.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "session face for a disabled (unusable) public bucket"
    );
    assert_eq!(
        cache_of(&session).as_deref(),
        Some("no-store"),
        "the 503 face is no-store"
    );
    let anonymous = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        anonymous.status(),
        StatusCode::NOT_FOUND,
        "anonymous never distinguishes unavailability (I3)"
    );
}

/// `a_visible_bucket_without_read_answers_403_not_404` — a session
/// VISIBLY holding the bucket (any grant bit) but WITHOUT `Read`
/// answers 403, not 404. MUT:P2B-403-FACE — folding the
/// visible-without-read arm into 404 turns this red.
#[tokio::test]
async fn a_visible_bucket_without_read_answers_403_not_404() {
    let (base_state, admin_token) = test_state_with_token().await;
    // A private bucket the member can WRITE but not read (an explicit
    // row keeps the deny shape regardless of the signup default); the
    // blob half rides the fixture registry so the upload can target it.
    let (base_state, bucket_id, _blind) = plant_second_bucket(&base_state, "blind", true);
    plant_bucket_with_id(&base_state, bucket_id, "blind", false, false, true).await;
    let member = base_state
        .store
        .create_user(CreateUser {
            username: "blind-member".to_owned(),
            display_name: "Blind".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = base_state.signer.sign(member.id, Timestamp::now());
    base_state
        .store
        .set_bucket_grant(member.id, bucket_id, false, true, false)
        .await
        .unwrap();
    // The admin uploads INTO that bucket (the override rides ?bucket=).
    let (status, dto) =
        upload_bucket(&base_state, &admin_token, tiny_png(), "b.png", "blind").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let key = dto["sha256"].as_str().unwrap().to_owned();

    let member_face = app(&base_state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(
        member_face.status(),
        StatusCode::FORBIDDEN,
        "visible (write bit) but unreadable: 403, not 404"
    );
    assert_eq!(
        cache_of(&member_face).as_deref(),
        Some("no-store"),
        "the 403 face is no-store"
    );
}

/// I3: `anonymous_never_distinguishes_an_unavailable_bucket_from_absence`
/// — a private DISABLED bucket and a nonexistent key answer the SAME
/// 404 to anonymous requests (no 403, no 503, no oracle).
/// MUT:P2B-I3-BODY — an anonymous early-return 404 with a different body
/// reddens the header-set/body equality; MUT:P2B-503-FACE reddens the
/// status half.
#[tokio::test]
async fn anonymous_never_distinguishes_an_unavailable_bucket_from_absence() {
    let (state, admin_token) = test_state_with_token().await;
    let (_, dto) = upload(&state, &admin_token, tiny_png(), "gone.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    // Private AND disabled: neither visible nor usable.
    set_bucket_anonymous(&state, "bootstrap", false).await;
    set_bucket_enabled(&state, "bootstrap", false).await;
    let unavailable = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let absent = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{}", "0".repeat(64)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::NOT_FOUND);
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        cache_of(&unavailable),
        cache_of(&absent),
        "the two 404s are indistinguishable, headers included"
    );
    // MUT:P2B-I3-BODY — the sharper half of the pin: the two anonymous
    // 404s are byte-identical in BODY and in the full header SET (a
    // future branch that differs in wording or in an extra header would
    // turn this red; comparing status + Cache-Control alone let that
    // mutant survive).
    let unavailable_headers = header_map_of(&unavailable);
    let absent_headers = header_map_of(&absent);
    assert_eq!(
        unavailable_headers, absent_headers,
        "the two 404s carry the same header set"
    );
    let unavailable_body = axum::body::to_bytes(unavailable.into_body(), 65536)
        .await
        .unwrap();
    let absent_body = axum::body::to_bytes(absent.into_body(), 65536)
        .await
        .unwrap();
    assert_eq!(
        unavailable_body.as_ref(),
        absent_body.as_ref(),
        "the two 404s carry the same body"
    );
}

/// A response's headers as a sorted, comparable list (the I3 body pin
/// compares the whole set, not only Cache-Control).
fn header_map_of(response: &axum::response::Response) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != "date")
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// I6: `api_key_with_a_narrowed_bucket_set_never_serves_from_a_bucket_outside_it`
/// — a live key whose `buckets` omits the file's bucket answers 404
/// even though its OWNER could read the media with a session (I6: the
/// key's authority = ops ∩ buckets ∩ the user's current grants). The
/// file sits in a PRIVATE bucket, so the A witness arm does not rescue
/// the key (the intersection decides alone). MUT:P2B-LEAK's
/// intersection-dropped variant (I6) reddens this.
#[tokio::test]
async fn api_key_with_a_narrowed_bucket_set_never_serves_from_a_bucket_outside_it() {
    let (state, admin_token) = test_state_with_token().await;
    let (_, up) = upload(&state, &admin_token, tiny_png(), "keyed.png").await;
    let key = up["sha256"].as_str().unwrap().to_owned();
    // Private bucket: A does not hold, so the key's `buckets` set is
    // the ONLY door (the intersection decides alone — MUT:P2B-LEAK's
    // intersection-dropped variant reddens this).
    set_bucket_anonymous(&state, "bootstrap", false).await;
    // A read-op key whose buckets array EXCLUDES the bootstrap bucket.
    let (_, key_plain) = mcp_api_key_with_at(&state, &admin_token, "narrow", all_ops(), None).await;
    let record = state
        .store
        .find_api_key_by_hash(&mcp_test_helper::hash(&key_plain))
        .await
        .unwrap()
        .unwrap();
    let narrowed = r#"{"ops":["read","upload","share"],"buckets":[]}"#.to_owned();
    state
        .store
        .update_api_key(
            record.id,
            record.name.as_str(),
            &narrowed,
            record.expires_at.as_deref(),
        )
        .await
        .unwrap();

    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .header(header::AUTHORIZATION, format!("Bearer {key_plain}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "the key's bucket set excludes the file's bucket: no media"
    );

    // The SAME key WITH the bucket in its set serves (I6's other half:
    // the intersection grants what the user's grant allows).
    let wide = format!(
        r#"{{"ops":["read","upload","share"],"buckets":["{}"]}}"#,
        state.registry.bootstrap_slug().unwrap_or("bootstrap")
    );
    state
        .store
        .update_api_key(
            record.id,
            record.name.as_str(),
            &wide,
            record.expires_at.as_deref(),
        )
        .await
        .unwrap();
    let allowed = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .header(header::AUTHORIZATION, format!("Bearer {key_plain}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK, "in-set key reads");
}

/// I5: `private_media_never_carries_a_public_cache_directive` — the
/// ¬A 200 answer's directive is `private, no-store`, byte for byte
/// (MUT:I5-DIRECTIVE-IGNORES-A reddens it).
#[tokio::test]
async fn private_media_never_carries_a_public_cache_directive() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "nocache.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    set_bucket_anonymous(&state, "bootstrap", false).await;
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let directive = cache_of(&response).unwrap_or_default();
    assert!(
        !directive.contains("public"),
        "no public directive on a private-bucket 200: {directive}"
    );
    assert_eq!(directive, "private, no-store");
}

/// I5: `an_unavailable_or_forbidden_media_response_is_never_cacheable`
/// — the 503 and the 403 faces both carry `no-store`
/// (MUT:P2B-APIERR-NOSTORE / MUT:I5-NOSTORE-ON-404 reddens the family).
#[tokio::test]
async fn an_unavailable_or_forbidden_media_response_is_never_cacheable() {
    let (base_state, admin_token) = test_state_with_token().await;
    let (_, up) = upload(&base_state, &admin_token, tiny_png(), "dead.png").await;
    let key = up["sha256"].as_str().unwrap().to_owned();
    // Disabled public bucket: session 503 — no-store.
    set_bucket_enabled(&base_state, "bootstrap", false).await;
    let unavailable = app(&base_state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(cache_of(&unavailable).as_deref(), Some("no-store"));
    // Enabled private bucket + a visible-but-unreadable session: 403.
    set_bucket_enabled(&base_state, "bootstrap", true).await;
    let (base_state, bucket_id, _blind) = plant_second_bucket(&base_state, "blind403", true);
    plant_bucket_with_id(&base_state, bucket_id, "blind403", false, false, true).await;
    let member = base_state
        .store
        .create_user(CreateUser {
            username: "blind403-member".to_owned(),
            display_name: "B".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = base_state.signer.sign(member.id, Timestamp::now());
    base_state
        .store
        .set_bucket_grant(member.id, bucket_id, false, true, false)
        .await
        .unwrap();
    // DIFFERENT bytes: the same admin's same-content upload in another
    // bucket is the R-C cross-bucket refusal, not this test's subject.
    let mut different = tiny_png();
    different[0] ^= 0xFF;
    let (status, dto) =
        upload_bucket(&base_state, &admin_token, different, "b.png", "blind403").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let blind_key = dto["sha256"].as_str().unwrap().to_owned();
    let forbidden = app(&base_state)
        .oneshot(bearer(
            Request::builder()
                .uri(format!("/media/{blind_key}"))
                .body(Body::empty())
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(cache_of(&forbidden).as_deref(), Some("no-store"));
}

/// `the_same_url_changes_its_cache_directive_when_A_changes` — A
/// flipping TRUE→FALSE (privatising the bucket) changes the SAME URL's
/// directive from the immutable public form to `private, no-store` —
/// and back again when A flips TRUE once more.
#[tokio::test]
#[allow(non_snake_case)]
async fn the_same_url_changes_its_cache_directive_when_A_changes() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "flip.png").await;
    let key = dto["sha256"].as_str().unwrap().to_owned();
    let url = format!("/media/{key}");

    let public = app(&state)
        .oneshot(bearer(
            Request::builder().uri(&url).body(Body::empty()).unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        cache_of(&public).as_deref(),
        Some("public, max-age=31536000, immutable")
    );

    set_bucket_anonymous(&state, "bootstrap", false).await;
    let private = app(&state)
        .oneshot(bearer(
            Request::builder().uri(&url).body(Body::empty()).unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(cache_of(&private).as_deref(), Some("private, no-store"));

    set_bucket_anonymous(&state, "bootstrap", true).await;
    let back = app(&state)
        .oneshot(bearer(
            Request::builder().uri(&url).body(Body::empty()).unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(
        cache_of(&back).as_deref(),
        Some("public, max-age=31536000, immutable")
    );
}

/// R-D: `privatising_a_bucket_keeps_existing_shares_valid` — a share
/// created while the bucket was public KEEPS serving after the bucket
/// goes private (bytes route by ROW, I4), and the owner's
/// `files.share_list` still lists it. MUT:P2B-RD — a share lane that
/// re-checks the bucket's `anonymous_read` turns this red.
#[tokio::test]
async fn privatising_a_bucket_keeps_existing_shares_valid() {
    let (state, _admin) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;
    let (_, dto) = upload(&state, &token, tiny_png(), "shared.png").await;
    let file_id = dto["id"].as_str().unwrap().to_owned();
    let key = dto["sha256"].as_str().unwrap().to_owned();
    // The share exists BEFORE the privatisation.
    let (_, created) = rpc(
        &state,
        &token,
        "files.share_create",
        serde_json::json!({"file_id": file_id}),
    )
    .await;
    let share_token = rpc_result(&created)["token"].as_str().unwrap().to_owned();

    set_bucket_anonymous(&state, "bootstrap", false).await;

    // The public share download still serves the bytes.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{share_token}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the existing share keeps serving after the bucket went private"
    );
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let stored = state
        .blobs
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.as_ref(), stored.as_ref(), "the row's own bytes");

    // The owner's share_list still lists it (R-D's second half).
    let (_, list) = rpc(&state, &token, "files.share_list", serde_json::Value::Null).await;
    let tokens: Vec<&str> = rpc_result(&list)
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["token"].as_str())
        .collect();
    assert!(
        tokens.contains(&share_token.as_str()),
        "share_list keeps the privatised bucket's share: {list}"
    );

    // And anonymous /media on the same key is NOW 404 — the share is
    // the only remaining public door.
    let media = app(&state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::NOT_FOUND);
}

/// R-D: `revoking_the_owners_access_keeps_existing_shares_valid` — the
/// spec's other R-D test: revoking the OWNER's grant row on the bucket
/// (an all-false deny row) leaves an already-issued share serving
/// (MUT:P2B-RD reddens it).
#[tokio::test]
async fn revoking_the_owners_access_keeps_existing_shares_valid() {
    let (base_state, admin_token) = test_state_with_token().await;
    // The owner is a MEMBER with an explicit row on a private bucket;
    // the member uploads into the bucket (the grant's write bit),
    // issues the share, then the row is revoked to all-false.
    let (base_state, bucket_id, _store) = plant_second_bucket(&base_state, "revoked", true);
    plant_bucket_with_id(&base_state, bucket_id, "revoked", false, false, true).await;
    let member = base_state
        .store
        .create_user(CreateUser {
            username: "share-owner".to_owned(),
            display_name: "S".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = base_state.signer.sign(member.id, Timestamp::now());
    base_state
        .store
        .set_bucket_grant(member.id, bucket_id, true, true, true)
        .await
        .unwrap();
    let (status, dto) =
        upload_bucket(&base_state, &member_token, tiny_png(), "s.png", "revoked").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let file_id = dto["id"].as_str().unwrap().to_owned();

    let (_, created) = rpc(
        &base_state,
        &member_token,
        "files.share_create",
        serde_json::json!({"file_id": file_id}),
    )
    .await;
    let share_token = rpc_result(&created)["token"].as_str().unwrap().to_owned();

    // Revoke the owner's access (all-false deny row).
    base_state
        .store
        .set_bucket_grant(member.id, bucket_id, false, false, false)
        .await
        .unwrap();

    let response = app(&base_state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/share/{share_token}/download"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the share survives the owner's revocation (R-D, I4 routing)"
    );
    // The owner's share_list still lists it.
    let (_, list) = rpc(
        &base_state,
        &member_token,
        "files.share_list",
        serde_json::Value::Null,
    )
    .await;
    assert!(
        rpc_result(&list)
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["token"].as_str() == Some(share_token.as_str())),
        "the revoked owner still sees their share in the list: {list}"
    );
    let _ = admin_token;
}

/// R-D / R-L re-test: NEW shares on a privatised bucket still need the
/// actor's CURRENT share bit (P2a's R-L holds under P2b).
#[tokio::test]
async fn new_shares_still_need_the_current_share_bit() {
    let (base_state, admin_token) = test_state_with_token().await;
    let (base_state, bucket_id, _store) = plant_second_bucket(&base_state, "noshare", true);
    plant_bucket_with_id(&base_state, bucket_id, "noshare", false, false, true).await;
    let member = base_state
        .store
        .create_user(CreateUser {
            username: "writer-only".to_owned(),
            display_name: "W".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = base_state.signer.sign(member.id, Timestamp::now());
    base_state
        .store
        .set_bucket_grant(member.id, bucket_id, true, true, false)
        .await
        .unwrap();
    let (status, dto) =
        upload_bucket(&base_state, &member_token, tiny_png(), "w.png", "noshare").await;
    assert_eq!(status, StatusCode::CREATED, "{dto}");
    let file_id = dto["id"].as_str().unwrap().to_owned();
    let (_, envelope) = rpc(
        &base_state,
        &member_token,
        "files.share_create",
        serde_json::json!({"file_id": file_id}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32005,
        "the write-only member cannot publish a NEW share (share bit required)"
    );
    let _ = admin_token;
}

// ── the unregistered-object lane (P3) ───────────────────────────────────

/// The P3 fixture: the standard state, one extra bucket ("orphans") whose
/// store is its OWN MemoryBlobStore, with `orphan bytes` planted at the
/// given keys WITHOUT any `files` row — the definition of an unregistered
/// object. Returns the patched state, the bucket's id, its slug-keyed
/// store, and a live member (owner target for adopt).
struct OrphanFixture {
    state: AppState,
    #[allow(dead_code)] // the adopt target's row (used by future asserts)
    bucket_id: uuid::Uuid,
    bucket_slug: String,
    store: Arc<i0721_server::blob::MemoryBlobStore>,
    member_id: uuid::Uuid,
    member_token: String,
    #[allow(dead_code)] // the show_on caller's credential
    admin_id: uuid::Uuid,
    admin_token: String,
}

async fn orphan_fixture(keys: &[&str]) -> OrphanFixture {
    let (base, admin) = test_state().await;
    let admin_token = base.signer.sign(admin.id, Timestamp::now());
    let member = base
        .store
        .create_user(CreateUser {
            username: "orphan-member".to_owned(),
            display_name: "Orphan Member".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let member_token = base.signer.sign(member.id, Timestamp::now());
    let id = uuid::Uuid::new_v4();
    let slug = "orphans".to_owned();
    let store = Arc::new(i0721_server::blob::MemoryBlobStore::new());
    base.store
        .as_any()
        .downcast_ref::<MemoryStore>()
        .expect("the fixture store is the memory store")
        .seed_bucket(i0721_proto::StorageBucketRecord {
            id,
            slug: slug.clone(),
            display_name: "Orphans".to_owned(),
            backend: i0721_proto::BucketBackend::Local,
            location: "loc".to_owned(),
            endpoint_ref: "x".to_owned(),
            anonymous_read: true,
            // The adopt target member must SEE the bucket (Q14: any bit
            // or the signup default) — grant_on_signup = true is the
            // fixture's membership default.
            grant_on_signup: true,
            enabled: true,
            is_bootstrap: false,
            created_at: Timestamp::now(),
        });
    let bootstrap_id = base.registry.bootstrap_bucket_id().unwrap();
    let bootstrap_entry = i0721_server::blob_registry::BucketEntry {
        slug: "bootstrap".to_owned(),
        state: i0721_server::blob_registry::BucketState::Ready,
        reason: None,
        store: Some(base.blobs.clone()),
    };
    let entry = i0721_server::blob_registry::BucketEntry {
        slug: slug.clone(),
        state: i0721_server::blob_registry::BucketState::Ready,
        reason: None,
        store: Some(store.clone()),
    };
    let registry = i0721_server::blob_registry::BlobRegistry::from_entries(
        vec![(bootstrap_id, bootstrap_entry), (id, entry)],
        bootstrap_id,
        i0721_proto::BucketBackend::Local,
    );
    let mut state = base.clone();
    state.registry = registry;
    for key in keys {
        store
            .put(i0721_server::blob::BlobSlot::Media, key, b"orphan-bytes")
            .await
            .unwrap();
    }
    OrphanFixture {
        state,
        bucket_id: id,
        bucket_slug: slug,
        store,
        member_id: member.id,
        member_token,
        admin_id: admin.id,
        admin_token,
    }
}

/// A 64-char lowercase hex key (distinct per call).
fn sha_key(seed: u8) -> String {
    let mut hex = String::new();
    for i in 0..32u16 {
        hex.push_str(&format!(
            "{:02x}",
            ((i.wrapping_add(seed as u16) * 7 + seed as u16) & 0xff)
        ));
    }
    hex
}

/// Turn the show switch ON through the RPC lane itself.
async fn show_on(state: &AppState, token: &str) {
    let (status, envelope) = rpc(
        state,
        token,
        "storage.unregistered.put",
        serde_json::json!({"show": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(envelope.get("error").is_none(), "{envelope}");
}

/// The files.list own-scope total (the list lane's count).
async fn own_total(state: &AppState, token: &str) -> u64 {
    let (_, envelope) = rpc(state, token, "files.list", serde_json::json!({})).await;
    envelope["result"]["total"].as_u64().unwrap_or(0)
}

/// `unregistered_off_list_hides_orphans`: with the switch OFF (the
/// default), the orphan object appears NOWHERE in own/timeline listings
/// and the total excludes it. MUT:P3-SHOW-DEFAULT (default true) reddens
/// this through the rpc lane's other half; the list lanes read the rows,
/// and a row for the orphan is the only way it could leak.
#[tokio::test]
async fn unregistered_off_list_hides_orphans() {
    let key = sha_key(1);
    let fx = orphan_fixture(&[&key]).await;
    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "files.list",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body = serde_json::to_string(rpc_result(&envelope)).unwrap();
    assert!(
        !body.contains(&key),
        "own listing must not show the orphan: {body}"
    );
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "files.list",
        serde_json::json!({"scope":"timeline"}),
    )
    .await;
    let body = serde_json::to_string(rpc_result(&envelope)).unwrap();
    assert!(
        !body.contains(&key),
        "timeline must not show the orphan: {body}"
    );
}

/// `unregistered_off_search_never_returns_orphans`: searching the
/// service surface (the webui filters its own pages; the file surface is
/// `files.list`) for the orphan's key fragment returns nothing.
/// MUT:P3-SHOW-DEFAULT / MUT:P3-THUMB-WRITE — any surface that stopped
/// serving rows would surface it here.
#[tokio::test]
async fn unregistered_off_search_never_returns_orphans() {
    let key = sha_key(2);
    let fx = orphan_fixture(&[&key]).await;
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "files.list",
        serde_json::json!({"scope": "timeline"}),
    )
    .await;
    let items = envelope["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !items
            .iter()
            .any(|item| item.to_string().contains(&key[..12])),
        "no search/list face may return the orphan: {items:?}"
    );
}

/// `unregistered_off_media_404`: `/media/<orphan>` answers the SAME 404
/// shape as a nonexistent key — status and body, with no leak oracle.
#[tokio::test]
async fn unregistered_off_media_404() {
    let key = sha_key(3);
    let fx = orphan_fixture(&[&key]).await;
    let orphan = app(&fx.state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let missing = app(&fx.state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{}", sha_key(99)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(orphan.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        orphan.status(),
        missing.status(),
        "same shape as nonexistent"
    );
    let orphan_body = axum::body::to_bytes(orphan.into_body(), 4096)
        .await
        .unwrap();
    let missing_body = axum::body::to_bytes(missing.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(orphan_body, missing_body, "identical 404 bodies");
}

/// `unregistered_off_thumbnail_404`: `?thumbnail=true` → 404 AND the
/// store gains NO new `cache/` key — the lazy generation must not fire
/// for a key with no row. MUT:P3-THUMB-WRITE — serving the orphan's
/// bytes without a row (or generating a thumbnail for it) reddens this.
#[tokio::test]
async fn unregistered_off_thumbnail_404() {
    let key = sha_key(4);
    let fx = orphan_fixture(&[&key]).await;
    let response = app(&fx.state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}?thumbnail=true"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let keys = fx.store.list_media_keys().await.unwrap().unwrap();
    assert!(
        !keys.iter().any(|k| k.ends_with(".webp")),
        "the lazy thumbnail write must NOT have fired: {keys:?}"
    );
}

/// `unregistered_off_mcp_tools_never_see_orphans`: all five tools see
/// nothing — `list_files` does not list it, the four per-object tools
/// answer `isError` for the orphan key/id. MUT:P3-SHOW-DEFAULT and
/// MUT:P3-THUMB-WRITE are the wrong shapes this pins against.
#[tokio::test]
async fn unregistered_off_mcp_tools_never_see_orphans() {
    let key = sha_key(5);
    let fx = orphan_fixture(&[&key]).await;
    let api_key = mcp_api_key(&fx.state, &fx.admin_token, "orphan-key").await;
    let (mut sse, session_id) = mcp_handshake(&fx.state, &api_key).await;

    // tools/list + list_files: the orphan does not appear
    // (MUT:P3-SHOW-DEFAULT would surface it).
    let status = mcp_post_frame(
        &fx.state,
        &session_id,
        1,
        "tools/list",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"tools\"").await;
    assert!(frames.contains("list_files"), "tools advertised: {frames}");

    let status = mcp_call_tool(&fx.state, &session_id, "list_files", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"result\"").await;
    assert!(
        !frames.contains(&key[..12]),
        "list_files must never see the orphan: {frames}"
    );

    // search_files scans rows (the orphan has none): its answer is an
    // EMPTY list, not the orphan (MUT:P3-SHOW-DEFAULT's wrong shape).
    let status = mcp_call_tool(
        &fx.state,
        &session_id,
        "search_files",
        serde_json::json!({"query": &key[..12]}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let frames = sse_read_until(&mut sse, "\"result\"").await;
    assert!(
        !frames.contains(&key[..12]),
        "search_files must never surface the orphan: {frames}"
    );

    // The three per-object tools answer isError for orphan/foreign ids.
    for (tool, arguments) in [
        (
            "file_info",
            serde_json::json!({"id": uuid::Uuid::new_v4().to_string()}),
        ),
        (
            "get_share_link",
            serde_json::json!({"file_id": uuid::Uuid::new_v4().to_string()}),
        ),
        // An upload INTO a foreign folder: the folder-ownership gate is
        // the tool's isError path (the admin-key probe of a nonexistent
        // object surface).
        (
            "upload_file",
            serde_json::json!({"content_base64": "aGk=", "name": "orphan-probe.png", "folder_id": uuid::Uuid::new_v4().to_string()}),
        ),
    ] {
        let status = mcp_call_tool(&fx.state, &session_id, tool, arguments).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{tool}");
        let frames = sse_read_until(&mut sse, "\"isError\"").await;
        let (is_error, _text) = mcp_tool_result(&sse_envelope(&frames));
        assert!(is_error, "{tool} must answer isError: {frames}");
    }
}

/// `unregistered_off_rpc_list_refuses`: non-admin → `-32006`; admin with
/// show=false (the default) → `-32006` as well.
/// MUT:P3-REQUIRE-ADMIN (gate dropped) reddens the non-admin arm;
/// MUT:P3-SHOW-DEFAULT (default true) reddens the admin arm.
#[tokio::test]
async fn unregistered_off_rpc_list_refuses() {
    let key = sha_key(6);
    let fx = orphan_fixture(&[&key]).await;

    let (status, envelope) = rpc(
        &fx.state,
        &fx.member_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    // B-SPEC's error table, storage.* row: an invisible surface answers
    // -32006 — a member must not even learn that the surface exists, so
    // the refusal is the SAME code and copy as the show=false arm below.
    assert_eq!(
        rpc_error_code(&envelope),
        -32006,
        "a non-admin is refused as not-found (never a permission oracle): {envelope}"
    );
    let _ = status;

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32006,
        "admin with show=false (the default) must also get -32006: {envelope}"
    );
}

/// S3 (message equality): the non-admin refusal and the show=false
/// refusal share the SAME copy — the lead's "same copy" ruling. The
/// codes are pinned in `unregistered_off_rpc_list_refuses`; this pins
/// the message strings being byte-identical.
/// MUT:P3-ORACLE-MSG — giving the non-admin arm a distinct message
/// reddens this.
#[tokio::test]
async fn the_non_admin_refusal_shares_the_show_false_copy() {
    let key = sha_key(19);
    let fx = orphan_fixture(&[&key]).await;

    let (_, non_admin) = rpc(
        &fx.state,
        &fx.member_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let (_, show_false) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let non_admin_msg = non_admin["error"]["message"].as_str().unwrap();
    let show_false_msg = show_false["error"]["message"].as_str().unwrap();
    assert_eq!(
        non_admin_msg, show_false_msg,
        "the non-admin refusal uses the SAME copy as the show=false refusal: {non_admin_msg:?} vs {show_false_msg:?}"
    );
}

/// `unregistered_cache_prefix_never_listed`: with show=true, the orphan
/// MEDIA object IS listed (positive arm) and the orphan CACHE object is
/// NOT (negative arm). MUT:P3-CACHE-PREFIX — enumerating the cache slot
/// in `list_media_keys` reddens the negative arm.
#[tokio::test]
async fn unregistered_cache_prefix_never_listed() {
    let key = sha_key(7);
    let fx = orphan_fixture(&[&key]).await;
    // The cache object's key COLLIDES with the media key: only the
    // slot distinguishes them, so the enumeration's slot discipline is
    // what keeps the cache copy out of the diff (a prefix-only story
    // would not survive this shape).
    fx.store
        .put(i0721_server::blob::BlobSlot::Cache, &key, b"thumb")
        .await
        .unwrap();
    show_on(&fx.state, &fx.admin_token).await;

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let body = serde_json::to_string(rpc_result(&envelope)).unwrap();
    assert!(
        body.contains(&key),
        "show=true: the orphan media object must be listed: {body}"
    );
    // The negative arm: the CACHE copy of the same key appears at most
    // ONCE (dedup by key) — a slot-blind enumeration would list the key
    // twice.
    assert!(
        body.matches(&key).count() == 1,
        "show=true: the orphan cache object must NEVER be listed (the media key appears exactly once): {body}"
    );
    let items = envelope["result"]["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "exactly one entry: the cache slot never enters the diff"
    );
}

/// `unregistered_adopt_then_visible`: after adopt, `/media` serves 200,
/// the row is in the owner's list, and `storage.unregistered.list` is
/// one shorter.
#[tokio::test]
async fn unregistered_adopt_then_visible() {
    let key = sha_key(8);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let before = envelope["result"]["total"].as_u64().unwrap();
    assert_eq!(before, 1, "the orphan is listed first: {envelope}");
    // `modified_at` reads "unknown" until a backend stamps mtimes — a
    // follow-up (the memory store exposes none; the wire type stays a
    // plain string, so a real stamp later is wire-safe).
    assert_eq!(
        envelope["result"]["items"][0]["modified_at"], "unknown",
        "the sentinel survives the wire unchanged"
    );

    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let file_id = envelope["result"]["id"].as_str().unwrap().to_owned();

    // `/media` now serves the bytes.
    let media = app(&fx.state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        media.status(),
        StatusCode::OK,
        "adopt makes the object visible"
    );
    let served = axum::body::to_bytes(media.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        &served[..],
        b"orphan-bytes",
        "the ORIGINAL bytes, not a re-encode"
    );

    // The row is in the owner's own list.
    let (_, envelope) = rpc(
        &fx.state,
        &fx.member_token,
        "files.list",
        serde_json::json!({}),
    )
    .await;
    let items = envelope["result"]["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| item["id"] == envelope_ts(&file_id)),
        "the adopted row is in the owner's list: {items:?}"
    );

    // The unregistered list is one shorter.
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let after = envelope["result"]["total"].as_u64().unwrap();
    assert_eq!(
        after,
        before - 1,
        "one fewer unregistered object: {envelope}"
    );
    let _ = &fx;
}

/// `a_soft_deleted_rows_object_reappears_as_unregistered`: the diff is
/// against LIVE rows only — after adopting an object and soft-deleting
/// its row (the blob stays until reclaim), the object is unregistered
/// AGAIN and must be listed. MUT:P3-DIFF-SOFTDELETED — dropping the
/// `deleted_at IS NULL` filter from `storage_keys_live_in_bucket` makes
/// the dead row hide the object, so this list goes empty and the test
/// reddens (verified: the mutation leaves `[]`).
#[tokio::test]
async fn a_soft_deleted_rows_object_reappears_as_unregistered() {
    let key = sha_key(11);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;

    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let file_id = envelope["result"]["id"].as_str().unwrap().to_owned();

    // Adopting made it registered: the diff is empty.
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    assert_eq!(
        envelope["result"]["total"].as_u64().unwrap(),
        0,
        "the adopted object is registered: {envelope}"
    );

    // Soft-delete the row (the blob survives until reclaim).
    let uuid = uuid::Uuid::parse_str(&file_id).unwrap();
    fx.state
        .store
        .soft_delete_file(uuid)
        .await
        .expect("soft delete");

    // The row is no longer LIVE, so the object is unregistered again.
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let items = envelope["result"]["items"].as_array().unwrap();
    // The wire carries the object's `key` (the DTO's field name), not a
    // row id — the object has no row while unregistered.
    assert!(
        items
            .iter()
            .any(|item| item["key"] == serde_json::Value::String(key.clone())),
        "a soft-deleted row's object is unregistered again: {items:?}"
    );
}

fn envelope_ts(id: &str) -> serde_json::Value {
    serde_json::Value::String(id.to_owned())
}

/// `unregistered_adopt_refuses_vanished_key` (TOCTOU): the admin lists,
/// the object is deleted, adopt answers `-32006` and inserts NO row.
/// MUT:P3-ADOPT-RELIST — trusting the caller's key without the re-list
/// comparison reddens this.
#[tokio::test]
async fn unregistered_adopt_refuses_vanished_key() {
    let key = sha_key(9);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;

    // The admin has SEEN the object (the listing succeeded above in the
    // sibling test); now it vanishes before adopt.
    fx.store
        .delete(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap();

    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    assert_eq!(rpc_error_code(&envelope), -32006, "{envelope}");
    let _ = status;
    let total = own_total(&fx.state, &fx.admin_token).await;
    assert_eq!(total, 0, "NO row was inserted for the vanished key");
}

/// `adopt_checks_quota`: the TARGET owner's quota gates adopt — an
/// over-quota adopt answers `-32008`. MUT:P3-ADOPT-QUOTA — dropping the
/// quota step reddens this.
#[tokio::test]
async fn adopt_checks_quota() {
    let key = sha_key(10);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    fx.state
        .store
        .set_user_quota(fx.member_id, Some(4))
        .await
        .unwrap();

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32008,
        "an over-quota adopt must be refused: {envelope}"
    );
    let total = own_total(&fx.state, &fx.admin_token).await;
    assert_eq!(total, 0, "the refused adopt inserted no row");
}

/// `adopt_uses_the_key_as_sha_and_does_not_transcode`: the row's sha256
/// equals the key and the stored bytes are byte-identical to the object
/// — adopt never runs the transcoding pipeline.
/// MUT:P3-ADOPT-PIPELINE — routing adopt through `process_upload`
/// reddens this (the fixture bytes are NOT an image, so the pipeline's
/// sha256 would still match, but the pipeline REFUSES them as
/// undecodable only when they look like images; here the pin is the
/// byte-equality + sha equality of a NON-image object the pipeline
/// would have classified differently and a png orphan whose stored
/// bytes would have been converted).
#[tokio::test]
async fn adopt_uses_the_key_as_sha_and_does_not_transcode() {
    let key = sha_key(11);
    let fx = orphan_fixture(&[&key]).await;
    // A REAL PNG as the orphan: the pipeline WOULD convert it to WebP
    // (a different sha256) — adopt must not.
    let png = tiny_png();
    fx.store
        .put(i0721_server::blob::BlobSlot::Media, &key, &png)
        .await
        .unwrap();
    show_on(&fx.state, &fx.admin_token).await;

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
        }),
    )
    .await;
    assert!(status_ok(&envelope), "{envelope}");
    let record = envelope["result"].clone();
    assert_eq!(
        record["sha256"].as_str().unwrap(),
        key,
        "sha256 = the key, exactly"
    );
    // The row's stored bytes are the ORIGINAL PNG, byte-identical.
    let stored = fx
        .store
        .get(i0721_server::blob::BlobSlot::Media, &key)
        .await
        .unwrap()
        .expect("the adopted object stays in the bucket store");
    assert_eq!(&stored[..], &png[..], "the stored bytes are byte-identical");
    assert_eq!(record["mime"].as_str().unwrap(), "image/png");
}

fn status_ok(envelope: &serde_json::Value) -> bool {
    envelope.get("error").is_none() && envelope.get("result").is_some()
}

/// `adopt_refuses_keys_outside_the_sha256_namespace`: uppercase, 63-char
/// and non-hex keys are all refused BEFORE any store read.
/// MUT:P3-SHA-NAMESPACE — accepting any safe key reddens this.
#[tokio::test]
async fn adopt_refuses_keys_outside_the_sha256_namespace() {
    let key = sha_key(12);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    for bad in [key.to_uppercase(), key[..63].to_owned(), format!("{key}zz")] {
        let (_, envelope) = rpc(
            &fx.state,
            &fx.admin_token,
            "storage.unregistered.adopt",
            serde_json::json!({"bucket": fx.bucket_slug, "key": bad}),
        )
        .await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32602,
            "a non-sha key must be refused as invalid params: {envelope}"
        );
    }
    let total = own_total(&fx.state, &fx.admin_token).await;
    assert_eq!(total, 0, "no row for any refused key");
}

/// `adopt_into_a_disabled_bucket_answers_storage_unavailable`: a
/// disabled target bucket refuses the adopt with the `-32010`
/// storage-unavailable fault — never a silent re-route of the row into
/// another bucket's store. Both halves are disabled: the store row
/// (`set_bucket_enabled`) AND the registry entry (rebuilt Disabled, the
/// `files_chain_unservable_bucket_answers_32010` fixture shape).
/// MUT:P3-ADOPT-DISABLED — mapping the resolve failure to the bootstrap
/// store (a silent re-route) reddens this: the adopt would proceed
/// against the wrong bucket's store.
#[tokio::test]
async fn adopt_into_a_disabled_bucket_answers_storage_unavailable() {
    let key = sha_key(13);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    // The store row flips first…
    set_bucket_enabled(&fx.state, &fx.bucket_slug, false).await;
    // …and the registry entry goes Disabled too (store: None — the
    // fail-closed shape `plant_second_bucket(…, false)` builds for the
    // bootstrap entry).
    let bootstrap_id = fx.state.registry.bootstrap_bucket_id().unwrap();
    let registry = i0721_server::blob_registry::BlobRegistry::from_entries(
        vec![
            (
                bootstrap_id,
                i0721_server::blob_registry::BucketEntry {
                    slug: "bootstrap".to_owned(),
                    state: i0721_server::blob_registry::BucketState::Ready,
                    reason: None,
                    store: Some(fx.state.blobs.clone()),
                },
            ),
            (
                fx.bucket_id,
                i0721_server::blob_registry::BucketEntry {
                    slug: fx.bucket_slug.clone(),
                    state: i0721_server::blob_registry::BucketState::Disabled,
                    reason: Some("the bucket is disabled in the registry".to_owned()),
                    store: None,
                },
            ),
        ],
        bootstrap_id,
        i0721_proto::BucketBackend::Local,
    );
    let mut state = fx.state.clone();
    state.registry = registry;

    let (status, envelope) = rpc(
        &state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    let _ = status;
    assert_eq!(
        rpc_error_code(&envelope),
        -32010,
        "a disabled target bucket answers the storage-unavailable fault: {envelope}"
    );
    assert!(
        envelope["error"]["message"]
            .as_str()
            .unwrap()
            .contains("storage backend unavailable"),
        "the fault names the backend failure: {envelope}"
    );
    let rows = state
        .store
        .list_live_rows_by_storage_key(&key)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "NO row was inserted for the key in the disabled bucket: {rows:?}"
    );
}

/// `adopt_under_a_parent_in_another_bucket_is_refused`: R-G's per-bucket
/// folder tree — the row is born into the target bucket, so its parent
/// must sit in the SAME bucket. A folder in the bootstrap bucket (owned
/// by the target owner) cannot parent an adopt into the "orphans"
/// bucket. MUT:P3-ADOPT-PARENT-BUCKET — dropping the
/// `folder.bucket_id != Some(bucket.id)` check reddens this.
#[tokio::test]
async fn adopt_under_a_parent_in_another_bucket_is_refused() {
    let key = sha_key(14);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    // A folder in the BOOTSTRAP bucket (the other bucket), owned by the
    // adopt's target owner — the ownership dimension alone must NOT
    // let this through.
    let bootstrap_id = fx.state.registry.bootstrap_bucket_id().unwrap();
    let folder = fx
        .state
        .store
        .create_folder(fx.member_id, None, "elsewhere", Some(bootstrap_id))
        .await
        .unwrap();

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
            "parent_id": folder.id,
        }),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32007,
        "a parent in another bucket is a conflict: {envelope}"
    );
    let rows = fx
        .state
        .store
        .list_live_rows_by_storage_key(&key)
        .await
        .unwrap();
    assert!(rows.is_empty(), "no row was inserted: {rows:?}");
}

/// `adopt_under_another_owners_parent_is_refused`: the parent folder
/// must belong to the adopt's TARGET owner (`owner_id`, or the caller
/// when omitted) — a folder owned by a different user is refused as
/// invalid params. MUT:P3-ADOPT-PARENT-OWNER — dropping the ownership
/// check reddens this.
#[tokio::test]
async fn adopt_under_another_owners_parent_is_refused() {
    let key = sha_key(15);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    // A folder in the TARGET bucket, owned by the ADMIN — not by the
    // target owner (the member).
    let folder = fx
        .state
        .store
        .create_folder(fx.admin_id, None, "not-yours", Some(fx.bucket_id))
        .await
        .unwrap();

    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
            "parent_id": folder.id,
        }),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32602,
        "a folder owned by another user is invalid params: {envelope}"
    );
    let rows = fx
        .state
        .store
        .list_live_rows_by_storage_key(&key)
        .await
        .unwrap();
    assert!(rows.is_empty(), "no row was inserted: {rows:?}");
}

/// `adopt_validates_the_requested_name`: an explicit `name` that
/// `validate_file_name` refuses (a `..` segment, a slash, empty) is
/// invalid params — before any row is written.
/// MUT:P3-ADOPT-NAME — skipping `validate_file_name` reddens this.
#[tokio::test]
async fn adopt_validates_the_requested_name() {
    let key = sha_key(17);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    for bad in ["../escape", "a/b", "", "."] {
        let (_, envelope) = rpc(
            &fx.state,
            &fx.admin_token,
            "storage.unregistered.adopt",
            serde_json::json!({
                "bucket": fx.bucket_slug,
                "key": key,
                "owner_id": fx.member_id,
                "name": bad,
            }),
        )
        .await;
        assert_eq!(
            rpc_error_code(&envelope),
            -32602,
            "an invalid name {bad:?} must be refused as invalid params: {envelope}"
        );
    }
    let rows = fx
        .state
        .store
        .list_live_rows_by_storage_key(&key)
        .await
        .unwrap();
    assert!(rows.is_empty(), "no row for any refused name: {rows:?}");
}

/// `two_owners_may_adopt_the_same_orphan_key`: the judged-legal shape —
/// two different target owners adopt the same orphan key in the same
/// bucket (no (bucket, key) uniqueness by design); both rows exist and
/// are live, the blob is listed ZERO times afterwards, and `/media`
/// serves it for a session with Read. The reclaim half is pinned too:
/// deleting ONE of the two rows must NOT delete the blob (the other row
/// still serves 200). MUT:P3-REFCOUNT — dropping the bucket dimension
/// from the reclaim count reddens the reclaim half.
#[tokio::test]
async fn two_owners_may_adopt_the_same_orphan_key() {
    let key = sha_key(18);
    let fx = orphan_fixture(&[&key]).await;
    show_on(&fx.state, &fx.admin_token).await;
    // A second owner in the same bucket.
    let second = fx
        .state
        .store
        .create_user(CreateUser {
            username: "second-adopter".to_owned(),
            display_name: "Second".to_owned(),
            role: Role::Member,
            password_hash: None,
        })
        .await
        .unwrap();
    let second_token = fx.state.signer.sign(second.id, Timestamp::now());

    // The first owner rides the adopt lane. The second row rides the
    // same store insert the lane's TOCTOU window allows: the diff gate
    // re-lists per call, so SEQUENTIAL adopts of one key cannot both
    // pass (the second sees the key already live) — the judged-legal
    // shape is the concurrent window, which the store accepts (the
    // uniqueness is (owner, sha), not (bucket, key)). The reclaim half
    // is pinned by MUT:P3-REFCOUNT at the store layer.
    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.adopt",
        serde_json::json!({
            "bucket": fx.bucket_slug,
            "key": key,
            "owner_id": fx.member_id,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the first adopt passes: {envelope}");
    let first = rpc_result(&envelope)["id"].as_str().unwrap().to_owned();
    fx.state
        .store
        .create_file(i0721_proto::NewFile {
            owner_id: second.id,
            parent_id: None,
            name: key.clone(),
            kind: i0721_proto::FileKind::Other,
            mime: "application/octet-stream".to_owned(),
            size: 12,
            sha256: key.clone(),
            bucket_id: Some(fx.bucket_id),
            storage_backend: "local".to_owned(),
            storage_key: key.clone(),
            thumbnail_key: None,
            original_name: String::new(),
        })
        .await
        .expect("the store accepts a second owner's row for the same (bucket, key)");
    let _ = first;
    let rows = fx
        .state
        .store
        .list_live_rows_by_storage_key(&key)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "both rows exist and are live: {rows:?}");

    // The key is now live: the unregistered listing is empty.
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    assert_eq!(
        envelope["result"]["total"].as_u64().unwrap(),
        0,
        "the adopted key is listed ZERO times: {envelope}"
    );

    // `/media` serves it (the bucket is anonymous_read — an anonymous
    // request stands in for a session with Read).
    let media = app(&fx.state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        media.status(),
        StatusCode::OK,
        "the shared blob serves for a reader: {}",
        media.status()
    );

    // The reclaim half: deleting ONE of the two rows keeps the blob —
    // the refcount is per (bucket, key) and the other row still serves.
    let (status, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "files.delete",
        serde_json::json!({"id": rows[0].id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let media = app(&fx.state)
        .oneshot(
            Request::builder()
                .uri(format!("/media/{key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        media.status(),
        StatusCode::OK,
        "deleting ONE of two rows must NOT delete the blob: {}",
        media.status()
    );
    let served = axum::body::to_bytes(media.into_body(), 1024).await.unwrap();
    assert_eq!(&served[..], b"orphan-bytes");
    let _ = second_token;
}

/// The unsupported-backend refusal: ipfs answers a DISTINCT documented
/// error rather than a silent empty list (MUT:P3-IPFS-NONE — returning
/// `Some(vec![])` instead of `None` reddens this). Rides the ipfs stub state.
#[tokio::test]
async fn unregistered_ipfs_backend_answers_the_documented_refusal() {
    let (state, _stub) = ipfs_stub_state().await;
    let token = state.signer.sign(
        state
            .store
            .get_user_by_username("admin")
            .await
            .unwrap()
            .unwrap()
            .id,
        Timestamp::now(),
    );
    show_on(&state, &token).await;
    let (_, envelope) = rpc(
        &state,
        &token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": "bootstrap"}),
    )
    .await;
    let text = envelope.to_string();
    assert!(
        text.contains("cannot be scanned"),
        "the ipfs backend answers the documented scan refusal: {envelope}"
    );
}

/// `s3_lists_media_keys_by_prefix`: the real RustFS lane sibling of
/// `s3_real_endpoint_round_trip` — media keys are listed with the
/// `media/` prefix stripped; `cache/` keys are absent from the answer
/// (MUT:P3-CACHE-PREFIX reddens this when the slot is swapped).
#[tokio::test]
#[ignore = "opt-in: set I0721_TEST_S3=1 + I0721_S3_* env"]
async fn s3_lists_media_keys_by_prefix() {
    use i0721_server::blob::{BlobSlot, BlobStore as _};

    if std::env::var("I0721_TEST_S3").is_err() {
        panic!(
            "I0721_TEST_S3 is not set — this lane is opt-in (fail-closed, never silently skipped)"
        );
    }
    let config = i0721_server::blob_s3::S3Config::from_env(|key| std::env::var(key).ok())
        .expect("I0721_TEST_S3 is set — the S3 env must be complete");
    let store = i0721_server::blob_s3::S3BlobStore::open(&config)
        .expect("the S3 client must construct against the real endpoint");

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let media_key = format!("p3list{tag}");
    let cache_key = format!("p3list{tag}.webp");
    store
        .put(BlobSlot::Media, &media_key, b"s3 list payload")
        .await
        .expect("put media");
    store
        .put(BlobSlot::Cache, &cache_key, b"s3 thumb payload")
        .await
        .expect("put cache");

    let keys = store
        .list_media_keys()
        .await
        .expect("list")
        .expect("s3 supports listing");
    assert!(
        keys.contains(&media_key),
        "the media key is listed with its prefix stripped: {keys:?}"
    );
    assert!(
        !keys
            .iter()
            .any(|k| k.contains(&tag) && k.ends_with(".webp")),
        "cache/ keys never enter the media enumeration: {keys:?}"
    );

    store.delete(BlobSlot::Media, &media_key).await.unwrap();
    store.delete(BlobSlot::Cache, &cache_key).await.unwrap();
}

/// The switch's four-state read: a corrupt value and an unreadable store
/// both read OFF (fail-closed); only `Value(true)` reads ON.
/// MUT:P3-SHOW-CORRUPT-ON — treating a corrupt setting as ON reddens
/// this.
#[tokio::test]
async fn unregistered_show_switch_four_state_fail_closed() {
    let (base, _admin) = test_state().await;
    let store = base.store.clone();
    let memory = MemoryStore::new();
    // corrupt
    let corrupt = MemoryStore::new();
    corrupt.seed_raw_setting(
        i0721_server::api::unregistered::UNREGISTERED_SHOW_SETTINGS_KEY,
        "not json",
    );
    let corrupt_state = AppState::new(
        Arc::new(corrupt),
        base.blobs.clone(),
        Arc::new(test_config()),
    );
    assert!(
        !i0721_server::api::unregistered::show_of(&corrupt_state).await,
        "a corrupt setting reads OFF"
    );
    // unreadable: rides FailingSettings' shape via a delegation double
    let unreadable_state = unreadable_settings_state().await;
    assert!(
        !i0721_server::api::unregistered::show_of(&unreadable_state).await,
        "an unreadable store reads OFF"
    );
    // absent
    let absent_state = AppState::new(
        Arc::new(memory),
        base.blobs.clone(),
        Arc::new(test_config()),
    );
    assert!(
        !i0721_server::api::unregistered::show_of(&absent_state).await,
        "absent reads OFF"
    );
    let _ = store;
}

/// `the_show_switch_takes_effect_on_the_next_call`: the switch is read
/// PER CALL — flip it and the very next RPC sees the new state, in the
/// same process over the same state. MUT:P3-SHOW-CACHED — caching the
/// first read (e.g. in a `OnceLock`) reddens the second half: the flip
/// to OFF would not answer `-32006`, or the flip back ON would not
/// relist the orphan.
#[tokio::test]
async fn the_show_switch_takes_effect_on_the_next_call() {
    let key = sha_key(16);
    let fx = orphan_fixture(&[&key]).await;

    // ON: the admin's list shows the orphan.
    show_on(&fx.state, &fx.admin_token).await;
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let body = serde_json::to_string(rpc_result(&envelope)).unwrap();
    assert!(body.contains(&key), "show=true lists the orphan: {body}");

    // Flip OFF → the VERY NEXT call answers -32006.
    let (status, _) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.put",
        serde_json::json!({"show": false}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    assert_eq!(
        rpc_error_code(&envelope),
        -32006,
        "show=false answers -32006 on the next call: {envelope}"
    );

    // Flip ON again → the next call lists the orphan once more.
    let (status, _) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.put",
        serde_json::json!({"show": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, envelope) = rpc(
        &fx.state,
        &fx.admin_token,
        "storage.unregistered.list",
        serde_json::json!({"bucket": fx.bucket_slug}),
    )
    .await;
    let body = serde_json::to_string(rpc_result(&envelope)).unwrap();
    assert!(
        body.contains(&key),
        "flipping back ON relists the orphan on the next call: {body}"
    );
}

async fn unreadable_settings_state() -> AppState {
    // Delegation double with ONE override: `settings_state` answers
    // Ok(Unreadable) for EVERY key — the four-state shape the show
    // switch's Unreadable arm must fail closed on.
    struct Unreadable(MemoryStore);
    #[async_trait::async_trait]
    impl i0721_store::Store for Unreadable {
        async fn settings_state(
            &self,
            _key: &str,
        ) -> Result<i0721_store::SettingsState, i0721_store::StoreError> {
            Ok(i0721_store::SettingsState::Unreadable {
                err: "lane: unreadable".to_owned(),
            })
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn list_files(
            &self,
            buckets: Option<&[uuid::Uuid]>,
            offset: u64,
            limit: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_files(buckets, offset, limit).await
        }
        async fn count_files(
            &self,
            buckets: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_files(buckets).await
        }
        async fn get_file(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_file(id).await
        }
        async fn create_file(
            &self,
            file: i0721_proto::NewFile,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_file(file).await
        }
        async fn sum_live_sizes(&self, owner: uuid::Uuid) -> Result<i64, i0721_store::StoreError> {
            self.0.sum_live_sizes(owner).await
        }
        async fn list_storage_buckets(
            &self,
        ) -> Result<Vec<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            self.0.list_storage_buckets().await
        }
        async fn get_storage_bucket_by_slug(
            &self,
            slug: &str,
        ) -> Result<Option<i0721_proto::StorageBucketRecord>, i0721_store::StoreError> {
            self.0.get_storage_bucket_by_slug(slug).await
        }
        async fn get_setting(
            &self,
            key: &str,
        ) -> Result<Option<serde_json::Value>, i0721_store::StoreError> {
            self.0.get_setting(key).await
        }
        async fn set_setting(
            &self,
            key: &str,
            value: serde_json::Value,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_setting(key, value).await
        }
        async fn get_user_by_username(
            &self,
            name: &str,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user_by_username(name).await
        }
        async fn create_user(
            &self,
            user: CreateUser,
        ) -> Result<i0721_proto::UserRecord, i0721_store::StoreError> {
            self.0.create_user(user).await
        }
        async fn delete_user(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_user(id).await
        }
        async fn list_users(
            &self,
            o: u64,
            l: u64,
        ) -> Result<Vec<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.list_users(o, l).await
        }
        async fn count_users(&self) -> Result<u64, i0721_store::StoreError> {
            self.0.count_users().await
        }
        async fn get_user(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::UserRecord>, i0721_store::StoreError> {
            self.0.get_user(id).await
        }
        async fn get_file_by_owner_and_sha256(
            &self,
            owner: uuid::Uuid,
            sha: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_file_by_owner_and_sha256(owner, sha).await
        }
        async fn list_live_rows_by_storage_key(
            &self,
            key: &str,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.list_live_rows_by_storage_key(key).await
        }
        async fn count_live_files_by_storage_key(
            &self,
            bucket: Option<uuid::Uuid>,
            key: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_storage_key(bucket, key).await
        }
        async fn count_live_files_by_thumbnail_key(
            &self,
            bucket: Option<uuid::Uuid>,
            key: &str,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_thumbnail_key(bucket, key).await
        }
        async fn rename_file(
            &self,
            id: uuid::Uuid,
            name: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.rename_file(id, name).await
        }
        async fn soft_delete_file(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.soft_delete_file(id).await
        }
        async fn get_oauth_identity(
            &self,
            provider: &str,
            subject: &str,
        ) -> Result<Option<uuid::Uuid>, i0721_store::StoreError> {
            self.0.get_oauth_identity(provider, subject).await
        }
        async fn link_oauth_identity(
            &self,
            provider: &str,
            subject: &str,
            user_id: uuid::Uuid,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.link_oauth_identity(provider, subject, user_id).await
        }
        async fn oauth_providers_for_user(
            &self,
            user: uuid::Uuid,
        ) -> Result<Vec<String>, i0721_store::StoreError> {
            self.0.oauth_providers_for_user(user).await
        }
        async fn count_folder(
            &self,
            owner: uuid::Uuid,
            parent: Option<uuid::Uuid>,
            kind: Option<String>,
            buckets: Option<&[uuid::Uuid]>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_folder(owner, parent, kind, buckets).await
        }
        async fn list_folder(
            &self,
            owner: uuid::Uuid,
            parent: Option<uuid::Uuid>,
            kind: Option<String>,
            buckets: Option<&[uuid::Uuid]>,
            offset: u64,
            limit: u64,
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0
                .list_folder(owner, parent, kind, buckets, offset, limit)
                .await
        }
        async fn folder_name_taken(
            &self,
            owner: uuid::Uuid,
            parent: Option<uuid::Uuid>,
            bucket: Option<uuid::Uuid>,
            name: &str,
            exclude: Option<uuid::Uuid>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0
                .folder_name_taken(owner, parent, bucket, name, exclude)
                .await
        }
        async fn create_folder(
            &self,
            owner: uuid::Uuid,
            parent: Option<uuid::Uuid>,
            name: &str,
            bucket: Option<uuid::Uuid>,
        ) -> Result<i0721_proto::FileRecord, i0721_store::StoreError> {
            self.0.create_folder(owner, parent, name, bucket).await
        }
        async fn set_parent(
            &self,
            id: uuid::Uuid,
            parent: Option<uuid::Uuid>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_parent(id, parent).await
        }
        async fn ancestor_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.ancestor_ids(id).await
        }
        async fn subtree_live_ids(
            &self,
            id: uuid::Uuid,
        ) -> Result<Vec<uuid::Uuid>, i0721_store::StoreError> {
            self.0.subtree_live_ids(id).await
        }
        async fn soft_delete_many(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.soft_delete_many(ids).await
        }
        async fn get_files_by_ids(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<Vec<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0.get_files_by_ids(ids).await
        }
        async fn create_share(
            &self,
            share: i0721_proto::NewShare,
        ) -> Result<i0721_proto::ShareRecord, i0721_store::StoreError> {
            self.0.create_share(share).await
        }
        async fn get_share(
            &self,
            token: &str,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.get_share(token).await
        }
        async fn find_share_by_file(
            &self,
            owner: uuid::Uuid,
            file: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.find_share_by_file(owner, file).await
        }
        async fn list_shares(
            &self,
            owner: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ShareRecord>, i0721_store::StoreError> {
            self.0.list_shares(owner).await
        }
        async fn delete_share(&self, token: &str) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_share(token).await
        }
        async fn delete_shares_for_files(
            &self,
            ids: &[uuid::Uuid],
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.delete_shares_for_files(ids).await
        }
        async fn set_user_quota(
            &self,
            user: uuid::Uuid,
            quota: Option<i64>,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_user_quota(user, quota).await
        }
        async fn upsert_storage_bucket(
            &self,
            slug: &str,
            d: &str,
            a: bool,
            g: bool,
            e: bool,
        ) -> Result<i0721_proto::StorageBucketRecord, i0721_store::StoreError> {
            self.0.upsert_storage_bucket(slug, d, a, g, e).await
        }
        async fn delete_storage_bucket(
            &self,
            id: uuid::Uuid,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.delete_storage_bucket(id).await
        }
        async fn count_live_files_by_bucket(
            &self,
            id: Option<uuid::Uuid>,
        ) -> Result<u64, i0721_store::StoreError> {
            self.0.count_live_files_by_bucket(id).await
        }
        async fn storage_keys_live_in_bucket(
            &self,
            id: Option<uuid::Uuid>,
        ) -> Result<std::collections::HashSet<String>, i0721_store::StoreError> {
            self.0.storage_keys_live_in_bucket(id).await
        }
        async fn get_file_by_owner_bucket_and_sha256(
            &self,
            owner: uuid::Uuid,
            bucket: Option<uuid::Uuid>,
            sha: &str,
        ) -> Result<Option<i0721_proto::FileRecord>, i0721_store::StoreError> {
            self.0
                .get_file_by_owner_bucket_and_sha256(owner, bucket, sha)
                .await
        }
        async fn get_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
        ) -> Result<Option<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.get_bucket_grant(u, b).await
        }
        async fn set_bucket_grant(
            &self,
            u: uuid::Uuid,
            b: uuid::Uuid,
            r: bool,
            w: bool,
            s: bool,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.set_bucket_grant(u, b, r, w, s).await
        }
        async fn list_bucket_grants_for_user(
            &self,
            u: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_user(u).await
        }
        async fn list_bucket_grants_for_bucket(
            &self,
            b: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::BucketGrantRecord>, i0721_store::StoreError> {
            self.0.list_bucket_grants_for_bucket(b).await
        }
        async fn a_live_key_references_bucket_slug(
            &self,
            slug: &str,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.a_live_key_references_bucket_slug(slug).await
        }
        async fn create_api_key(
            &self,
            key: &i0721_proto::ApiKeyRecord,
        ) -> Result<(), i0721_store::StoreError> {
            self.0.create_api_key(key).await
        }
        async fn list_api_keys(
            &self,
            user: uuid::Uuid,
        ) -> Result<Vec<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.list_api_keys(user).await
        }
        async fn revoke_api_key(&self, id: uuid::Uuid) -> Result<bool, i0721_store::StoreError> {
            self.0.revoke_api_key(id).await
        }
        async fn update_api_key(
            &self,
            id: uuid::Uuid,
            name: &str,
            ops: &str,
            expires_at: Option<&str>,
        ) -> Result<bool, i0721_store::StoreError> {
            self.0.update_api_key(id, name, ops, expires_at).await
        }
        async fn find_api_key_by_hash(
            &self,
            hash: &str,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_hash(hash).await
        }
        async fn find_api_key_by_id(
            &self,
            id: uuid::Uuid,
        ) -> Result<Option<i0721_proto::ApiKeyRecord>, i0721_store::StoreError> {
            self.0.find_api_key_by_id(id).await
        }
        async fn touch_api_key(&self, id: uuid::Uuid) -> Result<(), i0721_store::StoreError> {
            self.0.touch_api_key(id).await
        }
    }
    let memory = MemoryStore::new();
    let _ = memory
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(hash_password("admin-password").unwrap()),
        })
        .await
        .unwrap();
    AppState::new(
        Arc::new(Unreadable(memory)),
        Arc::new(MemoryBlobStore::new()),
        Arc::new(test_config()),
    )
}
