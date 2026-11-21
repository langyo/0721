//! Shared application state handed to every handler.

use std::sync::Arc;

use i0721_store::Store;
use tokio::sync::Semaphore;

use crate::auth::{JwtSigner, LoginLimiter};
use crate::blob::BlobStore;
use crate::config::Config;

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    /// Metadata store (PostgreSQL in production, memory in tests).
    pub store: Arc<dyn Store>,
    /// Blob backend.
    pub blobs: Arc<dyn BlobStore>,
    /// Process configuration.
    pub config: Arc<Config>,
    /// Transition-era token signer.
    pub signer: Arc<JwtSigner>,
    /// Login rate limiter (per source IP).
    pub login_limiter: Arc<LoginLimiter>,
    /// Upload-pipeline concurrency slots (the legacy limit was 4).
    pub upload_slots: Arc<Semaphore>,
}

impl AppState {
    /// Assemble a state from its parts (tests use this with fakes).
    #[must_use]
    pub fn new(store: Arc<dyn Store>, blobs: Arc<dyn BlobStore>, config: Arc<Config>) -> Self {
        let signer = Arc::new(JwtSigner::new(&config.jwt_secret));
        let upload_slots = Arc::new(Semaphore::new(config.upload_concurrency));
        Self {
            store,
            blobs,
            config,
            signer,
            login_limiter: Arc::new(LoginLimiter::default()),
            upload_slots,
        }
    }
}
