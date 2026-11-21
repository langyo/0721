use anyhow::Result;

use axum::{routing::get_service, Router};
use tower_http::services::ServeFile;

use _database::RouteEnv;
use _types::consts::WEBSITE_RES_DIR;

pub async fn route(env: RouteEnv) -> Result<Router> {
    let website = WEBSITE_RES_DIR.clone();
    let router = Router::new()
        .route_service(
            "/",
            get_service(ServeFile::new(website.join("index.html"))),
        )
        .route_service(
            "/favicon.ico",
            get_service(ServeFile::new(website.join("favicon.ico"))),
        )
        .route_service(
            "/bg.webp",
            get_service(ServeFile::new(website.join("bg.webp"))),
        )
        .route_service(
            "/logo.webp",
            get_service(ServeFile::new(website.join("logo.webp"))),
        )
        .with_state(env);

    Ok(router)
}
