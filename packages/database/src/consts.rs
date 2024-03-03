use once_cell::sync::{Lazy, OnceCell};
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

pub static MEDIA_RES_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf())
        .unwrap_or(std::env::current_dir().unwrap().join("res"));
    path.push("media");

    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static DATABASE_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf())
        .unwrap_or(std::env::current_dir().unwrap().join("res"));
    path.push("database");

    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});
pub static LOG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let mut path = std::env::var("ROOT_DIR")
        .ok()
        .map(|dir| Path::new(&dir).to_path_buf())
        .unwrap_or(std::env::current_dir().unwrap().join("res"));
    path.push("log");

    if !path.exists() {
        std::fs::create_dir_all(&path).unwrap();
    }
    path
});

pub static DB_CONN: OnceCell<redb::Database> = OnceCell::new();
