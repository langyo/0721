use anyhow::{anyhow, Result};

use reqwest::{
    multipart::{Form, Part},
    Client,
};

use crate::utils::get_host;
use _database::types::response::UserInfo;

pub async fn insert(token: UserInfo, data: Vec<u8>) -> Result<String> {
    let res = Client::new()
        .post(format!("{}/api/media/insert", get_host()?))
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
