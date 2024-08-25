use anyhow::Result;

use axum::{body::Body, http::Request, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::{ExtractAuthInfo, ExtractLanguageInfo};
use _client::app::AppStates;
use _types::config::load_config;

#[tracing::instrument]
pub async fn query(
    ExtractLanguageInfo(language): ExtractLanguageInfo,
    ExtractAuthInfo(auth): ExtractAuthInfo,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let uri = req.uri().to_string();
    let title = language
        .to_config()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .header
        .users;

    let ret = super::html_render(
        uri,
        AppStates {
            title,
            auth,
            language,
            config: load_config()
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(ret.into_response())
}
