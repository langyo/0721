mod routes;
mod utils;

use anyhow::Result;
use log::info;
use std::net::SocketAddr;

use axum::serve;
use tokio::net::TcpListener;

use crate::routes::route;
use _database::{init, InitRouteEnvParams, RouteEnv};

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(not(debug_assertions))]
    {
        use _types::consts::LOG_DIR;
        use tracing_subscriber::fmt::writer::MakeWriterExt;

        let file_appender = tracing_appender::rolling::daily((*LOG_DIR).clone(), "log");
        let std_out = std::io::stdout.with_max_level(tracing::Level::INFO);
        tracing_subscriber::fmt()
            .with_writer(std_out.and(file_appender))
            .init();
    }

    #[cfg(debug_assertions)]
    {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_writer(std::io::stdout)
            .init();
    }

    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80);
    let env = RouteEnv::new(InitRouteEnvParams::Native).await?;
    init(env.clone()).await?;

    let router = route(env.clone())
        .await?
        .into_make_service_with_connect_info::<SocketAddr>();

    info!("Site will run on port {port}");

    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind");
    serve(listener, router).await?;

    Ok(())
}
