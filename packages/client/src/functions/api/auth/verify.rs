use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage as _};
use reqwest::Client;

use crate::utils::get_host;
use _database::types::response::UserInfo;

pub async fn verify() -> Result<()> {
    match LocalStorage::get::<UserInfo>("auth") {
        Ok(info) => {
            let res = Client::new()
                .get(format!("{}/api/auth/verify", get_host()?))
                .bearer_auth(info.token)
                .send()
                .await?;

            if res.status().is_success() {
                Ok(())
            } else {
                Err(anyhow!("{} - {}", res.status(), res.text().await?))
            }
        }
        _ => Err(anyhow!("No token found")),
    }
}
