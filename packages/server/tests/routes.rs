//! Route-level tests: the real router over the in-memory store and blob
//! fake, exercised with `tower::ServiceExt::oneshot`. No external
//! services — PostgreSQL is covered by the opt-in lane in
//! `i0721-store`'s `pg_lane` tests.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use i0721_proto::{Role, SiteConfigStrict, UserRecord};
use i0721_store::{CreateUser, MemoryStore, Store};
use jiff::Timestamp;
use tower::ServiceExt;

use i0721_server::auth::hash_password;
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
        cookie.contains("SameSite=Strict"),
        "the cookie must be Strict: {cookie}"
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
    // delete handler reaches the unlink.
    *racy.payload.lock().unwrap() = Some((
        i0721_proto::NewFile {
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
    assert!(cookie.to_str().unwrap().contains("Max-Age=0"));

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
