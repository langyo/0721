use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::CONFIG_DIR;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub portal: config_item::Portal,
    pub router: config_item::Router,
    pub upload: config_item::Upload,
}

pub mod config_item {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "kebab-case")]
    pub struct Portal {
        pub title_suffix: String,
        pub footer_banner: Vec<FooterBannerItem>,
        pub language: String,
        pub timezone: i32,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "kebab-case")]
    pub struct FooterBannerItem {
        pub text: String,
        pub url: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "kebab-case")]
    pub struct Router {
        pub media_entry_path: String,
        pub limit_referrer_host: Option<Vec<String>>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    #[serde(rename_all = "kebab-case")]
    pub struct Upload {
        pub image_size_limit: String,
        pub webp_auto_convert: bool,
        pub use_source_file_name: bool,
    }
}

pub static CONFIG: Lazy<Arc<Mutex<Config>>> = Lazy::new(|| {
    let raw = std::fs::read_to_string(CONFIG_DIR.clone()).unwrap();
    let ret: Config = toml::from_str(raw.as_str()).unwrap();
    Arc::new(Mutex::new(ret))
});

pub fn load_config() -> Result<Config> {
    Ok(CONFIG.lock().map_err(|err| anyhow!("{}", err))?.clone())
}

pub fn update_config(config: Config) -> Result<()> {
    *CONFIG.lock().map_err(|err| anyhow!("{}", err))? = config.clone();

    let raw = toml::to_string(&config)?;
    std::fs::write("Config.toml", raw)?;
    Ok(())
}
