//! Central image-decoding entry point.
//!
//! `image` itself has no built-in JPEG XL codec. The pure-Rust
//! [`jxl_image_rs_integration`] crate plugs a JPEG XL decoder into `image`'s own
//! format-detection/decoding hook system; calling [`register_image_decoders`] once makes
//! `image::load_from_memory` (and `ImageReader`) understand both bare JXL codestreams and
//! ISO-BMFF `.jxl` containers anywhere in this workspace.
//!
//! JPEG 2000 and AVIF stills, which `image` does not cover either (its AVIF feature is the
//! encoder), are decoded here through pure-Rust backends: `jpeg2k`'s `openjp2` port and
//! `avif-decode`'s `rav1d` AV1 decoder. Neither is reachable through `image`'s format guesser, so
//! this module recognises their signature boxes itself.
//!
//! Keep using the helpers in this module instead of calling `image::load_from_memory` directly:
//! the first call performs the one-time hook registration, so a source page can be JXL and still
//! flow through the existing thumbnail/resize/dimension paths without every caller having to
//! remember to register anything itself.

use std::sync::Once;

use image::{DynamicImage, ImageBuffer, ImageReader};

static REGISTER_IMAGE_DECODERS: Once = Once::new();

/// Registers the JPEG XL decoding hook exactly once. Safe to call from any thread; subsequent
/// calls are no-ops.
pub fn register_image_decoders() {
    // `Once::call_once` wants a closure returning `()`; the hook reports whether it actually
    // installed anything, which `Once` has already answered by construction.
    REGISTER_IMAGE_DECODERS.call_once(|| {
        jxl_image_rs_integration::register_image_decoding_hook();
    });
}

/// Drop-in replacement for `image::load_from_memory` that also understands JPEG XL and JPEG 2000.
pub fn load_from_memory(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    register_image_decoders();
    if is_jpeg2000(bytes) {
        return decode_jpeg2000(bytes);
    }
    if is_avif(bytes) {
        return decode_avif(bytes);
    }
    image::load_from_memory(bytes)
}

/// `avif` (still) and `avis` (sequence) ISO-BMFF brands. Only stills are handled here: an `avis`
/// page is routed to the animated path before it can reach this function, and `avif-decode` has no
/// frame decoder for it anyway.
fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis")
}

fn decode_avif(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    use avif_decode::Image as AvifImage;

    let decoded = avif_decode::Decoder::from_avif(bytes)
        .and_then(avif_decode::Decoder::to_image)
        .map_err(|error| {
            image::ImageError::Decoding(image::error::DecodingError::new(
                image::error::ImageFormatHint::Name("avif".to_string()),
                error,
            ))
        })?;
    // `ImgVec`'s accessors live on `ImgRef`: `width()`/`height()`/`pixels()`. `rgb`'s `Gray` keeps
    // its component behind `value()`.
    Ok(match decoded {
        AvifImage::Rgb8(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().flat_map(|p| [p.r, p.g, p.b]).collect();
            DynamicImage::ImageRgb8(decoded_buffer::<image::Rgb<u8>>(
                "avif", width, height, data,
            )?)
        }
        AvifImage::Rgba8(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect();
            DynamicImage::ImageRgba8(decoded_buffer::<image::Rgba<u8>>(
                "avif", width, height, data,
            )?)
        }
        AvifImage::Rgb16(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().flat_map(|p| [p.r, p.g, p.b]).collect();
            DynamicImage::ImageRgb16(decoded_buffer::<image::Rgb<u16>>(
                "avif", width, height, data,
            )?)
        }
        AvifImage::Rgba16(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().flat_map(|p| [p.r, p.g, p.b, p.a]).collect();
            DynamicImage::ImageRgba16(decoded_buffer::<image::Rgba<u16>>(
                "avif", width, height, data,
            )?)
        }
        AvifImage::Gray8(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().map(|p| p.value()).collect();
            DynamicImage::ImageLuma8(decoded_buffer::<image::Luma<u8>>(
                "avif", width, height, data,
            )?)
        }
        AvifImage::Gray16(image) => {
            let (width, height) = (image.width() as u32, image.height() as u32);
            let data = image.pixels().map(|p| p.value()).collect();
            DynamicImage::ImageLuma16(decoded_buffer::<image::Luma<u16>>(
                "avif", width, height, data,
            )?)
        }
    })
}

/// Drop-in replacement for `image::ImageReader::new` that also understands JPEG XL sources.
pub fn image_reader<R: std::io::BufRead + std::io::Seek>(reader: R) -> ImageReader<R> {
    register_image_decoders();
    ImageReader::new(reader)
}

/// The JP2 signature box, or a bare JPEG 2000 codestream (SOC + SIZ markers).
fn is_jpeg2000(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C, b'j', b'P', b' ', b' '])
        || bytes.starts_with(&[0xFF, 0x4F, 0xFF, 0x51])
        || bytes.windows(4).take(32).any(|window| window == b"jpx ")
}

/// Builds a decoded buffer, reporting the same "layout does not match the header" error for both
/// pure-Rust decoders below.
fn decoded_buffer<P>(
    format: &'static str,
    width: u32,
    height: u32,
    data: Vec<P::Subpixel>,
) -> image::ImageResult<ImageBuffer<P, Vec<P::Subpixel>>>
where
    P: image::Pixel + 'static,
    P::Subpixel: 'static,
{
    ImageBuffer::from_raw(width, height, data).ok_or_else(|| {
        image::ImageError::Decoding(image::error::DecodingError::new(
            image::error::ImageFormatHint::Name(format.to_string()),
            "decoded buffer does not match the reported dimensions",
        ))
    })
}

/// `jpeg2k` reports 8/16-bit grey, grey+alpha and RGB(A) buffers; every other layout it can
/// produce is outside what a reader page needs.
fn decode_jpeg2000(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    use jpeg2k::{Image as J2kImage, ImagePixelData};

    let image = J2kImage::from_bytes(bytes).map_err(|error| {
        image::ImageError::Decoding(image::error::DecodingError::new(
            image::error::ImageFormatHint::Name("jpeg2000".to_string()),
            error,
        ))
    })?;
    // sYCC (common in colour JP2, and what ffmpeg writes by default) is *not* converted: the
    // pure-Rust `openjp2` backend leaves component data null for it, and `ImageComponent::data()`
    // is an unchecked `from_raw_parts` — reading it aborts the process. `get_pixels` below reports
    // the unsupported colour space safely instead, and the caller then serves the original bytes.
    let pixels = image.get_pixels(None).map_err(|error| {
        image::ImageError::Decoding(image::error::DecodingError::new(
            image::error::ImageFormatHint::Name("jpeg2000".to_string()),
            error,
        ))
    })?;
    let (width, height) = (pixels.width, pixels.height);
    Ok(match pixels.data {
        ImagePixelData::L8(data) => DynamicImage::ImageLuma8(decoded_buffer::<image::Luma<u8>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::La8(data) => DynamicImage::ImageLumaA8(decoded_buffer::<image::LumaA<u8>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::Rgb8(data) => DynamicImage::ImageRgb8(decoded_buffer::<image::Rgb<u8>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::Rgba8(data) => DynamicImage::ImageRgba8(decoded_buffer::<image::Rgba<u8>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::L16(data) => DynamicImage::ImageLuma16(decoded_buffer::<image::Luma<u16>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::La16(data) => {
            DynamicImage::ImageLumaA16(decoded_buffer::<image::LumaA<u16>>(
                "jpeg2000", width, height, data,
            )?)
        }
        ImagePixelData::Rgb16(data) => DynamicImage::ImageRgb16(decoded_buffer::<image::Rgb<u16>>(
            "jpeg2000", width, height, data,
        )?),
        ImagePixelData::Rgba16(data) => {
            DynamicImage::ImageRgba16(decoded_buffer::<image::Rgba<u16>>(
                "jpeg2000", width, height, data,
            )?)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 JPEG XL codestream generated by `zune-jpegxl` (the same source used by the frontend's
    /// tiny browser-detection data URI). Embedding it here verifies the `image` crate hook path
    /// really decodes JXL bytes, not just that the dependency compiles.
    const TINY_JXL: &[u8] = &[
        0xFF, 0x0A, 0x08, 0x00, 0x02, 0x80, 0x48, 0x08, 0x02, 0x01, 0x00, 0xCC, 0x02, 0x4B, 0x18,
        0x9B, 0x9C, 0x71, 0x84, 0x03, 0x38, 0x80, 0x03, 0x38, 0x20, 0x4A, 0xC0, 0x39, 0x05, 0x01,
        0x00, 0x20, 0x44, 0x80, 0x08, 0x10, 0x01, 0x22, 0x40, 0x84, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF,
        0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55,
        0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE, 0x86, 0xFF, 0xF7,
        0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92, 0x24, 0x09, 0x01,
        0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE,
        0x86, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92,
        0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB,
        0xEE, 0xEE, 0xEE, 0x86, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED,
        0x73, 0x6F, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF,
        0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE, 0x3E, 0x00, 0xC7, 0xBF, 0x00, 0x3C, 0x00, 0x1E, 0xFE,
        0x8F, 0xFE, 0xE7, 0xFE, 0x87, 0xFF, 0xD5, 0x7F, 0xF2, 0xE3, 0xD1, 0x0F,
    ];

    /// A 4x4 JPEG 2000 image, generated with `ffmpeg -c:v jpeg2000`, proving the `jpeg2k` path is
    /// actually wired in (the `image` crate cannot route a `.jp2` file at all).
    const TINY_JP2: &[u8] = include_bytes!("../tests/fixtures/still_4x4.jp2");

    /// A 2x2 AVIF, generated with `ffmpeg -c:v libaom-av1 -f avif`. Decoding it exercises
    /// `image`'s `avif-native` (dav1d) decoder, which is the path a still AVIF page takes.
    const TINY_AVIF: &[u8] = include_bytes!("../tests/fixtures/still_2x2.avif");

    #[test]
    fn decodes_a_still_avif() {
        let decoded = load_from_memory(TINY_AVIF).expect("AVIF should decode through avif-native");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    #[test]
    fn decodes_a_jpeg2000_image() {
        let decoded = load_from_memory(TINY_JP2).expect("JP2 should decode through jpeg2k");
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
    }

    #[test]
    fn decodes_a_jxl_codestream_through_the_image_hook() {
        let decoded = load_from_memory(TINY_JXL).expect("JXL hook should decode a bare codestream");
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }
}
