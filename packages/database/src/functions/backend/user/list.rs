use anyhow::{anyhow, Result};

use redb::ReadableTable as _;

use crate::{models::*, DB_CONN};

pub async fn count() -> Result<u64> {
    let count = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(user::TABLE)?;
        table.len()?
    };

    Ok(count)
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<user::Model>> {
    let ret = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(user::TABLE)?;
        let ret = table
            .iter()?
            .skip(offset)
            .take(limit)
            .map(|raw| raw.unwrap().1.value().into())
            .map(|raw: Vec<u8>| postcard::from_bytes::<user::Model>(&raw.as_slice()).unwrap())
            .collect::<Vec<_>>();
        ret
    };

    Ok(ret)
}
