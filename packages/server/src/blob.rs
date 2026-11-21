//! Blob storage — where file bytes live.
//!
//! Metadata is PostgreSQL ([`i0721_store`]); bytes go to a [`BlobStore`].
//! The local layout keeps the legacy `MEDIA_DIR` / `MEDIA_CACHE_DIR`
//! semantics: two directories below one env-configured root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bytes::Bytes;

/// Which directory a blob address names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlobSlot {
    /// Original blobs (legacy `MEDIA_DIR`).
    Media,
    /// Thumbnails (legacy `MEDIA_CACHE_DIR`).
    Cache,
}

/// A blob backend.
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    /// Write a blob (overwrites).
    async fn put(&self, slot: BlobSlot, key: &str, data: &[u8]) -> Result<(), std::io::Error>;

    /// Read a blob; `None` when absent.
    async fn get(&self, slot: BlobSlot, key: &str) -> Result<Option<Bytes>, std::io::Error>;

    /// Remove a blob (absent keys are fine).
    async fn delete(&self, slot: BlobSlot, key: &str) -> Result<(), std::io::Error>;
}

/// Reject keys that could escape their slot directory.
#[must_use]
pub fn is_safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

/// The local-filesystem [`BlobStore`]: `root/media/` and
/// `root/media-cache/`.
#[derive(Debug, Clone)]
pub struct LocalBlobStore {
    media_dir: PathBuf,
    cache_dir: PathBuf,
}

impl LocalBlobStore {
    /// Create the store, ensuring both directories exist.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when a directory cannot be created.
    pub fn open(root: &Path) -> Result<Self, std::io::Error> {
        let media_dir = root.join("media");
        let cache_dir = root.join("media-cache");
        std::fs::create_dir_all(&media_dir)?;
        std::fs::create_dir_all(&cache_dir)?;
        Ok(Self {
            media_dir,
            cache_dir,
        })
    }

    fn path(&self, slot: BlobSlot, key: &str) -> Result<PathBuf, std::io::Error> {
        if !is_safe_key(key) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe blob key {key:?}"),
            ));
        }
        let dir = match slot {
            BlobSlot::Media => &self.media_dir,
            BlobSlot::Cache => &self.cache_dir,
        };
        Ok(dir.join(key))
    }
}

#[async_trait::async_trait]
impl BlobStore for LocalBlobStore {
    async fn put(&self, slot: BlobSlot, key: &str, data: &[u8]) -> Result<(), std::io::Error> {
        let path = self.path(slot, key)?;
        let data = data.to_vec();
        // Write-then-rename keeps readers away from partial blobs.
        let tmp = path.with_extension("part");
        tokio::fs::write(&tmp, &data).await?;
        tokio::fs::rename(&tmp, &path).await
    }

    async fn get(&self, slot: BlobSlot, key: &str) -> Result<Option<Bytes>, std::io::Error> {
        let path = self.path(slot, key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(Bytes::from(bytes))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn delete(&self, slot: BlobSlot, key: &str) -> Result<(), std::io::Error> {
        let path = self.path(slot, key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

/// The in-memory [`BlobStore`] (tests, and the embeddable lane).
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: std::sync::Mutex<HashMap<(BlobSlot, String), Bytes>>,
}

impl MemoryBlobStore {
    /// A fresh empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl BlobStore for MemoryBlobStore {
    async fn put(&self, slot: BlobSlot, key: &str, data: &[u8]) -> Result<(), std::io::Error> {
        if !is_safe_key(key) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe blob key {key:?}"),
            ));
        }
        self.blobs
            .lock()
            .expect("blob mutex")
            .insert((slot, key.to_owned()), Bytes::copy_from_slice(data));
        Ok(())
    }

    async fn get(&self, slot: BlobSlot, key: &str) -> Result<Option<Bytes>, std::io::Error> {
        Ok(self
            .blobs
            .lock()
            .expect("blob mutex")
            .get(&(slot, key.to_owned()))
            .cloned())
    }

    async fn delete(&self, slot: BlobSlot, key: &str) -> Result<(), std::io::Error> {
        self.blobs
            .lock()
            .expect("blob mutex")
            .remove(&(slot, key.to_owned()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_store_round_trip() {
        let root = std::env::temp_dir().join(format!("i0721-blob-{}", std::process::id()));
        let store = LocalBlobStore::open(&root).unwrap();
        store
            .put(BlobSlot::Media, "abc123", b"hello")
            .await
            .unwrap();
        assert_eq!(
            store.get(BlobSlot::Media, "abc123").await.unwrap(),
            Some(Bytes::from_static(b"hello"))
        );
        assert_eq!(store.get(BlobSlot::Media, "gone").await.unwrap(), None);
        assert_eq!(store.get(BlobSlot::Cache, "abc123").await.unwrap(), None);
        store.delete(BlobSlot::Media, "abc123").await.unwrap();
        assert_eq!(store.get(BlobSlot::Media, "abc123").await.unwrap(), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn unsafe_keys_are_refused() {
        assert!(!is_safe_key("../escape"));
        assert!(!is_safe_key("a/b"));
        assert!(!is_safe_key(""));
        assert!(is_safe_key("deadbeef.webp"));
        let store = MemoryBlobStore::new();
        assert!(store.put(BlobSlot::Media, "../escape", b"x").await.is_err());
    }
}
