use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::CONFIG_DIR;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub portal: Portal,
    pub router: Router,
    pub upload: Upload,
    pub user: User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Portal {
    pub title_suffix: String,
    pub footer_banner: Vec<FooterBannerItem>,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FooterBannerItem {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Router {
    pub media_entry_path: String,
    pub backend_entry_path: String,
    pub limit_referrer_host: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Upload {
    pub image_size_limit: String,
    pub webp_auto_convert: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct User {
    pub allow_register: bool,
}

pub static CONFIG: Lazy<Arc<Mutex<Config>>> = Lazy::new(|| {
    let raw = std::fs::read_to_string(CONFIG_DIR.clone()).unwrap();
    let ret: Config = toml::from_str(raw.as_str()).unwrap();
    Arc::new(Mutex::new(ret))
});

pub fn load_config() -> Result<Config> {
    Ok(CONFIG
        .lock()
        .map_err(|err| anyhow::anyhow!("Failed to lock config: {}", err))?
        .clone())
}

pub fn update_config(config: Config) -> Result<()> {
    *CONFIG
        .lock()
        .map_err(|err| anyhow::anyhow!("Failed to lock config: {}", err))? = config.clone();

    let raw = toml::to_string(&config)?;
    std::fs::write("Config.toml", raw)?;
    Ok(())
}
