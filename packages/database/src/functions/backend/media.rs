use anyhow::{anyhow, ensure, Context, Result};
use base64::prelude::*;
use bytes::Bytes;
use once_cell::sync::Lazy;

use chrono::Utc;
use image::{EncodableLayout as _, GenericImageView, ImageFormat};
use sha3::{Digest as _, Sha3_256};

use crate::init::RouteEnv;
use _types::{
    config::load_config,
    consts::{MEDIA_CACHE_DIR, MEDIA_DIR},
    models::media::*,
};
use tairitsu_database::prelude::*;

pub static IS_ENABLE_WEBP_AUTO_CONVERT: Lazy<bool> = Lazy::new(|| {
    let config = load_config().unwrap();
    config.upload.webp_auto_convert
});
pub static IS_USE_SOURCE_FILE_NAME: Lazy<bool> = Lazy::new(|| {
    let config = load_config().unwrap();
    config.upload.use_source_file_name
});

pub const GLOBAL_CONFIG_MEDIA_COUNT_KEY: &str = "media_count";

pub async fn get(env: RouteEnv, id: String) -> Result<Option<Model>> {
    if let Some(ret) = env.kv.images.get(id).await? {
        Ok(serde_json::from_str(&ret)?)
    } else {
        Ok(None)
    }
}

pub async fn count(env: RouteEnv) -> Result<usize> {
    let count = env
        .kv
        .global_config
        .get(GLOBAL_CONFIG_MEDIA_COUNT_KEY.to_string())
        .await?
        .map(|x| x.parse::<usize>().unwrap_or(0))
        .unwrap_or(0);

    Ok(count)
}

pub async fn list(env: RouteEnv, offset: usize, limit: usize) -> Result<Vec<Model>> {
    let keys = env
        .kv
        .images
        .list_by_prefix("".to_string(), Some(limit), Some(offset.to_string()))
        .await?;
    let values = keys
        .iter()
        .map(|key| async {
            let value = env
                .kv
                .images
                .get(key.clone())
                .await?
                .ok_or(anyhow!("Key not found"))?;
            Ok(serde_json::from_str(&value)?)
        })
        .collect::<Vec<_>>();
    let values: Vec<Result<Model>> = futures::future::join_all(values).await;
    let values = values.into_iter().collect::<Result<Vec<Model>>>()?;

    Ok(values)
}

pub async fn set(
    env: RouteEnv,
    uploader: String,
    data: Bytes,
    file_name_raw: Option<String>,
) -> Result<String> {
    let now = Utc::now();
    let hash = Sha3_256::digest(&data).to_vec();
    let hash = BASE64_URL_SAFE_NO_PAD.encode(&hash);
    let size = data.len() as u64;

    let db_key = if *IS_USE_SOURCE_FILE_NAME {
        if let Some(file_name) = file_name_raw {
            file_name
        } else {
            hash.clone()
        }
    } else {
        hash.clone()
    };

    // Check if the image is already uploaded
    ensure!(
        env.kv.images.get(db_key.clone()).await?.is_none(),
        "Image already uploaded"
    );

    let data = if *IS_ENABLE_WEBP_AUTO_CONVERT {
        use image::{codecs::gif::GifDecoder, AnimationDecoder};

        let image = image::load_from_memory(&data)?;
        let mime = image::guess_format(&data)?;

        match mime {
            ImageFormat::WebP => data,
            ImageFormat::Gif => {
                let dimensions = image.dimensions();
                let decoder = GifDecoder::new(std::io::Cursor::new(data))?;
                let frames = decoder.into_frames().collect_frames()?;

                let mut encoder = webp_animation::Encoder::new(dimensions)?;
                let mut delay_count = 0.;
                for frame in frames.iter() {
                    let delay = frame.delay().numer_denom_ms();
                    let delay = delay.0 as f32 / delay.1 as f32;
                    let frame = frame.buffer();
                    encoder.add_frame(frame, delay_count as i32)?;
                    delay_count += delay;
                }

                let data = encoder.finalize(delay_count as i32)?;
                data.to_vec().into()
            }
            _ => {
                let encoder = webp::Encoder::from_image(&image)
                    .map_err(|err| anyhow!("Failed to create webp encoder from image: {}", err))?;
                let data = encoder.encode(100.0);
                data.to_vec().into()
            }
        }
    } else {
        data
    };
    let mime = image::guess_format(&data)?;

    let file_path = MEDIA_DIR.clone().join(format!("{}.{}", db_key, {
        if *IS_ENABLE_WEBP_AUTO_CONVERT {
            "webp"
        } else {
            mime.extensions_str().first().ok_or(anyhow!(
                "Failed to get extension from MIME type: {}",
                mime.to_mime_type()
            ))?
        }
    }));
    if !file_path.exists() {
        tokio::fs::write(&file_path, data.clone()).await?;
    }

    let value = Model {
        uploader: uploader.clone(),
        permission: None,
        created_at: now,

        name: db_key.clone(),
        hash: hash.clone(),
        size,
        mime: mime.to_mime_type().to_string(),
    };
    env.kv
        .images
        .set(db_key.clone(), serde_json::to_string(&value)?)
        .await?;

    let count = env
        .kv
        .global_config
        .get(GLOBAL_CONFIG_MEDIA_COUNT_KEY.to_string())
        .await?
        .map(|x| x.parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    env.kv
        .global_config
        .set(
            GLOBAL_CONFIG_MEDIA_COUNT_KEY.to_string(),
            (count + 1).to_string(),
        )
        .await?;

    std::thread::spawn({
        let db_key = db_key.clone();
        let hash = hash.clone();
        move || {
            generate_thumbnail(&hash, data)
                .context(anyhow!(
                    "Failed to generate thumbnail for image: {} (hash: {})",
                    db_key,
                    hash
                ))
                .unwrap();
        }
    });

    Ok(db_key)
}

pub fn generate_thumbnail(hash: impl ToString, data: Bytes) -> Result<Bytes> {
    let image = image::load_from_memory(&data)?;

    let old_width = image.width();
    let old_height = image.height();

    let width = if old_width > old_height {
        128
    } else {
        128 * old_width / old_height
    };
    let height = if old_width < old_height {
        128
    } else {
        128 * old_height / old_width
    };

    let image = image::imageops::thumbnail(&image, width, height);
    let image = image::DynamicImage::from(image);

    let image = webp::Encoder::from_image(&image)
        .map_err(|err| anyhow!("Failed to create webp encoder from image: {}", err))?;
    let image = image.encode(100.0);
    let image = Bytes::from(image.as_bytes().to_vec());

    std::fs::write(
        {
            let mut path = MEDIA_CACHE_DIR.clone();
            path.push(format!("{}.webp", hash.to_string()));
            path
        },
        &image,
    )?;

    Ok(image)
}

pub async fn delete(env: RouteEnv, id: String) -> Result<()> {
    env.kv.images.delete(id).await?;

    let count = env
        .kv
        .global_config
        .get(GLOBAL_CONFIG_MEDIA_COUNT_KEY.to_string())
        .await?
        .map(|x| x.parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    env.kv
        .global_config
        .set(
            GLOBAL_CONFIG_MEDIA_COUNT_KEY.to_string(),
            (count - 1).to_string(),
        )
        .await?;

    Ok(())
}
