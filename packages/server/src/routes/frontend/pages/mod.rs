mod images;
mod login;
pub mod not_found;
mod portal;
mod users;

use anyhow::Result;

use axum::{routing::get, Router};
use hikari_boot::Application;
use hyper::HeaderMap;

use _client::app::{App, AppStates};

pub async fn html_render(uri: String, states: AppStates) -> Result<(HeaderMap, String)> {
    let mut headers = HeaderMap::new();
    headers.insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("text/html; charset=utf-8"),
    );

    Ok((headers, App::render_to_string(uri, states).await))
}

pub async fn route() -> Result<Router> {
    let router = Router::new()
        .route("/", get(portal::query))
        .route("/login", get(login::query))
        .route("/images", get(images::query))
        .route("/users", get(users::query));

    Ok(router)
}
