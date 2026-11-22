//! The upload pipeline — pure bytes-to-bytes logic, mirroring the legacy
//! engine's `media::set` behavior:
//!
//! 1. enforce the configured size limit (`413`);
//! 2. hash the content (hex SHA-256 — the dedupe key);
//! 3. WebP auto-convert when enabled: still WebP passes through, animated
//!    GIF becomes animated WebP (frame delays carried over), every other
//!    decodable image becomes still WebP (quality 100, like before);
//! 4. classify the file and pick the stored MIME type;
//! 5. images get a 128 px WebP thumbnail.
//!
//! One deliberate widening versus the legacy engine: non-image bytes no
//! longer fail the upload — they pass through unconverted. The schema
//! already carries `document`/`archive` kinds for the netdisk waves; the
//! pipeline honors them from day one.

use bytes::Bytes;
use i0721_proto::{classify_kind, FileKind};
use image::{AnimationDecoder, ImageFormat};
use sha2::{Digest, Sha256};

/// The upload-relevant slice of the site configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRules {
    /// Byte limit for one upload.
    pub size_limit: u64,
    /// Whether images are auto-converted to WebP.
    pub webp_auto_convert: bool,
}

/// The product of the pipeline, ready for blob storage and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedUpload {
    /// The (possibly converted) bytes to store.
    pub data: Bytes,
    /// MIME type of the stored bytes.
    pub mime: String,
    /// Hex SHA-256 of the stored bytes.
    pub sha256: String,
    /// Coarse content class.
    pub kind: FileKind,
    /// WebP thumbnail bytes for images.
    pub thumbnail: Option<Bytes>,
}

/// Why the pipeline rejected an upload.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The upload exceeds the configured limit.
    #[error("upload is {actual} bytes, the limit is {limit}")]
    TooLarge {
        /// The configured limit.
        limit: u64,
        /// The actual size.
        actual: u64,
    },
    /// The bytes claim to be an image but cannot be decoded.
    #[error("cannot decode the uploaded image: {0}")]
    Decode(String),
    /// Re-encoding to WebP failed.
    #[error("cannot encode WebP: {0}")]
    Encode(String),
}

/// Thumbnail edge in pixels (the legacy value).
pub const THUMBNAIL_EDGE: u32 = 128;

/// Run the pipeline.
///
/// # Errors
///
/// [`PipelineError`] as documented on each variant.
pub fn process_upload(
    original: &[u8],
    original_name: Option<&str>,
    rules: &UploadRules,
) -> Result<ProcessedUpload, PipelineError> {
    if original.len() as u64 > rules.size_limit {
        return Err(PipelineError::TooLarge {
            limit: rules.size_limit,
            actual: original.len() as u64,
        });
    }

    let detected = image::guess_format(original).ok();
    let (data, mime, is_image) = match detected {
        Some(ImageFormat::WebP) => (
            Bytes::copy_from_slice(original),
            "image/webp".to_owned(),
            true,
        ),
        Some(ImageFormat::Gif) if rules.webp_auto_convert => (
            gif_to_animated_webp(original)?,
            "image/webp".to_owned(),
            true,
        ),
        Some(format) if rules.webp_auto_convert && is_decodable_image(format) => (
            image_to_webp(original, format)?,
            "image/webp".to_owned(),
            true,
        ),
        Some(_) => {
            // A known image format with auto-convert off: store as-is.
            let mime = mime_of(original_name, None);
            (Bytes::copy_from_slice(original), mime, true)
        }
        None => {
            // Not an image (or unknown): pass through, classified by name.
            let mime = mime_of(original_name, None);
            (Bytes::copy_from_slice(original), mime, false)
        }
    };

    let sha256 = hex_sha256(&data);
    let kind = classify_kind(&mime, original_name.unwrap_or_default());
    let thumbnail = if is_image && kind == FileKind::Image {
        Some(make_thumbnail(&data)?)
    } else {
        None
    };

    Ok(ProcessedUpload {
        data,
        mime,
        sha256,
        kind,
        thumbnail,
    })
}

/// Formats `image::load_from_memory` is expected to decode with the
/// workspace feature set (gif/jpeg/png/webp).
fn is_decodable_image(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Gif
            | ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Tiff
    )
}

fn gif_to_animated_webp(original: &[u8]) -> Result<Bytes, PipelineError> {
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(original))
        .map_err(|err| PipelineError::Decode(err.to_string()))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|err| PipelineError::Decode(err.to_string()))?;
    let first = frames
        .first()
        .ok_or_else(|| PipelineError::Decode("GIF has no frames".to_owned()))?;
    let (width, height) = (first.buffer().width(), first.buffer().height());

    let mut encoder = webp_animation::Encoder::new((width, height)).map_err(encode_failure)?;
    let mut timeline = 0_f32;
    for frame in &frames {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = numer as f32 / denom as f32;
        encoder
            .add_frame(frame.buffer(), timeline as i32)
            .map_err(encode_failure)?;
        timeline += delay;
    }
    let encoded = encoder.finalize(timeline as i32).map_err(encode_failure)?;
    Ok(Bytes::from(encoded.to_vec()))
}

fn image_to_webp(original: &[u8], format: ImageFormat) -> Result<Bytes, PipelineError> {
    debug_assert!(format != ImageFormat::WebP);
    let decoded = image::load_from_memory_with_format(original, format)
        .map_err(|err| PipelineError::Decode(err.to_string()))?;
    let encoder = webp::Encoder::from_image(&decoded).map_err(encode_failure)?;
    let encoded = encoder.encode(100.0);
    Ok(Bytes::from(encoded.to_vec()))
}

fn make_thumbnail(data: &[u8]) -> Result<Bytes, PipelineError> {
    let image =
        image::load_from_memory(data).map_err(|err| PipelineError::Decode(err.to_string()))?;
    let (old_width, old_height) = (image.width(), image.height());
    // Legacy geometry: the longer edge becomes 128 px.
    let (width, height) = if old_width >= old_height {
        (
            THUMBNAIL_EDGE,
            THUMBNAIL_EDGE * old_height / old_width.max(1),
        )
    } else {
        (
            THUMBNAIL_EDGE * old_width / old_height.max(1),
            THUMBNAIL_EDGE,
        )
    };
    let thumb = image::imageops::thumbnail(&image, width.max(1), height.max(1));
    let dynamic = image::DynamicImage::ImageRgba8(thumb);
    let encoder = webp::Encoder::from_image(&dynamic).map_err(encode_failure)?;
    let encoded = encoder.encode(100.0);
    Ok(Bytes::from(encoded.to_vec()))
}

/// The media route's lazy path: regenerate a thumbnail from stored blob
/// bytes (same geometry and quality as the upload-time thumbnail).
///
/// # Errors
///
/// [`PipelineError::Decode`]/[`PipelineError::Encode`] when the blob
/// cannot be decoded or re-encoded.
pub fn thumbnail_for_media(data: &[u8]) -> Result<Bytes, PipelineError> {
    make_thumbnail(data)
}

fn mime_of(original_name: Option<&str>, fallback: Option<&str>) -> String {
    if let Some(hint) = original_name.and_then(|name| mime_guess::from_path(name).first()) {
        return hint.essence_str().to_owned();
    }
    if let Some(hint) = fallback {
        return hint.to_owned();
    }
    "application/octet-stream".to_owned()
}

fn encode_failure(err: impl std::fmt::Display) -> PipelineError {
    PipelineError::Encode(err.to_string())
}

fn hex_sha256(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use i0721_proto::FileKind;

    /// A 4×4 red PNG.
    fn tiny_png() -> Vec<u8> {
        let image = image::DynamicImage::new_rgb8(4, 4);
        let mut bytes = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// A 2-frame GIF (4×4).
    fn tiny_gif() -> Vec<u8> {
        use image::{Delay, Frame};
        let frame = |color: [u8; 4]| {
            let mut buffer = image::RgbaImage::new(4, 4);
            for px in buffer.pixels_mut() {
                *px = image::Rgba(color);
            }
            Frame::from_parts(buffer, 0, 0, Delay::from_numer_denom_ms(100, 1))
        };
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode_frames([frame([255, 0, 0, 255]), frame([0, 0, 255, 255])])
            .unwrap();
        bytes
    }

    fn rules(limit: u64, convert: bool) -> UploadRules {
        UploadRules {
            size_limit: limit,
            webp_auto_convert: convert,
        }
    }

    #[test]
    fn png_becomes_webp_with_thumbnail() {
        let out = process_upload(&tiny_png(), Some("cat.png"), &rules(1024 * 1024, true))
            .expect("pipeline");
        assert_eq!(out.mime, "image/webp");
        assert_eq!(out.kind, FileKind::Image);
        assert_eq!(out.sha256.len(), 64);
        assert_eq!(image::guess_format(&out.data).unwrap(), ImageFormat::WebP);
        let thumb = out.thumbnail.expect("image gets a thumbnail");
        assert_eq!(image::guess_format(&thumb).unwrap(), ImageFormat::WebP);
    }

    #[test]
    fn gif_becomes_animated_webp() {
        let out = process_upload(&tiny_gif(), Some("loop.gif"), &rules(1024 * 1024, true))
            .expect("pipeline");
        assert_eq!(out.mime, "image/webp");
        assert_eq!(out.kind, FileKind::Image);
        assert!(out.thumbnail.is_some());
    }

    #[test]
    fn webp_passes_through() {
        let webp = process_upload(&tiny_png(), None, &rules(1024 * 1024, true))
            .unwrap()
            .data;
        let out = process_upload(&webp, Some("cat.webp"), &rules(1024 * 1024, true)).unwrap();
        assert_eq!(out.data, webp, "already-WebP content is not re-encoded");
    }

    #[test]
    fn conversion_off_keeps_original() {
        let png = tiny_png();
        let out = process_upload(&png, Some("cat.png"), &rules(1024 * 1024, false)).unwrap();
        assert_eq!(out.mime, "image/png");
        assert_eq!(out.data.as_ref(), png.as_slice());
    }

    #[test]
    fn non_image_passes_through_and_classifies() {
        let pdf: &[u8] = b"%PDF-1.4 fake document";
        let out = process_upload(pdf, Some("doc.pdf"), &rules(1024, true)).unwrap();
        assert_eq!(out.mime, "application/pdf");
        assert_eq!(out.kind, FileKind::Document);
        assert!(out.thumbnail.is_none());
    }

    #[test]
    fn unknown_name_defaults_to_octet_stream_other() {
        let out = process_upload(b"\x00\x01\x02", Some("blob.dat"), &rules(1024, true)).unwrap();
        assert_eq!(out.mime, "application/octet-stream");
        assert_eq!(out.kind, FileKind::Other);
    }

    #[test]
    fn oversize_is_rejected_before_any_decoding() {
        let err = process_upload(&[0_u8; 16], None, &rules(8, true)).unwrap_err();
        assert!(matches!(
            err,
            PipelineError::TooLarge {
                limit: 8,
                actual: 16
            }
        ));
    }

    #[test]
    fn corrupt_image_bytes_are_rejected() {
        // PNG magic followed by garbage: detectable as PNG, undecodable.
        let corrupt = {
            let mut bytes = tiny_png();
            bytes.truncate(bytes.len() / 2);
            bytes[20] ^= 0xff;
            bytes
        };
        let err = process_upload(&corrupt, Some("cat.png"), &rules(1024 * 1024, true)).unwrap_err();
        assert!(matches!(err, PipelineError::Decode(_)));
    }

    #[test]
    fn sha256_is_hex_of_the_stored_bytes() {
        let out = process_upload(b"hello", Some("a.txt"), &rules(1024, true)).unwrap();
        assert_eq!(
            out.sha256,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
