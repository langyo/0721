use anyhow::Result;
use once_cell::sync::Lazy;

use chrono::{DateTime, Utc};

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

pub async fn list(offset: usize, limit: usize) -> Result<Vec<(DateTime<Utc>, String)>> {
    let ret = DB
        .iter()
        .rev()
        .skip(offset)
        .take(limit)
        .map(|item| item.unwrap())
        .map(|(key, value)| {
            (
                String::from_utf8(key.to_vec()).unwrap(),
                String::from_utf8(value.to_vec()).unwrap(),
            )
        })
        .map(|(date, hash)| (DateTime::parse_from_rfc3339(&date).unwrap().to_utc(), hash))
        .collect::<Vec<_>>();

    Ok(ret)
}

pub async fn insert(hash: String) -> Result<DateTime<Utc>> {
    let now = Utc::now();
    DB.insert(now.to_rfc3339(), hash.as_str())?;

    Ok(now)
}

pub async fn get(date: impl ToString) -> Result<Option<String>> {
    let ret = DB
        .get(date.to_string().as_bytes())
        .map(|item| item.map(|v| String::from_utf8(v.to_vec()).unwrap()))?;

    Ok(ret)
}

pub async fn delete(date: impl ToString) -> Result<()> {
    DB.remove(date.to_string().as_bytes())?;

    Ok(())
}
