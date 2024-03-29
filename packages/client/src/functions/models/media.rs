use anyhow::{anyhow, Result};

use reqwest::{
    multipart::{Form, Part},
    Client,
};

use crate::utils::{get_auth_cache, get_host};
use _database::{models::media::Model, types::request::PageArgs};

pub async fn count() -> Result<usize> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .get(format!("{}/api/media/count", get_host()?))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.json().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn list(offset: Option<usize>, limit: Option<usize>) -> Result<Vec<Model>> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .get(format!("{}/api/media/list", get_host()?))
        .query(&PageArgs { offset, limit })
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(res.json().await?)
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}

pub async fn insert(data: Vec<u8>, file_name: impl ToString) -> Result<String> {
    let token = get_auth_cache()?;
    let res = Client::new()
        .put(format!(
            "{}/api/media/insert?name={}",
            get_host()?,
            percent_encoding::utf8_percent_encode(
                &file_name.to_string(),
                percent_encoding::NON_ALPHANUMERIC,
            )
        ))
        .multipart(Form::new().part("file", Part::bytes(data)))
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
        .delete(format!("{}/api/media/delete/{}", get_host()?, id))
        .bearer_auth(token.token)
        .send()
        .await?;

    if res.status().is_success() {
        Ok(())
    } else {
        Err(anyhow!("{} - {}", res.status(), res.text().await?))
    }
}
