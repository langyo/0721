use anyhow::{anyhow, Result};
use bytes::Bytes;

use image::ImageFormat;
use redb::ReadableTable as _;

use crate::{
    models::{user::Permission, *},
    types::response::AuthInfo,
    DB_CONN, MEDIA_RES_DIR,
};

pub async fn get_file(auth: AuthInfo, hash: String) -> Result<(String, Bytes)> {
    let item = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(media::TABLE)?;
        let raw = table
            .get(hash.as_str())?
            .ok_or(anyhow!("Image not found"))?;
        postcard::from_bytes::<media::Model>(raw.value())?
    };

    // Check permission
    match auth {
        AuthInfo::User(info) => {
            if info.permission < item.permission {
                return Err(anyhow!("No permission"));
            }
        }
        _ => {
            if item.permission != Permission::Guest {
                return Err(anyhow!("No permission"));
            }
        }
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
