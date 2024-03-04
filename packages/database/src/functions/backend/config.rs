use anyhow::Result;
use once_cell::sync::Lazy;
use std::collections::HashMap;

#[cfg(not(target_arch = "wasm32"))]
pub const DB: Lazy<sled::Db> = Lazy::new(|| {
    let db = sled::open({
        let mut path = (*crate::DATABASE_DIR).clone();
        path.push("config.db");
        path
    })
    .unwrap();
    db
});

pub async fn list() -> Result<HashMap<String, String>> {
    let ret = DB
        .iter()
        .map(|r| r.unwrap())
        .map(|r| (r.0.to_vec(), r.1.to_vec()))
        .map(|(k, v)| (String::from_utf8(k).unwrap(), String::from_utf8(v).unwrap()))
        .collect::<HashMap<String, String>>();

    Ok(ret)
}

pub async fn set(key: impl ToString, value: impl ToString) -> Result<()> {
    DB.insert(key.to_string().as_bytes(), value.to_string().as_bytes())?;

    Ok(())
}

pub async fn get(key: impl ToString) -> Result<Option<String>> {
    let ret = DB
        .get(key.to_string().as_bytes())?
        .map(|r| String::from_utf8(r.to_vec()).unwrap());

    Ok(ret)
}

pub async fn delete(id: String) -> Result<()> {
    DB.remove(id.as_bytes())?;

    Ok(())
}
