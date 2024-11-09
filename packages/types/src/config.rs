use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use yuuka::derive_struct;

use crate::consts::CACHE_DIR;

derive_struct!(
    #[derive(PartialEq, Serialize, Deserialize)]
    #[macros_recursive(serde(rename_all = "kebab-case"))]
    pub Config {
        portal: {
            title_suffix: String,
            footer_banner: [{
                text: String,
                url: Option<String>,
            }],
            language: String,
            timezone: i32,
        },
        router: {
            media_entry_path: String,
            limit_referrer_host: Option<Vec<String>>,
        },
        upload: {
            image_size_limit: String,
            webp_auto_convert: bool,
            use_source_file_name: bool,
        }
    }
);

pub static CONFIG: Lazy<Arc<Mutex<Config>>> = Lazy::new(|| {
    let raw = std::fs::read_to_string(CACHE_DIR.clone()).unwrap();
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
