use anyhow::Result;
use once_cell::sync::Lazy;

use redb::{Database, ReadableTable as _};

use crate::models::user::*;

#[cfg(not(target_arch = "wasm32"))]
pub const DB: Lazy<Database> = Lazy::new(|| {
    let db = Database::create({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("user.redb");
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

pub async fn count() -> Result<u64> {
    let count = DB.begin_read()?.open_table(TABLE)?.len()?;

    Ok(count)
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<Model>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .iter()?
        .skip(offset)
        .take(limit)
        .map(|raw| raw.unwrap().1.value().into())
        .map(|raw: Vec<u8>| postcard::from_bytes::<Model>(&raw.as_slice()).unwrap())
        .collect::<Vec<_>>();

    Ok(ret)
}

pub async fn set(key: impl ToString, value: &Model) -> Result<()> {
    let raw = postcard::to_allocvec(value)?;
    DB.begin_write()?
        .open_table(TABLE)?
        .insert(key.to_string().as_str(), &raw.as_ref())?;

    Ok(())
}

pub async fn get(key: impl ToString) -> Result<Option<Model>> {
    let ret = DB
        .begin_read()?
        .open_table(TABLE)?
        .get(key.to_string().as_str())?
        .map(|r| postcard::from_bytes(r.value()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.begin_write()?.open_table(TABLE)?.remove(&id.as_str())?;

    Ok(())
}
