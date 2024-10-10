use anyhow::Result;

use axum::{
    extract::DefaultBodyLimit,
    middleware::{from_extractor, from_extractor_with_state},
    routing::get,
    Router,
};

use crate::utils::{ExtractAuthInfo, ExtractIP, ExtractLanguageInfo};
use _database::RouteEnv;

pub mod backend;
pub mod frontend;

pub async fn route(env: RouteEnv) -> Result<Router> {
    let router = Router::new()
        .nest("/", frontend::route(env.clone()).await?)
        .nest("/api", backend::route(env.clone()).await?)
        .fallback(get(frontend::pages::not_found::query).with_state(env.clone()))
        .layer(from_extractor_with_state::<ExtractAuthInfo, _>(env.clone()))
        .layer(from_extractor::<ExtractIP>())
        .layer(from_extractor::<ExtractLanguageInfo>())
        .layer(DefaultBodyLimit::max(1024 * 1024 * 16)); // 16 MiB

    Ok(router)
}
