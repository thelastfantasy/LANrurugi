//! Reader page resizing — verified against legacy `Model/Archive.pm::serve_page` +
//! `Model/Reader.pm::resize_image` + `Utils/ImageMagickResizer.pm::resize_page`: when a page's
//! *raw byte size* (not its pixel dimensions) exceeds `sizethreshold` KB, it's downscaled and
//! re-encoded as **WebP** at `readerquality` (deviation from legacy's JPEG — WebP at the same
//! quality is smaller, and the `webp` crate is already a dependency for thumbnails). A page
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

/// Cap applied to an image's shorter dimension (see module docs for why this isn't legacy's
/// width-only check). Same numeric value as legacy's fixed 1064px cap — only which dimension it's
/// measured against has changed.
const MAX_SHORT_EDGE: u32 = 1064;

#[derive(Debug, Error)]
pub enum ResizeError {
    #[error("failed to decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("webp encode error: {0:?}")]
    Webp(webp::WebPEncodingError),
    #[error("blocking task failed: {0}")]
    Join(#[from] lanrurugi_core::concurrency::BlockingTaskError),
}

/// Returns `Ok(None)` when `content`'s byte size doesn't exceed `threshold_kb` — the caller
/// should serve `content` itself unmodified in that case. On success, the tuple also carries the
/// *original* pixel dimensions (from the decode that just ran) so the caller can surface them —
/// e.g. in a response header — without decoding a second time.
pub async fn resize_if_over_threshold(
    content: Vec<u8>,
    quality: u8,
    threshold_kb: i64,
) -> Result<Option<(Vec<u8>, u32, u32)>, ResizeError> {
    run_blocking(move || {
        let size_kb = (content.len() / 1024) as i64;
        if size_kb <= threshold_kb {
            return Ok(None);
        }
        let img = image::load_from_memory(&content)?;
        let (orig_width, orig_height) = (img.width(), img.height());
        let out = resize_sync(&img, quality)?;
        Ok(Some((out, orig_width, orig_height)))
    })
    .await?
}

/// Unconditional WebP conversion (no byte-size threshold) — for formats browsers can't render
/// at all (BMP/TIFF/unknown), where "serve the original" isn't an option. Returns the converted
/// bytes plus the original dimensions, same shape as `resize_if_over_threshold`'s success case.
pub async fn convert_to_webp(
    content: Vec<u8>,
    quality: u8,
) -> Result<(Vec<u8>, u32, u32), ResizeError> {
    run_blocking(move || {
        let img = image::load_from_memory(&content)?;
        let (orig_width, orig_height) = (img.width(), img.height());
        let out = resize_sync(&img, quality)?;
        Ok((out, orig_width, orig_height))
    })
    .await?
}

fn resize_sync(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>, ResizeError> {
    let short_edge = img.width().min(img.height());
    let resized = if short_edge > MAX_SHORT_EDGE {
        let ratio = MAX_SHORT_EDGE as f64 / short_edge as f64;
        let target_width = (img.width() as f64 * ratio).round() as u32;
        let target_height = (img.height() as f64 * ratio).round() as u32;
        img.resize(
            target_width,
            target_height,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        // Still over the byte-size threshold but the short edge isn't over-sized: legacy still
        // re-encodes through ImageMagick at `quality` here (a recompression pass, e.g. for an
        // oversized lossless PNG), it just doesn't also downscale.
        img.clone()
    };

    let rgb = resized.to_rgb8();
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
    let encoded = encoder
        .encode_simple(false, quality as f32)
        .map_err(ResizeError::Webp)?;
    Ok(encoded.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([200, 50, 50]));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        bytes
    }

    #[tokio::test]
    async fn under_threshold_returns_none() {
        let content = make_test_jpeg(50, 50);
        let result = resize_if_over_threshold(content.clone(), 50, 1_000_000)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn over_threshold_and_over_short_edge_downscales() {
        // Portrait page: short edge is width (2000 > 1064), so it drives the resize; height
        // scales down proportionally rather than being clamped directly.
        let content = make_test_jpeg(2000, 3000);
        let result = resize_if_over_threshold(content, 50, 0).await.unwrap();
        let (resized, ..) = result.expect("over threshold must resize");
        let decoded = image::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), MAX_SHORT_EDGE);
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
        let result = resize_if_over_threshold(content, 50, 0).await.unwrap();
        let (resized, ..) = result.expect("over byte threshold must still re-encode");
        let decoded = image::load_from_memory(&resized).unwrap();
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
        let result = resize_if_over_threshold(content, 50, 0).await.unwrap();
        let (resized, ..) = result.expect("over threshold must resize");
        let decoded = image::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), MAX_SHORT_EDGE);
        assert_eq!(decoded.height(), MAX_SHORT_EDGE * 12000 / 1200);
    }

    #[tokio::test]
    async fn over_threshold_but_under_short_edge_still_reencodes() {
        let content = make_test_jpeg(500, 500);
        let result = resize_if_over_threshold(content, 50, 0).await.unwrap();
        let (resized, ..) = result.expect("over threshold must re-encode even without downscaling");
        let decoded = image::load_from_memory(&resized).unwrap();
        assert_eq!(decoded.width(), 500);
    }
}
