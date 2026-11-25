//! lacrymira entity models — one per table, mirroring the schema of
//! [`crate::migrate::migrations`].
//!
//! Field-type notes:
//! - `uuid::Uuid` maps to the `UUID` wire type; `jiff::Timestamp` to
//!   `TIMESTAMPTZ` (RFC 3339 text on the `Value` wire).
//! - lacrymira 0.1 has no `Option<jiff::Timestamp>` round-trip, so
//!   *nullable* time columns (`deleted_at`, `expires_at`, `last_used_at`,
//!   `revoked_at`) are `TEXT` carrying RFC 3339 strings. They are
//!   internal markers (soft delete / expiry), compared in Rust after
//!   parsing — no SQL time arithmetic rides on them.

use i0721_proto::{
    BucketBackend, BucketGrantRecord, FileKind, FileRecord, NewFile, Role, StorageBucketRecord,
    UserRecord,
};
use jiff::Timestamp;
use uuid::Uuid;

use crate::StoreError;

// ── users ──

/// The `users` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "users")]
pub struct UserModel {
    /// Primary key.
    #[lacrymira(primary_key)]
    pub id: Uuid,
    /// Login name, unique.
    pub username: String,
    /// Human-facing name.
    pub display_name: String,
    /// `admin` or `member` (CHECK constraint).
    pub role: String,
    /// Argon2id hash; NULL = no password login.
    pub password_hash: Option<String>,
    /// Per-user storage quota (bytes); NULL = the global default.
    pub quota_bytes: Option<i64>,
    /// Account creation time.
    pub created_at: Timestamp,
}

pub use users_entity::Column as UserColumn;

/// The `oauth_identities` table — built in this wave, populated in the
/// authentication wave (W4).
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "oauth_identities")]
pub struct OAuthIdentityModel {
    /// Provider id (e.g. `erp`, `github`).
    #[lacrymira(primary_key)]
    pub provider: String,
    /// Provider-side subject id.
    #[lacrymira(primary_key)]
    pub subject: String,
    /// The linked local user.
    pub user_id: Uuid,
}

pub use oauth_identities_entity::Column as OAuthIdentityColumn;

// ── files ──

/// The `files` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "files")]
pub struct FileModel {
    /// Primary key.
    #[lacrymira(primary_key)]
    pub id: Uuid,
    /// Owning user.
    pub owner_id: Uuid,
    /// Parent directory; NULL = root.
    pub parent_id: Option<Uuid>,
    /// Display name.
    pub name: String,
    /// `image`/`document`/`media`/`archive`/`other` (CHECK constraint).
    pub kind: String,
    /// MIME type of the blob.
    pub mime: String,
    /// Blob size in bytes.
    pub size: i64,
    /// Hex SHA-256 — dedupe key among live files.
    pub sha256: String,
    /// The bucket this row lives in; NULL = legacy rows awaiting the
    /// bootstrap backfill (the column stays nullable forever).
    pub bucket_id: Option<Uuid>,
    /// Storage backend tag; `local` today.
    pub storage_backend: String,
    /// Blob key (public media path segment).
    pub storage_key: String,
    /// Thumbnail blob key.
    pub thumbnail_key: Option<String>,
    /// The name the file had when uploaded.
    pub original_name: String,
    /// Upload time.
    pub created_at: Timestamp,
    /// Soft-delete marker (RFC 3339 text); NULL = live.
    pub deleted_at: Option<String>,
}

pub use files_entity::ActiveModel as FileActiveModel;
pub use files_entity::Column as FileColumn;

// ── shares (table only this wave; routes arrive with the netdisk wave) ──

/// The `shares` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "shares")]
pub struct ShareModel {
    /// Share token (URL-safe slug).
    #[lacrymira(primary_key)]
    pub token: String,
    /// The shared file.
    pub file_id: Uuid,
    /// Owning user.
    pub owner_id: Uuid,
    /// Expiry (RFC 3339 text); NULL = never expires.
    pub expires_at: Option<String>,
    /// Whether preview (vs. download-only) is allowed.
    pub allow_preview: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

pub use shares_entity::Column as ShareColumn;

// ── api_keys (table only this wave; logic arrives with the MCP wave) ──

/// The `api_keys` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "api_keys")]
pub struct ApiKeyModel {
    /// Primary key.
    #[lacrymira(primary_key)]
    pub id: Uuid,
    /// Owning user.
    pub user_id: Uuid,
    /// Human-facing label.
    pub name: String,
    /// Visible key prefix (masked listing).
    pub key_prefix: String,
    /// Hash of the full key (never the key itself).
    pub key_hash: String,
    /// Granted scopes (the `{"ops":[…]}` object; raw JSON, never
    /// interpreted here).
    pub scopes: serde_json::Value,
    /// Expiry time (RFC 3339 text); NULL = never expires.
    pub expires_at: Option<String>,
    /// Last use time (RFC 3339 text); NULL = never used.
    pub last_used_at: Option<String>,
    /// Revocation time (RFC 3339 text); NULL = active.
    pub revoked_at: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
}

pub use api_keys_entity::Column as ApiKeyColumn;

// ── settings ──

/// The `settings` table (KV, JSONB values).
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "settings")]
pub struct SettingModel {
    /// The key.
    #[lacrymira(primary_key)]
    pub key: String,
    /// The value.
    pub value: serde_json::Value,
}

pub use settings_entity::Column as SettingColumn;

// ── storage_buckets (the bucket registry; migration 0007) ──

/// The `storage_buckets` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "storage_buckets")]
pub struct StorageBucketModel {
    /// Primary key.
    #[lacrymira(primary_key)]
    pub id: Uuid,
    /// Stable identifier.
    pub slug: String,
    /// Human-facing name.
    pub display_name: String,
    /// `s3` / `local` / `ipfs` (CHECK constraint).
    pub backend: String,
    /// Human-readable placement (never a credential).
    pub location: String,
    /// The env prefix token carrying the credentials.
    pub endpoint_ref: String,
    /// Whether anonymous `/media` reads may serve this bucket.
    pub anonymous_read: bool,
    /// Whether new signups default to grants on this bucket.
    pub grant_on_signup: bool,
    /// Disabled buckets never serve.
    pub enabled: bool,
    /// Bootstrap buckets are born in migration 0007, immutable.
    pub is_bootstrap: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

pub use storage_buckets_entity::Column as StorageBucketColumn;

// ── bucket_grants (per user × bucket; P2a writes it) ──

/// The `bucket_grants` table.
#[derive(Debug, Clone, PartialEq, lacrymira::DeriveEntity)]
#[lacrymira(table = "bucket_grants")]
pub struct BucketGrantModel {
    /// The granted user (composite PK half).
    #[lacrymira(primary_key)]
    pub user_id: Uuid,
    /// The granted bucket (composite PK half).
    #[lacrymira(primary_key)]
    pub bucket_id: Uuid,
    /// Read bit.
    pub can_read: bool,
    /// Write bit.
    pub can_write: bool,
    /// Share bit.
    pub can_share: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

pub use bucket_grants_entity::Column as BucketGrantColumn;

// ── conversions ──

/// Decode failures (corrupt role/kind tags) surface as [`StoreError::Db`].
fn db_corrupt(what: &str, value: &str) -> StoreError {
    StoreError::Db(format!("corrupt {what} tag in database: {value:?}"))
}

/// Convert a [`UserModel`] into its record form.
///
/// # Errors
///
/// [`StoreError::Db`] when the role tag is not `admin`/`member`.
pub fn user_model_to_record(model: UserModel) -> Result<UserRecord, StoreError> {
    let role = Role::parse(&model.role).ok_or_else(|| db_corrupt("role", &model.role))?;
    Ok(UserRecord {
        id: model.id,
        username: model.username,
        display_name: model.display_name,
        role,
        password_hash: model.password_hash,
        quota_bytes: model.quota_bytes,
        created_at: model.created_at,
    })
}

/// Build a [`UserModel`] ready for insert.
#[must_use]
pub fn user_model_from_new(
    id: Uuid,
    username: &str,
    display_name: &str,
    role: Role,
    password_hash: Option<&str>,
    created_at: Timestamp,
) -> UserModel {
    UserModel {
        id,
        username: username.to_owned(),
        display_name: display_name.to_owned(),
        role: role.as_str().to_owned(),
        password_hash: password_hash.map(str::to_owned),
        quota_bytes: None,
        created_at,
    }
}

/// Convert a [`FileModel`] into its record form.
///
/// # Errors
///
/// [`StoreError::Db`] when the kind tag is unknown.
pub fn file_model_to_record(model: FileModel) -> Result<FileRecord, StoreError> {
    let kind = FileKind::parse(&model.kind).ok_or_else(|| db_corrupt("kind", &model.kind))?;
    Ok(FileRecord {
        id: model.id,
        owner_id: model.owner_id,
        parent_id: model.parent_id,
        name: model.name,
        kind,
        mime: model.mime,
        size: model.size,
        sha256: model.sha256,
        bucket_id: model.bucket_id,
        storage_backend: model.storage_backend,
        storage_key: model.storage_key,
        thumbnail_key: model.thumbnail_key,
        original_name: model.original_name,
        created_at: model.created_at,
        deleted_at: model.deleted_at,
    })
}

/// Build a [`FileModel`] ready for insert.
#[must_use]
pub fn file_model_from_new(id: Uuid, file: &NewFile, created_at: Timestamp) -> FileModel {
    FileModel {
        id,
        owner_id: file.owner_id,
        parent_id: file.parent_id,
        name: file.name.clone(),
        kind: file.kind.as_str().to_owned(),
        mime: file.mime.clone(),
        size: file.size,
        sha256: file.sha256.clone(),
        bucket_id: file.bucket_id,
        storage_backend: file.storage_backend.clone(),
        storage_key: file.storage_key.clone(),
        thumbnail_key: file.thumbnail_key.clone(),
        original_name: file.original_name.clone(),
        created_at,
        deleted_at: None,
    }
}

/// Convert a [`StorageBucketModel`] into its record form.
///
/// # Errors
///
/// [`StoreError::Db`] when the backend tag is not `s3`/`local`/`ipfs`.
pub fn storage_bucket_model_to_record(
    model: StorageBucketModel,
) -> Result<StorageBucketRecord, StoreError> {
    let backend = BucketBackend::parse(&model.backend)
        .ok_or_else(|| db_corrupt("bucket backend", &model.backend))?;
    Ok(StorageBucketRecord {
        id: model.id,
        slug: model.slug,
        display_name: model.display_name,
        backend,
        location: model.location,
        endpoint_ref: model.endpoint_ref,
        anonymous_read: model.anonymous_read,
        grant_on_signup: model.grant_on_signup,
        enabled: model.enabled,
        is_bootstrap: model.is_bootstrap,
        created_at: model.created_at,
    })
}

/// Convert a [`BucketGrantModel`] into its record form.
#[must_use]
pub fn bucket_grant_model_to_record(model: BucketGrantModel) -> BucketGrantRecord {
    BucketGrantRecord {
        user_id: model.user_id,
        bucket_id: model.bucket_id,
        can_read: model.can_read,
        can_write: model.can_write,
        can_share: model.can_share,
        created_at: model.created_at,
    }
}

/// Convert a [`ShareModel`] into its record form.
#[must_use]
pub fn share_model_to_record(model: ShareModel) -> i0721_proto::ShareRecord {
    i0721_proto::ShareRecord {
        token: model.token,
        file_id: model.file_id,
        owner_id: model.owner_id,
        expires_at: model.expires_at,
        allow_preview: model.allow_preview,
        created_at: model.created_at,
    }
}

/// Convert an [`ApiKeyModel`] into its record form.
#[must_use]
pub fn api_key_model_to_record(model: ApiKeyModel) -> i0721_proto::ApiKeyRecord {
    i0721_proto::ApiKeyRecord {
        id: model.id,
        user_id: model.user_id,
        name: model.name,
        key_prefix: model.key_prefix,
        key_hash: model.key_hash,
        // `to_string()` on a serde_json::Value IS the inverse of the
        // write path's `from_str`: every value (arrays, objects, and a
        // JSON string alike) serializes back to the exact text that
        // produced it. No special case — a String arm would strip the
        // quotes and break the round trip for quoted payloads.
        scopes: model.scopes.to_string(),
        expires_at: model.expires_at,
        last_used_at: model.last_used_at,
        revoked_at: model.revoked_at,
        created_at: model.created_at,
    }
}
