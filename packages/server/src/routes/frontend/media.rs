use anyhow::Result;
use bytes::Bytes;
use image::EncodableLayout;
use serde::{Deserialize, Serialize};
use std::io::prelude::*;

use axum::{
    extract::{Path, Query},
    response::IntoResponse,
};
use flate2::{write::GzEncoder, Compression};
use hyper::{HeaderMap, StatusCode};

use crate::utils::ExtractAuthInfo;
use _database::functions::frontend::image::get_file;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Args {
    pub height: Option<u32>,
    pub width: Option<u32>,
}

impl Args {
    pub fn is_some(self) -> bool {
        self.height.is_some() || self.width.is_some()
    }
}

#[tracing::instrument]
pub async fn download_media(
    headers: HeaderMap,
    Path(hash): Path<String>,
    ExtractAuthInfo(auth): ExtractAuthInfo,
    Query(args): Query<Args>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // TODO - Check the referer header that the domain is allowed in the global config
    //        If not, return 403 Forbidden

    let enable_gzip = headers
        .get(hyper::header::ACCEPT_ENCODING)
        .map(|value| value.to_str().unwrap_or("").contains("gzip"))
        .unwrap_or(false);
    let (mime, file) = get_file(auth, hash)
        .await
        .map_err(|err| (StatusCode::NOT_FOUND, err.to_string()))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_str(mime.as_str())
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
    );

    if enable_gzip {
        headers.insert(
            hyper::header::CONTENT_ENCODING,
            hyper::header::HeaderValue::from_static("gzip"),
        );
    }

    let image = if args.is_some() {
        let image = image::load_from_memory(&file)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        let width = args
            .width
            .map(|n| n as f32)
            .unwrap_or(
                image.width() as f32 * (args.height.unwrap_or(120) as f32 / image.height() as f32),
            )
            .round() as u32;
        let height = args
            .height
            .map(|n| n as f32)
            .unwrap_or(
                image.height() as f32 * (args.width.unwrap_or(120) as f32 / image.width() as f32),
            )
            .round() as u32;

        let image = image::imageops::thumbnail(&image, width, height);
        let image = image::DynamicImage::from(image);

        let image = webp::Encoder::from_image(&image)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        let image = image.encode(100.0);
        Bytes::from(image.as_bytes().to_vec())
    } else {
        file
    };

    log::info!("enable_gzip: {}", enable_gzip);
    log::info!("Before size: {}", image.len());
    let image = if enable_gzip {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&image)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
        Bytes::from(
            encoder
                .finish()
                .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
        )
    } else {
        image
    };
    log::info!("After size: {}", image.len());

    Ok((headers, image).into_response())
}
