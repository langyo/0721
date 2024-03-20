use anyhow::{anyhow, Result};
use base64::prelude::*;
use bytes::Bytes;
use once_cell::sync::Lazy;
use sha3::{Digest as _, Sha3_256};

use crate::{
    models::{media::*, user::Permission},
    MEDIA_RES_DIR,
};

#[cfg(not(target_arch = "wasm32"))]
pub static DB: Lazy<sled::Db> = Lazy::new(|| {
    sled::open({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("media.db");
        path
    })
    .unwrap()
});

pub async fn count() -> Result<usize> {
    Ok(DB.len())
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<Model>> {
    let ret = DB
        .iter()
        .skip(offset)
        .take(limit)
        .map(|r| r.unwrap())
        .map(|r| postcard::from_bytes(r.1.to_vec().as_slice()).unwrap())
        .collect::<Vec<_>>();

    Ok(ret)
}

pub async fn set(uploader: String, data: Bytes) -> Result<String> {
    let hash = Sha3_256::digest(&data).to_vec();
    let hash = BASE64_URL_SAFE_NO_PAD.encode(&hash);
    let size = data.len() as u64;

    // TODO - Use MIME map instead of guessing from image library to support audios and videos
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
        hash: hash.clone(),
        size,
        mime: mime.to_mime_type().to_string(),
    };
    let raw = postcard::to_allocvec(&value)?;
    DB.insert(hash.as_str(), raw)?;

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
