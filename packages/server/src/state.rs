//! Shared application state handed to every handler.

use std::sync::Arc;

use i0721_store::Store;
use tokio::sync::Semaphore;

use crate::auth::{JwtSigner, LoginLimiter};
use crate::blob::BlobStore;
use crate::blob_registry::BlobRegistry;
use crate::config::Config;
use crate::oauth::{DiscoveryCache, LoginAttemptStore};
use crate::ws_tickets::WsTicketStore;

/// Everything a handler needs.
#[derive(Clone)]
pub struct AppState {
    /// Metadata store (PostgreSQL in production, memory in tests).
    pub store: Arc<dyn Store>,
    /// The per-bucket blob registry (stage A: every legacy call site
    /// still sees `Arc<dyn BlobStore>` through the single-entry
    /// registry; per-row routing lands in stage B).
    pub blobs: Arc<dyn BlobStore>,
    /// The registry itself (per-bucket resolution + health).
    pub registry: Arc<BlobRegistry>,
    /// Process configuration.
    pub config: Arc<Config>,
    /// Transition-era token signer.
    pub signer: Arc<JwtSigner>,
    /// Login rate limiter (per source IP).
    pub login_limiter: Arc<LoginLimiter>,
    /// Upload-pipeline concurrency slots (the legacy limit was 4).
    pub upload_slots: Arc<Semaphore>,
    /// One-time WebSocket admission tickets (in-memory: single-instance
    /// deployments — see `ws_tickets`).
    pub ws_tickets: Arc<WsTicketStore>,
    /// The server-side HTTP client for OIDC discovery. The OAuth token
    /// exchange and UserInfo read do NOT ride it: they go through kirino's
    /// `HttpFetcher` seam (`plana-identity-relay`, built on first use).
    pub http_client: reqwest::Client,
    /// In-process OIDC discovery cache (positive + negative).
    pub discovery: Arc<DiscoveryCache>,
    /// In-flight OAuth sign-ins (kirino's `LoginAttemptStore`; one-time and
    /// TTL-bounded — see `oauth`).
    pub oauth_attempts: Arc<LoginAttemptStore>,
    /// MCP SSE sessions (in-memory, TTL-pruned).
    pub mcp_sessions: crate::mcp::McpSessions,
}

impl AppState {
    /// Assemble a state from its parts (tests use this with fakes).
    #[must_use]
    pub fn new(store: Arc<dyn Store>, blobs: Arc<dyn BlobStore>, config: Arc<Config>) -> Self {
        Self::with_registry(
            store,
            BlobRegistry::single_tagged(
                blobs.clone(),
                config
                    .storage_backend
                    .as_deref()
                    .and_then(i0721_proto::BucketBackend::parse)
                    .unwrap_or(i0721_proto::BucketBackend::Local),
            ),
            config,
            blobs,
        )
    }

    /// Assemble a state from an explicit registry (production path).
    #[must_use]
    pub fn with_registry(
        store: Arc<dyn Store>,
        registry: Arc<BlobRegistry>,
        config: Arc<Config>,
        blobs: Arc<dyn BlobStore>,
    ) -> Self {
        let signer = Arc::new(JwtSigner::new(&config.jwt_secret));
        let upload_slots = Arc::new(Semaphore::new(config.upload_concurrency));
        Self {
            store,
            blobs,
            registry,
            config,
            signer,
            login_limiter: Arc::new(LoginLimiter::default()),
            upload_slots,
            ws_tickets: Arc::new(WsTicketStore::new()),
            http_client: reqwest::Client::new(),
            discovery: Arc::new(DiscoveryCache::new()),
            oauth_attempts: Arc::new(LoginAttemptStore::new()),
            mcp_sessions: crate::mcp::McpSessions::new(),
        }
    }
}
