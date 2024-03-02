use anyhow::{anyhow, Result};

use crate::{models::*, DB_CONN};

pub async fn insert(name: String, vo: user::Model) -> Result<()> {
    let raw = postcard::to_allocvec(&vo)?;
    let ctx = DB_CONN
        .get()
        .ok_or(anyhow!("Failed to get database connection"))?
        .begin_write()?;
    {
        let mut table = ctx.open_table(user::TABLE)?;
        table.insert(&name.as_str(), &raw.as_slice())?;
    }
    ctx.commit()?;

    Ok(())
}
