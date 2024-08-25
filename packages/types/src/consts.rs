use once_cell::sync::Lazy;
use std::path::{Path, PathBuf};

pub static WASM_DIR: Lazy<PathBuf> = Lazy::new(|| {
    std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf())
        .unwrap_or(std::env::current_dir().unwrap().join("target/wasm32-html"))
});
pub static WEBSITE_RES_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf())
        .unwrap_or(std::env::current_dir().unwrap().join("res"));
    path.push("website");
    path
});

pub static CONFIG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
        .unwrap_or(std::env::current_dir().unwrap().join("cache"));
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
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
        .unwrap_or(std::env::current_dir().unwrap().join("cache"));
    path.push("media");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static MEDIA_CACHE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
        .unwrap_or(std::env::current_dir().unwrap().join("cache"));
    path.push("media-cache");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static DATABASE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
        .unwrap_or(std::env::current_dir().unwrap().join("cache"));
    path.push("database");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static LOG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf().join("cache"))
        .unwrap_or(std::env::current_dir().unwrap().join("cache"));
    path.push("log");

    // If directory does not exist, create it
    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
