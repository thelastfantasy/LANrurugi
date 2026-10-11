//! Thumbnail generation (research.md §4): `image` crate for decode/resize, dispatched via
//! `rayon`/`spawn_blocking` per constitution Principle III — never run inline in an async handler.
//!
//! Legacy resizes to a fixed height of 500px (verified: `Utils/Archive.pm::generate_thumbnail`).
//! Format (JPEG / WebP / JPEG XL) and quality/effort are library-wide settings (`enablewebp`/
//! `jxlthumbpages`/`webpquality` in `lanrurugi-api::settings`, read via [`read_settings`]) rather
//! than baked in here — this module just encodes whatever [`ThumbFormat`]
//! and quality it's given. Cover
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

/// The target edge every thumbnail is resized *down* to (legacy: `Utils/Archive.pm::generate_thumbnail`
/// resizes to a fixed *height*), the other edge following the source's aspect ratio. `pub` so
/// callers naming a saved thumbnail after the target edge (`_t500.jpg`) can't drift from the number
/// generation actually uses.
///
/// A source shorter than this keeps its own height (never enlarged), and a source whose bytes the
/// target edge can't beat gets a smaller one — so the `_t500` name is the library's thumbnail
/// standard, not a promise about every stored file's exact dimensions.
pub const THUMBNAIL_HEIGHT: u32 = 500;

/// Quality steps tried after the configured one, in order, when an attempt encodes to at least as
/// many bytes as the source page — see [`generate`]'s own docs on the size invariant.
const QUALITY_FALLBACKS: [u8; 3] = [70, 50, 30];

/// The lowest target edge the size invariant's last-resort shrink will go to. Only reached by a
/// source whose own bytes are already at the encoder's floor.
const MIN_THUMBNAIL_HEIGHT: u32 = 96;

/// Which codec a thumbnail is written/served as. Format is a per-library-wide setting
/// (`enablewebp` in `lanrurugi-api::settings`), not per-file, so every thumbnail on disk is the
/// same format at any given time — switching it triggers a full regen rather than leaving a mix
/// behind (constitution: keep on-disk state unambiguous for the read-back path in
/// `archives::get_archive_thumbnail` to probe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbFormat {
    Jpeg,
    Webp,
    Jxl,
}

impl ThumbFormat {
    /// Every on-disk format, used by cleanup/regen sweeps. Per-request read order is now chosen by
    /// the client's advertised capability (`archives::thumbnail_formats_for`) rather than this
    /// constant, because JXL must only be offered to browsers that can decode it.
    pub const ALL: [ThumbFormat; 3] = [ThumbFormat::Jxl, ThumbFormat::Webp, ThumbFormat::Jpeg];

    pub const fn extension(self) -> &'static str {
        match self {
            ThumbFormat::Jpeg => "jpg",
            ThumbFormat::Webp => "webp",
            ThumbFormat::Jxl => "jxl",
        }
    }

    pub const fn content_type(self) -> &'static str {
        match self {
            ThumbFormat::Jpeg => "image/jpeg",
            ThumbFormat::Webp => "image/webp",
            ThumbFormat::Jxl => "image/jxl",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThumbSettings {
    pub format: ThumbFormat,
    pub quality: u8,
}

/// Reads the live `enablewebp`/`jxlthumbpages`/`webpquality` values from the same `LRR_CONFIG`
/// hash `lanrurugi-api::settings` reads/writes, so a setting change takes effect on the very next
/// thumbnail generated without a server restart. Missing/unparsable values fall back to the same
/// defaults `settings::get_settings` reports (`enablewebp = true`, `jxlthumbpages = true`,
/// `webpquality = 85`).
///
/// The one quality setting covers all three output formats: JPEG and WebP as encoder quality, JXL
/// as encoding effort (its output is lossless). Legacy's separate `hqthumbpages` toggle — JPEG page
/// thumbnails at quality 50 instead of the cover's 80, meaningless for the other two codecs — was
/// removed rather than kept as a control that does nothing whenever the format isn't JPEG.
///
/// `lanrurugi-api::settings::DEFAULT_WEBP_QUALITY`'s own value — duplicated here (rather than
/// imported) since `lanrurugi-scanner` can't depend on `lanrurugi-api` (the dependency runs the
/// other way), but the two must still be kept in sync by hand if the Settings page's own default
/// ever changes. `pub` so the bench and this module's own tests can generate at the real default.
pub const DEFAULT_WEBP_QUALITY: u8 = 85;

pub async fn read_settings<C>(conn: &mut C) -> ThumbSettings
where
    C: deadpool_redis::redis::aio::ConnectionLike + Send + Sync,
{
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_storage::keys::CONFIG_KEY;

    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    settings_from_fields(&fields)
}

/// The pure half of [`read_settings`] — which format to write and at what quality — split out so
/// the mapping is unit-testable without a Redis connection.
fn settings_from_fields(fields: &std::collections::HashMap<String, String>) -> ThumbSettings {
    let enablewebp = fields.get("enablewebp").map(|v| v != "0").unwrap_or(true);
    let jxlthumbpages = fields
        .get("jxlthumbpages")
        .map(|v| v != "0")
        .unwrap_or(true);
    let quality: u8 = fields
        .get("webpquality")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_WEBP_QUALITY);

    // Every format takes that one quality value: JPEG/WebP as encoder quality, JXL as encoding
    // effort (its output is lossless, so the reader's `readerquality`-style semantics don't apply).
    // A stale `hqthumbpages` key left behind by an older config or a legacy import is deliberately
    // ignored — see this module's own docs.
    let format = if jxlthumbpages {
        ThumbFormat::Jxl
    } else if enablewebp {
        ThumbFormat::Webp
    } else {
        ThumbFormat::Jpeg
    };
    ThumbSettings { format, quality }
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
    #[error("jxl encode error: {0}")]
    Jxl(#[from] crate::jxl::JxlEncodeError),
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

    let img = crate::image_decode::load_from_memory(&bytes)?;
    // A thumbnail exists to be *smaller* than the page it stands in for — in pixels and in bytes.
    // Both halves of that are enforced here, because neither is guaranteed by the encoder:
    //
    //   * `min(THUMBNAIL_HEIGHT, height)` never enlarges. Legacy's ImageMagick geometry
    //     (`500x1000`, `Utils/ImageMagickResizer.pm`) enlarges a source shorter than the target
    //     edge, which adds no detail and can easily outweigh the source once re-encoded.
    //   * every candidate must encode to fewer bytes than the page itself, or the reader would be
    //     served *more* data than the original for the same picture. Quality drops first (cheap,
    //     and the usual reason an attempt overshoots); if even the floor quality doesn't fit, the
    //     target edge halves and the ladder runs again.
    let source_len = bytes.len();
    let mut target_height = THUMBNAIL_HEIGHT.min(img.height()).max(1);
    let mut smallest: Option<Vec<u8>> = None;

    loop {
        let ratio = target_height as f64 / img.height() as f64;
        let target_width = ((img.width() as f64) * ratio).round().max(1.0) as u32;
        let rgb = if target_width == img.width() && target_height == img.height() {
            // Already at or under the target edge: re-encoding it at a different size would only
            // throw detail away, so this attempt uses the decoded pixels as-is.
            img.to_rgb8()
        } else {
            img.resize(
                target_width,
                target_height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgb8()
        };

        for attempt_quality in std::iter::once(quality).chain(QUALITY_FALLBACKS.iter().copied()) {
            let encoded = encode_thumbnail(&rgb, format, attempt_quality)?;
            let fits = encoded.len() < source_len;
            let is_smallest = smallest.as_ref().is_none_or(|s| encoded.len() < s.len());
            if is_smallest {
                // Keep the smallest thing produced so far: if every attempt overshoots (a source of
                // a few hundred bytes, already at the encoder's own floor), storing the smallest
                // attempt is still the best available answer.
                smallest = Some(encoded);
            }
            if fits {
                break;
            }
        }
        if smallest.as_ref().is_some_and(|s| s.len() < source_len) {
            break;
        }
        if target_height <= MIN_THUMBNAIL_HEIGHT {
            break;
        }
        target_height = (target_height / 2).max(MIN_THUMBNAIL_HEIGHT);
    }

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, smallest.ok_or(ThumbnailError::NoPages)?)?;
    Ok(cover_hash)
}

/// Encodes `rgb` as `format` at `quality`, in memory — so a candidate that turns out larger than
/// the source page can be discarded instead of already sitting on disk.
fn encode_thumbnail(
    rgb: &image::RgbImage,
    format: ThumbFormat,
    quality: u8,
) -> Result<Vec<u8>, ThumbnailError> {
    match format {
        ThumbFormat::Jpeg => {
            let mut out = Vec::new();
            let encoder = JpegEncoder::new_with_quality(&mut out, quality);
            encoder.write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
            Ok(out)
        }
        ThumbFormat::Webp => {
            let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
            let encoded = encoder
                .encode_simple(false, quality as f32)
                .map_err(ThumbnailError::Webp)?;
            Ok(encoded.to_vec())
        }
        ThumbFormat::Jxl => crate::jxl::encode_rgb(rgb, quality).map_err(ThumbnailError::Jxl),
    }
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

    /// A source comfortably *larger* than [`THUMBNAIL_HEIGHT`], so the resize-on-the-way-down path
    /// is what the assertions exercise (the default 200×300 fixture is smaller than the target edge
    /// and must never be enlarged — see that test).
    fn make_large_test_zip_with_image() -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (name, color) in [("page1.jpg", [255, 0, 0]), ("page2.jpg", [0, 255, 0])] {
            writer.start_file(name, options).unwrap();
            let img = image::RgbImage::from_pixel(900, 1200, image::Rgb(color));
            let mut bytes = Vec::new();
            img.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    /// An archive whose page is a *tiny* file (a flat colour at low JPEG quality comes out to a few
    /// hundred bytes) plus that page's own byte length — the case the size invariant exists for:
    /// re-encoding it as any thumbnail format at the configured quality easily produces *more*
    /// bytes than the page itself. Returns `(archive, page_bytes)`.
    fn make_tiny_source_zip() -> (tempfile::NamedTempFile, usize) {
        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        let img = image::RgbImage::from_pixel(200, 300, image::Rgb([12, 34, 56]));
        let mut page = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut page, 30);
        encoder
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        writer.start_file("page1.jpg", options).unwrap();
        writer.write_all(&page).unwrap();
        writer.finish().unwrap();
        let len = page.len();
        (file, len)
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

    /// The quality/format mapping (and the removal of the legacy `hqthumbpages` toggle): the one
    /// quality setting covers all three formats, and a stale `hqthumbpages` key changes nothing.
    #[test]
    fn every_thumbnail_format_takes_the_one_quality_setting() {
        let fields = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<std::collections::HashMap<String, String>>()
        };

        // Empty config: this repo's own defaults (JXL, default quality).
        let defaults = settings_from_fields(&fields(&[]));
        assert_eq!(defaults.format, ThumbFormat::Jxl);
        assert_eq!(defaults.quality, DEFAULT_WEBP_QUALITY);

        // WebP, then plain JPEG (both modern formats off) — same quality value in each case.
        for (pairs, expected) in [
            (
                vec![("jxlthumbpages", "0"), ("webpquality", "70")],
                ThumbFormat::Webp,
            ),
            (
                vec![
                    ("jxlthumbpages", "0"),
                    ("enablewebp", "0"),
                    ("webpquality", "70"),
                ],
                ThumbFormat::Jpeg,
            ),
        ] {
            let settings = settings_from_fields(&fields(&pairs));
            assert_eq!(settings.format, expected);
            assert_eq!(
                settings.quality, 70,
                "{expected:?} takes the same quality setting"
            );
        }

        // The removed legacy toggle is inert, whichever way it was left set.
        for stale in ["0", "1"] {
            let settings = settings_from_fields(&fields(&[
                ("jxlthumbpages", "0"),
                ("enablewebp", "0"),
                ("webpquality", "70"),
                ("hqthumbpages", stale),
            ]));
            assert_eq!(settings.format, ThumbFormat::Jpeg);
            assert_eq!(
                settings.quality, 70,
                "stale hqthumbpages={stale} must be ignored"
            );
        }
    }

    #[tokio::test]
    async fn generates_a_resized_jpeg_thumbnail() {
        let archive = make_large_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jpg");

        generate(
            archive.path().to_path_buf(),
            1,
            output.clone(),
            ThumbFormat::Jpeg,
            DEFAULT_WEBP_QUALITY,
        )
        .await
        .unwrap();

        let generated = image::open(&output).unwrap();
        assert_eq!(generated.height(), THUMBNAIL_HEIGHT);
    }

    #[tokio::test]
    async fn generates_a_resized_webp_thumbnail() {
        let archive = make_large_test_zip_with_image();
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
    async fn generates_a_resized_jxl_thumbnail() {
        let archive = make_large_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        let output = out_dir.path().join("thumb.jxl");

        generate(
            archive.path().to_path_buf(),
            1,
            output.clone(),
            ThumbFormat::Jxl,
            85,
        )
        .await
        .unwrap();

        let bytes = std::fs::read(&output).unwrap();
        let generated = crate::image_decode::load_from_memory(&bytes).unwrap();
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
            // The fixture is 200x300 — shorter than the target edge, so it keeps its own height
            // rather than being enlarged (see the dedicated test below).
            assert!(
                generated.height() <= 300,
                "{ext} source must not be enlarged"
            );
        }
    }

    /// A source shorter than the target edge keeps its own dimensions: enlarging it adds bytes
    /// without adding detail, and is exactly how a "smaller than the original" thumbnail stops
    /// being smaller than the original.
    #[tokio::test]
    async fn a_source_smaller_than_the_target_edge_is_never_enlarged() {
        let archive = make_test_zip_with_image();
        let out_dir = tempfile::tempdir().unwrap();
        for (format, name) in [
            (ThumbFormat::Jpeg, "thumb.jpg"),
            (ThumbFormat::Webp, "thumb.webp"),
            (ThumbFormat::Jxl, "thumb.jxl"),
        ] {
            let output = out_dir.path().join(name);
            generate(
                archive.path().to_path_buf(),
                1,
                output.clone(),
                format,
                DEFAULT_WEBP_QUALITY,
            )
            .await
            .unwrap();
            let bytes = std::fs::read(&output).unwrap();
            let generated = crate::image_decode::load_from_memory(&bytes).unwrap();
            assert!(
                generated.height() <= 300,
                "{format:?} thumbnail of a 300px-tall source came out {}px tall",
                generated.height()
            );
        }
    }

    /// The invariant the whole pipeline exists for: the stored thumbnail must be *fewer bytes* than
    /// the page it replaces, whatever format and quality the library is configured for — otherwise
    /// the list/overview view would be served more data than the original page.
    #[tokio::test]
    async fn a_thumbnail_is_never_larger_than_the_page_it_replaces() {
        let (archive, page_bytes) = make_tiny_source_zip();
        let out_dir = tempfile::tempdir().unwrap();
        for (format, name) in [
            (ThumbFormat::Jpeg, "thumb.jpg"),
            (ThumbFormat::Webp, "thumb.webp"),
            (ThumbFormat::Jxl, "thumb.jxl"),
        ] {
            let output = out_dir.path().join(name);
            generate(
                archive.path().to_path_buf(),
                1,
                output.clone(),
                format,
                // The configured quality is deliberately the highest the UI allows, so the size
                // guard — not a lucky default — is what has to hold here.
                100,
            )
            .await
            .unwrap();
            let stored = std::fs::read(&output).unwrap();
            assert!(
                stored.len() < page_bytes,
                "{format:?} thumbnail is {} bytes for a {page_bytes}-byte page",
                stored.len()
            );
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
            DEFAULT_WEBP_QUALITY,
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
            DEFAULT_WEBP_QUALITY,
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
            DEFAULT_WEBP_QUALITY,
        )
        .await;

        assert!(
            matches!(result, Err(ThumbnailError::Decode(_))),
            "expected a Decode error for genuinely undecodable image bytes, got {result:?}"
        );
    }
}
