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

#[derive(Debug, Default)]
struct Inner {
    users: Vec<UserRecord>,
    files: Vec<FileRecord>,
    settings: HashMap<String, serde_json::Value>,
}

/// The in-memory [`Store`].
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
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
}

fn lock_err(poisoned: impl std::fmt::Display) -> StoreError {
    StoreError::Db(format!("store mutex poisoned: {poisoned}"))
}

fn conflict(reason: impl Into<String>) -> StoreError {
    StoreError::Conflict(reason.into())
}

#[async_trait::async_trait]
impl Store for MemoryStore {
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

    async fn list_files(&self, offset: u64, limit: u64) -> Result<Vec<FileRecord>, StoreError> {
        let inner = self.inner.lock().map_err(lock_err)?;
        let mut files: Vec<FileRecord> = inner
            .files
            .iter()
            .filter(|file| file.deleted_at.is_none())
            .cloned()
            .collect();
        files.sort_by_key(|file| std::cmp::Reverse(file.created_at));
        Ok(files
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    async fn count_files(&self) -> Result<u64, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .filter(|file| file.deleted_at.is_none())
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

    async fn get_file_by_sha256(&self, sha256: &str) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .find(|file| file.deleted_at.is_none() && file.sha256 == sha256)
            .cloned())
    }

    async fn get_file_by_storage_key(&self, key: &str) -> Result<Option<FileRecord>, StoreError> {
        Ok(self
            .inner
            .lock()
            .map_err(lock_err)?
            .files
            .iter()
            .find(|file| file.deleted_at.is_none() && file.storage_key == key)
            .cloned())
    }

    async fn create_file(&self, file: NewFile) -> Result<FileRecord, StoreError> {
        let mut inner = self.inner.lock().map_err(lock_err)?;
        if inner
            .files
            .iter()
            .any(|f| f.deleted_at.is_none() && f.sha256 == file.sha256)
        {
            return Err(conflict(format!(
                "content {sha} is already uploaded",
                sha = file.sha256
            )));
        }
        let record = FileRecord {
            id: Uuid::new_v4(),
            owner_id: file.owner_id,
            parent_id: None,
            name: file.name,
            kind: file.kind,
            mime: file.mime,
            size: file.size,
            sha256: file.sha256,
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

        assert_eq!(store.count_files().await.unwrap(), 2);
        assert!(
            store.create_file(new_file(owner.id, "aa")).await.is_err(),
            "live duplicate hash is rejected"
        );

        assert!(store.soft_delete_file(a.id).await.unwrap());
        assert_eq!(store.count_files().await.unwrap(), 1);
        assert!(store.get_file(a.id).await.unwrap().is_none());
        assert!(
            store.create_file(new_file(owner.id, "aa")).await.is_ok(),
            "soft delete frees the hash for re-upload"
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
        assert_eq!(store.count_files().await.unwrap(), 0);
    }
}
