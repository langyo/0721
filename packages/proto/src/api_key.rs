//! API key shapes (the MCP wave).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One permission an API key can carry.
///
/// The vocabulary is shared with the next wave (which only APPENDS a
/// `buckets` field inside `scopes`; it never changes `ops`). The wire
/// form is snake_case, matching the MCP tool names it gates.
///
/// The variants are listed in the CANONICAL order the list response
/// reports them in (`read`, `upload`, `share`); [`KeyOp::ALL`] is that
/// order and is the single place the order is written down.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../webui/src/types/")]
pub enum KeyOp {
    /// Read the caller's own tree: list, inspect, search.
    Read,
    /// Upload content through the shared upload chain.
    Upload,
    /// Mint a public share link for an existing file.
    Share,
}

impl KeyOp {
    /// Every operation, in the canonical response order.
    pub const ALL: [KeyOp; 3] = [KeyOp::Read, KeyOp::Upload, KeyOp::Share];

    /// The snake_case wire tag (same text serde emits).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            KeyOp::Read => "read",
            KeyOp::Upload => "upload",
            KeyOp::Share => "share",
        }
    }

    /// Parse the wire tag; `None` for anything outside the vocabulary.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        KeyOp::ALL.into_iter().find(|op| op.as_str() == raw)
    }
}

/// A stored API key, as seen by the data layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    /// The RAW `scopes` JSON text exactly as stored — the data layer
    /// never interprets it. Parsing lives in exactly one place
    /// (`i0721_server::key_scopes::parse_scopes`, fail-closed).
    #[serde(skip_serializing)]
    pub scopes: String,
    /// Optional expiry as RFC 3339 text (same shape as `revoked_at`);
    /// NULL = the key never expires.
    pub expires_at: Option<String>,
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
    /// The key's permissions in the canonical order (`read`, `upload`,
    /// `share`) — never the raw stored JSON.
    pub ops: Vec<KeyOp>,
    /// RFC 3339 expiry instant, or `None` for a key that never expires.
    #[ts(optional)]
    pub expires_at: Option<String>,
    /// Whether the key is past its `expires_at` right now (the webui
    /// renders the badge from this instead of parsing timestamps).
    pub expired: bool,
    #[ts(optional)]
    pub last_used_at: Option<String>,
    #[ts(optional)]
    pub revoked_at: Option<String>,
    #[ts(type = "string")]
    pub created_at: Timestamp,
}
