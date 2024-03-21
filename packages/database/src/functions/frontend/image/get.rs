use anyhow::{anyhow, Result};
use bytes::Bytes;

use image::ImageFormat;

use crate::{
    functions::backend::media::*, models::user::Permission, types::response::AuthInfo,
    MEDIA_RES_DIR,
};

pub async fn get_file(auth: AuthInfo, hash: String) -> Result<(String, Bytes)> {
    let item = get(hash).await?.ok_or(anyhow!("Image not found"))?;

    // Check permission
    if let Some(auth) = auth {
        if auth.permission < item.permission {
            return Err(anyhow!("No permission"));
        }
    } else if item.permission != Permission::Guest {
        return Err(anyhow!("No permission"));
    }

    let mime = ImageFormat::from_mime_type(&item.mime)
        .ok_or(anyhow!("Failed to get MIME type from image"))?;
    let path = MEDIA_RES_DIR.clone();
    let path = path.join(format!(
        "{}.{}",
        item.hash,
        mime.extensions_str().first().ok_or(anyhow!(
            "Failed to get extension from MIME type: {}",
            mime.to_mime_type()
        ))?
    ));

    let file = tokio::fs::read(path).await?;
    let file = Bytes::from(file);

    Ok((item.mime, file))
}
