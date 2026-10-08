//! JPEG XL encoding.
//!
//! The decoder side lives in [`crate::image_decode`]. This module is the output side the reader
//! and thumbnail pipelines use when the caller advertises JXL support: it turns an already-decoded
//! RGB image into a JPEG XL codestream.
//!
//! The workspace deliberately uses the permissive pure-Rust `zune-jpegxl` encoder rather than a
//! libjxl wrapper. It is a lossless modular encoder: `quality` is mapped to encoding effort rather
//! than a lossy quality target. That keeps the feature build-safe and license-compatible; if the
//! project later adopts libjxl for true lossy JXL, this module is the single swap point.

use image::RgbImage;
use thiserror::Error;
use zune_core::bit_depth::BitDepth;
use zune_core::colorspace::ColorSpace;
use zune_core::options::EncoderOptions;
use zune_jpegxl::JxlSimpleEncoder;

#[derive(Debug, Error)]
pub enum JxlEncodeError {
    #[error("jxl encode error: {0}")]
    Encode(#[from] zune_jpegxl::JxlEncodeErrors),
}

/// Encodes an 8-bit interleaved RGB image as a JPEG XL codestream.
pub fn encode_rgb(image: &RgbImage, quality: u8) -> Result<Vec<u8>, JxlEncodeError> {
    let options = EncoderOptions::new(
        image.width() as usize,
        image.height() as usize,
        ColorSpace::RGB,
        BitDepth::Eight,
    )
    .set_effort(quality_to_effort(quality));
    let encoder = JxlSimpleEncoder::new(image.as_raw(), options);
    let mut out = Vec::new();
    encoder.encode(&mut out)?;
    Ok(out)
}

/// `zune-jpegxl` is lossless, so the UI's 0..100 quality value becomes libjxl's effort range
/// (1..=10): higher effort means more CPU and a smaller lossless file.
fn quality_to_effort(quality: u8) -> u8 {
    let quality = quality.min(100);
    (((quality as u16 * 9) / 100) + 1).clamp(1, 10) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn encodes_a_rgb_image_that_the_decoder_can_read_back() {
        let image = RgbImage::from_pixel(8, 8, Rgb([10, 120, 240]));
        let encoded = encode_rgb(&image, 85).unwrap();
        let decoded = crate::image_decode::load_from_memory(&encoded).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (8, 8));
    }

    #[test]
    fn quality_maps_to_valid_effort_range() {
        assert_eq!(quality_to_effort(0), 1);
        assert_eq!(quality_to_effort(100), 10);
    }
}
