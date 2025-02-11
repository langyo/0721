use anyhow::{anyhow, ensure, Result};
use bytes::Bytes;

use image::ImageFormat;

use crate::functions::backend::media::*;
use crate::init::RouteEnv;
use _types::consts::MEDIA_DIR;
use _types::response::AuthInfo;

pub async fn get_file(
    env: RouteEnv,
    auth: AuthInfo,
    db_key: impl ToString,
) -> Result<(String, Bytes)> {
    if let Some(item) = get(env.clone(), db_key.to_string()).await? {
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

        // Check permission
        if let Some(item_permission) = item.permission {
            if let Some(auth) = auth {
                ensure!(auth.permission >= item_permission, "No permission");
            }
        }

        Ok((item.mime, file))
    } else {
        let path = MEDIA_DIR.clone();
        let path = path.join(format!("{}.webp", db_key.to_string()));

        let file = tokio::fs::read(path).await?;
        let file = Bytes::from(file);
        Ok(("image/webp".to_string(), file))
    }
}
