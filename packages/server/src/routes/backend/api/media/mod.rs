mod insert;
mod list;
mod update;

use anyhow::{Context, Result};
use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};

use _database::types::config::load_config;

pub async fn route() -> Result<Router> {
    let config = load_config()?;
    let max_size = config.upload.image_size_limit;
    // Parse the suffix that may be "MiB", "KiB" or none (Bytes).
    let max_size = match &max_size[max_size.len() - 3..] {
        "MiB" => {
            max_size[..max_size.len() - 3]
                .parse::<u64>()
                .context("Failed to parse image size limit (MiB)")?
                * 1024
                * 1024
        }
        "KiB" => {
            max_size[..max_size.len() - 3]
                .parse::<u64>()
                .context("Failed to parse image size limit (KiB)")?
                * 1024
        }
        _ => max_size
            .parse::<u64>()
            .context("Failed to parse image size limit (Bytes)")?,
    };

    let router = Router::new()
        .route("/count", get(list::count))
        .route("/list", get(list::list))
        .route(
            "/insert",
            post(insert::insert).layer(DefaultBodyLimit::max(max_size as usize)),
        )
        .route("/delete/:id", post(update::delete));

    Ok(router)
}
