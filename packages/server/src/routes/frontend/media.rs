use anyhow::Result;
use bytes::Bytes;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use axum::{
    extract::{Path, Query},
    response::IntoResponse,
};
use hyper::{HeaderMap, StatusCode};

use crate::utils::ExtractAuthInfo;
use _database::{
    functions::{backend::media::generate_thumbnail, frontend::image::get_file},
    MEDIA_CACHE_DIR,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Args {
    pub thumbnail: Option<bool>,
}

impl Args {
    pub fn is_some(self) -> bool {
        if let Some(flag) = self.thumbnail {
            flag
        } else {
            false
        }
    }
}

static WHITE_LIST: Lazy<Option<Vec<String>>> = Lazy::new(|| {
    let config = _database::types::config::load_config().unwrap();
    let white_list = config.router.limit_referrer_host.clone();
    white_list.filter(|white_list| !white_list.is_empty())
});

#[tracing::instrument]
pub async fn download_media(
    headers: HeaderMap,
    Path(db_key): Path<String>,
    ExtractAuthInfo(auth): ExtractAuthInfo,
    Query(args): Query<Args>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Check the referer header that the domain is allowed in the global config.

    let request_referer = headers
        .get(hyper::header::REFERER)
        .map(|v| v.to_str().unwrap().to_string());
    if let Some(referer) = request_referer {
        if let Some(white_list) = WHITE_LIST.as_ref() {
            let referer = url::Url::parse(&referer).map_err(|err| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Failed to parse referer URL: {}", err),
                )
            })?;
            if !white_list
                .iter()
                .any(|host| referer.host_str().unwrap() == host)
            {
                return Err((StatusCode::FORBIDDEN, "Referer not allowed".to_string()));
            }
        }
    }

    // Read the image file.

    let (mime, file) = get_file(auth, &db_key)
        .await
        .map_err(|err| (StatusCode::NOT_FOUND, err.to_string()))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_str(mime.as_str())
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
    );

    let image = if args.is_some() {
        // Try to read cache file.

        let mut path = MEDIA_CACHE_DIR.clone();
        path.push(&format!("{}.webp", db_key));

        if let Ok(file_raw) = tokio::fs::read(path).await {
            Bytes::from(file_raw)
        } else {
            generate_thumbnail(db_key, file)
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        }
    } else {
        file
    };

    Ok((headers, image).into_response())
}
