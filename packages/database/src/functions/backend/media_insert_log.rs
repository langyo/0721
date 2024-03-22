use anyhow::Result;
use once_cell::sync::Lazy;

use chrono::{DateTime, Utc};

use crate::models::media_insert_log::*;

pub static DB: Lazy<sled::Db> = Lazy::new(|| {
    sled::open({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("media-insert-log.db");
        path
    })
    .unwrap()
});

pub async fn count() -> Result<usize> {
    Ok(DB.len())
}

pub async fn list(offset: usize, limit: usize) -> Result<Vec<Model>> {
    let ret = DB
        .iter()
        .rev()
        .skip(offset)
        .take(limit)
        .map(|r| r.unwrap())
        .map(|r| postcard::from_bytes(r.1.to_vec().as_slice()).unwrap())
        .collect::<Vec<_>>();

    Ok(ret)
}

pub async fn insert(hash: String) -> Result<DateTime<Utc>> {
    let now = Utc::now();
    let value = Model {
        hash,
        update_at: now.clone(),
    };
    let raw = postcard::to_allocvec(&value)?;
    DB.insert(now.to_rfc3339(), raw)?;

    Ok(now)
}

pub async fn get(key: impl ToString) -> Result<Option<Model>> {
    let ret = DB
        .get(key.to_string().as_bytes())?
        .map(|r| postcard::from_bytes(r.to_vec().as_slice()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.remove(id.as_bytes())?;

    Ok(())
}
