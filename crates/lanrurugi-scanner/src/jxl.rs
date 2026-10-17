//! JPEG XL encoding.
//!
//! The decoder side lives in [`crate::image_decode`]. This module is the output side the reader
//! and thumbnail pipelines use when the caller advertises JXL support: it turns an already-decoded
//! RGB image into a JPEG XL codestream.
//!
//! The encoder is [`jixel`](https://crates.io/crates/jixel) via its `image`-crate bindings:
//!
//! * pure Rust, no C toolchain/CMake build step;
//! * `BSD-3-Clause OR Apache-2.0`, compatible with this project's license gate;
//! * lossy VarDCT, with the same 0..100 quality scale as the existing WebP path.
//!
//! That makes `readerquality` / thumbnail quality meaningful for JXL too, unlike the earlier
//! lossless-only encoder experiment.

use image::{ExtendedColorType, ImageEncoder, RgbImage};
use jixel_image_bindings::{EncodeConfig, JixelEncoder};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JxlEncodeError {
    #[error("jxl encode error: {0}")]
    Encode(#[from] image::ImageError),
}

/// Encodes an 8-bit interleaved RGB image as a lossy JPEG XL codestream at `quality` (0..=100).
pub fn encode_rgb(image: &RgbImage, quality: u8) -> Result<Vec<u8>, JxlEncodeError> {
    let mut config = EncodeConfig::default().with_quality(quality as f32);
    // The caller (reader page singleflight, thumbnail rayon batch, ...) already provides
    // concurrency. jixel defaults to one worker per logical CPU, which oversubscribes that outer
    // scheduler and made a single 1440x2040 encode peak at ~215MB RSS in our benchmark vs ~139MB
    // with one worker (same output size). Keep one worker here.
    config.num_threads = 1;
    let mut out = Vec::new();
    JixelEncoder::with_config(&mut out, config).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn encodes_a_rgb_image_that_the_decoder_can_read_back() {
        let image = RgbImage::from_pixel(64, 64, Rgb([10, 120, 240]));
        let encoded = encode_rgb(&image, 85).unwrap();
        let decoded = crate::image_decode::load_from_memory(&encoded).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 64));
    }

    #[test]
    fn rejects_quality_values_are_clamped_by_the_encoder() {
        let image = RgbImage::from_pixel(8, 8, Rgb([10, 120, 240]));
        // Quality is passed through to jixel's own clamp; this test just pins that extreme values
        // do not panic.
        assert!(encode_rgb(&image, 0).is_ok());
        assert!(encode_rgb(&image, 100).is_ok());
    }
}
