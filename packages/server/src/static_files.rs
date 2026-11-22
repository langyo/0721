//! Static asset and SPA serving for the web UI (the chest dual-mode
//! shape, adapted).
//!
//! With the `embedded-frontend` feature (default) the `dist/webui` tree
//! is compiled into the binary (`include_dir!`) and served from memory;
//! without it the same files are read from the configured static
//! directory (default `./dist/webui`) at request time, so a
//! non-embedded process must run with the dist tree on its working
//! directory or every asset 404s — the development mode.
//!
//! Routing: `/` renders the SPA shell, and every path no other route
//! matched falls back through [`fallback`]: a real file serves as an
//! asset, a path WITH a file extension that does not exist 404s (never
//! hand back `text/html` for a missing script), and everything else —
//! extension-less client routes — renders the SPA shell so deep links
//! survive a hard refresh. These routes sit outside `/api` and `/media`
//! and carry no authentication: whatever they serve is public to anyone
//! who can reach the port.
//!
//! Caching: only content-hashed files under `assets/` get a one-year
//! immutable `Cache-Control`; everything else is `no-store`, so a dist
//! swap is picked up on the next request instead of being pinned.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
#[cfg(not(feature = "embedded-frontend"))]
use tokio::io::AsyncReadExt;

use crate::state::AppState;

#[cfg(feature = "embedded-frontend")]
use include_dir::{include_dir, Dir};

/// The compiled-in webui tree (the webui package builds into the repo
/// root's `dist/webui`; `just webui` produces it before any embedded
/// build).
#[cfg(feature = "embedded-frontend")]
static WEBUI_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../dist/webui");

/// The SPA shell bytes — `index.html`.
async fn index_bytes(state: &AppState) -> Option<Vec<u8>> {
    #[cfg(feature = "embedded-frontend")]
    {
        let _ = state;
        WEBUI_DIR
            .get_file("index.html")
            .map(|f| f.contents().to_vec())
    }
    #[cfg(not(feature = "embedded-frontend"))]
    {
        read_disk(state, "index.html").await
    }
}

/// Read one file by dist-relative path, dev mode.
#[cfg(not(feature = "embedded-frontend"))]
async fn read_disk(state: &AppState, path: &str) -> Option<Vec<u8>> {
    // Confinement: the dev lane serves files from the configured static
    // directory only. A path carrying ParentDir is traversal, full stop.
    if path.split('/').any(|segment| segment == "..") {
        return None;
    }
    let root = state.config.static_dir.canonicalize().ok()?;
    let full = root.join(path);
    let resolved = full.canonicalize().ok()?;
    if !resolved.starts_with(&root) {
        return None;
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(&resolved)
        .await
        .ok()?
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    Some(bytes)
}

/// One asset's bytes by dist-relative path.
async fn asset_bytes(state: &AppState, path: &str) -> Option<Vec<u8>> {
    #[cfg(feature = "embedded-frontend")]
    {
        let _ = state;
        WEBUI_DIR.get_file(path).map(|f| f.contents().to_vec())
    }
    #[cfg(not(feature = "embedded-frontend"))]
    {
        read_disk(state, path).await
    }
}

/// Content type by file extension (the dist tree's whole vocabulary).
fn content_type_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// `Cache-Control` for one asset: only content-hashed build artifacts
/// under `assets/` are immutable — their filenames change whenever the
/// content does. Fixed-name files at the dist root are regenerated on
/// every build, so caching them for a year would pin a visitor to the
/// first build they ever loaded.
fn cache_control_of(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache, no-store, must-revalidate"
    }
}

/// `GET /` — the SPA shell. A dist without `index.html` yields a 404
/// whose body names the webui build step, never an empty 200.
pub async fn serve_root(State(state): State<AppState>) -> Response {
    serve_shell(&state).await
}

/// `GET /assets/{*path}` — hashed build artifacts.
pub async fn serve_asset(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    // URL-carried ParentDir in an asset route is traversal — legitimate
    // names are content-hashed and never contain `..`.
    if path.split('/').any(|segment| segment == "..") {
        return StatusCode::FORBIDDEN.into_response();
    }
    match asset_bytes(&state, &format!("assets/{path}")).await {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type_of(&path)),
                (
                    header::CACHE_CONTROL,
                    cache_control_of(&format!("assets/{path}")),
                ),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_shell(state: &AppState) -> Response {
    match index_bytes(state).await {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
            ],
            bytes,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "the web UI dist is missing — build it with `just webui` \
             (or point I0721_STATIC_DIR at a built dist/webui in dev mode)",
        )
            .into_response(),
    }
}

/// Router fallback for every path no other route matched: SPA deep
/// links and the dist-root static files (`favicon.ico`, `logo.webp`, …).
///
/// The order of decisions is the contract. Unknown `/api` and `/api/*`
/// paths are **API territory**: they answer a 404 JSON body (the REST
/// surface's uniform error shape), never the SPA shell — a client
/// probing the API must not receive `text/html` (the master-era
/// semantics). A traversal-free relative path is served as a file when
/// it exists; a path WITH a file extension that does not exist 404s
/// (serving `index.html` for a missing `.js` request would hand back
/// `text/html` and break script loading); everything else —
/// extension-less client routes — renders the SPA shell.
pub async fn fallback(State(state): State<AppState>, req: axum::extract::Request) -> Response {
    let raw_path = req.uri().path();
    if raw_path == "/api" || raw_path.starts_with("/api/") {
        return unknown_api_route();
    }
    let path = raw_path.trim_start_matches('/').replace('\\', "/");
    if !path.is_empty() && !path.contains("..") && !path.contains('~') && !path.starts_with('/') {
        if let Some(bytes) = asset_bytes(&state, &path).await {
            return (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type_of(&path)),
                    (header::CACHE_CONTROL, cache_control_of(&path)),
                ],
                bytes,
            )
                .into_response();
        }
        let has_extension = path
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.rsplit('.').nth(1).is_some());
        if has_extension {
            return StatusCode::NOT_FOUND.into_response();
        }
    }
    serve_shell(&state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_cover_the_dist_vocabulary() {
        assert_eq!(content_type_of("a/b.js"), "application/javascript");
        assert_eq!(content_type_of("x.webp"), "image/webp");
        assert_eq!(content_type_of("y.ico"), "image/x-icon");
        assert_eq!(content_type_of("none"), "application/octet-stream");
    }

    #[test]
    fn hashed_assets_are_immutable_root_files_are_not() {
        assert_eq!(
            cache_control_of("assets/index-Ab12.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_of("logo.webp"),
            "no-cache, no-store, must-revalidate"
        );
    }
}

/// The uniform 404 for an unknown API route: the REST surface's JSON
/// error body, so API clients never see the SPA shell's HTML.
fn unknown_api_route() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"route not found"}"#,
    )
        .into_response()
}
