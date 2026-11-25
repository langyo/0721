//! Process entry point for the 0721 resource service.

use std::net::SocketAddr;
use std::sync::Arc;

use i0721_proto::Role;
use i0721_store::{CreateUser, PgStore, Store};

use i0721_server::auth;
use i0721_server::config::Config;
use i0721_server::routes;
use i0721_server::state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::from_env()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&config.log_filter))
        .init();

    let store = PgStore::connect(&config.database_url).await?;

    // R-U: the backend decision happens BEFORE any pending migration
    // runs. A missing/invalid `I0721_STORAGE_BACKEND` while 0007 is
    // pending fails the start with the whole `migrate()` unexecuted
    // (0006 among others stays unapplied — no half-built database).
    let pending = store
        .pending_versions()
        .await?
        .iter()
        .any(|v| v == "20261122000007");
    let env_backend = config.storage_backend.clone();
    let bootstrap_backend = i0721_server::blob_selection::bootstrap_backend_for_startup(
        env_backend.as_deref(),
        pending,
    )
    .map_err(|err| -> Box<dyn std::error::Error> { err.into() })?;
    match (pending, bootstrap_backend) {
        // 0007 is about to run: carry the resolved facts into it.
        (true, Some(backend)) => {
            let params = bootstrap_params_for(&backend, &config);
            tracing::info!(backend, "the bucket migration is pending — bootstrapping its registry bucket");
            i0721_store::PgStore::set_bootstrap_params(params);
            config.storage_backend = None;
        }
        // 0007 applied: the variable is never read again.
        (false, _) => {
            if env_backend.is_some() {
                tracing::info!(
                    "I0721_STORAGE_BACKEND is set but the bucket migration is already applied — it is ignored"
                );
            }
            config.storage_backend = None;
        }
        // Unreachable: a pending 0007 without a backend already failed above.
        (true, None) => unreachable!("pending 0007 without a backend is refused above"),
    }

    let migration = store.migrate().await?;
    tracing::info!(
        applied = migration.applied.len(),
        skipped = migration.skipped.len(),
        pre_history = migration.pre_history.len(),
        "migrations up to date"
    );
    seed_default_admin(&store, &config.seed_admin_password).await?;

    // The per-bucket registry: DB rows are the authority; a read failure
    // or an EMPTY table fails the start (the registry never invents rows).
    let rows = store.list_storage_buckets().await?;
    let registry = i0721_server::blob_registry::BlobRegistry::build(
        rows,
        |key| std::env::var(key).ok(),
    )
    .map_err(|err| -> Box<dyn std::error::Error> { err.into() })?;
    let blobs: Arc<dyn i0721_server::blob::BlobStore> = registry
        .bootstrap_store()
        .map_err(|err| -> Box<dyn std::error::Error> { err.into() })?;

    let listen = config.listen;
    let state = AppState::with_registry(Arc::new(store), Arc::new(registry), Arc::new(config), blobs);

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

/// The bootstrap bucket's migration facts: the backend label the startup
/// decided plus its location (S3 bucket name / media root / kubo
/// host:port), read from the SAME env the legacy backend selection used.
fn bootstrap_params_for(
    backend: &str,
    config: &Config,
) -> i0721_store::BootstrapBucketParams {
    let location = match backend {
        "s3" => std::env::var("I0721_S3_BUCKET").unwrap_or_else(|_| "unknown".to_owned()),
        "ipfs" => std::env::var("I0721_IPFS_API").unwrap_or_else(|_| "unknown".to_owned()),
        // local: the config's own media root (its default included).
        _ => config.media_root.display().to_string(),
    };
    i0721_store::BootstrapBucketParams {
        id: uuid::Uuid::new_v4(),
        location,
        backend: backend.to_owned(),
    }
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
