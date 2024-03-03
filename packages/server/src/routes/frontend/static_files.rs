use anyhow::Result;

use axum::{routing::get_service, Router};
use tower_http::services::ServeFile;

use _database::{WASM_DIR, WEBSITE_RES_DIR};

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route_service(
            "/client.js",
            get_service(ServeFile::new(WASM_DIR.clone().join("client.js"))),
        )
        .route_service(
            "/client.wasm",
            get_service(ServeFile::new(WASM_DIR.clone().join("client_bg.wasm"))),
        )
        .route_service(
            "/favicon.ico",
            get_service(ServeFile::new(WEBSITE_RES_DIR.clone().join("favicon.ico"))),
        )
        .route_service(
            "/bg.webp",
            get_service(ServeFile::new(WEBSITE_RES_DIR.clone().join("bg.webp"))),
        )
        .route_service(
            "/logo.webp",
            get_service(ServeFile::new(WEBSITE_RES_DIR.clone().join("logo.webp"))),
        );

    Ok(router)
}
