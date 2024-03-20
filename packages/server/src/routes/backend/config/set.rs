use anyhow::Result;
use log::info;

use axum::{extract::Json, response::IntoResponse};
use hyper::StatusCode;

use _database::types::config::{update_config, Config as VO};

#[tracing::instrument]
pub async fn set(Json(vo): Json<VO>) -> Result<impl IntoResponse, (StatusCode, String)> {
    update_config(vo).map_err(|err| {
        tracing::error!("Failed to update config: {}", err);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to update config".to_string(),
        )
    })?;

    std::thread::spawn(|| {
        info!("Wait for 1 second to ensure the response is sent");
        std::thread::sleep(std::time::Duration::from_secs(1));

        info!("Exit the process to restart the server and apply the new config");
        std::process::exit(0);
    });

    Ok(())
}
