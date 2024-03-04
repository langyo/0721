use anyhow::Result;
use once_cell::sync::Lazy;

use redb::{Database, ReadableTable as _};

use crate::models::config::*;

#[cfg(not(target_arch = "wasm32"))]
pub const DB: Lazy<Database> = Lazy::new(|| {
    let db = Database::create({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("config.redb");
        path
    })
    .unwrap();
    let ctx = db.begin_write().unwrap();
    {
        ctx.open_table(TABLE).unwrap();
    }
    ctx.commit().unwrap();
    db
});

pub async fn list() -> Result<Vec<String>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .iter()?
        .map(|raw| raw.unwrap().1.value().into())
        .collect::<Vec<_>>();

    Ok(ret)
}

pub async fn set(key: impl ToString, value: impl ToString) -> Result<()> {
    DB.begin_write()?
        .open_table(TABLE)?
        .insert(key.to_string().as_str(), &value.to_string().as_str())?;

    Ok(())
}

pub async fn get(key: impl ToString) -> Result<Option<String>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .get(key.to_string().as_str())?
        .map(|r| r.value().to_string());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.begin_write()?.open_table(TABLE)?.remove(&id.as_str())?;

    Ok(())
}
