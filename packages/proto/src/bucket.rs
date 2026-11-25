//! The bucket registry records (the B-P1 slice's new tables).
//!
//! `storage_buckets` is the registry of object stores the service can
//! route bytes to; `bucket_grants` (per user × bucket) is created here
//! with its record type but has no write path yet — the grants RPC
//! arrives with the P2a slice.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The backend tag of a registered bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../webui/src/types/")]
pub enum BucketBackend {
    /// Local filesystem root.
    Local,
    /// S3-compatible bucket.
    S3,
    /// IPFS kubo node (MFS).
    Ipfs,
}

impl BucketBackend {
    /// The database tag of this backend.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            BucketBackend::Local => "local",
            BucketBackend::S3 => "s3",
            BucketBackend::Ipfs => "ipfs",
        }
    }

    /// Parse a backend from its database tag.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(BucketBackend::Local),
            "s3" => Some(BucketBackend::S3),
            "ipfs" => Some(BucketBackend::Ipfs),
            _ => None,
        }
    }
}

/// A row of `storage_buckets` (the registry the migration 0007 seeded;
/// the RPC to manage it arrives in P2a — this slice only reads it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageBucketRecord {
    /// Primary key.
    pub id: Uuid,
    /// Stable identifier the RPC / scopes / env names build on.
    pub slug: String,
    /// Human-facing name.
    pub display_name: String,
    /// The backend tag (CHECK-constrained in SQL; parsed fail-closed
    /// into [`BucketBackend`] here).
    pub backend: BucketBackend,
    /// Human-readable placement (S3 bucket name / local root / kubo
    /// host:port). Never a credential.
    pub location: String,
    /// Which env variable prefix carries this bucket's credentials (a
    /// token, never a URL or a value).
    pub endpoint_ref: String,
    /// Whether anonymous `/media` reads may serve this bucket's bytes
    /// (the P2b slice enforces it; recorded from day one).
    pub anonymous_read: bool,
    /// Whether new signups get default grants on this bucket (P2a
    /// consumes it; recorded from day one).
    pub grant_on_signup: bool,
    /// Disabled buckets stay reachable in the registry but report
    /// `Disabled` in health and never serve.
    pub enabled: bool,
    /// Bootstrap buckets are born in migration 0007 and are immutable.
    pub is_bootstrap: bool,
    /// Creation time.
    pub created_at: jiff::Timestamp,
}

/// A row of `bucket_grants` (per user × bucket; P2a writes it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BucketGrantRecord {
    /// The granted user.
    pub user_id: Uuid,
    /// The granted bucket.
    pub bucket_id: Uuid,
    /// Read permission bit.
    pub can_read: bool,
    /// Write permission bit.
    pub can_write: bool,
    /// Share permission bit.
    pub can_share: bool,
    /// Creation time.
    pub created_at: jiff::Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_round_trips_tags() {
        for backend in [BucketBackend::Local, BucketBackend::S3, BucketBackend::Ipfs] {
            assert_eq!(BucketBackend::parse(backend.as_str()), Some(backend));
        }
        assert_eq!(BucketBackend::parse("gcs"), None);
    }
}
