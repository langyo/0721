use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub header: Header,
    pub portal: Portal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Header {
    pub welcome: String,
    pub loading: String,

    pub login: String,
    pub logout: String,
    pub register: String,

    pub username: String,
    pub password: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Portal {
    pub upload: String,
    pub download: String,
    pub delete: String,
}

pub fn load_config() -> Result<Config> {
    // TODO - Multiple language support
    let raw = include_str!("../../../../res/languages/zh_hans.toml");
    let ret: Config = toml::from_str(raw)?;
    Ok(ret)
}

pub fn load_config_filtered_string() -> Result<String> {
    let raw = include_str!("../../../../res/languages/zh_hans.toml");
    Ok(raw
        .chars()
        .collect::<HashSet<char>>()
        .into_iter()
        .collect::<String>())
}
