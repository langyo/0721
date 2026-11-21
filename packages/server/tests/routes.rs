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
use i0721_proto::{Role, UserRecord};
use i0721_store::{CreateUser, MemoryStore, Store};
use jiff::Timestamp;
use tower::ServiceExt;

use i0721_server::auth::hash_password;
use i0721_server::blob::MemoryBlobStore;
use i0721_server::config::Config;
use i0721_server::routes;
use i0721_server::state::AppState;

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

fn test_config() -> Config {
    Config {
        database_url: "unused".to_owned(),
        listen: "127.0.0.1:8080".parse().unwrap(),
        jwt_secret: "route-test-secret".to_owned(),
        media_root: PathBuf::from("unused"),
        static_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../res/website"),
        upload_concurrency: 4,
        log_filter: "off".to_owned(),
    }
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

async fn login(state: &AppState, username: &str, password: &str) -> String {
    let response = ServiceExt::oneshot(app(state), login_request(username, password))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
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
async fn auth_flow_login_verify_refresh() {
    let (state, _) = test_state().await;
    let token = login(&state, "admin", "admin-password").await;

    let verify = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/auth/verify")
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(verify.status(), StatusCode::OK);
    let body = axum::body::to_bytes(verify.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["username"], "admin");
    assert_eq!(json["role"], "admin");
    assert!(json.get("password_hash").is_none());

    let refresh = app(&state)
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
    assert_eq!(refresh.status(), StatusCode::OK);

    // Tampered token: refused.
    let tampered = format!("{token}x");
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/auth/verify")
                .body(Body::empty())
                .unwrap(),
            &tampered,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // No token: refused.
    let response = app(&state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
    let verify = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/auth/verify")
                .body(Body::empty())
                .unwrap(),
            &first,
        ))
        .await
        .unwrap();
    assert_eq!(
        verify.status(),
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

    // List shows it.
    let list = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/files?offset=0&limit=10")
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = axum::body::to_bytes(list.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 1);
    assert_eq!(json["items"].as_array().unwrap().len(), 1);

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

    // Rename.
    let id = dto["id"].as_str().unwrap().to_owned();
    let renamed = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/files/{id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name": "renamed cat"}"#))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let body = axum::body::to_bytes(renamed.into_body(), 4096)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["name"], "renamed cat");

    // Soft delete → list shrinks, media 404s.
    let deleted = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{id}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let gone = app(&state)
        .oneshot(Request::builder().uri(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
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

    // Oversize: shrink the limit via settings, then upload past it.
    let token = login(&state, "admin", "admin-password").await;
    let put = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"upload": {"image-size-limit": "1KiB"}}).to_string(),
                ))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
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

    for uri in ["/api/v1/users", "/api/v1/settings"] {
        let response = app(&state)
            .oneshot(bearer(
                Request::builder().uri(uri).body(Body::empty()).unwrap(),
                &member_token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "member on {uri}");
    }

    // Admin creates a user.
    let admin_token = state.signer.sign(admin.id, Timestamp::now());
    let created = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/users")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "username": "newbie",
                        "password": "a-password",
                        "role": "member"
                    })
                    .to_string(),
                ))
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(created.into_body(), 4096)
        .await
        .unwrap();
    let newbie: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(newbie["username"], "newbie");
    let newbie_id = newbie["id"].as_str().unwrap().to_owned();

    // Duplicate name: 409.
    let dup = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("POST")
                .uri("/api/v1/users")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"username": "newbie", "password": "x"}).to_string(),
                ))
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // Cannot delete self.
    let self_delete = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/users/{}", admin.id))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(self_delete.status(), StatusCode::FORBIDDEN);

    // Delete the other: fine.
    let remove = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/users/{newbie_id}"))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(remove.status(), StatusCode::NO_CONTENT);

    // Admin list works.
    let list = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/users")
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
}

#[tokio::test]
async fn settings_get_put_and_referer_gate() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());

    // Defaults visible to the admin.
    let get = app(&state)
        .oneshot(bearer(
            Request::builder()
                .uri("/api/v1/settings")
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    let body = axum::body::to_bytes(get.into_body(), 4096).await.unwrap();
    let config: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(config["router"]["media-entry-path"], "/media");

    // Moving the media path is rejected (legacy links).
    let bad = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"router": {"media-entry-path": "/img"}}).to_string(),
                ))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    // A referer allow-list applies to the media path.
    let put = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"router": {"limit-referrer-host": ["good.example"]}})
                        .to_string(),
                ))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);

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
async fn placeholder_index_serves() {
    let (state, _) = test_state().await;
    let response = app(&state)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
