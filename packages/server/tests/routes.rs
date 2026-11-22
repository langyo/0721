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
                .body(Body::from(full_settings_json(|json| {
                    json["upload"]["image-size-limit"] = "1KiB".into();
                })))
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
                .body(Body::from(full_settings_json(|json| {
                    json["router"]["limit-referrer-host"] = serde_json::json!(["good.example"]);
                })))
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

#[tokio::test]
async fn settings_put_requires_the_complete_document() {
    let (state, admin) = test_state().await;
    let token = state.signer.sign(admin.id, Timestamp::now());

    // Missing a whole section: 400, and the stored config is untouched.
    let partial = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"router": {"media-entry-path": "/media", "limit-referrer-host": []}})
                        .to_string(),
                ))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::BAD_REQUEST);

    // Unknown leaf inside a section: 400.
    let unknown_leaf = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(full_settings_json(|json| {
                    json["upload"]["max-pixels"] = serde_json::json!(1);
                })))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(unknown_leaf.status(), StatusCode::BAD_REQUEST);

    // Unknown top-level section: 400.
    let unknown_section = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(full_settings_json(|json| {
                    json["experimental"] = serde_json::json!({});
                })))
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(unknown_section.status(), StatusCode::BAD_REQUEST);

    // The stored config still round-trips the seed defaults.
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
    assert_eq!(
        config,
        serde_json::to_value(i0721_proto::default_site_config()).unwrap()
    );
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

    // Deleting one owner's row must NOT remove the shared blob while
    // the other row still lives.
    let url = admin_file["url"].as_str().unwrap().to_owned();
    let id = admin_file["id"].as_str().unwrap().to_owned();
    let removed = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{id}"))
                .body(Body::empty())
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
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
    let removed_last = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{member_id}"))
                .body(Body::empty())
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(removed_last.status(), StatusCode::NO_CONTENT);
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
    for (method, uri) in [
        ("PATCH", format!("/api/v1/files/{admin_id}")),
        ("DELETE", format!("/api/v1/files/{admin_id}")),
    ] {
        let body = if method == "PATCH" {
            Body::from(r#"{"name": "hijacked"}"#)
        } else {
            Body::empty()
        };
        let response = app(&state)
            .oneshot(bearer(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body)
                    .unwrap(),
                &member_token,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{method} by member"
        );
    }

    // The member's own file: theirs to rename and delete.
    let (_, member_file) = upload(&state, &member_token, tiny_png(), "member.png").await;
    let member_id = member_file["id"].as_str().unwrap().to_owned();
    let renamed = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/files/{member_id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name": "my file"}"#))
                .unwrap(),
            &member_token,
        ))
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);

    // The admin may rename the member's file (owner-or-admin).
    let admin_renamed = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/files/{member_id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name": "admin says"}"#))
                .unwrap(),
            &admin_token,
        ))
        .await
        .unwrap();
    assert_eq!(admin_renamed.status(), StatusCode::OK);
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
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{file_id}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

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
    let response = app(&state)
        .oneshot(bearer(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/files/{file_id}"))
                .body(Body::empty())
                .unwrap(),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

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
