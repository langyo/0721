pub mod app;
pub mod components;
pub mod functions;
pub mod pages;
pub mod utils;

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
mod web_entry;

#[cfg(all(target_arch = "wasm32", not(target_os = "wasi")))]
pub use web_entry::*;
