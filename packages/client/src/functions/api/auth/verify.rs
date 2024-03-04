use anyhow::{anyhow, Result};

use gloo::storage::{LocalStorage, Storage as _};
use reqwest::Client;

use crate::utils::get_host;

pub async fn verify() -> Result<()> {
    match LocalStorage::get::<String>("token") {
        Ok(token) => {
            let res = Client::new()
                .get(format!("{}/api/auth/verify", get_host()?))
                .bearer_auth(token)
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
