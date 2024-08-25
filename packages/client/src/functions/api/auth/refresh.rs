use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage as _};
use reqwest::Client;

use crate::utils::get_host;
use _types::response::UserInfo;

pub async fn refresh() -> Result<UserInfo> {
    match LocalStorage::get::<UserInfo>("auth") {
        Ok(info) => {
            let res = Client::new()
                .get(format!("{}/api/auth/refresh", get_host()?,))
                .bearer_auth(info.token)
                .send()
                .await?;

            if res.status().is_success() {
                let ret: UserInfo = res.json().await?;
                LocalStorage::set("auth", ret.clone()).unwrap();

                Ok(ret)
            } else {
                Err(anyhow!("{} - {}", res.status(), res.text().await?))
            }
        }
        _ => Err(anyhow!("No token found")),
    }
}
