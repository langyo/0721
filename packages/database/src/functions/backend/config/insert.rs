use anyhow::{anyhow, Result};

use crate::{models::*, DB_CONN};

pub async fn insert(id: String, value: String) -> Result<()> {
    let ctx = DB_CONN
        .get()
        .ok_or(anyhow!("Failed to get database connection"))?
        .begin_write()?;
    {
        let mut table = ctx.open_table(config::TABLE)?;
        table.insert(&id.as_str(), &value.as_str())?;
    }
    ctx.commit()?;

    Ok(())
}
