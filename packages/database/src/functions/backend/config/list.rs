use anyhow::{anyhow, Result};
use std::collections::HashMap;

use redb::ReadableTable as _;

use crate::{models::*, DB_CONN};

pub async fn list() -> Result<HashMap<String, String>> {
    let ret = {
        let ctx = DB_CONN
            .get()
            .ok_or(anyhow!("Failed to get database connection"))?
            .begin_read()?;
        let table = ctx.open_table(config::TABLE)?;
        let ret = table
            .iter()?
            .map(|item| {
                let ret = item.unwrap();
                (ret.0.value().to_string(), ret.1.value().to_string())
            })
            .collect::<HashMap<_, _>>();
        ret
    };

    Ok(ret)
}
