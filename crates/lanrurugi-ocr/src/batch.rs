//! Batched OCR over several pages at once (research.md §2).
//!
//! **Concurrency shape (constitution Principle III)**: the whole batch is dispatched through a
//! single `rayon`-parallel call bridged by one `tokio::task::spawn_blocking`
//! (`lanrurugi_core::concurrency::parallel_map`). It is explicitly NOT a `for` loop issuing one
//! `spawn_blocking` per page — that shape technically keeps CPU work off the async reactor while
//! still running the collection strictly sequentially, and is the exact anti-pattern the
//! constitution names after this codebase shipped it once in a thumbnail-regeneration job.
//!
//! Pages enter here as they enter the look-ahead prefetch window, not as a bulk pre-scan of the
//! whole archive: most users never read every page, so eager whole-archive OCR would waste work
//! and delay the first page.

use std::sync::Arc;

use image::RgbImage;
use lanrurugi_core::concurrency::{parallel_map, BlockingTaskError};
use lanrurugi_core::ids::ArchiveId;
use thiserror::Error;

use crate::detect::{DetectionError, TextDetector};
use crate::entities::{DetectedTextRegion, PageNumber};
use crate::merge::{merge_lines, DetectedLine, MergeConfig};
use crate::style_estimate::{crop_region, estimate};

/// Abstracts over "how a crop actually gets transcribed" — GPU EP integration plan v9 (issue
/// #103) moved the real `TextRecognizer` (and its CUDA sessions) out of this crate's caller into
/// a separate `lanrurugi-gpu-worker` subprocess, so `OcrEngine` can no longer own a concrete
/// `TextRecognizer` directly (that would mean depending on `lanrurugi-api`, an upward dependency
/// this crate must not take). Implementations: a direct in-process `TextRecognizer` (used by this
/// crate's own tests/examples, and by anything not needing the subprocess split), or
/// `lanrurugi-api::gpu_worker_client`'s RPC-forwarding wrapper in production.
///
/// Synchronous, not `async fn` — called from inside a `rayon` parallel-iterator closure
/// (`run_batch`'s own `parallel_map` call below), which itself already runs off the async reactor
/// via `spawn_blocking` (constitution Principle III); an RPC-backed implementation is expected to
/// bridge to its own async client via `tokio::runtime::Handle::block_on`, which is sound exactly
/// because this closure never runs on a true async-reactor thread.
pub trait TextRecognizerHandle: Send + Sync {
    fn recognize(&self, crop: &RgbImage) -> Result<String, String>;
}

impl TextRecognizerHandle for crate::recognize::TextRecognizer {
    fn recognize(&self, crop: &RgbImage) -> Result<String, String> {
        crate::recognize::TextRecognizer::recognize(self, crop).map_err(|e| e.to_string())
    }
}

/// Batch size bounds from research.md §2 — big enough to amortise dispatch, small enough that a
/// reader who just opened an archive isn't waiting for a full batch to fill.
pub const MIN_BATCH_PAGES: usize = 1;
pub const MAX_BATCH_PAGES: usize = 8;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error(transparent)]
    Detection(#[from] DetectionError),
    #[error("OCR batch task failed: {0}")]
    Blocking(#[from] BlockingTaskError),
}

/// One page queued for OCR.
pub struct PageInput {
    pub archive_id: ArchiveId,
    pub page_number: PageNumber,
    /// Set from Phase 1's own page metadata — never inferred from OCR output (FR-008).
    pub is_cover: bool,
    pub image: RgbImage,
    /// Real detected speech-bubble masks for this page, if bubble segmentation ran and succeeded
    /// before this call — threaded into `merge_lines` so it can fold visually-separated columns
    /// sharing one bubble into a single region (research.md's own note on issue #108) in the same
    /// pass as its geometry-only merging, rather than as a second, independent pass over this
    /// call's own output (see `merge_lines`'s own doc comment for why a single unified pass is
    /// what actually fixed a real 2026-09-15 bug two separate passes could not). `None` when
    /// bubble segmentation wasn't run or failed for this page — callers degrade to the
    /// geometry-only merge result, same fallback discipline
    /// `translation_pipeline::composite_and_cache`'s own bubble-segmentation call already
    /// established for page erasure.
    pub bubbles: Option<Vec<crate::bubble_segment::DetectedBubble>>,
}

/// Everything OCR produced for one page.
#[derive(Debug, Clone)]
pub struct PageOcrResult {
    pub archive_id: ArchiveId,
    pub page_number: PageNumber,
    pub regions: Vec<DetectedTextRegion>,
}

/// The loaded models, shared across a batch.
pub struct OcrEngine {
    detector: TextDetector,
    recognizer: Arc<dyn TextRecognizerHandle>,
    merge_config: MergeConfig,
}

impl OcrEngine {
    pub fn new(detector: TextDetector, recognizer: Arc<dyn TextRecognizerHandle>) -> Self {
        Self {
            detector,
            recognizer,
            merge_config: MergeConfig::default(),
        }
    }

    pub fn with_merge_config(mut self, merge_config: MergeConfig) -> Self {
        self.merge_config = merge_config;
        self
    }
}

/// Runs detection, recognition, merging, and per-region style estimation over a batch of pages.
///
/// Detection runs once for the whole batch (the underlying adapter is natively multi-image);
/// per-region recognition and style estimation — the expensive, embarrassingly parallel part — are
/// then dispatched as one `parallel_map` over every region in the batch, so rayon sees the entire
/// unit of work at once rather than one page (or one region) at a time.
pub async fn run_batch(
    engine: Arc<OcrEngine>,
    pages: Vec<PageInput>,
) -> Result<Vec<PageOcrResult>, BatchError> {
    if pages.is_empty() {
        return Ok(Vec::new());
    }

    // --- Detection: one inference call covering the batch ------------------------------------
    // `detect_batch_with_raw_mask`, not `detect_batch` — the same single inference call either
    // way (`crate::detect::TextDetector::detect_batch_with_raw_mask`'s own doc comment covers
    // why), but this also surfaces each page's own raw text-probability map, which gets cropped
    // and attached to each merged region below (`DetectedTextRegion::raw_text_mask`) as a coarse
    // prior for `lanrurugi_inpaint::stroke_mask::local_background_stroke_mask`'s own downstream
    // coverage sanity check.
    let detect_engine = Arc::clone(&engine);
    let images: Vec<RgbImage> = pages.iter().map(|p| p.image.clone()).collect();
    let detected_per_page = lanrurugi_core::concurrency::run_blocking(move || {
        detect_engine.detector.detect_batch_with_raw_mask(images)
    })
    .await??;
    let boxes_per_page: Vec<Vec<crate::entities::BoundingBox>> = detected_per_page
        .iter()
        .map(|(boxes, _)| boxes.clone())
        .collect();

    // --- Flatten to per-region work items -----------------------------------------------------
    // Flattening before dispatch is what lets one `parallel_map` cover the whole batch: rayon can
    // then balance regions across cores regardless of how unevenly text is distributed between
    // pages (a title page with one bubble next to a dense dialogue page).
    struct RegionWork {
        page_index: usize,
        bbox: crate::entities::BoundingBox,
        crop: RgbImage,
    }

    let mut work: Vec<RegionWork> = Vec::new();
    for (page_index, (page, boxes)) in pages.iter().zip(boxes_per_page.iter()).enumerate() {
        for bbox in boxes {
            if let Some(crop) = crop_region(&page.image, bbox) {
                work.push(RegionWork {
                    page_index,
                    bbox: *bbox,
                    crop,
                });
            }
        }
    }

    if work.is_empty() {
        // A page with no detectable text is a normal case, not an error (spec.md Edge Cases).
        return Ok(pages
            .into_iter()
            .map(|p| PageOcrResult {
                archive_id: p.archive_id,
                page_number: p.page_number,
                regions: Vec::new(),
            })
            .collect());
    }

    // --- Recognition + style estimation: ONE batch-wide rayon dispatch ------------------------
    let recognize_engine = Arc::clone(&engine);
    let recognized = parallel_map(work, move |item| {
        // Recognition failure for one region degrades that region to empty text rather than
        // failing the batch — FR-020's per-page isolation applies within a page too.
        let text = match recognize_engine.recognizer.recognize(&item.crop) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(error = %e, "region recognition failed; skipping region");
                String::new()
            }
        };
        let style = estimate(&item.crop, 1);
        (item.page_index, item.bbox, text, style)
    })
    .await?;

    // --- Regroup, merge into paragraph-level regions, reattach styles -------------------------
    let mut lines_per_page: Vec<Vec<DetectedLine>> = vec![Vec::new(); pages.len()];
    let mut styles: Vec<
        Vec<(
            crate::entities::BoundingBox,
            crate::style_estimate::StyleEstimate,
        )>,
    > = vec![Vec::new(); pages.len()];

    for (page_index, bbox, text, style) in recognized {
        if text.trim().is_empty() {
            continue;
        }
        // A region whose transcription doesn't look like real Japanese text (garbled output from
        // stylised/deformed SFX lettering `manga-ocr` failed to read — see `looks_like_japanese`'s
        // own docs) is dropped here rather than carried forward: better to leave the original
        // artwork untouched than translate garbage and composite a garbled block over legible
        // lettering.
        if !crate::recognize::looks_like_japanese(&text) {
            tracing::warn!(%text, "OCR result doesn't look like Japanese text; dropping region");
            continue;
        }
        lines_per_page[page_index].push(DetectedLine::new(bbox, text));
        styles[page_index].push((bbox, style));
    }

    let mut raw_masks_per_page: Vec<Option<crate::detect::PageTextProbabilityMap>> =
        detected_per_page
            .into_iter()
            .map(|(_, mask)| Some(mask))
            .collect();

    Ok(pages
        .into_iter()
        .enumerate()
        .map(|(i, page)| {
            let bubbles = page.bubbles.as_deref();
            let mut regions = merge_lines(
                &page.archive_id,
                page.page_number,
                page.is_cover,
                std::mem::take(&mut lines_per_page[i]),
                &engine.merge_config,
                bubbles,
            );

            // A merged region inherits the style of the line boxes it absorbed. Each attribute is
            // carried independently so one unknown never suppresses the others (FR-008a).
            for region in &mut regions {
                if let Some((_, style)) = styles[i]
                    .iter()
                    .find(|(bbox, _)| region.bounding_box.iou(bbox) > 0.0)
                {
                    region.fg_color = style.fg_color;
                    region.bg_color = style.bg_color;
                    region.is_bold = style.is_bold;
                }
            }

            // Cropped directly from the *merged* region's own final `bounding_box`, not
            // stitched together from the individual pre-merge line boxes it absorbed — simpler,
            // and the page-wide probability map already covers the full union area regardless of
            // how many lines merged into it.
            if let Some(page_mask) = raw_masks_per_page[i].take() {
                for region in &mut regions {
                    region.raw_text_mask = Some(page_mask.crop_and_pack(&region.bounding_box));
                }
            }

            PageOcrResult {
                archive_id: page.archive_id,
                page_number: page.page_number,
                regions,
            }
        })
        .collect())
}

/// Splits a look-ahead queue into batches within the research.md §2 size bounds.
pub fn chunk_pages<T>(pages: Vec<T>, batch_size: usize) -> Vec<Vec<T>> {
    let size = batch_size.clamp(MIN_BATCH_PAGES, MAX_BATCH_PAGES);
    let mut out = Vec::new();
    let mut current = Vec::with_capacity(size);

    for page in pages {
        current.push(page);
        if current.len() == size {
            out.push(std::mem::replace(&mut current, Vec::with_capacity(size)));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_respects_the_requested_batch_size() {
        let batches = chunk_pages((0..10).collect(), 4);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 4);
        assert_eq!(batches[2].len(), 2, "trailing partial batch is kept");
    }

    #[test]
    fn chunking_clamps_an_oversized_batch_request() {
        let batches = chunk_pages((0..20).collect(), 500);
        assert!(batches.iter().all(|b| b.len() <= MAX_BATCH_PAGES));
    }

    #[test]
    fn chunking_clamps_a_zero_batch_request() {
        // A zero batch size would otherwise loop forever or produce empty batches.
        let batches = chunk_pages((0..3).collect(), 0);
        assert_eq!(batches.len(), 3);
        assert!(batches.iter().all(|b| b.len() == 1));
    }

    #[test]
    fn chunking_empty_input_yields_no_batches() {
        assert!(chunk_pages(Vec::<u32>::new(), 4).is_empty());
    }
}
