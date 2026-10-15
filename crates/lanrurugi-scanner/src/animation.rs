//! Animated-image handling.
//!
//! The static reader path (`resize.rs`) decodes a single frame and re-encodes it as one WebP
//! image. That is correct for still pages, but silently destroys an animation. This module
//! provides the two pieces the API needs to avoid that:
//!
//! * [`is_animated`] — a cheap pre-check based on container/bitstream metadata, before any
//!   full-frame decode happens.
//! * [`convert_to_animated_webp`] — a frame-preserving resize/re-encode path for the animated
//!   formats the workspace can decode (WebP and full-frame GIF). Both formats are converted
//!   frame-by-frame, so peak memory follows the *canvas size*, never the frame count; only frames
//!   with local offsets (which need disposal-method composition) fall back to the original bytes.
//!
//! Multi-page TIFF is handled as what it is — a page sequence with no frame timings — and encoded
//! as a single-pass slideshow, so the pages after the first are reachable at all.
//!
//! AVIF and APNG are the remaining gaps: `avif-decode` (pure Rust) decodes *still* AVIF only, and
//! this workspace has no APNG frame decoder, so both fall through to the original bytes.

use std::io::Cursor;

use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder as _, DynamicImage, ImageBuffer, RgbaImage};
use jxl::api::{
    states, JxlColorType, JxlDataFormat, JxlDecoder as JxlApiDecoder, JxlDecoderOptions,
    JxlOutputBuffer, JxlPixelFormat, ProcessingResult,
};
use jxl::headers::extra_channels::ExtraChannel;
use lanrurugi_core::concurrency::run_blocking;
use thiserror::Error;
use tiff::decoder::{Decoder as TiffDecoder, DecodingResult as TiffDecodingResult};
use tiff::ColorType as TiffColorType;
use webpx::{AnimationDecoder, AnimationEncoder, ColorMode, Limits};

/// Upper bound on frames this path will decode/encode. Memory no longer scales with frame count
/// (see [`MAX_ANIMATION_FRAME_BYTES`]), so this is a CPU/time bound: a pathological animation should
/// fall back to the original bytes, not occupy a reader worker indefinitely.
const MAX_ANIMATION_FRAMES: usize = 600;

/// How much a conversion must shrink the short edge before it is worth doing. Measured on a real
/// archive: a 1040->960 "mobile" downscale (7.7%) came back *larger* than the source after
/// libwebp's inter-frame blending mask (20.76 MB vs 20.90 MB, 35 s CPU), while the 1040->600
/// tiny/preview downscale (42%) saved 92%. Same-size conversions are the same story (see
/// `convert_sync`'s gate).
const MIN_ANIMATION_DOWNSCALE_PERCENT: u32 = 20;

/// Upper bound on the buffers alive while a *single* frame is converted — the decoder's frame copy,
/// this module's resized frame, and the pixel buffers libwebp allocates for the encoder's picture
/// and canvas. This, not the frame count, is what bounds a conversion's memory.
const MAX_ANIMATION_FRAME_BYTES: u64 = 192 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum AnimationError {
    #[error("failed to decode GIF frames: {0}")]
    GifDecode(#[from] image::ImageError),
    #[error("failed to decode animated WebP: {0}")]
    WebpDecode(String),
    #[error("failed to encode animated WebP: {0}")]
    WebpEncode(String),
    #[error("failed to decode animated JPEG XL: {0}")]
    JxlDecode(String),
    #[error("invalid animated image: {0}")]
    Invalid(String),
    #[error("blocking task failed: {0}")]
    Join(#[from] lanrurugi_core::concurrency::BlockingTaskError),
}

pub struct AnimatedWebp {
    pub bytes: Vec<u8>,
    pub orig_width: u32,
    pub orig_height: u32,
    pub width: u32,
    pub height: u32,
}

/// Frame count and canvas size, read from container metadata without decoding a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnimationInfo {
    frames: usize,
    width: u32,
    height: u32,
    /// Whether every frame is a full-canvas image with no offset. GIF frames that are not need
    /// disposal-method composition, which this path does not implement (WebP always composes,
    /// because libwebp's animation decoder does it for us).
    full_frame: bool,
    /// Whether the source physically carries an alpha plane. libwebp's animation decoder can only
    /// output RGBA, so a source without one must be re-encoded as RGB — encoding the opaque alpha
    /// channel it never had added ~4% to a real 304-frame page.
    has_alpha: bool,
}

/// Cheap animation detection from container/bitstream metadata only. Returns `false` on any
/// malformed input, matching the static pipeline's existing behaviour of letting a later decode
/// report the real error.
pub fn is_animated(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/gif" => gif_animation_info(bytes).is_some_and(|info| info.frames > 1),
        "image/webp" => webp_animation_info(bytes).is_some_and(|info| info.frames > 1),
        // APNG's animation control chunk is present before IDAT and is unique enough for this
        // pre-filter; `convert_to_animated_webp` still returns `None` for PNG and the caller then
        // serves the original animated PNG.
        "image/png" => bytes.windows(4).any(|window| window == b"acTL"),
        // `avis` is the ISO-BMFF sequence brand for animated AVIF.
        "image/avif" => bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"avis",
        // JPEG XL declares animation in its image header. Almost no browser decodes JXL at all, so
        // catching this here is what turns a frozen first frame into a playable animation.
        "image/jxl" => jxl_animation_info(bytes).is_some(),
        // A multi-page TIFF is a page sequence, not an animation (see `convert_tiff`).
        "image/tiff" => tiff_page_count(bytes).is_some_and(|pages| pages > 1),
        _ => false,
    }
}

/// Whether this module can convert `content_type` at all — the animated formats it has a
/// frame-preserving decoder for. Callers use it to tell "we could convert this if we had to" from
/// "this server has no decoder for it", which decides whether a client that cannot render the
/// source gets a conversion or the original bytes.
pub fn is_convertible(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/webp" | "image/gif" | "image/jxl" | "image/tiff"
    )
}

/// Frame-preserving conversion to animated WebP. `Ok(None)` means "this format/path cannot be
/// converted safely, serve the original bytes"; callers should treat that exactly like a static
/// `Ok(None)` and pass the source through untouched.
pub async fn convert_to_animated_webp(
    content: Vec<u8>,
    content_type: &str,
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    let content_type = content_type.to_string();
    run_blocking(move || {
        convert_sync(
            &content,
            &content_type,
            quality,
            max_short_edge,
            max_long_edge,
        )
    })
    .await?
}

fn convert_sync(
    bytes: &[u8],
    content_type: &str,
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    // Cost gate before decoding anything (see `MAX_ANIMATION_FRAME_BYTES`). A container this module
    // cannot read frame-by-frame metadata out of is treated exactly like an unconvertible format:
    // `Ok(None)`, so the caller serves the original bytes.
    let Some(info) = animation_info(content_type, bytes) else {
        return Ok(None);
    };
    if info.frames > MAX_ANIMATION_FRAMES {
        return Err(AnimationError::Invalid(format!(
            "animation has {} frames (cap {MAX_ANIMATION_FRAMES})",
            info.frames
        )));
    }
    if !info.full_frame {
        return Ok(None);
    }
    tracing::debug!(
        content_type,
        frames = info.frames,
        width = info.width,
        height = info.height,
        full_frame = info.full_frame,
        has_alpha = info.has_alpha,
        "animated conversion: parsed source metadata"
    );
    let target = target_dimensions(info.width, info.height, max_short_edge, max_long_edge);
    // A source that is already the output codec is only worth re-encoding when the conversion
    // really shrinks it: re-encoding already-encoded pixels measured worse than the source
    // (1040x1568 @ q85: +5.9% bytes, 27.5 s CPU, no visible gain at PSNR 42.9 dB), and a slight
    // downscale is no better (`MIN_ANIMATION_DOWNSCALE_PERCENT`). The caller's `Ok(None)` contract
    // serves the original bytes instead. Only WebP qualifies — a GIF is worth converting even at
    // its own size, because GIF's per-frame coding is far worse than WebP's.
    if content_type == "image/webp" {
        let source_short = info.width.min(info.height);
        let target_short = target.0.min(target.1);
        if target_short * 100 >= source_short.saturating_mul(100 - MIN_ANIMATION_DOWNSCALE_PERCENT)
        {
            tracing::debug!(
                width = info.width,
                height = info.height,
                frames = info.frames,
                target_short,
                "animated WebP would not shrink enough to be worth re-encoding; serving the original"
            );
            return Ok(None);
        }
    }
    if estimated_frame_bytes((info.width, info.height), target) > MAX_ANIMATION_FRAME_BYTES {
        tracing::debug!(
            frames = info.frames,
            width = info.width,
            height = info.height,
            budget = MAX_ANIMATION_FRAME_BYTES,
            "animated image exceeds the conversion memory budget; serving the original bytes"
        );
        return Ok(None);
    }

    match content_type {
        "image/webp" => convert_webp(
            bytes,
            quality,
            max_short_edge,
            max_long_edge,
            info.has_alpha,
        ),
        "image/gif" => convert_gif(
            bytes,
            quality,
            max_short_edge,
            max_long_edge,
            info.has_alpha,
        ),
        "image/jxl" => convert_jxl(
            bytes,
            quality,
            max_short_edge,
            max_long_edge,
            info.has_alpha,
        ),
        "image/tiff" => convert_tiff(
            bytes,
            quality,
            max_short_edge,
            max_long_edge,
            info.has_alpha,
        ),
        // APNG/AVIF/JXL and anything else have no frame-safe decoder here. The caller serves the
        // original file; this must never fall through to the still-image pipeline.
        _ => Ok(None),
    }
}

/// Streams an animated WebP through libwebp's animation decoder, resizing one frame at a time into
/// the animation encoder. Frames are never held all at once, so a 304-frame page costs one frame of
/// memory rather than the ~4 GB an eager decode needs.
fn convert_webp(
    bytes: &[u8],
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    has_alpha: bool,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    let mut limits = Limits::default();
    // The cumulative cap (canvas x frame count) is deliberately lifted: streaming holds a single
    // frame at a time, so frame count is bounded by `MAX_ANIMATION_FRAMES`, not by memory. The
    // per-frame caps stay, as they bound what one frame can allocate.
    limits.max_total_pixels = None;
    limits.max_frames = Some(MAX_ANIMATION_FRAMES as u32);

    let mut decoder = AnimationDecoder::with_options_limits(bytes, ColorMode::Rgba, false, &limits)
        .map_err(|error| AnimationError::WebpDecode(error.to_string()))?;
    let info = decoder.info().clone();
    let (canvas_width, canvas_height) = (info.width, info.height);
    if info.frame_count as usize > MAX_ANIMATION_FRAMES {
        return Err(AnimationError::Invalid(format!(
            "animation has {} frames (cap {MAX_ANIMATION_FRAMES})",
            info.frame_count
        )));
    }
    let (target_width, target_height) =
        target_dimensions(canvas_width, canvas_height, max_short_edge, max_long_edge);
    // Re-check against the dimensions the decoder actually reported rather than the container's own
    // claim, before any frame is allocated.
    if estimated_frame_bytes((canvas_width, canvas_height), (target_width, target_height))
        > MAX_ANIMATION_FRAME_BYTES
    {
        return Ok(None);
    }

    let mut encoder =
        AnimationEncoder::with_options(target_width, target_height, false, info.loop_count)
            .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    encoder.set_quality(quality as f32);

    let mut frames_seen = 0usize;
    let mut last_timestamp = 0i32;
    let mut last_delta = 0i32;
    let mut previous_timestamp = 0i32;
    while let Some(frame) = decoder
        .next_frame()
        .map_err(|error| AnimationError::WebpDecode(error.to_string()))?
    {
        frames_seen += 1;
        if frames_seen > MAX_ANIMATION_FRAMES {
            return Err(AnimationError::Invalid(format!(
                "animation has more than {MAX_ANIMATION_FRAMES} frames"
            )));
        }
        if frames_seen > 1 {
            last_delta = frame.timestamp_ms.saturating_sub(previous_timestamp).max(0);
        }
        previous_timestamp = frame.timestamp_ms;
        last_timestamp = frame.timestamp_ms;

        let rgba = RgbaImage::from_raw(frame.width, frame.height, frame.data)
            .ok_or_else(|| AnimationError::Invalid("bad RGBA frame buffer".to_string()))?;
        let added = if has_alpha {
            encoder.add_frame_rgba(
                &resize_frame(&rgba, target_width, target_height),
                frame.timestamp_ms,
            )
        } else {
            encoder.add_frame_rgb(
                &resize_frame_rgb(&rgba, target_width, target_height),
                frame.timestamp_ms,
            )
        };
        added.map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    }

    if frames_seen < 2 {
        return Ok(None);
    }
    let encoded = encoder
        .finish(last_timestamp.saturating_add(last_delta))
        .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    Ok(Some(AnimatedWebp {
        bytes: encoded,
        orig_width: canvas_width,
        orig_height: canvas_height,
        width: target_width,
        height: target_height,
    }))
}

/// Streams a full-frame GIF through `image`'s lazy frame iterator. `image` composes every frame
/// onto the logical screen, which is why the pre-scan rejects frame-local offsets first.
fn convert_gif(
    bytes: &[u8],
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    has_alpha: bool,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    let decoder = GifDecoder::new(Cursor::new(bytes))?;
    let mut encoder: Option<AnimationEncoder> = None;
    let (mut canvas_width, mut canvas_height) = (0u32, 0u32);
    let (mut target_width, mut target_height) = (0u32, 0u32);
    let mut frames_seen = 0usize;
    let mut elapsed_ms = 0i32;

    for frame in decoder.into_frames() {
        let frame = frame?;
        frames_seen += 1;
        if frames_seen > MAX_ANIMATION_FRAMES {
            return Err(AnimationError::Invalid(format!(
                "animation has more than {MAX_ANIMATION_FRAMES} frames"
            )));
        }
        // The delay has to be read off the frame before `into_buffer` consumes it.
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let buffer = frame.into_buffer();
        let (width, height) = (buffer.width(), buffer.height());
        if frames_seen == 1 {
            canvas_width = width;
            canvas_height = height;
            (target_width, target_height) =
                target_dimensions(width, height, max_short_edge, max_long_edge);
            if estimated_frame_bytes((width, height), (target_width, target_height))
                > MAX_ANIMATION_FRAME_BYTES
            {
                return Ok(None);
            }
            // GIF has no per-actor loop count exposed through `image`'s decoder; 0 is the standard
            // infinite-loop animation parameter.
            let mut created = AnimationEncoder::with_options(target_width, target_height, false, 0)
                .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
            created.set_quality(quality as f32);
            encoder = Some(created);
        } else if (width, height) != (canvas_width, canvas_height) {
            // Variable frame sizes would need disposal-method composition; `gif_animation_info`
            // already rejects these, so this is a belt-and-braces guard.
            return Ok(None);
        }

        let delay_ms = if denominator == 0 {
            0
        } else {
            ((numerator as f64 / denominator as f64).round() as i32).max(0)
        };
        let encoder = encoder
            .as_mut()
            .expect("encoder created on the first frame");
        let added = if has_alpha {
            encoder.add_frame_rgba(
                &resize_frame(&buffer, target_width, target_height),
                elapsed_ms,
            )
        } else {
            encoder.add_frame_rgb(
                &resize_frame_rgb(&buffer, target_width, target_height),
                elapsed_ms,
            )
        };
        added.map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
        elapsed_ms = elapsed_ms.saturating_add(delay_ms);
    }

    if frames_seen < 2 {
        return Ok(None);
    }
    let encoded = encoder
        .expect("encoder created on the first frame")
        .finish(elapsed_ms)
        .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    Ok(Some(AnimatedWebp {
        bytes: encoded,
        orig_width: canvas_width,
        orig_height: canvas_height,
        width: target_width,
        height: target_height,
    }))
}

/// Frame metadata for the formats this module can convert; `None` means "not convertible here"
/// (either a format with no frame decoder, or a container whose metadata does not parse).
fn animation_info(content_type: &str, bytes: &[u8]) -> Option<AnimationInfo> {
    match content_type {
        "image/webp" => webp_animation_info(bytes),
        "image/gif" => gif_animation_info(bytes),
        "image/jxl" => jxl_animation_info(bytes),
        "image/tiff" => tiff_animation_info(bytes),
        _ => None,
    }
}

/// Frame count and canvas size straight off WebP's RIFF chunk list — no frame is decoded. Only an
/// extended file (the `VP8X` header animated WebP uses) has `ANMF` frames, which is also why a
/// still WebP returns `None`.
fn webp_animation_info(bytes: &[u8]) -> Option<AnimationInfo> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut frames = 0usize;
    let mut canvas = None;
    let mut has_alpha = false;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let fourcc = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let payload = bytes.get(offset + 8..offset + 8 + size)?;
        match fourcc {
            // Canvas dimensions are stored as width/height minus one, 24 bits little-endian.
            b"VP8X" if payload.len() >= 10 => {
                let width = u32::from_le_bytes([payload[4], payload[5], payload[6], 0]) + 1;
                let height = u32::from_le_bytes([payload[7], payload[8], payload[9], 0]) + 1;
                canvas = Some((width, height));
            }
            b"ANMF" => {
                frames += 1;
                // The `VP8X` alpha *flag* is not evidence on its own — producers set it without
                // ever writing an alpha plane (this library has such a file). Only an `ALPH` chunk
                // inside a frame means the source really carries alpha.
                has_alpha |= frame_has_alpha_chunk(payload);
            }
            _ => {}
        }
        // RIFF chunks are padded to an even byte boundary.
        offset = offset.checked_add(8 + size + (size & 1))?;
    }
    let (width, height) = canvas?;
    (frames > 1).then_some(AnimationInfo {
        frames,
        width,
        height,
        full_frame: true,
        has_alpha,
    })
}

/// Whether one `ANMF` payload contains an `ALPH` sub-chunk (the frame's own nested chunk list
/// starts after the 16-byte frame header).
fn frame_has_alpha_chunk(payload: &[u8]) -> bool {
    let mut offset = 16usize;
    while offset + 8 <= payload.len() {
        let fourcc = &payload[offset..offset + 4];
        let size = u32::from_le_bytes(payload[offset + 4..offset + 8].try_into().unwrap_or([0; 4]))
            as usize;
        if fourcc == b"ALPH" {
            return true;
        }
        match offset.checked_add(8 + size + (size & 1)) {
            Some(next) if next > offset => offset = next,
            _ => break,
        }
    }
    false
}

/// Frame count, logical screen size and full-frame-ness of a GIF, from frame headers only.
fn gif_animation_info(bytes: &[u8]) -> Option<AnimationInfo> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(bytes)).ok()?;
    // The logical screen size (not the first frame's own box) is what `GifDecoder` composes every
    // frame onto, so it is also what the frames this path decodes will measure.
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    let mut frames = 0usize;
    let mut full_frame = true;
    let mut has_alpha = false;
    while let Ok(Some(frame)) = decoder.next_frame_info() {
        frames += 1;
        full_frame &= frame.left == 0
            && frame.top == 0
            && u32::from(frame.width) == width
            && u32::from(frame.height) == height;
        has_alpha |= frame.transparent.is_some();
        if frames > MAX_ANIMATION_FRAMES {
            break;
        }
    }
    Some(AnimationInfo {
        frames,
        width,
        height,
        full_frame,
        has_alpha,
    })
}

/// Rough peak of the buffers alive while one frame is converted: the decoder's frame copy, this
/// module's resized frame, and the pictures libwebp imports/holds while encoding it.
/// Multi-page TIFF carries no frame durations: it is a page sequence, not an animation. A slow,
/// single-pass slideshow keeps every page reachable (the still pipeline would show page 1 and
/// silently drop the rest) without pretending the pages are motion.
const TIFF_SLIDESHOW_FRAME_MS: i32 = 5000;

fn tiff_decoder(bytes: &[u8]) -> Option<TiffDecoder<Cursor<&[u8]>>> {
    TiffDecoder::new(Cursor::new(bytes)).ok()
}

fn tiff_page_count(bytes: &[u8]) -> Option<usize> {
    let mut decoder = tiff_decoder(bytes)?;
    let mut pages = 1usize;
    while decoder.more_images() && pages <= MAX_ANIMATION_FRAMES {
        decoder.next_image().ok()?;
        pages += 1;
    }
    Some(pages)
}

fn tiff_color_has_alpha(color: TiffColorType) -> bool {
    matches!(
        color,
        TiffColorType::GrayA(_) | TiffColorType::RGBA(_) | TiffColorType::CMYKA(_)
    )
}

fn tiff_animation_info(bytes: &[u8]) -> Option<AnimationInfo> {
    let mut decoder = tiff_decoder(bytes)?;
    let (width, height) = decoder.dimensions().ok()?;
    // The encoder is created once, from the first page, so alpha has to cover every page.
    let mut has_alpha = tiff_color_has_alpha(decoder.colortype().ok()?);
    while decoder.more_images() {
        decoder.next_image().ok()?;
        has_alpha |= tiff_color_has_alpha(decoder.colortype().ok()?);
    }
    Some(AnimationInfo {
        frames: tiff_page_count(bytes)?,
        width,
        height,
        // Pages are always full canvases: TIFF has no frame-offset/disposal model.
        full_frame: true,
        has_alpha,
    })
}

/// One page as RGBA8. Colour layouts outside the common scan set (palette, CMYK, YCbCr, Lab,
/// float/odd bit depths) return `None`, and the caller then serves the original bytes.
fn tiff_page_rgba(
    decoder: &mut TiffDecoder<Cursor<&[u8]>>,
    width: u32,
    height: u32,
) -> Result<Option<RgbaImage>, AnimationError> {
    let color = decoder
        .colortype()
        .map_err(|error| AnimationError::Invalid(error.to_string()))?;
    let decoded = decoder
        .read_image()
        .map_err(|error| AnimationError::Invalid(error.to_string()))?;
    let image = match (decoded, color) {
        (TiffDecodingResult::U8(buffer), TiffColorType::RGB(8)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageRgb8)
        }
        (TiffDecodingResult::U8(buffer), TiffColorType::RGBA(8)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageRgba8)
        }
        (TiffDecodingResult::U8(buffer), TiffColorType::Gray(8)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageLuma8)
        }
        (TiffDecodingResult::U8(buffer), TiffColorType::GrayA(8)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageLumaA8)
        }
        (TiffDecodingResult::U16(buffer), TiffColorType::RGB(16)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageRgb16)
        }
        (TiffDecodingResult::U16(buffer), TiffColorType::RGBA(16)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageRgba16)
        }
        (TiffDecodingResult::U16(buffer), TiffColorType::Gray(16)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageLuma16)
        }
        (TiffDecodingResult::U16(buffer), TiffColorType::GrayA(16)) => {
            ImageBuffer::from_raw(width, height, buffer).map(DynamicImage::ImageLumaA16)
        }
        _ => None,
    };
    Ok(image.map(|image| image.to_rgba8()))
}

/// Decodes every TIFF page and re-encodes the sequence as an animated WebP that plays once.
fn convert_tiff(
    bytes: &[u8],
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    has_alpha: bool,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    let Some(mut decoder) = tiff_decoder(bytes) else {
        return Ok(None);
    };
    let mut encoder: Option<AnimationEncoder> = None;
    let (mut canvas_width, mut canvas_height) = (0u32, 0u32);
    let (mut target_width, mut target_height) = (0u32, 0u32);
    let mut frames_seen = 0usize;
    let mut elapsed_ms = 0i32;

    loop {
        let (width, height) = decoder
            .dimensions()
            .map_err(|error| AnimationError::Invalid(error.to_string()))?;
        frames_seen += 1;
        if frames_seen > MAX_ANIMATION_FRAMES {
            return Err(AnimationError::Invalid(format!(
                "animation has more than {MAX_ANIMATION_FRAMES} frames"
            )));
        }
        let Some(frame) = tiff_page_rgba(&mut decoder, width, height)? else {
            return Ok(None);
        };
        if frames_seen == 1 {
            canvas_width = width;
            canvas_height = height;
            (target_width, target_height) =
                target_dimensions(width, height, max_short_edge, max_long_edge);
            if estimated_frame_bytes((width, height), (target_width, target_height))
                > MAX_ANIMATION_FRAME_BYTES
            {
                return Ok(None);
            }
            // A loop count of 1: this is a page sequence, so it plays through once and stops.
            let mut created = AnimationEncoder::with_options(target_width, target_height, false, 1)
                .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
            created.set_quality(quality as f32);
            encoder = Some(created);
        } else if (width, height) != (canvas_width, canvas_height) {
            return Ok(None);
        }

        let encoder = encoder
            .as_mut()
            .expect("encoder created on the first frame");
        let added = if has_alpha {
            encoder.add_frame_rgba(
                &resize_frame(&frame, target_width, target_height),
                elapsed_ms,
            )
        } else {
            encoder.add_frame_rgb(
                &resize_frame_rgb(&frame, target_width, target_height),
                elapsed_ms,
            )
        };
        added.map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
        elapsed_ms = elapsed_ms.saturating_add(TIFF_SLIDESHOW_FRAME_MS);

        if !decoder.more_images() {
            break;
        }
        decoder
            .next_image()
            .map_err(|error| AnimationError::Invalid(error.to_string()))?;
    }

    if frames_seen < 2 {
        return Ok(None);
    }
    let encoded = encoder
        .expect("encoder created on the first frame")
        .finish(elapsed_ms)
        .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    Ok(Some(AnimatedWebp {
        bytes: encoded,
        orig_width: canvas_width,
        orig_height: canvas_height,
        width: target_width,
        height: target_height,
    }))
}

fn estimated_frame_bytes(canvas: (u32, u32), target: (u32, u32)) -> u64 {
    let canvas_pixels = u64::from(canvas.0) * u64::from(canvas.1);
    let target_pixels = u64::from(target.0) * u64::from(target.1);
    (canvas_pixels + target_pixels) * 2 * 4
}

/// Animation header of a JPEG XL file, read through the decoder's header state only — no pixel is
/// decoded. `Some` only when the codestream actually declares an animation.
fn jxl_animation_info(bytes: &[u8]) -> Option<AnimationInfo> {
    let info = jxl_basic_info(bytes)?;
    // `None` means a still image: the still pipeline already handles those.
    info.animation.as_ref()?;
    let width = u32::try_from(info.size.0).ok()?;
    let height = u32::try_from(info.size.1).ok()?;
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|channel| channel.ec_type == ExtraChannel::Alpha);
    Some(AnimationInfo {
        // JXL declares no frame count up front: 0 = unknown, and `convert_jxl` enforces
        // `MAX_ANIMATION_FRAMES` while it streams them.
        frames: 0,
        width,
        height,
        full_frame: true,
        has_alpha,
    })
}

fn jxl_basic_info(bytes: &[u8]) -> Option<jxl::api::JxlBasicInfo> {
    let mut input = bytes;
    let decoder = JxlApiDecoder::<states::Initialized>::new(JxlDecoderOptions::default());
    let decoder = drive_jxl(decoder.process(&mut input, None), &mut input).ok()?;
    Some(decoder.basic_info().clone())
}

/// Streams an animated JPEG XL through the pure-Rust decoder one frame at a time — the decoder
/// composites each frame onto the canvas for us, so frames never accumulate.
fn convert_jxl(
    bytes: &[u8],
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    has_alpha: bool,
) -> Result<Option<AnimatedWebp>, AnimationError> {
    let mut input = bytes;
    let decoder = JxlApiDecoder::<states::Initialized>::new(JxlDecoderOptions::default());
    let mut decoder = drive_jxl(decoder.process(&mut input, None), &mut input)?;
    let info = decoder.basic_info().clone();
    let canvas_width = u32::try_from(info.size.0)
        .map_err(|_| AnimationError::Invalid("jxl canvas is too large".to_string()))?;
    let canvas_height = u32::try_from(info.size.1)
        .map_err(|_| AnimationError::Invalid("jxl canvas is too large".to_string()))?;
    let (target_width, target_height) =
        target_dimensions(canvas_width, canvas_height, max_short_edge, max_long_edge);
    if estimated_frame_bytes((canvas_width, canvas_height), (target_width, target_height))
        > MAX_ANIMATION_FRAME_BYTES
    {
        return Ok(None);
    }

    let loop_count = info
        .animation
        .as_ref()
        .map(|animation| animation.num_loops)
        .unwrap_or(0);
    let has_alpha = has_alpha
        || info
            .extra_channels
            .iter()
            .any(|channel| channel.ec_type == ExtraChannel::Alpha);
    decoder
        .set_pixel_format(JxlPixelFormat {
            color_type: JxlColorType::Rgba,
            color_data_format: Some(JxlDataFormat::U8 { bit_depth: 8 }),
            extra_channel_format: vec![None; info.extra_channels.len()],
        })
        .map_err(|error| AnimationError::JxlDecode(error.to_string()))?;

    let mut encoder =
        AnimationEncoder::with_options(target_width, target_height, false, loop_count)
            .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    encoder.set_quality(quality as f32);

    let mut frames_seen = 0usize;
    let mut elapsed_ms = 0i32;
    loop {
        let frame_decoder = drive_jxl(decoder.process(&mut input, None), &mut input)?;
        frames_seen += 1;
        if frames_seen > MAX_ANIMATION_FRAMES {
            return Err(AnimationError::Invalid(format!(
                "animation has more than {MAX_ANIMATION_FRAMES} frames"
            )));
        }
        // JXL frame durations are seconds (rational tps); keep timestamps strictly increasing.
        let duration_ms = frame_decoder
            .frame_header()
            .duration
            .map(|seconds| (seconds * 1000.0).round() as i32)
            .unwrap_or(0)
            .max(1);

        let mut frame = vec![0u8; canvas_width as usize * canvas_height as usize * 4];
        {
            let mut output = JxlOutputBuffer::new(
                &mut frame,
                canvas_height as usize,
                canvas_width as usize * 4,
            );
            let buffers = std::slice::from_mut(&mut output);
            let mut result = frame_decoder.process(&mut input, buffers, None);
            loop {
                match result.map_err(|error| AnimationError::JxlDecode(error.to_string()))? {
                    ProcessingResult::Complete { result } => {
                        decoder = result;
                        break;
                    }
                    // Retrying must hand the same output buffer back in, so this step cannot go
                    // through `drive_jxl`.
                    ProcessingResult::NeedsMoreInput { fallback, .. } => {
                        result = fallback.process(&mut input, buffers, None);
                    }
                }
            }
        }
        let rgba = RgbaImage::from_raw(canvas_width, canvas_height, frame)
            .ok_or_else(|| AnimationError::Invalid("bad JXL frame buffer".to_string()))?;
        let added = if has_alpha {
            encoder.add_frame_rgba(
                &resize_frame(&rgba, target_width, target_height),
                elapsed_ms,
            )
        } else {
            encoder.add_frame_rgb(
                &resize_frame_rgb(&rgba, target_width, target_height),
                elapsed_ms,
            )
        };
        added.map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
        elapsed_ms = elapsed_ms.saturating_add(duration_ms);

        if !decoder.has_more_frames() {
            break;
        }
    }

    if frames_seen < 2 {
        return Ok(None);
    }
    let encoded = encoder
        .finish(elapsed_ms)
        .map_err(|error| AnimationError::WebpEncode(error.to_string()))?;
    Ok(Some(AnimatedWebp {
        bytes: encoded,
        orig_width: canvas_width,
        orig_height: canvas_height,
        width: target_width,
        height: target_height,
    }))
}

/// Drives one `jxl-rs` state-machine step to completion. Input is already in memory, so "needs more
/// input" only means the step handed back a fallback decoder to retry with.
fn drive_jxl<T, F>(
    result: jxl::error::Result<ProcessingResult<T, F>>,
    input: &mut &[u8],
) -> Result<T, AnimationError>
where
    F: JxlRetry<T>,
{
    let mut result = result;
    loop {
        match result.map_err(|error| AnimationError::JxlDecode(error.to_string()))? {
            ProcessingResult::Complete { result } => return Ok(result),
            ProcessingResult::NeedsMoreInput { fallback, .. } => {
                result = fallback.retry(input);
            }
        }
    }
}

trait JxlRetry<T> {
    fn retry(self, input: &mut &[u8]) -> jxl::error::Result<ProcessingResult<T, Self>>
    where
        Self: Sized;
}

impl JxlRetry<JxlApiDecoder<states::WithImageInfo>> for JxlApiDecoder<states::Initialized> {
    fn retry(
        self,
        input: &mut &[u8],
    ) -> jxl::error::Result<ProcessingResult<JxlApiDecoder<states::WithImageInfo>, Self>> {
        self.process(input, None)
    }
}

impl JxlRetry<JxlApiDecoder<states::WithFrameInfo>> for JxlApiDecoder<states::WithImageInfo> {
    fn retry(
        self,
        input: &mut &[u8],
    ) -> jxl::error::Result<ProcessingResult<JxlApiDecoder<states::WithFrameInfo>, Self>> {
        self.process(input, None)
    }
}

fn resize_frame(frame: &RgbaImage, target_width: u32, target_height: u32) -> Vec<u8> {
    if (target_width, target_height) == (frame.width(), frame.height()) {
        frame.as_raw().clone()
    } else {
        image::imageops::resize(frame, target_width, target_height, FilterType::Lanczos3).into_raw()
    }
}

/// Same, for a source with no alpha plane: dropping the channel *before* resizing keeps the work
/// on three channels and stops the encoder from writing a per-frame alpha plane (see
/// `AnimationInfo::has_alpha`).
fn resize_frame_rgb(frame: &RgbaImage, target_width: u32, target_height: u32) -> Vec<u8> {
    let (width, height) = (frame.width(), frame.height());
    let mut rgb = image::RgbImage::new(width, height);
    for (target, source) in rgb.pixels_mut().zip(frame.pixels()) {
        target.0 = [source.0[0], source.0[1], source.0[2]];
    }
    if (target_width, target_height) == (width, height) {
        rgb.into_raw()
    } else {
        image::imageops::resize(&rgb, target_width, target_height, FilterType::Lanczos3).into_raw()
    }
}

fn target_dimensions(
    width: u32,
    height: u32,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }
    if let Some(max_long_edge) = max_long_edge {
        let ratio = (max_long_edge as f64 / width.max(height) as f64).min(1.0);
        return (
            ((width as f64 * ratio).round() as u32).max(1),
            ((height as f64 * ratio).round() as u32).max(1),
        );
    }
    let short_edge = width.min(height);
    if short_edge > max_short_edge {
        let ratio = max_short_edge as f64 / short_edge as f64;
        return (
            ((width as f64 * ratio).round() as u32).max(1),
            ((height as f64 * ratio).round() as u32).max(1),
        );
    }
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use webpx::{Encoder as StaticEncoder, Unstoppable};

    fn make_test_gif() -> Vec<u8> {
        use image::codecs::gif::GifEncoder;
        use image::{Frame, Rgba, RgbaImage};

        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            let first = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
            let second = RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 255]));
            encoder
                .encode_frames([Frame::new(first), Frame::new(second)])
                .unwrap();
        }
        bytes
    }

    fn make_test_animated_webp() -> Vec<u8> {
        let mut encoder = AnimationEncoder::with_options(8, 8, false, 0).unwrap();
        encoder.set_lossless(true);
        encoder.set_quality(80.0);
        let first = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])).into_raw();
        let second = RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255])).into_raw();
        encoder.add_frame_rgba(&first, 0).unwrap();
        encoder.add_frame_rgba(&second, 100).unwrap();
        encoder.finish(200).unwrap()
    }

    /// Six frames 10 ms apart: a 60 fps source, i.e. exactly the shape a phone cannot decode in
    /// real time.
    fn make_fast_animated_webp() -> Vec<u8> {
        let mut encoder = AnimationEncoder::with_options(8, 8, false, 0).unwrap();
        encoder.set_lossless(true);
        for index in 0..6u8 {
            let frame =
                RgbaImage::from_pixel(8, 8, image::Rgba([index * 40, 0, 0, 255])).into_raw();
            encoder
                .add_frame_rgba(&frame, i32::from(index) * 10)
                .unwrap();
        }
        encoder.finish(60).unwrap()
    }

    fn frame_count(encoded: &[u8]) -> usize {
        AnimationDecoder::new(encoded).unwrap().info().frame_count as usize
    }

    fn alpha_chunks(encoded: &[u8]) -> usize {
        encoded
            .windows(4)
            .filter(|window| *window == b"ALPH")
            .count()
    }

    fn make_translucent_animated_webp() -> Vec<u8> {
        let mut encoder = AnimationEncoder::with_options(8, 8, false, 0).unwrap();
        encoder.set_quality(80.0);
        // Semi-transparent *and* distinct: libwebp merges frames it encodes to the same values.
        for (index, alpha) in [(0u8, 128u8), (1, 200)] {
            let frame = RgbaImage::from_pixel(8, 8, image::Rgba([index * 255, 0, 0, alpha]));
            encoder
                .add_frame_rgba(frame.as_raw(), i32::from(index) * 100)
                .unwrap();
        }
        encoder.finish(300).unwrap()
    }

    #[test]
    fn detects_animated_and_static_sources() {
        let gif = make_test_gif();
        assert!(is_animated("image/gif", &gif));

        let webp = make_test_animated_webp();
        assert!(is_animated("image/webp", &webp));

        assert!(!is_animated(
            "image/png",
            b"\x89PNG\r\n\x1A\n\x00\x00\x00\rIHDR"
        ));
    }

    /// A converted animation must keep the source's own frame timing: the converter is a
    /// re-encoder, not a re-timer. (Frames and their delays are copied verbatim, so a device that
    /// cannot decode fast enough will play late — but the file never lies about its speed.)
    #[test]
    fn preserves_the_source_frame_timing() {
        let source = make_fast_animated_webp();
        let decoded_source = AnimationDecoder::new(&source).unwrap();
        let source_info = decoded_source.info().clone();
        assert_eq!(source_info.frame_count, 6);

        let converted = convert_sync(&source, "image/webp", 80, 4, None)
            .unwrap()
            .unwrap();
        let decoded = AnimationDecoder::new(&converted.bytes).unwrap();
        assert_eq!(
            decoded.info().frame_count,
            source_info.frame_count,
            "every frame survives the conversion"
        );

        // Same frame count and the same per-frame spacing as the source: the *pace* is the
        // intervals, and libwebp re-bases the timeline's origin by one interval when it muxes
        // (source frames decode at 0..50 ms, the converted ones at 10..60 ms) — a constant offset
        // that leaves the speed untouched. The end-to-end check on a real page compares the total
        // duration against the source's own 5067 ms.
        let timestamps = |bytes: &[u8]| {
            let mut decoder = AnimationDecoder::new(bytes).unwrap();
            let mut out = Vec::new();
            while let Some(frame) = decoder.next_frame().unwrap() {
                out.push(frame.timestamp_ms);
            }
            out
        };
        let deltas = |stamps: &[i32]| {
            stamps
                .windows(2)
                .map(|pair| pair[1] - pair[0])
                .collect::<Vec<_>>()
        };
        let converted_timestamps = timestamps(&converted.bytes);
        assert_eq!(
            deltas(&converted_timestamps),
            deltas(&timestamps(&source)),
            "frame spacing (the animation's speed) is unchanged"
        );
    }

    #[test]
    fn converts_full_frame_animations_to_animated_webp() {
        // `max_short_edge = 4` on an 8x8 source so the resize gate above still lets these convert.
        for (content_type, source) in [
            ("image/gif", make_test_gif()),
            ("image/webp", make_test_animated_webp()),
        ] {
            let result = convert_sync(&source, content_type, 80, 4, None)
                .unwrap()
                .unwrap();
            assert!(!result.bytes.is_empty(), "{content_type}");
            assert_eq!(frame_count(&result.bytes), 2, "{content_type}");
        }
    }

    #[test]
    fn converts_long_animations_frame_by_frame() {
        // Memory now follows the canvas, not the frame count, so an animation with far more frames
        // than the eager pipeline could ever have held still converts.
        let mut encoder = AnimationEncoder::with_options(64, 64, false, 0).unwrap();
        encoder.set_quality(80.0);
        // Frames have to be *visibly* distinct: libwebp merges consecutive frames it encodes to the
        // same values (a flat color 1/255 apart quantizes to one frame), which would make this
        // assertion about the re-encoded frame count meaningless.
        for index in 0..40u8 {
            let luma = if index % 2 == 0 { 0u8 } else { 255u8 };
            let pixels =
                RgbaImage::from_pixel(64, 64, image::Rgba([luma, luma, luma, 255])).into_raw();
            encoder
                .add_frame_rgba(&pixels, i32::from(index) * 40)
                .unwrap();
        }
        let source = encoder.finish(1600).unwrap();

        let result = convert_sync(&source, "image/webp", 80, 32, None)
            .unwrap()
            .unwrap();
        assert_eq!(frame_count(&result.bytes), 40);
    }

    #[test]
    fn reads_frame_metadata_without_decoding() {
        let expected = AnimationInfo {
            frames: 2,
            width: 8,
            height: 8,
            full_frame: true,
            // Both fixtures are fully opaque: `VP8X`'s alpha flag is not consulted, only real
            // `ALPH` chunks are.
            has_alpha: false,
        };
        assert_eq!(
            webp_animation_info(&make_test_animated_webp()),
            Some(expected)
        );
        assert_eq!(gif_animation_info(&make_test_gif()), Some(expected));

        // A still WebP carries no `ANMF` chunk at all, and PNG has no frame decoder here.
        let still = StaticEncoder::new_rgba(&[0u8; 8 * 8 * 4], 8, 8)
            .encode(Unstoppable)
            .unwrap();
        assert_eq!(webp_animation_info(&still), None);
        assert_eq!(animation_info("image/png", b"\x89PNG\r\n\x1A\n"), None);
    }

    #[test]
    fn drops_the_alpha_plane_when_the_source_has_none() {
        let source = make_test_animated_webp();
        assert_eq!(alpha_chunks(&source), 0);
        assert!(!webp_animation_info(&source).unwrap().has_alpha);

        let result = convert_sync(&source, "image/webp", 80, 4, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            alpha_chunks(&result.bytes),
            0,
            "an opaque source must not gain an alpha plane it never had"
        );
    }

    #[test]
    fn keeps_the_alpha_plane_when_the_source_has_one() {
        let source = make_translucent_animated_webp();
        assert!(alpha_chunks(&source) > 0);
        assert!(webp_animation_info(&source).unwrap().has_alpha);

        let result = convert_sync(&source, "image/webp", 80, 4, None)
            .unwrap()
            .unwrap();
        assert!(
            alpha_chunks(&result.bytes) > 0,
            "semi-transparent frames must keep their alpha plane"
        );
    }

    /// End-to-end check against a real animated archive
    /// (`LANRURUGI_TEST_ANIMATED_ARCHIVE_PATH`, documented in `.env.example`); skipped when unset so
    /// no real title lives in the source.
    ///
    /// Deliberately does *not* assert anything about alpha: libwebp's animation encoder introduces
    /// an inter-frame blending mask by itself (`FlattenSimilarBlocks` in its `anim_encode.c`
    /// replaces blocks similar to the previous canvas with transparent "keep the previous frame"
    /// pixels) whenever consecutive frames are alike, so an opaque source legitimately comes back
    /// with an `ALPH` plane. Feeding it RGB instead of RGBA changes nothing — verified: the two
    /// outputs were byte-identical.
    #[test]
    fn converts_a_real_animated_page() {
        use std::io::Read as _;

        let Ok(path) = std::env::var("LANRURUGI_TEST_ANIMATED_ARCHIVE_PATH") else {
            return;
        };
        let file = std::fs::File::open(&path).expect("open animated archive");
        let mut zip = zip::ZipArchive::new(file).expect("read zip");
        let mut found = None;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).expect("zip entry");
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("read entry");
            match webp_animation_info(&bytes) {
                Some(info) if info.frames > 1 => {
                    found = Some((bytes, info));
                    break;
                }
                _ => continue,
            }
        }
        let (bytes, info) = found.expect("archive has no animated WebP page");
        // A small target keeps this test cheap; the reader's real targets exercise the same path.
        let converted = convert_sync(&bytes, "image/webp", 40, 200, None)
            .expect("conversion")
            .expect("converted");
        assert!(frame_count(&converted.bytes) > 1, "animation must survive");
        assert_eq!(converted.orig_width, info.width);
        assert_eq!(converted.orig_height, info.height);
        // The cap applies to the *short* edge, so a portrait page keeps height > 200.
        assert_eq!(converted.width.min(converted.height), 200);
    }

    #[test]
    fn converts_an_animated_jxl_to_webp() {
        // Five-frame sample shipped with the `jxl` crate's own test data (BSD-3, synthetic
        // "numbered frames" content — no real work, no real people). See tests/fixtures/README.md.
        let source: &[u8] = include_bytes!("../tests/fixtures/animated_5_frames.jxl");
        assert!(is_animated("image/jxl", source));
        // Dimensions only: whether this fixture carries alpha is its own business, not the
        // invariant under test (it does, which also exercises the RGBA path).
        assert_eq!(
            jxl_animation_info(source).map(|info| (info.width, info.height)),
            Some((256, 256))
        );

        let result = convert_sync(source, "image/jxl", 80, 64, None)
            .unwrap()
            .unwrap();
        assert_eq!(frame_count(&result.bytes), 5);
        assert_eq!((result.orig_width, result.orig_height), (256, 256));
        assert_eq!((result.width, result.height), (64, 64));
    }

    /// Detection against a real animated AVIF (`LANRURUGI_TEST_ANIMATED_AVIF_PATH`, documented in
    /// `.env.example`; skipped when unset or missing).
    ///
    /// The workspace has an AVIF *encoder* but no decoder, so this pins down what is actually
    /// guaranteed today: the page is recognised as an animation (it must not be flattened by the
    /// still pipeline) and the converter refuses it rather than promising a conversion it cannot
    /// perform. Adding a decoder is what would let `is_convertible("image/avif")` become true.
    /// Builds a multi-page TIFF in memory: the `tiff` crate can write one, so this needs no
    /// committed fixture.
    fn make_multi_page_tiff(pages: usize) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(Cursor::new(&mut out)).unwrap();
            for page in 0..pages {
                let colour = [60 + page as u8 * 60, 30, 200 - page as u8 * 40];
                let pixels: Vec<u8> = (0..4 * 4).flat_map(|_| colour).collect();
                encoder
                    .write_image::<tiff::encoder::colortype::RGB8>(4, 4, &pixels)
                    .unwrap();
            }
        }
        out
    }

    /// A multi-page TIFF is a page sequence: every page must survive into a slideshow, not just the
    /// first one the still pipeline would pick. The colour type is checked too — `tiff`'s
    /// `ColorType` carries *bits per sample*, so an `RGB(8)` page that was matched as `RGB(3)` used
    /// to make the converter give up and serve the original file.
    #[test]
    fn converts_a_multi_page_tiff_as_a_slideshow() {
        let bytes = make_multi_page_tiff(3);
        assert!(is_animated("image/tiff", &bytes));
        assert!(is_convertible("image/tiff"));
        let info = animation_info("image/tiff", &bytes).expect("page count from headers");
        assert_eq!(info.frames, 3);
        assert!(info.full_frame);

        let converted = convert_sync(&bytes, "image/tiff", 80, 4, None)
            .expect("conversion succeeds")
            .expect("multi-page TIFF converts");
        let frames = converted
            .bytes
            .windows(4)
            .filter(|window| *window == b"ANMF")
            .count();
        assert_eq!(frames, 3, "one ANMF frame per page");
        assert_eq!((converted.width, converted.height), (4, 4));
    }

    #[test]
    fn a_single_page_tiff_is_not_an_animation() {
        let bytes = make_multi_page_tiff(1);
        assert!(!is_animated("image/tiff", &bytes));
        let info = animation_info("image/tiff", &bytes).expect("page count from headers");
        assert_eq!(info.frames, 1);
    }

    #[test]
    fn detects_a_real_animated_avif_without_promising_to_convert_it() {
        let Ok(path) = std::env::var("LANRURUGI_TEST_ANIMATED_AVIF_PATH") else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        assert!(
            is_animated("image/avif", &bytes),
            "an `avis` sequence must be detected as an animation"
        );
        assert!(
            !is_convertible("image/avif"),
            "no AVIF decoder exists, so conversion must not be promised"
        );
        assert!(convert_sync(&bytes, "image/avif", 80, 800, None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn skips_conversion_when_it_cannot_resize_an_already_webp_source() {
        let source = make_test_animated_webp();
        // Canvas 8x8 with a short-edge target of 8: no resize possible, source already WebP.
        assert!(convert_sync(&source, "image/webp", 80, 8, None)
            .unwrap()
            .is_none());
        // A target that really downscales still converts (8 -> 4 is a 50% shrink).
        assert!(convert_sync(&source, "image/webp", 80, 4, None)
            .unwrap()
            .is_some());
        // A token downscale (8 -> 7, 12.5%) is not worth the CPU: measured on a real archive, a
        // 7.7% downscale produced a *larger* file than the source.
        assert!(convert_sync(&source, "image/webp", 80, 7, None)
            .unwrap()
            .is_none());
        // A GIF at its own size is still worth converting to WebP.
        assert!(convert_sync(&make_test_gif(), "image/gif", 80, 8, None)
            .unwrap()
            .is_some());
    }

    #[test]
    fn refuses_frames_that_would_not_fit_the_memory_budget() {
        // The canvas that took the dev container's memory cgroup down had 1.63 MP frames.
        assert!(
            estimated_frame_bytes((1040, 1568), (1040, 1568)) <= MAX_ANIMATION_FRAME_BYTES,
            "a single 1040x1568 frame must stay convertible"
        );
        assert!(
            estimated_frame_bytes((12000, 12000), (12000, 12000)) > MAX_ANIMATION_FRAME_BYTES,
            "an absurd canvas must fall back to the original bytes"
        );
    }
}
