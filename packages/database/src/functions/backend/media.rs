use anyhow::{anyhow, Result};
use base64::prelude::*;
use bytes::Bytes;
use once_cell::sync::Lazy;
use sha3::{Digest as _, Sha3_256};

use redb::{Database, ReadableTable as _};

use crate::{
    models::{media::*, user::Permission},
    MEDIA_RES_DIR,
};

#[cfg(not(target_arch = "wasm32"))]
pub const DB: Lazy<Database> = Lazy::new(|| {
    let db = Database::create({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("media.redb");
        path
    })
    .unwrap();
    let ctx = db.begin_write().unwrap();
    {
        ctx.open_table(TABLE).unwrap();
    }
    ctx.commit().unwrap();
    db
});

pub async fn count() -> Result<u64> {
    let count = DB.begin_read()?.open_table(TABLE)?.len()?;

    Ok(count)
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<Model>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .iter()?
        .skip(offset)
        .take(limit)
        .map(|raw| raw.unwrap().1.value().into())
        .map(|raw: Vec<u8>| postcard::from_bytes::<Model>(&raw.as_slice()).unwrap())
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
    DB.begin_write()?
        .open_table(TABLE)?
        .insert(hash.to_string().as_str(), &raw.as_ref())?;

    Ok(hash)
}

pub async fn get(key: impl ToString) -> Result<Option<Model>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .get(key.to_string().as_str())?
        .map(|r| postcard::from_bytes(r.value()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.begin_write()?.open_table(TABLE)?.remove(&id.as_str())?;

    Ok(())
}
