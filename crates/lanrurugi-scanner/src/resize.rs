//! Reader page resizing — verified against legacy `Model/Archive.pm::serve_page` +
//! `Model/Reader.pm::resize_image` + `Utils/ImageMagickResizer.pm::resize_page`: when a page's
//! *raw byte size* (not its pixel dimensions) exceeds `sizethreshold` KB, it's downscaled and
//! re-encoded as **WebP or JPEG XL** depending on the client's capability. WebP remains the
//! universal fallback; JXL uses the pure-Rust lossy encoder in [`crate::jxl`]. A page
//! already under the threshold is served unmodified — this module returns `None` for that case
//! rather than a no-op re-encode, so the caller can skip writing a cache entry identical to the
//! source file.
//!
//! **Deliberate deviation from legacy**: legacy's `resize_page` compares raw *width* against the
//! 1064px cap (`if ($origw > 1064) { $img->Resize(geometry => '1064x') }`), unconditionally. That
//! makes width alone decide whether to resize, which mishandles tall/narrow images — a Korean
//! webtoon-style strip (e.g. 800x8000) is well under the width cap but often exceeds
//! `sizethreshold` by byte size alone (long page, lots of content), and a legitimately wide image
//! doesn't need protecting from a *width* cap so much as from having its readable dimension
//! (whichever one is smaller) crushed. This module instead compares the **shorter** of the two
//! dimensions against the cap: a tall narrow strip's short edge is its width, so it's judged
//! correctly; a normal-aspect or landscape page's short edge is its height, which the old
//! width-only check didn't consider at all. Either way, resizing (when triggered) still scales
//! both dimensions by the same ratio — never distorts, just changes which dimension decides
//! *whether* to scale at all.
use lanrurugi_core::concurrency::run_blocking;
use thiserror::Error;

/// Output codec for the reader's re-encode path. WebP remains the universal fallback; JXL is only
/// selected after the frontend advertises JXL decode support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeFormat {
    Webp,
    Jxl,
}

impl EncodeFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jxl => "jxl",
        }
    }

    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jxl => "image/jxl",
        }
    }
}

/// Cap applied to an image's shorter dimension (see module docs for why this isn't legacy's
/// width-only check). Same numeric value as legacy's fixed 1064px cap — only which dimension it's
/// measured against has changed.
pub const MAX_SHORT_EDGE_DESKTOP: u32 = 1064;
/// Mobile pages get a more aggressive dimension cap. Quality is unchanged; this only lowers the
/// pixel dimensions before re-encoding, which is usually the cheapest bandwidth win on a phone.
pub const MAX_SHORT_EDGE_MOBILE: u32 = 960;
/// Preview/tiny images cap the *longest* edge at this value. They are intentionally upscaled by the
/// browser/frontend to the full page size for a seamless swap, so a low resolution is acceptable.
pub const MAX_LONG_EDGE_PREVIEW: u32 = 600;

#[derive(Debug, Error)]
pub enum ResizeError {
    #[error("failed to decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("webp encode error: {0}")]
    Webp(String),
    #[error("jxl encode error: {0}")]
    Jxl(#[from] crate::jxl::JxlEncodeError),
    #[error("blocking task failed: {0}")]
    Join(#[from] lanrurugi_core::concurrency::BlockingTaskError),
}

/// Returns `Ok(None)` when `content`'s byte size doesn't exceed `threshold_kb` — the caller
/// should serve `content` itself unmodified in that case. On success, the tuple also carries the
/// *original* pixel dimensions (from the decode that just ran) so the caller can surface them —
/// e.g. in a response header — without decoding a second time.
///
/// Takes `Bytes` rather than `Vec<u8>`: the caller usually still needs the original page bytes
/// after this returns (`Ok(None)` and the error path both serve them), and a refcounted clone
/// costs nothing where a `Vec` clone would copy the whole page.
pub async fn resize_if_over_threshold(
    content: bytes::Bytes,
    quality: u8,
    threshold_kb: i64,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    preview_full_short_edge: Option<u32>,
    format: EncodeFormat,
) -> Result<Option<(Vec<u8>, u32, u32)>, ResizeError> {
    resize_if_over_threshold_with_jxl_threads(
        content,
        quality,
        threshold_kb,
        max_short_edge,
        max_long_edge,
        preview_full_short_edge,
        format,
        1,
    )
    .await
}

/// [`resize_if_over_threshold`] with an explicit JXL worker count — see
/// [`convert_to_format_with_jxl_threads`].
#[allow(clippy::too_many_arguments)]
pub async fn resize_if_over_threshold_with_jxl_threads(
    content: bytes::Bytes,
    quality: u8,
    threshold_kb: i64,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    preview_full_short_edge: Option<u32>,
    format: EncodeFormat,
    jxl_threads: usize,
) -> Result<Option<(Vec<u8>, u32, u32)>, ResizeError> {
    run_blocking(move || {
        let size_kb = (content.len() / 1024) as i64;
        // A zero threshold means "always re-encode" (preview output, and any page whose source
        // codec differs from the format the client prefers); only a positive threshold gives the
        // usual raw-byte-size pass-through.
        if threshold_kb > 0 && size_kb <= threshold_kb {
            return Ok(None);
        }
        let (orig_width, orig_height) = original_dims(&content);
        let img = crate::image_decode::load_for_min_edge(
            &content,
            decode_edge(max_short_edge, max_long_edge),
        )?;
        let out = resize_sync(
            &img,
            PageTarget {
                source: (orig_width, orig_height),
                max_short_edge,
                max_long_edge,
                preview_full_short_edge,
            },
            quality,
            format,
            jxl_threads,
        )?;
        Ok(Some((out, orig_width, orig_height)))
    })
    .await?
}

/// Unconditional conversion (no byte-size threshold) — for formats browsers can't render at all
/// (BMP/TIFF/unknown), where "serve the original" isn't an option. Returns the converted bytes
/// plus the original dimensions, same shape as `resize_if_over_threshold`'s success case.
pub async fn convert_to_format(
    content: bytes::Bytes,
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    preview_full_short_edge: Option<u32>,
    format: EncodeFormat,
) -> Result<(Vec<u8>, u32, u32), ResizeError> {
    convert_to_format_with_jxl_threads(
        content,
        quality,
        max_short_edge,
        max_long_edge,
        preview_full_short_edge,
        format,
        1,
    )
    .await
}

/// [`convert_to_format`] for a request someone is waiting on: `jxl_threads` workers encode the
/// JXL output instead of one (floor of 1). WebP output ignores it.
pub async fn convert_to_format_with_jxl_threads(
    content: bytes::Bytes,
    quality: u8,
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    preview_full_short_edge: Option<u32>,
    format: EncodeFormat,
    jxl_threads: usize,
) -> Result<(Vec<u8>, u32, u32), ResizeError> {
    run_blocking(move || {
        let (orig_width, orig_height) = original_dims(&content);
        let img = crate::image_decode::load_for_min_edge(
            &content,
            decode_edge(max_short_edge, max_long_edge),
        )?;
        let out = resize_sync(
            &img,
            PageTarget {
                source: (orig_width, orig_height),
                max_short_edge,
                max_long_edge,
                preview_full_short_edge,
            },
            quality,
            format,
            jxl_threads,
        )?;
        Ok((out, orig_width, orig_height))
    })
    .await?
}

/// The page's own pixel dimensions, read from the header alone. Needed because
/// [`crate::image_decode::load_for_min_edge`] deliberately hands back a *smaller* bitmap than the
/// file's, while the response reports the original size and the reader sizes its boxes from it.
/// `(0, 0)` when the header cannot be parsed; the caller still has a usable bitmap.
fn original_dims(content: &[u8]) -> (u32, u32) {
    crate::image_decode::image_reader(std::io::Cursor::new(content))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .unwrap_or((0, 0))
}

/// The smallest side the decode has to keep: the box the page is scaled into. A preview is capped
/// by its long edge, so its short edge only has to reach that smaller number.
fn decode_edge(max_short_edge: u32, max_long_edge: Option<u32>) -> u32 {
    max_long_edge.map_or(max_short_edge, |long| max_short_edge.min(long))
}

/// Where a page should end up: the box it is scaled into, and the page's own size (which every
/// ratio below is computed from — see `resize_sync`'s own note).
struct PageTarget {
    source: (u32, u32),
    max_short_edge: u32,
    max_long_edge: Option<u32>,
    preview_full_short_edge: Option<u32>,
}

fn resize_sync(
    img: &image::DynamicImage,
    page: PageTarget,
    quality: u8,
    format: EncodeFormat,
    jxl_threads: usize,
) -> Result<Vec<u8>, ResizeError> {
    let PageTarget {
        source,
        max_short_edge,
        max_long_edge,
        preview_full_short_edge,
    } = page;
    // Every dimension is derived from the page's own size (`source`), never from `img`: a scaled
    // decode hands back a slightly smaller bitmap (half of 2859 is 1430, not 1429.5), and deriving
    // the target from that rounded intermediate changes the final edge by a pixel.
    let (source_width, source_height) = if source.0 == 0 || source.1 == 0 {
        (img.width(), img.height())
    } else {
        source
    };
    let short_edge = source_width.min(source_height);
    let long_edge = source_width.max(source_height);
    let target = if let Some(max_long) = max_long_edge {
        scaled_dims_for(source, (max_long as f64 / long_edge as f64).min(1.0))
    } else if short_edge > max_short_edge {
        scaled_dims_for(source, max_short_edge as f64 / short_edge as f64)
    } else {
        // Still over the byte-size threshold but the short edge isn't over-sized: legacy still
        // re-encodes through ImageMagick at `quality` here (a recompression pass, e.g. for an
        // oversized lossless PNG), it just doesn't also downscale. No bitmap is copied for that —
        // the encoder reads the decoded image directly.
        None
    };
    // `resize_exact`, not `resize`: the box is derived from the source's own aspect ratio (rounded
    // to integers), so exact scaling stretches by at most a fraction of a pixel — whereas
    // aspect-preserving `resize` rounds differently depending on the size of the bitmap it starts
    // from, which is exactly what made a scaled decode land one pixel short (#330).
    let resized =
        target.map(|(w, h)| img.resize_exact(w, h, image::imageops::FilterType::Lanczos3));
    let base = resized.as_ref().unwrap_or(img);

    // Preview: after reducing content to ~600px long edge, force the output canvas to the same
    // pixel dimensions as the full WebP version. The content is not improved—this is a cheap
    // upscale—but both variants now share the exact same dimensions, so the seamless swap needs
    // no frontend layout adaptation.
    let preview = preview_full_short_edge.and_then(|full_short| {
        let short_edge = source_width.min(source_height);
        let ratio = (full_short as f64 / short_edge as f64).min(1.0);
        scaled_dims_for(source, ratio)
            .map(|(w, h)| base.resize_exact(w, h, image::imageops::FilterType::Triangle))
    });
    let final_image = preview.as_ref().unwrap_or(base);

    // Encode straight from the decoded buffer when the decoder already produced RGB — the common
    // case (every JPEG page). `to_rgb8()` would clone that whole buffer first, and for
    // `ImageRgb8` it is literally `x.clone()`, so the copy is pure waste.
    match final_image.as_rgb8() {
        Some(rgb) => encode_rgb_with_threads(rgb, quality, format, jxl_threads),
        None => encode_rgb_with_threads(&final_image.to_rgb8(), quality, format, jxl_threads),
    }
}

/// The rounded target dimensions for a `source`-sized page scaled by `ratio`, or `None` when that
/// is exactly its current size — `DynamicImage::resize` clones the entire bitmap when the target
/// equals the current dimensions, which is never what a caller wants here.
fn scaled_dims_for(source: (u32, u32), ratio: f64) -> Option<(u32, u32)> {
    let width = ((source.0 as f64) * ratio).round().max(1.0) as u32;
    let height = ((source.1 as f64) * ratio).round().max(1.0) as u32;
    ((width, height) != source).then_some((width, height))
}

/// JXL accepts a thread count; WebP's encoder here is single-threaded regardless, so the count is
/// only forwarded to the JXL path.
fn encode_rgb_with_threads(
    rgb: &image::RgbImage,
    quality: u8,
    format: EncodeFormat,
    jxl_threads: usize,
) -> Result<Vec<u8>, ResizeError> {
    match format {
        EncodeFormat::Webp => webpx::Encoder::new_rgb(rgb.as_raw(), rgb.width(), rgb.height())
            .quality(quality as f32)
            .encode(webpx::Unstoppable)
            .map_err(|error| ResizeError::Webp(error.to_string())),
        EncodeFormat::Jxl => crate::jxl::encode_rgb_with_threads(rgb, quality, jxl_threads)
            .map_err(ResizeError::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_jpeg(width: u32, height: u32) -> bytes::Bytes {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([200, 50, 50]));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        bytes.into()
    }

    #[tokio::test]
    async fn a_scaled_decode_still_reports_the_source_dimensions() {
        // The decode is deliberately smaller than the file (#330), but `orig_width`/`orig_height`
        // describe the page on disk — the response reports them and the reader sizes boxes from
        // them, so a scaled decode must not leak its own smaller size into either.
        let content = make_test_jpeg(2000, 3000);
        let (bytes, orig_width, orig_height) = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!((orig_width, orig_height), (2000, 3000));
        let decoded = crate::image_decode::load_from_memory(&bytes).unwrap();
        assert_eq!(
            decoded.width().min(decoded.height()),
            MAX_SHORT_EDGE_DESKTOP
        );
    }

    #[tokio::test]
    async fn a_scaled_decode_produces_the_same_output_size_as_a_full_one() {
        // 2859x4000 is a real page size, and it is the case that caught this: half of 2859 is
        // 1430, not 1429.5, so deriving the target from the scaled bitmap's own size produced
        // 1064x1488 where the full decode produced 1064x1489. The output resolution must not
        // depend on which decode path ran (#330).
        let content = make_test_jpeg(2859, 4000);
        let (bytes, orig_width, orig_height) = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!((orig_width, orig_height), (2859, 4000));
        let decoded = crate::image_decode::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1064, 1489));
    }

    #[tokio::test]
    async fn under_threshold_returns_none() {
        let content = make_test_jpeg(50, 50);
        let result = resize_if_over_threshold(
            content.clone(),
            50,
            1_000_000,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zero_threshold_always_reencodes() {
        let content = make_test_jpeg(50, 50);
        let result = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        assert!(result.is_some(), "threshold 0 must force a re-encode");
    }

    #[tokio::test]
    async fn over_threshold_and_over_short_edge_downscales() {
        // Portrait page: short edge is width (2000 > 1064), so it drives the resize; height
        // scales down proportionally rather than being clamped directly.
        let content = make_test_jpeg(2000, 3000);
        let result = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        let (resized, ..) = result.expect("over threshold must resize");
        let decoded = crate::image_decode::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), MAX_SHORT_EDGE_DESKTOP);
        assert_eq!(decoded.height(), 1596); // round(3000 * 1064 / 2000)
    }

    #[tokio::test]
    async fn tall_narrow_webtoon_strip_is_not_downscaled() {
        // A Korean-webtoon-style strip: narrow (short edge = width = 800, under the cap) but very
        // tall, so it's easy for its byte size to exceed sizethreshold on content alone. Legacy's
        // width-only check would leave this alone too (width 800 < 1064), but a strip whose width
        // instead exceeded 1064 would've had its only-1064-wide short edge crushed under legacy's
        // logic just the same as under this one — the real fix is for cases where the *short*
        // edge is what's near the cap on an unusual aspect ratio, covered by the landscape test
        // above and the wide-webtoon test below. This test pins the still-correct "well within
        // bounds" case so a regression can't sneak in via the short-edge computation itself.
        let content = make_test_jpeg(800, 8000);
        let result = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        let (resized, ..) = result.expect("over byte threshold must still re-encode");
        let decoded = crate::image_decode::load_from_memory(&resized).unwrap();
        assert_eq!(
            decoded.width(),
            800,
            "short edge (width) is under the cap: no downscale"
        );
        assert_eq!(decoded.height(), 8000);
    }

    #[tokio::test]
    async fn wide_webtoon_strip_downscales_by_short_edge_not_width() {
        // A webtoon strip wide enough that width alone would trip legacy's old width-only check
        // (1200 > 1064) even though it's still the short edge here — confirms the resize targets
        // the short edge's ratio (preserving aspect ratio, not distorting) rather than crushing
        // width to exactly 1064 regardless of which edge that constrains.
        let content = make_test_jpeg(1200, 12000);
        let result = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        let (resized, ..) = result.expect("over threshold must resize");
        let decoded = crate::image_decode::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), MAX_SHORT_EDGE_DESKTOP);
        assert_eq!(decoded.height(), MAX_SHORT_EDGE_DESKTOP * 12000 / 1200);
    }

    #[tokio::test]
    async fn over_threshold_but_under_short_edge_still_reencodes() {
        let content = make_test_jpeg(500, 500);
        let result = resize_if_over_threshold(
            content,
            50,
            0,
            MAX_SHORT_EDGE_DESKTOP,
            None,
            None,
            EncodeFormat::Webp,
        )
        .await
        .unwrap();
        let (resized, ..) = result.expect("over threshold must re-encode even without downscaling");
        let decoded = crate::image_decode::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), 500);
    }
}
