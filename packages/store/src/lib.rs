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

use i0721_proto::{FileRecord, NewShare, Role, ShareRecord, UserRecord};
use uuid::Uuid;

/// Mint a share token: 22 base62 characters (~131 bits of entropy —
/// enough that enumeration is hopeless against a 30-char online space).
#[must_use]
pub fn new_share_token() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut raw = [0u8; 22];
    getrandom::fill(&mut raw).expect("the system RNG is available");
    raw.iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

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

    /// Fetch a live file by owner and content hash (the per-owner dedupe
    /// probe — one user's upload must never answer with another's row).
    async fn get_file_by_owner_and_sha256(
        &self,
        owner_id: Uuid,
        sha256: &str,
    ) -> Result<Option<FileRecord>, StoreError>;

    /// Fetch a live file by its public storage key (the `/media/:key`
    /// lookup). Several rows may share one storage key (per-owner dedupe
    /// keeps the blob content-addressed); any of them authorizes serving.
    async fn get_file_by_storage_key(&self, key: &str) -> Result<Option<FileRecord>, StoreError>;

    /// Count live rows sharing a storage key — the caller's blob-reclaim
    /// probe (a blob goes away only with its last live row; full GC is a
    /// netdisk-wave concern).
    async fn count_live_files_by_storage_key(&self, key: &str) -> Result<u64, StoreError>;

    /// Record a freshly uploaded file; `(owner_id, sha256)` must be
    /// unique among that owner's live files ([`StoreError::Conflict`]
    /// otherwise). The storage key may be shared with other owners' rows
    /// (same content, same blob).
    async fn create_file(&self, file: i0721_proto::NewFile) -> Result<FileRecord, StoreError>;

    /// Rename a live file via a single guarded UPDATE
    /// (`WHERE id = $1 AND deleted_at IS NULL`); `None` when the id is
    /// unknown or already soft-deleted (no read-then-write race).
    async fn rename_file(&self, id: Uuid, name: &str) -> Result<Option<FileRecord>, StoreError>;

    /// Soft-delete a live file via a single guarded UPDATE
    /// (`WHERE id = $1 AND deleted_at IS NULL`); returns whether a row
    /// was marked (a second call answers `false`).
    async fn soft_delete_file(&self, id: Uuid) -> Result<bool, StoreError>;

    /// Read one settings row.
    /// The local user bound to an OAuth identity, if any.
    async fn get_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<Uuid>, StoreError>;

    /// Bind an OAuth identity to a local user. Conflict when the
    /// (provider, subject) binding already exists.
    async fn link_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
        user_id: Uuid,
    ) -> Result<(), StoreError>;

    // ── netdisk: scoped listing, folders, moves, shares, quota ──────

    /// List one folder's live children (own-tree scope): the caller's
    /// files whose parent is `parent_id` (None = root).
    async fn list_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError>;

    /// Whether a name is already taken in one owner's folder layer
    /// (live rows; `exclude` lets a rename/move keep its own name).
    async fn folder_name_taken(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
        exclude: Option<Uuid>,
    ) -> Result<bool, StoreError>;

    /// Create a folder row (sha256 = `dir:` placeholder).
    async fn create_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
    ) -> Result<FileRecord, StoreError>;

    /// Move a row to a new parent (caller pre-validates name/cycles).
    async fn set_parent(&self, id: Uuid, parent_id: Option<Uuid>) -> Result<(), StoreError>;

    /// The chain of ancestors of `id` (nearest first, excluding self).
    async fn ancestor_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError>;

    /// All live row ids in `id`'s subtree, `id` included.
    async fn subtree_live_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError>;

    /// Soft-delete a batch of rows (the subtree cascade).
    async fn soft_delete_many(&self, ids: &[Uuid]) -> Result<u64, StoreError>;

    /// Live rows by ids (the cascade's blob-reclaim input).
    async fn get_files_by_ids(&self, ids: &[Uuid]) -> Result<Vec<FileRecord>, StoreError>;

    /// Create (or return the existing) share for a file.
    async fn create_share(&self, share: NewShare) -> Result<ShareRecord, StoreError>;

    /// The share a token addresses (expiry checked by the caller).
    async fn get_share(&self, token: &str) -> Result<Option<ShareRecord>, StoreError>;

    /// One owner's shares, newest first.
    async fn list_shares(&self, owner_id: Uuid) -> Result<Vec<ShareRecord>, StoreError>;

    /// Revoke a share by token.
    async fn delete_share(&self, token: &str) -> Result<bool, StoreError>;

    /// Sum of live file sizes for an owner (folders count 0).
    async fn sum_live_sizes(&self, owner_id: Uuid) -> Result<i64, StoreError>;

    /// Set a user's per-user quota (None = the global default).
    async fn set_user_quota(
        &self,
        user_id: Uuid,
        quota_bytes: Option<i64>,
    ) -> Result<(), StoreError>;

    async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, StoreError>;

    /// Write one settings row (upsert).
    async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError>;
}
