use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::models::user::*;
use _types::consts::DATABASE_DIR;

pub static DB: Lazy<sled::Db> = Lazy::new(|| {
    sled::open({
        let mut path = (*DATABASE_DIR).clone();
        path.push("user");
        path
    })
    .unwrap()
});

pub async fn count() -> Result<usize> {
    Ok(DB.len())
}

pub async fn list() -> Result<HashMap<String, Model>> {
    let ret = DB
        .iter()
        .map(|r| r.unwrap())
        .map(|(key, value)| {
            (
                String::from_utf8(key.to_vec()).unwrap(),
                postcard::from_bytes(value.to_vec().as_slice()).unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();

    Ok(ret)
}

pub async fn set(key: impl ToString, value: &Model) -> Result<()> {
    let raw = postcard::to_allocvec(value)?;
    DB.insert(key.to_string().as_str(), raw)?;

    Ok(())
}

pub async fn get(key: impl ToString) -> Result<Option<Model>> {
    let ret = DB
        .get(key.to_string().as_str())?
        .map(|r| postcard::from_bytes(r.to_vec().as_slice()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.remove(id.as_str())?;

    Ok(())
}
