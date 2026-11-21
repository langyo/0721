//! Data access layer for the 0721 resource service.
//!
//! One trait ([`Store`]) covers every query the service performs; two
//! implementations ship in this crate:
//!
//! - [`PgStore`] — the real thing, [`lacrymira_pg::PgExecutor`] under a
//!   PostgreSQL database, with [`migrations()`] for
//!   [`lacrymira_pg::MigrationRunner`] (bookkeeping-compatible with the
//!   legacy `seaql_migrations` table).
//! - [`MemoryStore`] — an in-process fake so the HTTP surface and the
//!   pipeline logic stay testable without external services.
//!
//! Blob bytes never pass through here: files are addressed by
//! `storage_key` in the blob backend; this layer owns metadata only.

pub mod entity;
pub mod memory;
pub mod migrate;
pub mod pg;

pub use memory::MemoryStore;
pub use migrate::migrations;
pub use pg::PgStore;

use i0721_proto::{FileRecord, Role, UserRecord};
use uuid::Uuid;

/// Everything a user-creation call needs; the plaintext password never
/// reaches this layer — the caller hashes it first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateUser {
    /// Login name (validated upstream).
    pub username: String,
    /// Human-facing name.
    pub display_name: String,
    /// Admin or member.
    pub role: Role,
    /// Argon2id hash; `None` = password login disabled for this user.
    pub password_hash: Option<String>,
}

/// Why a store call failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Infrastructure failure (connection, decode, …).
    #[error("database error: {0}")]
    Db(String),
    /// A uniqueness or FK constraint rejected the write.
    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<lacrymira::Error> for StoreError {
    fn from(err: lacrymira::Error) -> Self {
        if let lacrymira::Error::Db {
            sqlstate: Some(code),
            ..
        } = &err
        {
            if code == "23505" {
                return StoreError::Conflict(err.to_string());
            }
        }
        StoreError::Db(err.to_string())
    }
}

/// The data access surface of the service.
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    /// List users by ascending creation, paged.
    async fn list_users(&self, offset: u64, limit: u64) -> Result<Vec<UserRecord>, StoreError>;

    /// Count all users.
    async fn count_users(&self) -> Result<u64, StoreError>;

    /// Fetch one user by id.
    async fn get_user(&self, id: Uuid) -> Result<Option<UserRecord>, StoreError>;

    /// Fetch one user by login name.
    async fn get_user_by_username(&self, username: &str) -> Result<Option<UserRecord>, StoreError>;

    /// Create a user; the username must be free
    /// ([`StoreError::Conflict`] otherwise).
    async fn create_user(&self, user: CreateUser) -> Result<UserRecord, StoreError>;

    /// Hard-delete a user (FK cascades remove their files/keys);
    /// returns whether a row was removed.
    async fn delete_user(&self, id: Uuid) -> Result<bool, StoreError>;

    /// List live files by descending upload time (the timeline order),
    /// paged.
    async fn list_files(&self, offset: u64, limit: u64) -> Result<Vec<FileRecord>, StoreError>;

    /// Count live files.
    async fn count_files(&self) -> Result<u64, StoreError>;

    /// Fetch one live file by id.
    async fn get_file(&self, id: Uuid) -> Result<Option<FileRecord>, StoreError>;

    /// Fetch a live file by content hash (the dedupe probe).
    async fn get_file_by_sha256(&self, sha256: &str) -> Result<Option<FileRecord>, StoreError>;

    /// Fetch a live file by its public storage key (the `/media/:key`
    /// lookup).
    async fn get_file_by_storage_key(&self, key: &str) -> Result<Option<FileRecord>, StoreError>;

    /// Record a freshly uploaded file; the sha256 must be unique among
    /// live files ([`StoreError::Conflict`] otherwise).
    async fn create_file(&self, file: i0721_proto::NewFile) -> Result<FileRecord, StoreError>;

    /// Rename a live file; `None` when the id is unknown or soft-deleted.
    async fn rename_file(&self, id: Uuid, name: &str) -> Result<Option<FileRecord>, StoreError>;

    /// Soft-delete a live file; returns whether a row was marked.
    async fn soft_delete_file(&self, id: Uuid) -> Result<bool, StoreError>;

    /// Read one settings row.
    async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, StoreError>;

    /// Write one settings row (upsert).
    async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError>;
}
