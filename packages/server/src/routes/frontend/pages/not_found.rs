use anyhow::Result;

use axum::{body::Body, http::Request, response::IntoResponse};
use hyper::StatusCode;

use crate::utils::ExtractAuthInfo;
use _client::app::AppStates;

#[tracing::instrument]
pub async fn query(
    ExtractAuthInfo(auth): ExtractAuthInfo,
    req: Request<Body>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let uri = req.uri().to_string();

    let ret = super::html_render(
        uri,
        AppStates {
            title: "Not found".to_string(),
            auth,
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(ret.into_response())
}
