use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage};
use reqwest::Client;

use crate::utils::get_host;
use _database::types::{request::LoginInfo, response::UserInfo};

pub async fn login(name: String, password_raw: String) -> Result<UserInfo> {
    let res = Client::new()
        .post(format!("{}/api/auth/login", get_host()?,))
        .json(&LoginInfo { name, password_raw })
        .send()
        .await?;

    if res.status().is_success() {
        let ret: UserInfo = res.json().await?;
        LocalStorage::set("token", ret.token.clone()).unwrap();

        Ok(ret)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}
