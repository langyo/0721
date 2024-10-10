use std::collections::HashMap;

use anyhow::{anyhow, Result};

use reqwest::Client;

use crate::utils::{get_auth_cache, get_host};
use _types::{request::RegisterParams, response::UserBasicInfo};

pub async fn count() -> Result<usize> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .get(format!("{}/api/user/count", get_host()?))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.json().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn list() -> Result<HashMap<String, UserBasicInfo>> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .get(format!("{}/api/user/list", get_host()?))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.json().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn register(data: &RegisterParams) -> Result<String> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .post(format!("{}/api/user/register", get_host()?))
        .json(&data)
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.text().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn update(id: String, data: RegisterParams) -> Result<String> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .post(format!("{}/api/user/update/{}", get_host()?, id))
        .json(&data)
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.text().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn delete(id: String) -> Result<()> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .delete(format!("{}/api/user/delete/{}", get_host()?, id))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}
