use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    pub title_suffix: String,
}

pub static CONFIG: Lazy<Arc<Mutex<Config>>> = Lazy::new(|| {
    let raw = std::fs::read_to_string("Config.toml").unwrap();
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
