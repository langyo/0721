//! API key shapes (the MCP wave).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A stored API key, as seen by the data layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    #[serde(skip_serializing)]
    pub scopes: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: Timestamp,
}

/// The create-key response — the plaintext key appears HERE and ONLY
/// here (copy-once semantics; the list never carries it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct ApiKeyCreated {
    pub id: Uuid,
    pub name: String,
    /// The full plaintext key (`i0721-<32 hex>`). Shown once at
    /// creation; never stored, never listed again.
    pub key: String,
}

/// The masked public projection (list responses).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct ApiKeyPublic {
    pub id: Uuid,
    pub name: String,
    /// The visible prefix (`i0721-abcd****`).
    pub key_prefix: String,
    #[ts(optional)]
    pub last_used_at: Option<String>,
    #[ts(optional)]
    pub revoked_at: Option<String>,
    #[ts(type = "string")]
    pub created_at: Timestamp,
}
