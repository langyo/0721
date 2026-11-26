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

    /// Enumerate the MEDIA-slot keys (`None` = this backend cannot be
    /// scanned — ipfs; the RPC surface answers a distinct refusal, never
    /// a silent empty list). The MEDIA slot only: `cache/` never enters
    /// the unregistered diff (thumbnails legitimately have no rows).
    async fn list_media_keys(&self) -> Result<Option<Vec<String>>, std::io::Error> {
        Ok(None)
    }
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

    async fn list_media_keys(&self) -> Result<Option<Vec<String>>, std::io::Error> {
        // The enumeration IS the prefix exclusion: only `media/` files
        // are read, so a stray `cache/` entry can never appear (and
        // orphan thumbnails are invisible to the unregistered lane by
        // construction, not by a post-filter).
        let mut keys = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.media_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_safe_key(&name) {
                keys.push(name);
            }
        }
        keys.sort();
        Ok(Some(keys))
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

    async fn list_media_keys(&self) -> Result<Option<Vec<String>>, std::io::Error> {
        let mut keys: Vec<String> = self
            .blobs
            .lock()
            .expect("blob mutex")
            .keys()
            .filter(|(slot, _)| *slot == BlobSlot::Media)
            .map(|(_, key)| key.clone())
            .collect();
        keys.sort();
        Ok(Some(keys))
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

    /// MUT:P3-CACHE-PREFIX — listing the CACHE slot too (or merging the
    /// two slots in `list_media_keys`) turns this red: a cache key shows
    /// up in the media enumeration.
    #[tokio::test]
    async fn memory_list_media_keys_excludes_the_cache_slot() {
        let store = MemoryBlobStore::new();
        store
            .put(BlobSlot::Media, "media-only", b"m")
            .await
            .unwrap();
        store
            .put(BlobSlot::Cache, "thumb.webp", b"t")
            .await
            .unwrap();
        let keys = store.list_media_keys().await.unwrap().unwrap();
        assert_eq!(keys, vec!["media-only".to_owned()]);
    }

    /// MUT:P3-LOCAL-LIST — returning an empty list (or reading the cache
    /// dir) reddens this: the tmp-dir scan must report every regular
    /// media file and nothing from `media-cache/`.
    #[tokio::test]
    async fn local_list_media_keys_scans_the_media_dir_only() {
        let root = std::env::temp_dir().join(format!("i0721-blob-list-{}", std::process::id()));
        let store = LocalBlobStore::open(&root).unwrap();
        store.put(BlobSlot::Media, "key-b", b"2").await.unwrap();
        store.put(BlobSlot::Media, "key-a", b"1").await.unwrap();
        store
            .put(BlobSlot::Cache, "thumb.webp", b"t")
            .await
            .unwrap();
        // A stray subdirectory under media/ is not a media blob.
        std::fs::create_dir_all(root.join("media").join("nested")).unwrap();
        let keys = store.list_media_keys().await.unwrap().unwrap();
        assert_eq!(
            keys,
            vec!["key-a".to_owned(), "key-b".to_owned()],
            "regular media files only, sorted, cache/ and dirs excluded"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// MUT:P3-LOCAL-SYMLINK — following symlinks (reading `metadata()`
    /// instead of the entry's `file_type()`) reddens this: a symlink
    /// pointing OUTSIDE the media dir must never enter the enumeration.
    #[tokio::test]
    async fn local_list_media_keys_never_lists_a_symlink_escape() {
        let root = std::env::temp_dir().join(format!("i0721-blob-symlink-{}", std::process::id()));
        let store = LocalBlobStore::open(&root).unwrap();
        store.put(BlobSlot::Media, "real-key", b"1").await.unwrap();
        // A REAL file outside the media dir, linked INTO it.
        let outside = root.join("outside-target");
        std::fs::write(&outside, b"secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("media").join("escape-link")).unwrap();
        let keys = store.list_media_keys().await.unwrap().unwrap();
        assert_eq!(
            keys,
            vec!["real-key".to_owned()],
            "a symlinked entry (whatever it points at) is never a media blob: {keys:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// MUT:P3-LOCAL-UNSAFE — relaxing `is_safe_key` to allow `/` (or
    /// any path-constructing character) reddens this: nested paths and
    /// dot-dot-style names must never enter the enumeration.
    #[tokio::test]
    async fn local_list_media_keys_never_lists_an_unsafe_name() {
        let root = std::env::temp_dir().join(format!("i0721-blob-unsafe-{}", std::process::id()));
        let store = LocalBlobStore::open(&root).unwrap();
        store.put(BlobSlot::Media, "safe-key", b"1").await.unwrap();
        // A nested path a/b under media/ (a file whose name contains a
        // `/` can only exist as a subdirectory entry).
        std::fs::create_dir_all(root.join("media").join("a")).unwrap();
        std::fs::write(root.join("media").join("a").join("b"), b"nested").unwrap();
        // A creatable name that `is_safe_key` refuses (a space is not in
        // the safe alphabet — the same class of unsafe name as `/` or
        // `..`, which the filesystem itself forbids as literal file
        // names).
        std::fs::write(root.join("media").join("bad name"), b"unsafe").unwrap();
        let keys = store.list_media_keys().await.unwrap().unwrap();
        assert_eq!(
            keys,
            vec!["safe-key".to_owned()],
            "nested paths and unsafe names are never media blobs: {keys:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
