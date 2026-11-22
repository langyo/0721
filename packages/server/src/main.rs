//! Process entry point for the 0721 resource service.

use std::net::SocketAddr;
use std::sync::Arc;

use i0721_proto::Role;
use i0721_store::{CreateUser, PgStore, Store};

use i0721_server::auth;
use i0721_server::blob::LocalBlobStore;
use i0721_server::config::Config;
use i0721_server::routes;
use i0721_server::state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&config.log_filter))
        .init();

    let store = PgStore::connect(&config.database_url).await?;
    let migration = store.migrate().await?;
    tracing::info!(
        applied = migration.applied.len(),
        skipped = migration.skipped.len(),
        pre_history = migration.pre_history.len(),
        "migrations up to date"
    );
    seed_default_admin(&store, &config.seed_admin_password).await?;

    let blobs: std::sync::Arc<dyn i0721_server::blob::BlobStore> =
        match config.storage_backend.as_str() {
            "local" => std::sync::Arc::new(LocalBlobStore::open(&config.media_root)?),
            "s3" => {
                let s3_config =
                    i0721_server::blob_s3::S3Config::from_env(|key| std::env::var(key).ok())
                        .map_err(|err| {
                            eprintln!("storage backend 's3' selected but misconfigured: {err}");
                            std::process::exit(1);
                        })?;
                std::sync::Arc::new(i0721_server::blob_s3::S3BlobStore::open(&s3_config)?)
            }
            "ipfs" => {
                let ipfs_config =
                    i0721_server::blob_ipfs::IpfsConfig::from_env(|key| std::env::var(key).ok())
                        .map_err(|err| {
                            eprintln!("storage backend 'ipfs' selected but misconfigured: {err}");
                            std::process::exit(1);
                        })?;
                std::sync::Arc::new(i0721_server::blob_ipfs::IpfsBlobStore::open(&ipfs_config))
            }
            other => {
                eprintln!(
                    "I0721_STORAGE_BACKEND={other:?} is not a valid backend (local | s3 | ipfs)"
                );
                std::process::exit(1);
            }
        };
    let listen = config.listen;
    let state = AppState::new(Arc::new(store), blobs, Arc::new(config));

    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(
        listen = %listen,
        version = env!("CARGO_PKG_VERSION"),
        "0721 resource service starting"
    );
    axum::serve(
        listener,
        routes::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// The legacy deployment shipped with an `admin`/`admin` account; keep
/// that bootstrap story: when the user table is empty, seed it with
/// [`Config::seed_admin_password`] (default `admin`, overridable via
/// `I0721_SEED_ADMIN_PASSWORD`; see the README warning). Forced
/// rotation on first login arrives with the authentication wave.
async fn seed_default_admin(
    store: &PgStore,
    seed_password: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if store.count_users().await? > 0 {
        return Ok(());
    }
    let password_hash = auth::hash_password(seed_password).map_err(|err| err.to_string())?;
    let admin = store
        .create_user(CreateUser {
            username: "admin".to_owned(),
            display_name: "Administrator".to_owned(),
            role: Role::Admin,
            password_hash: Some(password_hash),
        })
        .await?;
    tracing::warn!(
        username = %admin.username,
        "empty user table — seeded the default admin account; change its password after the first login"
    );
    Ok(())
}
