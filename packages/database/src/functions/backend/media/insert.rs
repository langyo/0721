use anyhow::{anyhow, Result};
use base64::prelude::*;
use bytes::Bytes;
use sha3::{Digest, Sha3_256};

use crate::{models::*, DB_CONN, MEDIA_RES_DIR};

pub async fn insert(uploader: String, data: Bytes) -> Result<()> {
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

    let raw = postcard::to_allocvec(&media::Model {
        uploader: uploader.clone(),
        permission: user::Permission::Guest,
        hash,
        size,
        mime: mime.to_mime_type().to_string(),
    })?;
    let ctx = DB_CONN
        .get()
        .ok_or(anyhow!("Failed to get database connection"))?
        .begin_write()?;
    {
        let mut table = ctx.open_table(user::TABLE)?;
        table.insert(&uploader.as_str(), &raw.as_slice())?;
    }
    ctx.commit()?;

    Ok(())
}
