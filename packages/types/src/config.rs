use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use std::sync::{Arc, Mutex};

use yuuka::derive_config;

use crate::consts::CONFIG_DIR;

derive_config!(Config {
    portal: Portal {
        title_suffix: String,
        footer_banner: [FooterBannerItem {
            text: String,
            url: Option<String>,
        }],
        language: String,
        timezone: i32,
    },
    router: Router {
        media_entry_path: String,
        limit_referrer_host: Option<Vec<String>>,
    },
    upload: Upload {
        image_size_limit: String,
        webp_auto_convert: bool,
        use_source_file_name: bool,
    }
});
pub use __Config::*;

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
