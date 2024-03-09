use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage as _};

use _database::types::response::UserInfo;

pub fn get_auth_cache() -> Result<UserInfo> {
    LocalStorage::get::<UserInfo>("auth").map_err(|_| anyhow!("No token found"))
}
