//! An in-memory [`Store`] — the fake that keeps `cargo test` free of
//! external services.
//!
//! Mirrors the PostgreSQL semantics that matter to the routes: live-only
//! file queries (soft-deleted rows are invisible), timeline ordering
//! (descending upload time), username uniqueness, and hash/storage-key
//! uniqueness among live files.

use std::collections::HashMap;
use std::sync::Mutex;

use i0721_proto::{FileRecord, NewFile, UserRecord};
use jiff::Timestamp;
use uuid::Uuid;

use crate::{CreateUser, Store, StoreError};
use i0721_proto::ShareRecord;

#[derive(Debug, Default, Clone)]
struct Inner {
    users: Vec<UserRecord>,
    files: Vec<FileRecord>,
    settings: HashMap<String, serde_json::Value>,
    /// Raw settings text (test fixtures only): a `Some` entry here makes
    /// `settings_state` read THIS text instead of the parsed map, so a
    /// test can plant a corrupt payload the typed `set_setting` cannot.
    settings_raw: HashMap<String, String>,
    oauth_identities: Vec<(String, String, Uuid)>,
    shares: Vec<i0721_proto::ShareRecord>,
    api_keys: Vec<i0721_proto::ApiKeyRecord>,
    buckets: Vec<i0721_proto::StorageBucketRecord>,
    grants: Vec<i0721_proto::BucketGrantRecord>,
}

/// The in-memory [`Store`].
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

// A snapshot Clone for test fixtures: a cloned handle starts from the
// SAME rows the original held at clone time (a wrapper mock wrapping a
// cloned handle sees the rows the fixture seeded through the original;
// later writes diverge, which is what a fixture wants).
impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            inner: Mutex::new(
                self.inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone(),
            ),
        }
    }
}

impl MemoryStore {
    /// A fresh empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a record directly (fixture setup for tests).
    pub fn seed_user(&self, user: UserRecord) {
        self.inner.lock().unwrap().users.push(user);
    }

    /// Insert a bucket row directly (fixture setup for tests — real
    /// buckets are born in migration 0007 only).
    pub fn seed_bucket(&self, bucket: i0721_proto::StorageBucketRecord) {
        self.inner.lock().unwrap().buckets.push(bucket);
    }

    /// Plant a RAW settings text that `settings_state` serves verbatim —
    /// the corrupt-payload fixture the typed `set_setting` cannot build.
    pub fn seed_raw_setting(&self, key: &str, raw: &str) {
        self.inner
            .lock()
            .unwrap()
            .settings_raw
            .insert(key.to_owned(), raw.to_owned());
    }
}

fn lock_err(poisoned: impl std::fmt::Display) -> StoreError {
    StoreError::Db(format!("store mutex poisoned: {poisoned}"))
}

fn conflict(reason: impl Into<String>) -> StoreError {
    StoreError::Conflict(reason.into())
}

/// Parse a scopes text's `buckets` array the SAME way the server's
/// fail-closed interpreter does: a well-formed string array of ≤ 64
/// entries answers the set; anything else answers `None` (the memory
/// twin of `parse_scopes_buckets` — the store cannot depend on the
/// server crate, so the shape is mirrored here and pinned equal by the
/// round-trip test).
fn scopes_buckets(raw: &str) -> Option<std::collections::BTreeSet<String>> {
    const MAX_BUCKETS: usize = 64;
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let buckets = value.get("buckets")?.as_array()?;
    if buckets.len() > MAX_BUCKETS {
        return None;
    }
    let mut out = std::collections::BTreeSet::new();
    for entry in buckets {
        let slug = entry.as_str()?;
        if slug.is_empty() {
            return None;
        }
        out.insert(slug.to_owned());
    }
    Some(out)
}

#[async_trait::async_trait]
impl Store for MemoryStore {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn list_users(&self, offset: u64, limit: u64) -> Result<Vec<UserRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut users = inner.users.clone();
        users.sort_by_key(|user| user.created_at);
        Ok(users
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_users(&self) -> Result<u64, StoreError> {
        Ok(self.inner.lock().map_err(lock_err)?.users.len() as u64)
    }

    async fn get_user(&self, id: Uuid) -> Result<Option<UserRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .users
            .iter()
            .find(|user| user.id == id)
            .cloned())
    }

    async fn get_user_by_username(&self, username: &str) -> Result<Option<UserRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .users
            .iter()
            .find(|user| user.username == username)
            .cloned())
    }

    async fn create_user(&self, user: CreateUser) -> Result<UserRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if inner.users.iter().any(|u| u.username == user.username) {
            return Err(conflict(format!(
                "username {:?} is already taken",
                user.username
            )));
        }
        let record = UserRecord {
            id: Uuid::new_v4(),
            username: user.username,
            display_name: user.display_name,
            role: user.role,
            password_hash: user.password_hash,
            quota_bytes: None,
            created_at: Timestamp::now(),
        };
        inner.users.push(record.clone());
        Ok(record)
    }

    async fn delete_user(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let before = inner.users.len();
        inner.users.retain(|user| user.id != id);
        inner.files.retain(|file| file.owner_id != id);
        Ok(inner.users.len() != before)
    }

    async fn list_files(
        &self,
        buckets: Option<&[Uuid]>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut files: Vec<FileRecord> = inner
            .files
            .iter()
            .filter(|file| {
                file.deleted_at.is_none()
                    && buckets.is_none_or(|set| file.bucket_id.is_none_or(|b| set.contains(&b)))
            })
            .cloned()
            .collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.created_at));
        Ok(files
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_live_files_by_storage_key(
        &self,
        bucket_id: Option<Uuid>,
        key: &str,
    ) -> Result<u64, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .filter(|file| {
                file.deleted_at.is_none() && file.bucket_id == bucket_id && file.storage_key == key
            })
            .count() as u64)
    }

    async fn count_files(&self, buckets: Option<&[Uuid]>) -> Result<u64, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .filter(|file| {
                file.deleted_at.is_none()
                    && buckets.is_none_or(|set| file.bucket_id.is_none_or(|b| set.contains(&b)))
            })
            .count() as u64)
    }

    async fn get_file(&self, id: Uuid) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .find(|file| file.id == id && file.deleted_at.is_none())
            .cloned())
    }

    async fn get_file_by_owner_and_sha256(
        &self,
        owner_id: Uuid,
        sha256: &str,
    ) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .find(|file| {
                file.deleted_at.is_none() && file.owner_id == owner_id && file.sha256 == sha256
            })
            .cloned())
    }

    async fn list_live_rows_by_storage_key(
        &self,
        key: &str,
    ) -> Result<Vec<FileRecord>, StoreError> {
        // `id ASC` — the same deterministic order the PG lane returns.
        let mut rows: Vec<FileRecord> = self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .filter(|file| file.deleted_at.is_none() && file.storage_key == key)
            .cloned()
            .collect();
        rows.sort_by_key(|file| file.id);
        Ok(rows)
    }

    async fn list_storage_buckets(
        &self,
    ) -> Result<Vec<i0721_proto::StorageBucketRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut rows = inner.buckets.clone();
        rows.sort_by_key(|b| (b.created_at, b.slug.clone()));
        Ok(rows)
    }

    async fn get_storage_bucket_by_slug(
        &self,
        slug: &str,
    ) -> Result<Option<i0721_proto::StorageBucketRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .buckets
            .iter()
            .find(|b| b.slug == slug)
            .cloned())
    }

    async fn upsert_storage_bucket(
        &self,
        slug: &str,
        display_name: &str,
        anonymous_read: bool,
        grant_on_signup: bool,
        enabled: bool,
    ) -> Result<i0721_proto::StorageBucketRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let row = inner
            .buckets
            .iter_mut()
            .find(|b| b.slug == slug)
            .ok_or_else(|| conflict(format!("no such bucket: {slug}")))?;
        row.display_name = display_name.to_owned();
        row.anonymous_read = anonymous_read;
        row.grant_on_signup = grant_on_signup;
        row.enabled = enabled;
        Ok(row.clone())
    }

    async fn delete_storage_bucket(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let before = inner.buckets.len();
        inner.buckets.retain(|b| b.id != id);
        // Grant rows cascade with the bucket (the FK's ON DELETE CASCADE).
        inner.grants.retain(|g| g.bucket_id != id);
        Ok(inner.buckets.len() != before)
    }

    async fn count_live_files_by_bucket(&self, bucket_id: Option<Uuid>) -> Result<u64, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .files
            .iter()
            .filter(|f| f.deleted_at.is_none() && f.bucket_id == bucket_id)
            .count() as u64)
    }

    async fn storage_keys_live_in_bucket(
        &self,
        bucket_id: Option<Uuid>,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .files
            .iter()
            .filter(|f| f.deleted_at.is_none() && f.bucket_id == bucket_id)
            .map(|f| f.storage_key.clone())
            .collect())
    }

    async fn get_file_by_owner_bucket_and_sha256(
        &self,
        owner_id: Uuid,
        bucket_id: Option<Uuid>,
        sha256: &str,
    ) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .find(|f| {
                f.deleted_at.is_none()
                    && f.owner_id == owner_id
                    && f.bucket_id == bucket_id
                    && f.sha256 == sha256
            })
            .cloned())
    }

    async fn get_bucket_grant(
        &self,
        user_id: Uuid,
        bucket_id: Uuid,
    ) -> Result<Option<i0721_proto::BucketGrantRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .grants
            .iter()
            .find(|g| g.user_id == user_id && g.bucket_id == bucket_id)
            .cloned())
    }

    async fn set_bucket_grant(
        &self,
        user_id: Uuid,
        bucket_id: Uuid,
        can_read: bool,
        can_write: bool,
        can_share: bool,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if let Some(row) = inner
            .grants
            .iter_mut()
            .find(|g| g.user_id == user_id && g.bucket_id == bucket_id)
        {
            row.can_read = can_read;
            row.can_write = can_write;
            row.can_share = can_share;
            return Ok(());
        }
        inner.grants.push(i0721_proto::BucketGrantRecord {
            user_id,
            bucket_id,
            can_read,
            can_write,
            can_share,
            created_at: Timestamp::now(),
        });
        Ok(())
    }

    async fn list_bucket_grants_for_user(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<i0721_proto::BucketGrantRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .grants
            .iter()
            .filter(|g| g.user_id == user_id)
            .cloned()
            .collect())
    }

    async fn list_bucket_grants_for_bucket(
        &self,
        bucket_id: Uuid,
    ) -> Result<Vec<i0721_proto::BucketGrantRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .grants
            .iter()
            .filter(|g| g.bucket_id == bucket_id)
            .cloned()
            .collect())
    }

    async fn settings_state(&self, key: &str) -> Result<crate::SettingsState, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        if let Some(raw) = inner.settings_raw.get(key) {
            return Ok(serde_json::from_str::<serde_json::Value>(raw)
                .map(crate::SettingsState::Value)
                .unwrap_or_else(|_| crate::SettingsState::Corrupt { raw: raw.clone() }));
        }
        match inner.settings.get(key) {
            None => Ok(crate::SettingsState::Absent),
            Some(value) => Ok(crate::SettingsState::Value(value.clone())),
        }
    }

    async fn a_live_key_references_bucket_slug(&self, slug: &str) -> Result<bool, StoreError> {
        // The memory twin of `BUCKET_KEY_REFERENCE_SQL`: live keys only,
        // and only the well-formed `buckets` arrays count (fail-closed
        // on malformed scopes — a corrupt row never blocks a delete).
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner.api_keys.iter().any(|key| {
            key.revoked_at.is_none()
                && scopes_buckets(&key.scopes).is_some_and(|set| set.contains(slug))
        }))
    }

    async fn create_file(&self, file: NewFile) -> Result<FileRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if inner.files.iter().any(|f| {
            f.deleted_at.is_none() && f.owner_id == file.owner_id && f.sha256 == file.sha256
        }) {
            return Err(conflict(format!(
                "content {sha} is already uploaded by this user",
                sha = file.sha256
            )));
        }
        if inner.files.iter().any(|f| {
            f.deleted_at.is_none()
                && f.owner_id == file.owner_id
                && f.parent_id == file.parent_id
                && f.name == file.name
        }) {
            return Err(conflict(format!(
                "a row with the name {name} already exists in this folder",
                name = file.name
            )));
        }
        let record = FileRecord {
            id: Uuid::new_v4(),
            owner_id: file.owner_id,
            parent_id: file.parent_id,
            name: file.name,
            kind: file.kind,
            mime: file.mime,
            size: file.size,
            sha256: file.sha256,
            bucket_id: file.bucket_id,
            storage_backend: file.storage_backend,
            storage_key: file.storage_key,
            thumbnail_key: file.thumbnail_key,
            original_name: file.original_name,
            created_at: Timestamp::now(),
            deleted_at: None,
        };
        inner.files.push(record.clone());
        Ok(record)
    }

    async fn rename_file(&self, id: Uuid, name: &str) -> Result<Option<FileRecord>, StoreError> {
        {
            // Same-layer uniqueness (the PG partial unique index's
            // mirror — the memory lane must refuse what the index would
            // reject, excluding the row itself).
            let inner = self.inner.lock().map_err(lock_err)?;
            if let Some(current) = inner
                .files
                .iter()
                .find(|f| f.id == id && f.deleted_at.is_none())
            {
                let owner = current.owner_id;
                let parent = current.parent_id;
                let taken = inner.files.iter().any(|f| {
                    f.deleted_at.is_none()
                        && f.owner_id == owner
                        && f.parent_id == parent
                        && f.name == name
                        && f.id != id
                });
                if taken {
                    return Err(conflict(format!(
                        "name {name:?} already exists in this folder"
                    )));
                }
            }
        }
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let Some(file) = inner
            .files
            .iter_mut()
            .find(|file| file.id == id && file.deleted_at.is_none())
        else {
            return Ok(None);
        };
        file.name = name.to_owned();
        Ok(Some(file.clone()))
    }

    async fn soft_delete_file(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let Some(file) = inner
            .files
            .iter_mut()
            .find(|file| file.id == id && file.deleted_at.is_none())
        else {
            return Ok(false);
        };
        file.deleted_at = Some(Timestamp::now().to_string());
        Ok(true)
    }

    async fn get_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<Uuid>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .oauth_identities
            .iter()
            .find(|(p, s, _)| p == provider && s == subject)
            .map(|(_, _, id)| *id))
    }

    async fn link_oauth_identity(
        &self,
        provider: &str,
        subject: &str,
        user_id: Uuid,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if inner
            .oauth_identities
            .iter()
            .any(|(p, s, _)| p == provider && s == subject)
        {
            return Err(conflict(format!(
                "identity {provider}/{subject} is already linked"
            )));
        }
        inner
            .oauth_identities
            .push((provider.to_owned(), subject.to_owned(), user_id));
        Ok(())
    }

    async fn oauth_providers_for_user(&self, user_id: Uuid) -> Result<Vec<String>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut providers: Vec<String> = inner
            .oauth_identities
            .iter()
            .filter(|(_, _, id)| *id == user_id)
            .map(|(provider, _, _)| provider.clone())
            .collect();
        // Two subjects of one provider are two rows but ONE method: the
        // fact the caller wants is "this provider is bound", not how many
        // identities it holds.
        providers.sort();
        providers.dedup();
        Ok(providers)
    }

    async fn count_live_files_by_thumbnail_key(
        &self,
        bucket_id: Option<Uuid>,
        key: &str,
    ) -> Result<u64, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .files
            .iter()
            .filter(|f| {
                f.deleted_at.is_none()
                    && f.bucket_id == bucket_id
                    && f.thumbnail_key.as_deref() == Some(key)
            })
            .count() as u64)
    }

    async fn create_api_key(&self, record: &i0721_proto::ApiKeyRecord) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        inner.api_keys.push(record.clone());
        Ok(())
    }

    async fn list_api_keys(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<i0721_proto::ApiKeyRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut rows: Vec<_> = inner
            .api_keys
            .iter()
            .filter(|k| k.user_id == user_id)
            .cloned()
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(rows)
    }

    async fn revoke_api_key(&self, id: Uuid) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let row = inner.api_keys.iter_mut().find(|k| k.id == id);
        match row {
            Some(key) if key.revoked_at.is_none() => {
                key.revoked_at = Some(Timestamp::now().to_string());
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn update_api_key(
        &self,
        id: Uuid,
        name: &str,
        scopes_json: &str,
        expires_at: Option<&str>,
    ) -> Result<bool, StoreError> {
        // The in-memory twin of the pg write: the payload must parse
        // before it can replace the stored text, and the stored form is
        // serde_json's re-serialization (what the pg JSONB column hands
        // back), so both backends answer byte-identical scopes.
        let scopes = serde_json::from_str::<serde_json::Value>(scopes_json).map_err(|err| {
            StoreError::Db(format!(
                "api key scopes must be valid JSON ({err}): {scopes_json:?}"
            ))
        })?;
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let row = inner
            .api_keys
            .iter_mut()
            .find(|k| k.id == id && k.revoked_at.is_none());
        match row {
            Some(key) => {
                key.name = name.to_owned();
                key.scopes = scopes.to_string();
                key.expires_at = expires_at.map(str::to_owned);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn find_api_key_by_hash(
        &self,
        key_hash: &str,
    ) -> Result<Option<i0721_proto::ApiKeyRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .api_keys
            .iter()
            .find(|k| k.key_hash == key_hash && k.revoked_at.is_none())
            .cloned())
    }

    async fn find_api_key_by_id(
        &self,
        id: Uuid,
    ) -> Result<Option<i0721_proto::ApiKeyRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        // Any state: the ownership lanes must see revoked/expired rows.
        Ok(inner.api_keys.iter().find(|k| k.id == id).cloned())
    }

    async fn touch_api_key(&self, id: Uuid) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if let Some(key) = inner.api_keys.iter_mut().find(|k| k.id == id) {
            key.last_used_at = Some(Timestamp::now().to_string());
        }
        Ok(())
    }

    async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .settings
            .get(key)
            .cloned())
    }

    async fn set_setting(&self, key: &str, value: serde_json::Value) -> Result<(), StoreError> {
        self.inner
            .lock()
            .map_err(lock_err)?
            .settings
            .insert(key.to_owned(), value);
        Ok(())
    }

    // ── netdisk lane ─────────────────────────────────────────────────

    async fn count_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        buckets: Option<&[Uuid]>,
    ) -> Result<u64, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let n = inner
            .files
            .iter()
            .filter(|f| {
                f.deleted_at.is_none()
                    && f.owner_id == owner_id
                    && f.parent_id == parent_id
                    && kind.as_deref().is_none_or(|k| f.kind.as_str() == k)
                    // §7 pushdown: rows outside the visible set vanish
                    // from the count (Q14: items AND total shrink).
                    && buckets.is_none_or(|set| {
                        f.bucket_id.is_none_or(|b| set.contains(&b))
                    })
            })
            .count();
        Ok(n as u64)
    }

    async fn list_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        kind: Option<String>,
        buckets: Option<&[Uuid]>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut rows: Vec<FileRecord> = inner
            .files
            .iter()
            .filter(|f| {
                f.deleted_at.is_none()
                    && f.owner_id == owner_id
                    && f.parent_id == parent_id
                    && kind.as_ref().is_none_or(|k| *k == f.kind.as_str())
                    && buckets.is_none_or(|set| f.bucket_id.is_none_or(|b| set.contains(&b)))
            })
            .cloned()
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn folder_name_taken(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        bucket_id: Option<Uuid>,
        name: &str,
        exclude: Option<Uuid>,
    ) -> Result<bool, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner.files.iter().any(|f| {
            f.deleted_at.is_none()
                && f.owner_id == owner_id
                && f.parent_id == parent_id
                && f.bucket_id == bucket_id
                && f.name == name
                && Some(f.id) != exclude
        }))
    }

    async fn create_folder(
        &self,
        owner_id: Uuid,
        parent_id: Option<Uuid>,
        name: &str,
        bucket_id: Option<Uuid>,
    ) -> Result<FileRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if inner.files.iter().any(|f| {
            f.deleted_at.is_none()
                && f.owner_id == owner_id
                && f.parent_id == parent_id
                && f.bucket_id == bucket_id
                && f.name == name
        }) {
            return Err(conflict(format!(
                "name {name:?} already exists in this folder"
            )));
        }
        // R-G: a child inherits its parent's bucket — a caller passing a
        // different bucket than the parent's is refused, not silently
        // re-seated (the same refusal `set_parent` gives a move).
        if let Some(parent_id) = parent_id {
            let parent_bucket = inner
                .files
                .iter()
                .find(|f| f.id == parent_id && f.deleted_at.is_none())
                .map(|f| f.bucket_id);
            if parent_bucket != Some(bucket_id) {
                return Err(conflict("cannot create a folder across buckets".to_owned()));
            }
        }
        let record = FileRecord {
            id: Uuid::new_v4(),
            owner_id,
            parent_id,
            name: name.to_owned(),
            kind: i0721_proto::FileKind::Folder,
            mime: "inode/directory".to_owned(),
            size: 0,
            sha256: format!("dir:{}", Uuid::new_v4().simple()),
            bucket_id,
            storage_backend: "local".to_owned(),
            storage_key: String::new(),
            thumbnail_key: None,
            original_name: name.to_owned(),
            created_at: Timestamp::now(),
            deleted_at: None,
        };
        inner.files.push(record.clone());
        Ok(record)
    }

    async fn set_parent(&self, id: Uuid, parent_id: Option<Uuid>) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        // R-G: the folder tree is per bucket. A move whose target parent
        // sits in another bucket is refused here — the ONE guard every
        // move path shares (the RPC lane has no second bypass). Computed
        // BEFORE the mutable borrow so the locks do not conflict.
        let row_bucket = inner
            .files
            .iter()
            .find(|f| f.id == id && f.deleted_at.is_none())
            .map(|f| f.bucket_id)
            .ok_or_else(|| conflict("file not found".to_owned()))?;
        let parent_bucket = match parent_id {
            Some(pid) => inner
                .files
                .iter()
                .find(|f| f.id == pid && f.deleted_at.is_none())
                .map(|f| f.bucket_id)
                .ok_or_else(|| conflict("the target folder does not exist".to_owned()))?,
            // Root: the tree is per bucket, so a row moved to its own
            // root keeps the bucket it already lives in.
            None => row_bucket,
        };
        if parent_bucket != row_bucket {
            return Err(conflict(
                "cannot move a row into a folder in another bucket".to_owned(),
            ));
        }
        let row = inner
            .files
            .iter_mut()
            .find(|f| f.id == id && f.deleted_at.is_none())
            .ok_or_else(|| conflict("file not found".to_owned()))?;
        row.parent_id = parent_id;
        Ok(())
    }

    async fn ancestor_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::from([id]);
        let mut cursor = id;
        while let Some(row) = inner.files.iter().find(|f| f.id == cursor) {
            let Some(parent) = row.parent_id else {
                break;
            };
            // Cycle guard: corrupted parent chains (a loop) must not
            // spin forever — bail on a revisited node.
            if !visited.insert(parent) {
                break;
            }
            chain.push(parent);
            cursor = parent;
        }
        Ok(chain)
    }

    async fn subtree_live_ids(&self, id: Uuid) -> Result<Vec<Uuid>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut ids = vec![id];
        let mut frontier = vec![id];
        while let Some(current) = frontier.pop() {
            for row in &inner.files {
                if row.deleted_at.is_none() && row.parent_id == Some(current) {
                    ids.push(row.id);
                    frontier.push(row.id);
                }
            }
        }
        Ok(ids)
    }

    async fn soft_delete_many(&self, ids: &[Uuid]) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let stamp = Timestamp::now().to_string();
        let mut touched = 0_u64;
        for row in inner.files.iter_mut() {
            if ids.contains(&row.id) && row.deleted_at.is_none() {
                row.deleted_at = Some(stamp.clone());
                touched += 1;
            }
        }
        Ok(touched)
    }

    async fn get_files_by_ids(&self, ids: &[Uuid]) -> Result<Vec<FileRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .files
            .iter()
            .filter(|f| ids.contains(&f.id))
            .cloned()
            .collect())
    }

    async fn create_share(&self, share: i0721_proto::NewShare) -> Result<ShareRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if let Some(existing) = inner
            .shares
            .iter()
            .find(|s| s.file_id == share.file_id && s.owner_id == share.owner_id)
        {
            return Ok(existing.clone());
        }
        let record = ShareRecord {
            token: crate::new_share_token(),
            file_id: share.file_id,
            owner_id: share.owner_id,
            expires_at: share.expires_at,
            allow_preview: share.allow_preview,
            created_at: Timestamp::now(),
        };
        inner.shares.push(record.clone());
        Ok(record)
    }

    async fn get_share(&self, token: &str) -> Result<Option<ShareRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner.shares.iter().find(|s| s.token == token).cloned())
    }

    async fn find_share_by_file(
        &self,
        owner_id: Uuid,
        file_id: Uuid,
    ) -> Result<Option<ShareRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .shares
            .iter()
            .filter(|s| s.file_id == file_id && s.owner_id == owner_id)
            .max_by_key(|s| s.created_at)
            .cloned())
    }

    async fn list_shares(&self, owner_id: Uuid) -> Result<Vec<ShareRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut rows: Vec<ShareRecord> = inner
            .shares
            .iter()
            .filter(|s| s.owner_id == owner_id)
            .cloned()
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(rows)
    }

    async fn delete_share(&self, token: &str) -> Result<bool, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let before = inner.shares.len();
        inner.shares.retain(|s| s.token != token);
        Ok(inner.shares.len() != before)
    }

    async fn delete_shares_for_files(&self, file_ids: &[Uuid]) -> Result<u64, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let before = inner.shares.len();
        inner.shares.retain(|s| !file_ids.contains(&s.file_id));
        Ok((before - inner.shares.len()) as u64)
    }

    async fn sum_live_sizes(&self, owner_id: Uuid) -> Result<i64, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        Ok(inner
            .files
            .iter()
            .filter(|f| f.deleted_at.is_none() && f.owner_id == owner_id)
            .map(|f| f.size)
            .sum())
    }

    async fn set_user_quota(
        &self,
        user_id: Uuid,
        quota_bytes: Option<i64>,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let row = inner
            .users
            .iter_mut()
            .find(|u| u.id == user_id)
            .ok_or_else(|| conflict("user not found".to_owned()))?;
        row.quota_bytes = quota_bytes;
        Ok(())
    }

    async fn set_user_role(
        &self,
        user_id: Uuid,
        role: i0721_proto::Role,
    ) -> Result<(), StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        let row = inner
            .users
            .iter_mut()
            .find(|u| u.id == user_id)
            .ok_or_else(|| conflict("user not found".to_owned()))?;
        row.role = role;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use i0721_proto::{FileKind, NewFile, Role};

    use super::*;
    use crate::CreateUser;

    fn new_user(name: &str) -> CreateUser {
        CreateUser {
            username: name.to_owned(),
            display_name: name.to_owned(),
            role: Role::Member,
            password_hash: None,
        }
    }

    fn new_file(owner: Uuid, sha: &str) -> NewFile {
        NewFile {
            bucket_id: None,
            parent_id: None,
            owner_id: owner,
            name: sha.to_owned(),
            kind: FileKind::Image,
            mime: "image/webp".to_owned(),
            size: 10,
            sha256: sha.to_owned(),
            storage_backend: "local".to_owned(),
            storage_key: sha.to_owned(),
            thumbnail_key: None,
            original_name: "cat.png".to_owned(),
        }
    }

    #[tokio::test]
    async fn users_are_unique_and_paged() {
        let store = MemoryStore::new();
        for i in 0..5 {
            store
                .create_user(new_user(&format!("user{i}")))
                .await
                .unwrap();
        }
        assert!(matches!(
            store.create_user(new_user("user1")).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(store.count_users().await.unwrap(), 5);
        let page = store.list_users(2, 2).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].username, "user2");
    }

    #[tokio::test]
    async fn files_timeline_hides_soft_deleted_and_frees_hash() {
        let store = MemoryStore::new();
        let owner = store.create_user(new_user("u")).await.unwrap();
        let a = store.create_file(new_file(owner.id, "aa")).await.unwrap();
        store.create_file(new_file(owner.id, "bb")).await.unwrap();

        assert_eq!(store.count_files(None).await.unwrap(), 2);
        assert!(
            store.create_file(new_file(owner.id, "aa")).await.is_err(),
            "the same owner's live duplicate hash is rejected"
        );

        assert!(store.soft_delete_file(a.id).await.unwrap());
        assert!(
            !store.soft_delete_file(a.id).await.unwrap(),
            "the guarded update marks 0 rows the second time"
        );
        assert_eq!(store.count_files(None).await.unwrap(), 1);
        assert!(store.get_file(a.id).await.unwrap().is_none());
        assert!(
            store.create_file(new_file(owner.id, "aa")).await.is_ok(),
            "soft delete frees the hash for re-upload"
        );
    }

    #[tokio::test]
    async fn dedupe_is_per_owner_and_shares_the_blob() {
        let store = MemoryStore::new();
        let owner_a = store.create_user(new_user("a")).await.unwrap();
        let owner_b = store.create_user(new_user("b")).await.unwrap();

        let first = store.create_file(new_file(owner_a.id, "aa")).await.unwrap();
        // Same content, different owner: a NEW row (no DTO leak), sharing
        // the same content-addressed storage key.
        let second = store.create_file(new_file(owner_b.id, "aa")).await.unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.storage_key, second.storage_key);
        assert_eq!(
            store
                .count_live_files_by_storage_key(None, "aa")
                .await
                .unwrap(),
            2
        );

        // Soft-deleting one row leaves the blob referenced by the other.
        assert!(store.soft_delete_file(first.id).await.unwrap());
        assert_eq!(
            store
                .count_live_files_by_storage_key(None, "aa")
                .await
                .unwrap(),
            1
        );
        assert!(
            store.soft_delete_file(second.id).await.unwrap(),
            "the last live row can go too"
        );
        assert_eq!(
            store
                .count_live_files_by_storage_key(None, "aa")
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn rename_after_soft_delete_answers_none() {
        let store = MemoryStore::new();
        let owner = store.create_user(new_user("u")).await.unwrap();
        let a = store.create_file(new_file(owner.id, "aa")).await.unwrap();
        store.soft_delete_file(a.id).await.unwrap();
        assert!(store.rename_file(a.id, "renamed").await.unwrap().is_none());
        assert!(
            store
                .rename_file(Uuid::new_v4(), "renamed")
                .await
                .unwrap()
                .is_none(),
            "unknown ids answer None too"
        );
    }

    #[tokio::test]
    async fn find_share_by_file_is_owner_scoped_and_answers_the_newest() {
        let store = MemoryStore::new();
        let owner = store.create_user(new_user("owner")).await.unwrap();
        let other = store.create_user(new_user("other")).await.unwrap();
        let a = store.create_file(new_file(owner.id, "aa")).await.unwrap();
        let b = store.create_file(new_file(owner.id, "bb")).await.unwrap();

        assert!(
            store
                .find_share_by_file(owner.id, a.id)
                .await
                .unwrap()
                .is_none(),
            "a file without a share answers None"
        );

        let share = store
            .create_share(i0721_proto::NewShare {
                file_id: a.id,
                owner_id: owner.id,
                expires_at: None,
                allow_preview: true,
            })
            .await
            .unwrap();
        let found = store
            .find_share_by_file(owner.id, a.id)
            .await
            .unwrap()
            .expect("the file's share is found");
        assert_eq!(found.token, share.token);
        assert!(
            store
                .find_share_by_file(owner.id, b.id)
                .await
                .unwrap()
                .is_none(),
            "the lookup is per file"
        );
        assert!(
            store
                .find_share_by_file(other.id, a.id)
                .await
                .unwrap()
                .is_none(),
            "the lookup is per owner (no cross-owner token leak)"
        );
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let store = MemoryStore::new();
        assert!(store.get_setting("site").await.unwrap().is_none());
        store
            .set_setting("site", serde_json::json!({"ok": true}))
            .await
            .unwrap();
        assert_eq!(
            store.get_setting("site").await.unwrap(),
            Some(serde_json::json!({"ok": true}))
        );
        store
            .set_setting("site", serde_json::json!({"ok": false}))
            .await
            .unwrap();
        assert_eq!(
            store.get_setting("site").await.unwrap(),
            Some(serde_json::json!({"ok": false})),
            "set upserts"
        );
    }

    #[tokio::test]
    async fn deleting_a_user_removes_their_files() {
        let store = MemoryStore::new();
        let owner = store.create_user(new_user("u")).await.unwrap();
        store.create_file(new_file(owner.id, "aa")).await.unwrap();
        assert!(store.delete_user(owner.id).await.unwrap());
        assert_eq!(store.count_files(None).await.unwrap(), 0);
    }

    /// `set_user_role` writes the row and reads back through `get_user`
    /// (up AND down — a role sync must be able to demote).
    #[tokio::test]
    async fn set_user_role_writes_the_row_both_ways() {
        let store = MemoryStore::new();
        let user = store.create_user(new_user("role-user")).await.unwrap();
        assert_eq!(user.role, Role::Member);

        store.set_user_role(user.id, Role::Admin).await.unwrap();
        assert_eq!(
            store.get_user(user.id).await.unwrap().unwrap().role,
            Role::Admin,
            "a member is promoted to admin"
        );

        store.set_user_role(user.id, Role::Member).await.unwrap();
        assert_eq!(
            store.get_user(user.id).await.unwrap().unwrap().role,
            Role::Member,
            "an admin is demoted back to member"
        );
        // The write touches ONE row.
        assert_eq!(store.count_users().await.unwrap(), 1);
    }

    /// An unknown id is `Conflict` — the same answer `set_user_quota` gives.
    #[tokio::test]
    async fn set_user_role_of_a_missing_user_is_a_conflict() {
        let store = MemoryStore::new();
        assert!(matches!(
            store.set_user_role(Uuid::new_v4(), Role::Admin).await,
            Err(StoreError::Conflict(_))
        ));
    }
}
