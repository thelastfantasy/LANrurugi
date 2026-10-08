//! Animated-image handling.
//!
//! The static reader path (`resize.rs`) decodes a single frame and re-encodes it as one WebP
//! image. That is correct for still pages, but silently destroys an animation. This module
//! provides the two pieces the API needs to avoid that:
//!
//! * [`is_animated`] — a cheap pre-check based on container/bitstream metadata, before any
//!   full-frame decode happens.
//! * [`convert_to_animated_webp`] — a frame-preserving resize/re-encode path for the animated
//!   formats the workspace can decode (WebP and full-frame GIF). Formats without a frame decoder
//!   or with frame-local offsets fall back to the original bytes rather than being flattened.
//!
//! JPEG XL is deliberately not converted here: the current `jxl-image-rs-integration` hook exposes
//! only still-image decoding, and a source JXL animation should be passed through to a JXL-capable
//! browser rather than guessed at.

use std::io::Cursor;

use image::codecs::gif::GifDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, RgbaImage};
use lanrurugi_core::concurrency::run_blocking;
use thiserror::Error;
use webp::{
    AnimDecoder, AnimEncoder, AnimFrame as WebpFrame, BitstreamFeatures, PixelLayout, WebPConfig,
};

/// Upper bound on frames this path will decode/encode. A malicious or pathological animation
/// should fall back to the original bytes, not occupy the reader queue indefinitely.
const MAX_ANIMATION_FRAMES: usize = 600;

#[derive(Debug, Error)]
pub enum AnimationError {
    #[error("failed to decode GIF frames: {0}")]
    GifDecode(#[from] image::ImageError),
    #[error("failed to decode animated WebP: {0}")]
    WebpDecode(String),
    #[error("failed to encode animated WebP: {0:?}")]
    WebpEncode(webp::AnimEncodeError),
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

struct DecodedFrame {
    rgba: RgbaImage,
    time_ms: i32,
}

/// Cheap animation detection from container/bitstream metadata only. Returns `false` on any
/// malformed input, matching the static pipeline's existing behaviour of letting a later decode
/// report the real error.
pub fn is_animated(content_type: &str, bytes: &[u8]) -> bool {
    match content_type {
        "image/gif" => gif_frame_count(bytes).is_some_and(|count| count > 1),
        "image/webp" => {
            BitstreamFeatures::new(bytes).is_some_and(|features| features.has_animation())
        }
        // APNG's animation control chunk is present before IDAT and is unique enough for this
        // pre-filter; `convert_to_animated_webp` still returns `None` for PNG and the caller then
        // serves the original animated PNG.
        "image/png" => bytes.windows(4).any(|window| window == b"acTL"),
        // `avis` is the ISO-BMFF sequence brand for animated AVIF.
        "image/avif" => bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && &bytes[8..12] == b"avis",
        _ => false,
    }
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
    let (frames, loop_count) = match content_type {
        "image/webp" => match decode_webp_frames(bytes)? {
            Some(decoded) => decoded,
            None => return Ok(None),
        },
        "image/gif" => match decode_gif_frames(bytes)? {
            Some(decoded) => decoded,
            None => return Ok(None),
        },
        // APNG/AVIF/JXL and anything else have no frame-safe decoder here. The caller serves
        // the original file; this must never fall through to the still-image pipeline.
        _ => return Ok(None),
    };

    if frames.len() < 2 {
        return Ok(None);
    }
    if frames.len() > MAX_ANIMATION_FRAMES {
        return Err(AnimationError::Invalid(format!(
            "animation has {} frames (cap {MAX_ANIMATION_FRAMES})",
            frames.len()
        )));
    }

    let canvas_width = frames[0].rgba.width();
    let canvas_height = frames[0].rgba.height();
    if frames
        .iter()
        .any(|frame| frame.rgba.width() != canvas_width || frame.rgba.height() != canvas_height)
    {
        return Ok(None);
    }

    let (target_width, target_height) =
        target_dimensions(canvas_width, canvas_height, max_short_edge, max_long_edge);

    let mut resized_pixels: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    for frame in &frames {
        let resized = if (target_width, target_height) == (canvas_width, canvas_height) {
            frame.rgba.clone()
        } else {
            image::imageops::resize(
                &frame.rgba,
                target_width,
                target_height,
                FilterType::Lanczos3,
            )
        };
        resized_pixels.push(resized.into_raw());
    }

    let mut config = WebPConfig::new()
        .map_err(|_| AnimationError::Invalid("failed to create WebP config".to_string()))?;
    config.lossless = 0;
    config.quality = quality as f32;
    config.alpha_compression = 1;
    config.alpha_filtering = 1;

    let mut encoder = AnimEncoder::new(target_width, target_height, &config);
    encoder.set_loop_count(loop_count as i32);
    for (index, pixels) in resized_pixels.iter().enumerate() {
        encoder.add_frame(WebpFrame::from_rgba(
            pixels,
            target_width,
            target_height,
            frames[index].time_ms,
        ));
    }

    let encoded = encoder.try_encode().map_err(AnimationError::WebpEncode)?;
    Ok(Some(AnimatedWebp {
        bytes: encoded.to_vec(),
        orig_width: canvas_width,
        orig_height: canvas_height,
        width: target_width,
        height: target_height,
    }))
}

fn decode_gif_frames(bytes: &[u8]) -> Result<Option<(Vec<DecodedFrame>, u32)>, AnimationError> {
    let decoder = GifDecoder::new(Cursor::new(bytes))?;
    let frames = decoder.into_frames().collect_frames()?;
    if frames.len() < 2 {
        return Ok(None);
    }

    let canvas_width = frames[0].buffer().width();
    let canvas_height = frames[0].buffer().height();
    if frames.iter().any(|frame| {
        frame.left() != 0
            || frame.top() != 0
            || frame.buffer().width() != canvas_width
            || frame.buffer().height() != canvas_height
    }) {
        // Frame-local offsets and variable frame sizes require full disposal-method composition;
        // rather than emit a visibly wrong animation, fall back to the original GIF.
        return Ok(None);
    }

    let mut time_ms = 0i32;
    let mut decoded = Vec::with_capacity(frames.len());
    for frame in frames {
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay_ms = if denominator == 0 {
            0
        } else {
            ((numerator as f64 / denominator as f64).round() as i32).max(0)
        };
        time_ms = time_ms.saturating_add(delay_ms);
        decoded.push(DecodedFrame {
            rgba: frame.into_buffer(),
            time_ms,
        });
    }

    // GIF has no per-actor loop count exposed through `image`'s decoder; 0 is the standard
    // infinite-loop animation parameter.
    Ok(Some((decoded, 0)))
}

fn decode_webp_frames(bytes: &[u8]) -> Result<Option<(Vec<DecodedFrame>, u32)>, AnimationError> {
    let animation = AnimDecoder::new(bytes)
        .decode()
        .map_err(AnimationError::WebpDecode)?;
    if !animation.has_animation() {
        return Ok(None);
    }

    let mut decoded = Vec::with_capacity(animation.len());
    for index in 0..animation.len() {
        let frame = animation
            .get_frame(index)
            .ok_or_else(|| AnimationError::Invalid("missing WebP animation frame".to_string()))?;
        let width = frame.width();
        let height = frame.height();
        let rgba = match frame.get_layout() {
            PixelLayout::Rgba => RgbaImage::from_raw(width, height, frame.get_image().to_vec())
                .ok_or_else(|| AnimationError::Invalid("bad RGBA frame buffer".to_string()))?,
            PixelLayout::Rgb => {
                let rgb = image::RgbImage::from_raw(width, height, frame.get_image().to_vec())
                    .ok_or_else(|| AnimationError::Invalid("bad RGB frame buffer".to_string()))?;
                DynamicImage::ImageRgb8(rgb).to_rgba8()
            }
        };
        decoded.push(DecodedFrame {
            rgba,
            time_ms: frame.get_time_ms(),
        });
    }

    Ok(Some((decoded, animation.loop_count)))
}

fn gif_frame_count(bytes: &[u8]) -> Option<usize> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(bytes)).ok()?;
    let mut count = 0usize;
    while decoder.next_frame_info().ok()?.is_some() {
        count += 1;
        if count > MAX_ANIMATION_FRAMES {
            break;
        }
    }
    Some(count)
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
        let mut config = WebPConfig::new().unwrap();
        config.lossless = 1;
        config.quality = 80.0;
        let mut encoder = AnimEncoder::new(8, 8, &config);
        let first = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255])).into_raw();
        let second = RgbaImage::from_pixel(8, 8, image::Rgba([0, 255, 0, 255])).into_raw();
        encoder.add_frame(WebpFrame::from_rgba(&first, 8, 8, 0));
        encoder.add_frame(WebpFrame::from_rgba(&second, 8, 8, 100));
        encoder.try_encode().unwrap().to_vec()
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

    #[test]
    fn converts_a_full_frame_animation_to_animated_webp() {
        let gif = make_test_gif();
        let result = convert_sync(&gif, "image/gif", 80, 8, None)
            .unwrap()
            .unwrap();
        assert!(!result.bytes.is_empty());
        let decoded = AnimDecoder::new(&result.bytes).decode().unwrap();
        assert!(decoded.has_animation());
        assert_eq!(decoded.len(), 2);
    }
}
