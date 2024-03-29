use anyhow::{anyhow, ensure, Result};
use bytes::Bytes;

use image::ImageFormat;

use crate::{
    functions::backend::media::*, models::user::Permission, types::response::AuthInfo, MEDIA_DIR,
};

pub async fn get_file(auth: AuthInfo, db_key: impl ToString) -> Result<(String, Bytes)> {
    let item = get(db_key).await?.ok_or(anyhow!("Image not found"))?;

    // Check permission
    if let Some(auth) = auth {
        ensure!(auth.permission >= item.permission, "No permission");
    }
    ensure!(item.permission == Permission::Guest, "No permission");

    let mime = ImageFormat::from_mime_type(&item.mime)
        .ok_or(anyhow!("Failed to get MIME type from image"))?;
    let path = MEDIA_DIR.clone();
    let path = path.join(format!(
        "{}.{}",
        item.name,
        mime.extensions_str().first().ok_or(anyhow!(
            "Failed to get extension from MIME type: {}",
            mime.to_mime_type()
        ))?
    ));

    let file = tokio::fs::read(path).await?;
    let file = Bytes::from(file);

    Ok((item.mime, file))
}
