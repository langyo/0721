//! Shared application state handed to every handler.

use std::sync::Arc;

use i0721_store::Store;
use tokio::sync::Semaphore;

use crate::auth::{JwtSigner, LoginLimiter};
use crate::blob::BlobStore;
use crate::config::Config;
use crate::oauth::DiscoveryCache;
use crate::ws_tickets::WsTicketStore;

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
    /// One-time WebSocket admission tickets (in-memory: single-instance
    /// deployments — see `ws_tickets`).
    pub ws_tickets: Arc<WsTicketStore>,
    /// The server-side OAuth HTTP client (token exchange + discovery).
    pub http_client: reqwest::Client,
    /// In-process OIDC discovery cache (positive + negative).
    pub discovery: Arc<DiscoveryCache>,
    /// MCP SSE sessions (in-memory, TTL-pruned).
    pub mcp_sessions: crate::mcp::McpSessions,
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
            ws_tickets: Arc::new(WsTicketStore::new()),
            http_client: reqwest::Client::new(),
            discovery: Arc::new(DiscoveryCache::new()),
            mcp_sessions: crate::mcp::McpSessions::new(),
        }
    }
}
