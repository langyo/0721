use anyhow::{anyhow, Result};

use reqwest::Client;

use crate::utils::{get_auth_cache, get_host};
use _database::types::config::Config as Model;

pub async fn get() -> Result<Model> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .get(format!("{}/api/config", get_host()?))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.json().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn set(data: Model) -> Result<()> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .put(format!("{}/api/config", get_host()?))
        .json(&data)
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}
