use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage};
use reqwest::Client;

use crate::utils::get_host;
use _database::types::response::UserInfo;

pub async fn refresh() -> Result<UserInfo> {
    match LocalStorage::get::<String>("token") {
        Ok(token) => {
            let res = Client::new()
                .get(format!("{}/api/auth/refresh", get_host()?,))
                .bearer_auth(token)
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
        _ => Err(anyhow!("No token found")),
    }
}
