//! File records, kind classification and name validation.
//!
//! The schema carries the netdisk fields (parent directory, kinds, soft
//! delete) from day one so later waves never re-migrate the table.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The coarse content class of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    /// Raster/vector images.
    Image,
    /// Text documents, PDF, office files.
    Document,
    /// Audio and video.
    Media,
    /// Archive containers.
    Archive,
    /// Everything else.
    Other,
}

impl FileKind {
    /// The database tag of this kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Image => "image",
            FileKind::Document => "document",
            FileKind::Media => "media",
            FileKind::Archive => "archive",
            FileKind::Other => "other",
        }
    }

    /// Parse a kind from its database tag.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "image" => Some(FileKind::Image),
            "document" => Some(FileKind::Document),
            "media" => Some(FileKind::Media),
            "archive" => Some(FileKind::Archive),
            "other" => Some(FileKind::Other),
            _ => None,
        }
    }
}

/// Classify a file by MIME type (with the file name as a tie-breaker).
///
/// Images and audio/video keep their obvious buckets; text, PDF and office
/// formats are documents; archive containers by extension. Everything
/// else is `other`.
#[must_use]
pub fn classify_kind(mime: &str, file_name: &str) -> FileKind {
    let mime = mime.trim().to_ascii_lowercase();
    let (type_, subtype) = match mime.split_once('/') {
        Some(parts) => parts,
        None => return FileKind::Other,
    };
    match type_ {
        "image" => FileKind::Image,
        "audio" | "video" => FileKind::Media,
        "text" => FileKind::Document,
        "application" => match subtype {
            "pdf" | "json" | "xml" | "rtf" | "xhtml+xml" => FileKind::Document,
            "zip" | "x-tar" | "gzip" | "x-7z-compressed" | "x-rar-compressed" | "vnd.rar"
            | "x-bzip2" | "x-xz" => FileKind::Archive,
            // Office Open XML / legacy Office: documents, not archives,
            // even though they are physically zip containers.
            "vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "vnd.openxmlformats-officedocument.presentationml.presentation"
            | "msword"
            | "vnd.ms-excel"
            | "vnd.ms-powerpoint" => FileKind::Document,
            _ => fallback_by_extension(file_name),
        },
        _ => fallback_by_extension(file_name),
    }
}

fn fallback_by_extension(file_name: &str) -> FileKind {
    let lowered = file_name.to_ascii_lowercase();
    let ext = lowered.rsplit('.').next().unwrap_or_default();
    match ext {
        "txt" | "md" | "markdown" | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
        | "odt" | "ods" | "csv" | "log" => FileKind::Document,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => FileKind::Archive,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "mp4" | "mkv" | "webm" | "mov" | "avi" => {
            FileKind::Media
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "avif" | "heic" => {
            FileKind::Image
        }
        _ => FileKind::Other,
    }
}

/// A stored file, as seen by the data layer.
///
/// Blob bytes never live here — `storage_key` addresses them in the
/// configured storage backend (`local` today).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Primary key.
    pub id: Uuid,
    /// Owning user.
    pub owner_id: Uuid,
    /// Parent directory (`None` = root); directories arrive in the netdisk wave.
    pub parent_id: Option<Uuid>,
    /// Display name (source name or content hash, per configuration).
    pub name: String,
    /// Coarse content class.
    pub kind: FileKind,
    /// MIME type served with the blob.
    pub mime: String,
    /// Size in bytes of the stored (possibly converted) blob.
    pub size: i64,
    /// Hex SHA-256 of the stored blob — the dedupe key.
    pub sha256: String,
    /// Storage backend tag; `local` today.
    pub storage_backend: String,
    /// Blob key inside the storage backend (also the public media path).
    pub storage_key: String,
    /// Thumbnail blob key, when a thumbnail exists.
    pub thumbnail_key: Option<String>,
    /// The name the file had when uploaded.
    pub original_name: String,
    /// Upload time.
    pub created_at: Timestamp,
    /// Soft-delete marker (RFC 3339 text in storage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

impl FileRecord {
    /// Whether this record is live (not soft-deleted).
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// The public projection of a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileDto {
    /// Primary key.
    pub id: Uuid,
    /// Owning user.
    pub owner_id: Uuid,
    /// Display name.
    pub name: String,
    /// Coarse content class.
    pub kind: FileKind,
    /// MIME type of the blob.
    pub mime: String,
    /// Size in bytes.
    pub size: i64,
    /// Hex SHA-256 of the blob.
    pub sha256: String,
    /// The name the file had when uploaded.
    pub original_name: String,
    /// Public URL path of the blob (`/media/<storage_key>`).
    pub url: String,
    /// Upload time.
    pub created_at: Timestamp,
}

impl From<FileRecord> for FileDto {
    fn from(record: FileRecord) -> Self {
        Self {
            id: record.id,
            owner_id: record.owner_id,
            name: record.name,
            kind: record.kind,
            mime: record.mime,
            size: record.size,
            sha256: record.sha256,
            original_name: record.original_name,
            url: format!("/media/{}", record.storage_key),
            created_at: record.created_at,
        }
    }
}

/// Parameters for recording a freshly uploaded (and processed) file.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFile {
    /// Owning user.
    pub owner_id: Uuid,
    /// Display name.
    pub name: String,
    /// Coarse content class.
    pub kind: FileKind,
    /// MIME type of the stored blob.
    pub mime: String,
    /// Byte length of the stored blob.
    pub size: i64,
    /// Hex SHA-256 of the stored blob.
    pub sha256: String,
    /// Storage backend tag.
    pub storage_backend: String,
    /// Blob key.
    pub storage_key: String,
    /// Thumbnail blob key.
    pub thumbnail_key: Option<String>,
    /// The name the file had when uploaded.
    pub original_name: String,
}

/// Why a file name is not acceptable.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("file name must be 1..=255 chars without path separators or NUL, got {value:?}")]
pub struct InvalidFileName {
    /// The rejected value.
    pub value: String,
}

/// File names: 1..=255 characters, no path separators, no NUL, no `..`.
///
/// # Errors
///
/// Returns [`InvalidFileName`] outside that shape.
pub fn validate_file_name(value: &str) -> Result<(), InvalidFileName> {
    let ok = !value.is_empty()
        && value.len() <= 255
        && !value.contains(['/', '\\', '\0'])
        && value != "."
        && value != "..";
    if ok {
        Ok(())
    } else {
        Err(InvalidFileName {
            value: value.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trips_tags() {
        for kind in [
            FileKind::Image,
            FileKind::Document,
            FileKind::Media,
            FileKind::Archive,
            FileKind::Other,
        ] {
            assert_eq!(FileKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(FileKind::parse("executable"), None);
    }

    #[test]
    fn classify_by_mime() {
        assert_eq!(classify_kind("image/png", "x"), FileKind::Image);
        assert_eq!(classify_kind("video/mp4", "x"), FileKind::Media);
        assert_eq!(classify_kind("audio/ogg", "x"), FileKind::Media);
        assert_eq!(classify_kind("text/plain", "x"), FileKind::Document);
        assert_eq!(
            classify_kind("application/pdf", "report.pdf"),
            FileKind::Document
        );
        assert_eq!(
            classify_kind("application/zip", "bundle"),
            FileKind::Archive
        );
        assert_eq!(
            classify_kind("application/octet-stream", "a"),
            FileKind::Other
        );
    }

    #[test]
    fn office_documents_are_not_archives() {
        assert_eq!(
            classify_kind(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "doc.docx"
            ),
            FileKind::Document
        );
        assert_eq!(
            classify_kind("application/octet-stream", "doc.docx"),
            FileKind::Document
        );
    }

    #[test]
    fn extension_fallback_for_opaque_mime() {
        assert_eq!(
            classify_kind("application/octet-stream", "song.mp3"),
            FileKind::Media
        );
        assert_eq!(
            classify_kind("application/octet-stream", "pack.7z"),
            FileKind::Archive
        );
        assert_eq!(
            classify_kind("application/octet-stream", "photo.heic"),
            FileKind::Image
        );
    }

    #[test]
    fn file_name_shape() {
        assert!(validate_file_name("cat photo.png").is_ok());
        assert!(validate_file_name("..").is_err());
        assert!(validate_file_name("a/b").is_err());
        assert!(validate_file_name("a\\b").is_err());
        assert!(validate_file_name("").is_err());
        assert!(validate_file_name(&"x".repeat(256)).is_err());
        assert!(validate_file_name(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn dto_url_uses_the_storage_key() {
        let record = FileRecord {
            id: Uuid::new_v4(),
            owner_id: Uuid::new_v4(),
            parent_id: None,
            name: "hash".to_owned(),
            kind: FileKind::Image,
            mime: "image/webp".to_owned(),
            size: 10,
            sha256: "abc".to_owned(),
            storage_backend: "local".to_owned(),
            storage_key: "deadbeef".to_owned(),
            thumbnail_key: None,
            original_name: "cat.png".to_owned(),
            created_at: Timestamp::now(),
            deleted_at: None,
        };
        assert_eq!(FileDto::from(record).url, "/media/deadbeef");
    }
}
