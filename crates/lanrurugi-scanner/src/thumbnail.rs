//! Thumbnail generation (research.md §4): `image` crate for decode/resize, dispatched via
//! `rayon`/`spawn_blocking` per constitution Principle III — never run inline in an async handler.
//!
//! Legacy resizes to a fixed height of 500px (verified: `Utils/Archive.pm::generate_thumbnail`).
//! Format (JPEG or WebP) and quality are a library-wide setting (`enablewebp`/`webpquality`/
//! `hqthumbpages` in `lanrurugi-api::settings`, read via [`read_settings`]) rather than baked in
//! here — this module just encodes whatever [`ThumbFormat`] and quality it's given. Cover
//! thumbnails land at `<thumb_dir>/<id[0:2]>/<id>.<ext>`; this module writes that file —
//! sharding/placement itself is the caller's concern (`lanrurugi-api::archives::get_archive_thumbnail`
//! reads it back).

use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::ImageEncoder;
use lanrurugi_core::concurrency::{parallel_map, run_blocking, BlockingTaskError};
use sha1::{Digest, Sha1};
use thiserror::Error;

use crate::archive_format::{self, ArchiveFormatError};

const THUMBNAIL_HEIGHT: u32 = 500;

/// Legacy's own HQ-switch quality numbers (`Utils/Archive.pm::generate_thumbnail`: `quality = 50`
/// normally / `80` for `use_hq`), reused verbatim for the JPEG format's `hqthumbpages` setting.
pub const JPEG_QUALITY_NORMAL: u8 = 50;
pub const JPEG_QUALITY_HQ: u8 = 80;

/// Which codec a thumbnail is written/served as. Format is a per-library-wide setting
/// (`enablewebp` in `lanrurugi-api::settings`), not per-file, so every thumbnail on disk is the
/// same format at any given time — switching it triggers a full regen rather than leaving a mix
/// behind (constitution: keep on-disk state unambiguous for the read-back path in
/// `archives::get_archive_thumbnail` to probe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbFormat {
    Jpeg,
    Webp,
}

impl ThumbFormat {
    /// Read-back probe order (`archives::get_archive_thumbnail`,
    /// `tankoubons::get_tankoubon_thumbnail`): WebP first since it's the default going forward,
    /// JPEG second for thumbnails written before a format switch's regen job has caught up.
    pub const ALL: [ThumbFormat; 2] = [ThumbFormat::Webp, ThumbFormat::Jpeg];

    pub const fn extension(self) -> &'static str {
        match self {
            ThumbFormat::Jpeg => "jpg",
            ThumbFormat::Webp => "webp",
        }
    }

    pub const fn content_type(self) -> &'static str {
        match self {
            ThumbFormat::Jpeg => "image/jpeg",
            ThumbFormat::Webp => "image/webp",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThumbSettings {
    pub format: ThumbFormat,
    pub quality: u8,
}

/// Reads the live `enablewebp`/`webpquality`/`hqthumbpages` values from the same `LRR_CONFIG`
/// hash `lanrurugi-api::settings` reads/writes, so a setting change takes effect on the very next
/// thumbnail generated without a server restart. Missing/unparsable values fall back to the same
/// defaults `settings::get_settings` reports (`enablewebp = true`, `webpquality = 85`,
/// `hqthumbpages = false`).
/// `lanrurugi-api::settings::DEFAULT_WEBP_QUALITY`'s own value — duplicated here (rather than
/// imported) since `lanrurugi-scanner` can't depend on `lanrurugi-api` (the dependency runs the
/// other way), but the two must still be kept in sync by hand if the Settings page's own default
/// ever changes.
const DEFAULT_WEBP_QUALITY: u8 = 85;

pub async fn read_settings<C>(conn: &mut C) -> ThumbSettings
where
    C: deadpool_redis::redis::aio::ConnectionLike + Send + Sync,
{
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_storage::keys::CONFIG_KEY;

    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    let enablewebp = fields.get("enablewebp").map(|v| v != "0").unwrap_or(true);
    let hqthumbpages = fields
        .get("hqthumbpages")
        .map(|v| v != "0")
        .unwrap_or(false);
    let webpquality: u8 = fields
        .get("webpquality")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_WEBP_QUALITY);

    if enablewebp {
        ThumbSettings {
            format: ThumbFormat::Webp,
            quality: webpquality,
        }
    } else {
        ThumbSettings {
            format: ThumbFormat::Jpeg,
            quality: if hqthumbpages {
                JPEG_QUALITY_HQ
            } else {
                JPEG_QUALITY_NORMAL
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum ThumbnailError {
    #[error("archive read error: {0}")]
    Archive(#[from] ArchiveFormatError),
    #[error("patch read error: {0}")]
    Patch(#[from] crate::patch::PatchError),
    #[error("archive has no pages to thumbnail")]
    NoPages,
    #[error("failed to decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task failed: {0}")]
    Join(#[from] lanrurugi_core::concurrency::BlockingTaskError),
    #[error("webp encode error: {0:?}")]
    Webp(webp::WebPEncodingError),
}

/// Generates a thumbnail from `page` (1-indexed) of `archive_path` and writes it to
/// `output_path`, resizing to `THUMBNAIL_HEIGHT` px tall, preserving aspect ratio. When `page`
/// is the cover (`1`), also returns the SHA-1 hex digest of the *raw, pre-resize* source image
/// bytes — matches legacy's `thumbhash` exactly (`Utils/Archive.pm::extract_thumbnail`:
/// `shasum_str($arcimg, 1)` on the extracted cover image, hashed before any resizing), which
/// `lanrurugi-api`'s duplicate-detection scan compares via Hamming distance across archives.
pub async fn generate(
    archive_path: PathBuf,
    page: usize,
    output_path: PathBuf,
    format: ThumbFormat,
    quality: u8,
) -> Result<Option<String>, ThumbnailError> {
    run_blocking(move || generate_sync(&archive_path, page, &output_path, format, quality)).await?
}

/// Like [`generate`], but for many `(archive_path, page, output_path)` jobs at once, run across
/// rayon's whole thread pool in one batch rather than one `spawn_blocking` round-trip per job —
/// for a library-wide regen or a many-page archive, the difference is real parallelism (multiple
/// images decoding/resizing/encoding at once) instead of serialized one-at-a-time throughput.
/// Results come back in the same order as `jobs`.
pub async fn generate_batch(
    jobs: Vec<(PathBuf, usize, PathBuf)>,
    format: ThumbFormat,
    quality: u8,
) -> Result<Vec<Result<Option<String>, ThumbnailError>>, BlockingTaskError> {
    parallel_map(jobs, move |(archive_path, page, output_path)| {
        generate_sync(&archive_path, page, &output_path, format, quality)
    })
    .await
}

fn generate_sync(
    archive_path: &Path,
    page: usize,
    output_path: &Path,
    format: ThumbFormat,
    quality: u8,
) -> Result<Option<String>, ThumbnailError> {
    // Merges in a sidecar `.patch.zip`'s own pages, if one exists (`crate::patch`, issue #77's own
    // follow-on design) — this is the shared thumbnail-generation path (cover, per-page, and
    // batch regen all go through here), so a patched page gets the exact same thumbnail treatment
    // as an original one rather than the overview grid (the one caller that actually surfaces
    // per-page thumbnails to a user) silently having no thumbnail for it.
    let original_pages = archive_format::list_pages(archive_path)?;
    let effective = crate::patch::effective_pages(archive_path, &original_pages);
    let entry = effective
        .get(page.saturating_sub(1))
        .ok_or(ThumbnailError::NoPages)?;
    let bytes = crate::patch::read_page(archive_path, entry)?;

    let cover_hash = (page == 1).then(|| {
        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        hex_encode(&hasher.finalize())
    });

    let img = image::load_from_memory(&bytes)?;
    let ratio = THUMBNAIL_HEIGHT as f64 / img.height() as f64;
    let target_width = (img.width() as f64 * ratio).round() as u32;
    let resized = img.resize(
        target_width,
        THUMBNAIL_HEIGHT,
        image::imageops::FilterType::Lanczos3,
    );

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rgb = resized.to_rgb8();
    match format {
        ThumbFormat::Jpeg => {
            let mut out = std::fs::File::create(output_path)?;
            let encoder = JpegEncoder::new_with_quality(&mut out, quality);
            encoder.write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        ThumbFormat::Webp => {
            let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
            let encoded = encoder
                .encode_simple(false, quality as f32)
                .map_err(ThumbnailError::Webp)?;
            std::fs::write(output_path, &*encoded)?;
        }
    }
    Ok(cover_hash)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_test_zip_with_image() -> tempfile::NamedTempFile {
        make_test_zip_with_image_format("page1.jpg", "page2.jpg", image::ImageFormat::Jpeg)
    }

    /// Builds a single-entry-pair archive whose pages are encoded as `format` (source format
    /// entering [`generate`], distinct from the *output* `ThumbFormat` under test) — used to
    /// verify decode-then-re-encode-to-webp works for arbitrary source formats, not just JPEG.
    fn make_test_zip_with_image_format(
        name1: &str,
        name2: &str,
        format: image::ImageFormat,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();

        for (name, color) in [(name1, [255, 0, 0]), (name2, [0, 255, 0])] {
            writer.start_file(name, options).unwrap();
            let img = image::RgbImage::from_pixel(200, 300, image::Rgb(color));
            let mut bytes = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut bytes), format)
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[tokio::test]
    async fn generates_a_resized_jpeg_thumbnail() {
        let archive = make_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jpg");

        generate(
            archive.path().to_path_buf(),
            1,
            output.clone(),
            ThumbFormat::Jpeg,
            JPEG_QUALITY_NORMAL,
        )
        .await
        .unwrap();

        let generated = image::open(&output).unwrap();
        assert_eq!(generated.height(), THUMBNAIL_HEIGHT);
    }

    #[tokio::test]
    async fn generates_a_resized_webp_thumbnail() {
        let archive = make_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.webp");

        generate(
            archive.path().to_path_buf(),
            1,
            output.clone(),
            ThumbFormat::Webp,
            85,
        )
        .await
        .unwrap();

        let generated = image::open(&output).unwrap();
        assert_eq!(generated.height(), THUMBNAIL_HEIGHT);
    }

    #[tokio::test]
    async fn converts_arbitrary_source_formats_to_webp() {
        // A source archive's pages can be any format `archive_format::is_image_name` accepts
        // (png/jpg/gif/bmp/webp/...) — thumbnail generation must decode whichever one it finds
        // and re-encode it as the *configured* thumbnail format, not just pass through JPEG.
        for (ext, format) in [
            ("png", image::ImageFormat::Png),
            ("gif", image::ImageFormat::Gif),
            ("bmp", image::ImageFormat::Bmp),
            ("webp", image::ImageFormat::WebP),
        ] {
            let name1 = format!("page1.{ext}");
            let name2 = format!("page2.{ext}");
            let archive = make_test_zip_with_image_format(&name1, &name2, format);
            let out_dir = tempfile::tempdir().unwrap();
            let output = out_dir.path().join("thumb.webp");

            generate(
                archive.path().to_path_buf(),
                1,
                output.clone(),
                ThumbFormat::Webp,
                85,
            )
            .await
            .unwrap_or_else(|e| panic!("generate failed for {ext} source: {e}"));

            let generated = image::open(&output)
                .unwrap_or_else(|e| panic!("output for {ext} source isn't a valid image: {e}"));
            assert_eq!(generated.height(), THUMBNAIL_HEIGHT);
        }
    }

    #[tokio::test]
    async fn cover_page_returns_a_sha1_hex_digest_of_the_source_bytes() {
        let archive = make_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jpg");

        let hash = generate(
            archive.path().to_path_buf(),
            1,
            output,
            ThumbFormat::Jpeg,
            JPEG_QUALITY_NORMAL,
        )
        .await
        .unwrap();

        let hash = hash.expect("page 1 is the cover and must return a hash");
        assert_eq!(hash.len(), 40, "SHA-1 hex digest is 40 characters");
    }

    #[tokio::test]
    async fn non_cover_pages_return_no_hash() {
        let archive = make_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jpg");

        let hash = generate(
            archive.path().to_path_buf(),
            2,
            output,
            ThumbFormat::Jpeg,
            JPEG_QUALITY_NORMAL,
        )
        .await
        .unwrap();
        assert!(hash.is_none());
    }

    /// The "some page's image bytes are corrupt" case (issue #2) — distinct from an unreadable
    /// *archive container* (`archive_format::list_pages` itself failing, which
    /// `full_scan::heal_pagecounts` handles): the ZIP structure is perfectly intact and
    /// `list_pages` finds a real entry with an image extension, but that entry's bytes aren't a
    /// decodable image at all. Must surface as `ThumbnailError::Decode`, not `Archive`/`Io`/`Join`
    /// — `archives::generate_page_thumbnails` only marks a page `corrupted_pages` on a `Decode`
    /// error specifically, so other failure kinds don't wrongly stick a permanent placeholder onto
    /// a perfectly good page.
    #[tokio::test]
    async fn corrupt_page_bytes_fail_with_a_decode_error() {
        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer.start_file("page1.jpg", options).unwrap();
        use std::io::Write;
        writer.write_all(b"not actually a jpeg").unwrap();
        writer.finish().unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jpg");

        let result = generate(
            file.path().to_path_buf(),
            1,
            output,
            ThumbFormat::Jpeg,
            JPEG_QUALITY_NORMAL,
        )
        .await;

        assert!(
            matches!(result, Err(ThumbnailError::Decode(_))),
            "expected a Decode error for genuinely undecodable image bytes, got {result:?}"
        );
    }
}
