//! Share links and quota shapes (the netdisk wave).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A stored share link (token-addressed, public download lane).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareRecord {
    /// The public token (22 base62 characters).
    pub token: String,
    /// The shared file.
    pub file_id: Uuid,
    /// The sharing user.
    pub owner_id: Uuid,
    /// Expiry (RFC 3339 text); `None` = does not expire.
    pub expires_at: Option<String>,
    /// Whether the public page may inline-preview.
    pub allow_preview: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

/// The public projection of a share.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct ShareDto {
    pub token: String,
    pub file_id: Uuid,
    pub owner_id: Uuid,
    #[ts(optional)]
    pub expires_at: Option<String>,
    pub allow_preview: bool,
    #[ts(type = "string")]
    pub created_at: Timestamp,
    /// The shared file's display name (list convenience).
    pub file_name: String,
    /// The shared file's size in bytes.
    pub file_size: i64,
}

impl From<(ShareRecord, String, i64)> for ShareDto {
    fn from((record, file_name, file_size): (ShareRecord, String, i64)) -> Self {
        Self {
            token: record.token,
            file_id: record.file_id,
            owner_id: record.owner_id,
            expires_at: record.expires_at,
            allow_preview: record.allow_preview,
            created_at: record.created_at,
            file_name,
            file_size,
        }
    }
}

/// A user's storage quota view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../webui/src/types/")]
pub struct QuotaInfo {
    /// Live bytes used (folders count 0).
    #[ts(type = "number")]
    pub used_bytes: i64,
    /// The limit; `None` = unlimited.
    #[ts(optional)]
    pub quota_bytes: Option<i64>,
}

/// Create-share parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewShare {
    pub file_id: Uuid,
    pub owner_id: Uuid,
    pub expires_at: Option<String>,
    pub allow_preview: bool,
}
