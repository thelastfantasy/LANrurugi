//! Forced re-encoding of an arbitrary uploaded image to WebP, at its original pixel dimensions.
//!
//! Feedback attachments are the only caller: the tracker stores what it is given and renders only
//! WebP/PNG/JPEG/GIF inline, so one output format keeps both the byte cost and the rendering path
//! predictable. Unlike the reader's resize path, this never downscales — a bug screenshot keeps
//! every pixel — and it preserves animation wherever a frame decoder exists (GIF, animated JXL,
//! APNG) instead of silently freezing the first frame. An animated WebP needs no work at all: it is
//! already the output codec.

use std::io::Cursor;

use image::codecs::png::PngDecoder;
use image::metadata::LoopCount;
use image::AnimationDecoder as _;
use image::ImageDecoder as _;
use lanrurugi_core::concurrency::{run_blocking, BlockingTaskError};
use thiserror::Error;
use webpx::AnimationEncoder;

use crate::animation;

/// WebP's own canvas limit (libwebp rejects anything larger). Not a policy choice.
const MAX_WEBP_EDGE: u32 = 16_383;
/// Same CPU/time bound the reader's animation path uses.
const MAX_FRAMES: usize = 600;

#[derive(Debug, Error)]
pub enum WebpError {
    #[error("not a recognised image format")]
    Unsupported,
    #[error("could not decode the image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("could not encode WebP: {0}")]
    Encode(String),
    #[error("image is {0}x{1}, beyond WebP's {MAX_WEBP_EDGE}px limit")]
    TooLarge(u32, u32),
    #[error("the animation has more than {MAX_FRAMES} frames")]
    TooManyFrames,
    #[error("blocking task failed: {0}")]
    Join(#[from] BlockingTaskError),
}

/// Converts any recognised image (still or animated) to WebP, preserving pixel dimensions.
/// A WebP input is returned unchanged: it already is the target codec, and re-encoding it would
/// cost quality and CPU for nothing.
pub async fn convert_any_to_webp(content: Vec<u8>, quality: u8) -> Result<Vec<u8>, WebpError> {
    let kind = sniff(&content).ok_or(WebpError::Unsupported)?;
    if kind == Kind::Webp {
        return Ok(content);
    }
    // Refcounted from here so the animated attempt and the still fallback below share one buffer.
    let content = bytes::Bytes::from(content);
    if kind == Kind::Png && is_apng(&content) {
        // A frame cap or a frame the decoder rejects falls through to the still path, same as the
        // other animated formats below: the note is worth sending even if its motion is not.
        let apng = content.clone();
        match run_blocking(move || encode_apng(&apng, quality)).await? {
            Ok(bytes) => return Ok(bytes),
            Err(error) => {
                tracing::debug!(%error, "APNG conversion failed; keeping one frame")
            }
        }
    }
    // GIF and animated JXL: the reader's animation path already streams these frame by frame.
    // `None` means it cannot convert safely (offset frames, unknown container), in which case the
    // still path below keeps the first frame rather than failing the upload.
    if kind.supports_animation() {
        let content_type = kind.content_type();
        // A conversion that bails (a frame cap, offset frames) falls through to the still path:
        // the note is worth sending even if its animation cannot be preserved.
        match animation::convert_to_animated_webp(
            content.clone(),
            content_type,
            quality,
            u32::MAX,
            None,
        )
        .await
        {
            Ok(Some(animated)) => return Ok(animated.bytes),
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(%error, content_type, "animation conversion failed; keeping one frame")
            }
        }
    }
    run_blocking(move || encode_still(&content, quality)).await?
}

/// Input formats this module can recognise, by magic bytes. The enum exists so animation support
/// and the reported content type stay in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Jxl,
    Tiff,
    Avif,
    Jp2,
    Bmp,
}

impl Kind {
    fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Jxl => "image/jxl",
            Self::Tiff => "image/tiff",
            Self::Avif => "image/avif",
            Self::Jp2 => "image/jp2",
            Self::Bmp => "image/bmp",
        }
    }

    /// Whether the animation path has a frame decoder for this format. PNG is handled separately
    /// (APNG via `image`'s own decoder); TIFF is left to the still path (a multi-page fax is not a
    /// screenshot), and AVIF sequences and JP2 have no frame decoder at all.
    fn supports_animation(self) -> bool {
        matches!(self, Self::Gif | Self::Jxl)
    }
}

fn sniff(bytes: &[u8]) -> Option<Kind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(Kind::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(&bytes[8..12], b"avif" | b"avis")
    {
        Some(Kind::Avif)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(Kind::Tiff)
    } else if bytes.starts_with(b"BM") {
        Some(Kind::Bmp)
    } else if bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[
            0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
        ])
    {
        Some(Kind::Jxl)
    } else if bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' '])
        || bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51])
    {
        Some(Kind::Jp2)
    } else {
        None
    }
}

/// The `acTL` chunk, read by the PNG decoder itself rather than guessed from a byte scan (those
/// four bytes can also appear inside a text or image chunk of a still PNG).
fn is_apng(bytes: &[u8]) -> bool {
    PngDecoder::new(Cursor::new(bytes))
        .ok()
        .and_then(|decoder| decoder.is_apng().ok())
        .unwrap_or(false)
}

fn encode_still(bytes: &[u8], quality: u8) -> Result<Vec<u8>, WebpError> {
    let image = crate::image_decode::load_from_memory(bytes)?;
    if image.width() > MAX_WEBP_EDGE || image.height() > MAX_WEBP_EDGE {
        return Err(WebpError::TooLarge(image.width(), image.height()));
    }
    // Keep alpha only where the source has it: an opaque alpha plane is bytes for nothing.
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        webpx::Encoder::new_rgba(rgba.as_raw(), rgba.width(), rgba.height())
            .quality(quality as f32)
            .encode(webpx::Unstoppable)
            .map_err(|error| WebpError::Encode(error.to_string()))
    } else {
        let rgb = image.to_rgb8();
        webpx::Encoder::new_rgb(rgb.as_raw(), rgb.width(), rgb.height())
            .quality(quality as f32)
            .encode(webpx::Unstoppable)
            .map_err(|error| WebpError::Encode(error.to_string()))
    }
}

/// APNG → animated WebP. `image`'s decoder composes every frame onto the full canvas, so no
/// per-frame offset/disposal handling is needed here (unlike the GIF path).
fn encode_apng(bytes: &[u8], quality: u8) -> Result<Vec<u8>, WebpError> {
    let decoder = PngDecoder::new(Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions();
    if width > MAX_WEBP_EDGE || height > MAX_WEBP_EDGE {
        return Err(WebpError::TooLarge(width, height));
    }
    let apng = decoder.apng()?;
    let loop_count = match apng.loop_count() {
        LoopCount::Infinite => 0,
        LoopCount::Finite(count) => count.get(),
    };
    let mut encoder = AnimationEncoder::with_options(width, height, true, loop_count)
        .map_err(|error| WebpError::Encode(error.to_string()))?;
    encoder.set_quality(quality as f32);
    let frames = apng.into_frames();
    let mut count = 0usize;
    // WebP timestamps are frame *start* times; APNG delays are frame durations.
    let mut start_ms: i64 = 0;
    for frame in frames {
        let frame = frame?;
        count += 1;
        if count > MAX_FRAMES {
            return Err(WebpError::TooManyFrames);
        }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        // `checked_div` rather than a `denominator == 0` branch: clippy's `manual_checked_ops`,
        // and it keeps the divide and its guard on the same expression.
        let delay_ms = numerator
            .checked_div(denominator)
            .map(i64::from)
            .unwrap_or(0);
        let buffer = frame.into_buffer();
        encoder
            .add_frame_rgba(buffer.as_raw(), start_ms as i32)
            .map_err(|error| WebpError::Encode(error.to_string()))?;
        start_ms += delay_ms.max(1);
    }
    // A one-frame "animation" is just the still image.
    if count < 2 {
        return encode_still(bytes, quality);
    }
    encoder
        .finish(start_ms as i32)
        .map_err(|error| WebpError::Encode(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        if alpha {
            let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 128]));
            image
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
        } else {
            let image = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
            image
                .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
        }
        bytes
    }

    /// Three-frame APNG. Written through the `png` crate directly because the `image` crate's PNG
    /// encoder cannot produce animation.
    fn apng() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(3, 0).unwrap();
            let mut writer = encoder.write_header().unwrap();
            for color in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
                writer.set_frame_delay(1, 10).unwrap();
                writer.write_image_data(&color.repeat(4)).unwrap();
            }
        }
        bytes
    }

    fn gif() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut bytes, 2, 2, &[]).unwrap();
            encoder.set_repeat(gif::Repeat::Infinite).unwrap();
            // Distinct colours: libwebp coalesces identical consecutive frames, which would make
            // the frame-count assertion below lie about the input.
            for color in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
                let mut pixels = color.repeat(4);
                let mut frame = gif::Frame::from_rgba(2, 2, &mut pixels);
                frame.delay = 10;
                encoder.write_frame(&frame).unwrap();
            }
        }
        bytes
    }

    #[tokio::test]
    async fn a_png_keeps_its_pixel_dimensions() {
        let out = convert_any_to_webp(png(37, 23, false), 80).await.unwrap();
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (37, 23));
    }

    #[tokio::test]
    async fn a_gif_stays_animated() {
        let out = convert_any_to_webp(gif(), 80).await.unwrap();
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        // An animated WebP carries the ANIM chunk and one ANMF per frame.
        assert!(out.windows(4).any(|window| window == b"ANIM"));
        assert!(out.windows(4).any(|window| window == b"ANMF"));
        let frames = out.windows(4).filter(|window| *window == b"ANMF").count();
        assert_eq!(frames, 3);
    }

    #[tokio::test]
    async fn an_apng_stays_animated() {
        let out = convert_any_to_webp(apng(), 80).await.unwrap();
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        assert!(out.windows(4).any(|window| window == b"ANIM"));
        let frames = out.windows(4).filter(|window| *window == b"ANMF").count();
        assert_eq!(frames, 3);
    }

    #[tokio::test]
    async fn a_webp_is_passed_through_untouched() {
        let webp = convert_any_to_webp(png(4, 4, false), 80).await.unwrap();
        let again = convert_any_to_webp(webp.clone(), 80).await.unwrap();
        assert_eq!(again, webp);
    }

    #[tokio::test]
    async fn something_that_is_not_an_image_is_refused() {
        let error = convert_any_to_webp(b"<html>".to_vec(), 80)
            .await
            .unwrap_err();
        assert!(matches!(error, WebpError::Unsupported), "{error}");
    }

    #[tokio::test]
    async fn alpha_is_kept() {
        let out = convert_any_to_webp(png(5, 5, true), 80).await.unwrap();
        assert_eq!(&out[0..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        let decoded = image::load_from_memory(&out).unwrap();
        assert!(
            decoded.color().has_alpha(),
            "alpha must survive the re-encode"
        );
    }
}
