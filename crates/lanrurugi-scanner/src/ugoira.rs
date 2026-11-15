//! Pixiv `.ugoira` support.
//!
//! A `.ugoira` file is a ZIP of image frames plus an `animation.json` that carries each frame's
//! delay. Treating it as an ordinary archive would page through its frames as if they were
//! unrelated images; what the file actually is, is one animated picture. So [`crate::archive_format`]
//! reports a single synthetic page for it and composes that page here, through the same
//! [`webpx::AnimationEncoder`] the animated-image path already uses. The reader then plays it with
//! a plain `<img>`, exactly like an animated WebP/GIF.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use image::imageops::FilterType;
use serde::Deserialize;
use thiserror::Error;
use webpx::AnimationEncoder;
use zip::ZipArchive;

/// The single page a `.ugoira` file reports. The extension is what the API uses to pick a served
/// filename and a disposition; the bytes themselves are what decide the content type.
pub const PAGE_NAME: &str = "ugoira.webp";

/// CPU/time bound, mirroring the animated-image path's own frame cap.
const MAX_FRAMES: usize = 600;

/// Used when the manifest is missing or unreadable: Pixiv's tooling defaults to ~100 ms.
const DEFAULT_DELAY_MS: i32 = 100;

/// Frame delays below this are treated as a broken manifest rather than a 0 ms animation.
const MIN_DELAY_MS: i32 = 10;

/// A delay longer than this is almost certainly a unit mistake (seconds written as milliseconds).
const MAX_DELAY_MS: i32 = 10_000;

const QUALITY: f32 = 80.0;

/// Canvas bound: an ugoira is a small web animation, not a 4K video.
const MAX_CANVAS_PIXELS: u64 = 4096 * 4096;

#[derive(Debug, Error)]
pub enum UgoiraError {
    #[error("failed to read .ugoira: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to read .ugoira: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error(".ugoira contains no image frames")]
    NoFrames,
    #[error("failed to decode frame {frame} ({name}): {source}")]
    Decode {
        frame: usize,
        name: String,
        #[source]
        source: image::ImageError,
    },
    #[error("failed to encode animated WebP: {0}")]
    Encode(String),
    #[error("invalid animation.json: {0}")]
    Manifest(String),
}

pub fn is_ugoira_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ugoira"))
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    frames: Vec<ManifestFrame>,
}

#[derive(Debug, Deserialize)]
struct ManifestFrame {
    file: String,
    #[serde(default)]
    delay: Option<i64>,
    #[serde(default)]
    delay_ms: Option<i64>,
}

/// One frame to encode: the ZIP entry name and how long it stays on screen.
struct PlannedFrame {
    name: String,
    delay_ms: i32,
}

fn is_image_entry(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
    )
}

fn find_manifest(archive: &mut ZipArchive<File>) -> Option<usize> {
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry
            .name()
            .is_ok_and(|name| name.eq_ignore_ascii_case("animation.json"))
        {
            return Some(index);
        }
    }
    None
}

fn read_manifest(archive: &mut ZipArchive<File>, index: usize) -> Result<Manifest, UgoiraError> {
    let mut entry = archive.by_index(index)?;
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|error| UgoiraError::Manifest(error.to_string()))
}

/// The frames to encode, in playback order.
///
/// `animation.json` is the authority when it parses; otherwise every image entry, sorted by name
/// (Pixiv names frames `000000.jpg`, `000001.jpg`, …), at the default delay. A manifest that
/// names entries the ZIP does not contain is rejected rather than silently shortened — that would
/// play a different animation than the one the file describes.
fn plan_frames(archive: &mut ZipArchive<File>) -> Result<Vec<PlannedFrame>, UgoiraError> {
    if let Some(index) = find_manifest(archive) {
        if let Ok(manifest) = read_manifest(archive, index) {
            if !manifest.frames.is_empty() {
                let mut planned = Vec::with_capacity(manifest.frames.len());
                for frame in &manifest.frames {
                    let name = frame.file.trim_start_matches("./").to_string();
                    let delay = frame.delay.or(frame.delay_ms).unwrap_or(0);
                    planned.push(PlannedFrame {
                        name,
                        delay_ms: clamp_delay(delay),
                    });
                }
                return Ok(planned);
            }
        }
    }

    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        let Ok(name) = entry.name() else {
            continue;
        };
        let name = name.to_string();
        if is_image_entry(&name) {
            names.push(name);
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(UgoiraError::NoFrames);
    }
    Ok(names
        .into_iter()
        .map(|name| PlannedFrame {
            name,
            delay_ms: DEFAULT_DELAY_MS,
        })
        .collect())
}

fn clamp_delay(delay_ms: i64) -> i32 {
    let delay_ms = i32::try_from(delay_ms).unwrap_or(MAX_DELAY_MS);
    delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS)
}

/// Composes the archive's frames into one animated WebP, honouring `animation.json`'s delays.
pub fn compose(path: &Path) -> Result<Vec<u8>, UgoiraError> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let frames = plan_frames(&mut archive)?;
    let frames = &frames[..frames.len().min(MAX_FRAMES)];

    let mut encoder: Option<AnimationEncoder> = None;
    let mut canvas = (0u32, 0u32);
    let mut timestamp_ms = 0i32;

    for (position, planned) in frames.iter().enumerate() {
        let mut entry = archive.by_name(&planned.name).map_err(|error| {
            UgoiraError::Manifest(format!(
                "frame {position} names {:?}, which the archive cannot open: {error}",
                planned.name
            ))
        })?;
        let mut raw = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut raw)?;
        let decoded = image::load_from_memory(&raw)
            .map_err(|source| UgoiraError::Decode {
                frame: position,
                name: planned.name.clone(),
                source,
            })?
            .to_rgba8();

        let frame = if canvas == (0, 0) {
            canvas = decoded.dimensions();
            let pixels = u64::from(canvas.0) * u64::from(canvas.1);
            if pixels > MAX_CANVAS_PIXELS {
                return Err(UgoiraError::Manifest(format!(
                    "frame canvas {}x{} is larger than this server composes",
                    canvas.0, canvas.1
                )));
            }
            decoded
        } else if decoded.dimensions() == canvas {
            decoded
        } else {
            // Pixiv frames are all the same size; a stray smaller/larger one is padded by the
            // resize rather than rejected, so a slightly inconsistent file still plays.
            image::imageops::resize(&decoded, canvas.0, canvas.1, FilterType::Triangle)
        };

        let active = match encoder.as_mut() {
            Some(active) => active,
            None => {
                let mut created = AnimationEncoder::with_options(canvas.0, canvas.1, true, 0)
                    .map_err(|error| UgoiraError::Encode(error.to_string()))?;
                created.set_quality(QUALITY);
                encoder = Some(created);
                encoder.as_mut().expect("just created")
            }
        };
        active
            .add_frame_rgba(&frame, timestamp_ms)
            .map_err(|error| UgoiraError::Encode(error.to_string()))?;
        timestamp_ms = timestamp_ms.saturating_add(planned.delay_ms);
    }

    let encoder = encoder.ok_or(UgoiraError::NoFrames)?;
    let bytes = encoder
        .finish(timestamp_ms)
        .map_err(|error| UgoiraError::Encode(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use image::RgbaImage;

    use super::*;

    fn frame_bytes(color: image::Rgba<u8>) -> Vec<u8> {
        let image = RgbaImage::from_pixel(4, 4, color);
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn make_ugoira(manifest: Option<&str>) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(".ugoira").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, color) in [
            ("000000.png", image::Rgba([255, 0, 0, 255])),
            ("000001.png", image::Rgba([0, 255, 0, 255])),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(&frame_bytes(color)).unwrap();
        }
        if let Some(manifest) = manifest {
            writer.start_file("animation.json", options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[test]
    fn composes_an_animated_webp_using_the_manifest_delays() {
        let file = make_ugoira(Some(
            r#"{"frames":[{"file":"000001.png","delay":70},{"file":"000000.png","delay":230}]}"#,
        ));
        let bytes = compose(file.path()).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        // ANIM chunk carries the loop count and background; frame durations live in ANMF chunks.
        assert!(bytes.windows(4).any(|chunk| chunk == b"ANIM"));
        assert_eq!(
            bytes.windows(4).filter(|chunk| *chunk == b"ANMF").count(),
            2
        );
        // Thumbnails decode the composed page through the ordinary still-image path.
        image::load_from_memory(&bytes).expect("composed animation must decode as an image");
    }

    #[test]
    fn falls_back_to_sorted_image_entries_without_a_manifest() {
        let file = make_ugoira(None);
        let bytes = compose(file.path()).unwrap();
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(
            bytes.windows(4).filter(|chunk| *chunk == b"ANMF").count(),
            2
        );
    }

    #[test]
    fn a_manifest_naming_a_missing_frame_is_rejected() {
        let file = make_ugoira(Some(r#"{"frames":[{"file":"nope.png","delay":50}]}"#));
        assert!(matches!(
            compose(file.path()),
            Err(UgoiraError::Manifest(_))
        ));
    }
}
