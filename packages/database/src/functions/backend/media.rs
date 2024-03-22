use anyhow::{anyhow, Result};
use base64::prelude::*;
use bytes::Bytes;
use once_cell::sync::Lazy;

use chrono::Utc;
use sha3::{Digest as _, Sha3_256};

use crate::{
    functions::backend::media_insert_log::list as list_log,
    models::{media::*, user::Permission},
    types::config::load_config,
    MEDIA_RES_DIR,
};

pub static DB: Lazy<sled::Db> = Lazy::new(|| {
    sled::open({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("media.db");
        path
    })
    .unwrap()
});

pub static IS_ENABLE_WEBP_AUTO_CONVERT: Lazy<bool> = Lazy::new(|| {
    let config = load_config().unwrap();
    config.upload.webp_auto_convert
});

pub async fn count() -> Result<usize> {
    Ok(DB.len())
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<Model>> {
    let ret = list_log(offset, limit)
        .await?
        .into_iter()
        .map(|(_time, hash)| {
            let raw = DB
                .get(hash.clone())?
                .ok_or(anyhow!("Image not found: {}", hash))?;
            let value = postcard::from_bytes(raw.as_ref()).unwrap();
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ret)
}

pub async fn set(uploader: String, data: Bytes) -> Result<String> {
    let now = Utc::now();
    let hash = Sha3_256::digest(&data).to_vec();
    let hash = BASE64_URL_SAFE_NO_PAD.encode(&hash);
    let size = data.len() as u64;

    let data = if *IS_ENABLE_WEBP_AUTO_CONVERT {
        let image = image::load_from_memory(&data)?;
        let encoder = webp::Encoder::from_image(&image)
            .map_err(|err| anyhow!("Failed to create webp encoder from image: {}", err))?;
        let data = encoder.encode(100.0);
        data.to_vec().into()
    } else {
        data
    };

    let mime = image::guess_format(&data)?;
    let file_name = format!(
        "{}.{}",
        hash,
        mime.extensions_str().first().ok_or(anyhow!(
            "Failed to get extension from MIME type: {}",
            mime.to_mime_type()
        ))?
    );
    let file_path = MEDIA_RES_DIR.clone().join(&file_name);

    if file_path.exists() {
        return Err(anyhow!("Image already exists: {}", hash));
    }

    tokio::fs::write(&file_path, data).await?;

    let value = Model {
        uploader: uploader.clone(),
        permission: Permission::Guest,
        created_at: now,
        hash: hash.clone(),
        size,
        mime: mime.to_mime_type().to_string(),
    };
    let raw = postcard::to_allocvec(&value)?;
    DB.insert(hash.as_str(), raw)?;

    super::media_insert_log::insert(hash.clone()).await?;

    Ok(hash)
}

pub async fn get(key: impl ToString) -> Result<Option<Model>> {
    let ret = DB
        .get(key.to_string().as_bytes())?
        .map(|r| postcard::from_bytes(r.to_vec().as_slice()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.remove(id.as_bytes())?;

    Ok(())
}
