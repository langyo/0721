use once_cell::sync::Lazy;
use std::path::PathBuf;

pub static WASM_DIR: Lazy<PathBuf> = Lazy::new(|| {
    #[cfg(debug_assertions)]
    {
        std::env::current_dir().unwrap().join("target/wasm32-html")
    }
    #[cfg(not(debug_assertions))]
    {
        std::env::var("ROOT_DIR")
            .ok()
            .map(|dir| Path::new(&dir).to_path_buf())
            .expect("ROOT_DIR is not set")
    }
});
pub static WEBSITE_RES_DIR: Lazy<PathBuf> = Lazy::new(|| {
    #[cfg(debug_assertions)]
    {
        std::env::current_dir().unwrap().join("res")
    }
    #[cfg(not(debug_assertions))]
    {
        let mut path = std::env::var("ROOT_DIR")
            .ok()
            .map(|dir| Path::new(&dir).to_path_buf())
            .expect("ROOT_DIR is not set");
        path.push("website");
        path
    }
});

pub static CACHE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = {
        #[cfg(debug_assertions)]
        {
            std::env::current_dir().unwrap().join("target/cache")
        }
        #[cfg(not(debug_assertions))]
        {
            std::env::var("ROOT_DIR")
                .ok()
                .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
                .expect("ROOT_DIR is not set")
        }
    };

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }

    path.push("Config.toml");

    // If file does not exist, create it
    if !path.exists() {
        std::fs::write(&path, include_str!("../../../res/Config.default.toml")).unwrap();
    }

    path
});
pub static MEDIA_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = CACHE_DIR.clone();
    path.push("media");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static MEDIA_CACHE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = CACHE_DIR.clone();
    path.push("media-cache");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});

pub static LOG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = CACHE_DIR.clone();
    path.push("log");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
