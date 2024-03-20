use anyhow::Result;
use once_cell::sync::Lazy;

use crate::models::user::*;

#[cfg(not(target_arch = "wasm32"))]
pub static DB: Lazy<sled::Db> = Lazy::new(|| {
    sled::open({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("user.redb");
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
        .skip(offset)
        .take(limit)
        .map(|r| r.unwrap())
        .map(|r| postcard::from_bytes(r.1.to_vec().as_slice()).unwrap())
        .collect::<Vec<_>>();

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
