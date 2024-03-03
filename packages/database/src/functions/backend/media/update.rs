use anyhow::{anyhow, Result};

use crate::{models::*, DB_CONN};

pub async fn delete(id: String) -> Result<()> {
    let ctx = DB_CONN
        .get()
        .ok_or(anyhow!("Failed to get database connection"))?
        .begin_write()?;
    {
        let mut table = ctx.open_table(media::TABLE)?;
        table.remove(&id.as_str())?;
    }
    ctx.commit()?;

    Ok(())
}
