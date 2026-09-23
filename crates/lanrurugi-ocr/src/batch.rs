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
use crate::entities::{BoundingBox, DetectedTextRegion, PageNumber, WritingDirection};
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
    fn recognize(&self, crop: &RgbImage) -> Result<String, RecognizeHandleError>;
}

/// Why one crop couldn't be transcribed, split by whether re-running it later could plausibly
/// succeed.
///
/// The distinction is what lets `ensure_detected` decide whether a page's detection result is
/// safe to persist: an `Infrastructure` failure means this page's regions are silently incomplete
/// through no fault of the page itself, so caching them would freeze a degraded result forever
/// (issue #101). A `Content` rejection is a real, reproducible verdict about the crop and caching
/// it is correct.
#[derive(Debug, Clone, Error)]
pub enum RecognizeHandleError {
    /// The recognizer never got to judge the crop: the GPU worker RPC timed out, wasn't ready,
    /// failed to spawn/connect, or the transport itself broke. Transient by nature — the same
    /// crop under less resource pressure would likely transcribe fine.
    #[error("recognition infrastructure failure: {0}")]
    Infrastructure(String),
    /// The model ran and rejected the crop on its merits (low decode confidence, malformed output).
    /// Deterministic enough that re-running is expected to reach the same conclusion.
    #[error("recognition rejected the crop: {0}")]
    Content(String),
}

impl TextRecognizerHandle for crate::recognize::TextRecognizer {
    fn recognize(&self, crop: &RgbImage) -> Result<String, RecognizeHandleError> {
        crate::recognize::TextRecognizer::recognize(self, crop).map_err(|e| match e {
            // In-process recognition has no RPC layer to fail: a session-build, vocabulary-load,
            // or ORT inference error is a real infrastructure problem. `BadOutput` — which is
            // where the low-decode-confidence rejection lands — is the model's own verdict.
            crate::recognize::RecognitionError::Session { .. }
            | crate::recognize::RecognitionError::Vocab { .. }
            | crate::recognize::RecognitionError::Inference(_) => {
                RecognizeHandleError::Infrastructure(e.to_string())
            }
            crate::recognize::RecognitionError::BadOutput(_) => {
                RecognizeHandleError::Content(e.to_string())
            }
        })
    }
}

/// The reading axis OCR's projection pass could see in a crop.
///
/// Deliberately three-way rather than `Option<WritingDirection>`: `Ambiguous` is the signal that
/// triggers a second, rotated recognition pass, while `as_writing_direction` turns only the two
/// confident axes into persisted metadata (an ambiguous crop must leave `writing_direction: None`
/// so compositing's legacy aspect/script fallback still applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Vertical,
    Horizontal,
    Ambiguous,
}

impl Axis {
    fn as_writing_direction(self) -> Option<WritingDirection> {
        match self {
            Self::Vertical => Some(WritingDirection::Vertical),
            Self::Horizontal => Some(WritingDirection::Horizontal),
            Self::Ambiguous => None,
        }
    }
}

/// Rotates a crop 90° clockwise. Pure and public so callers/tests can reuse exactly the same
/// orientation that `run_batch` uses for its alternate candidate.
///
/// For a vertical Japanese text block (columns read right-to-left, each column top-to-bottom),
/// rotating the image clockwise makes the rightmost column the top row — the recognizer's
/// natural left-to-right reading order then matches the original reading order.
pub fn rotate_cw(crop: &RgbImage) -> RgbImage {
    image::imageops::rotate90(crop)
}

/// Finds contiguous runs in a projection whose value clears a small fraction of the projection's
/// own peak. Runs shorter than `min_run_len` are ignored as isolated stroke noise or crop-edge
/// artifacts; a real character column/row is at least ~15% of the crop's own length.
fn projection_runs(projection: &[u32], min_run_len: usize) -> Vec<(usize, usize)> {
    let peak = projection.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let threshold = ((peak as f32) * 0.20).ceil() as u32;
    let threshold = threshold.max(1);

    let mut raw: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &value) in projection.iter().chain(std::iter::once(&0u32)).enumerate() {
        if value >= threshold && start.is_none() {
            start = Some(i);
        } else if value < threshold {
            if let Some(start_index) = start.take() {
                raw.push((start_index, i - 1));
            }
        }
    }

    // Small one/two-pixel valleys between strokes of the same character are still the same
    // visual column/row; only a real inter-character gap should split a run.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in raw {
        if let Some(last) = merged.last_mut() {
            if start.saturating_sub(last.1) <= 2 {
                last.1 = end;
                continue;
            }
        }
        merged.push((start, end));
    }

    merged
        .into_iter()
        .filter(|(start, end)| end.saturating_sub(*start) + 1 >= min_run_len)
        .collect()
}

/// Coarse reading-axis inference from the crop's own ink projections.
///
/// A single vertical column is one x-run with several y-runs (characters stacked top-to-bottom);
/// a single horizontal line is one y-run with several x-runs. Both axes showing multiple runs is
/// the ambiguous grid case (a 2×N label, or a two-column bubble the detector cut into overlapping
/// boxes) that this feature exists to hand to the rotated/merged-crop candidate.
fn reading_axis(crop: &RgbImage, prior: &[bool]) -> Axis {
    let (w, h) = crop.dimensions();
    if w < 2 || h < 2 {
        return Axis::Ambiguous;
    }

    let use_prior = prior.len() == (w * h) as usize && prior.iter().any(|&p| p);
    let mut x_projection = vec![0u32; w as usize];
    let mut y_projection = vec![0u32; h as usize];
    let mut any_ink = false;
    for y in 0..h {
        for x in 0..w {
            let index = (y * w + x) as usize;
            let pixel = crop.get_pixel(x, y);
            let luma = (u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3;
            let dark = luma < 180;
            let inside_prior = !use_prior || prior[index];
            if dark && inside_prior {
                any_ink = true;
                x_projection[x as usize] += 1;
                y_projection[y as usize] += 1;
            }
        }
    }
    if !any_ink {
        return Axis::Ambiguous;
    }

    let min_x_run = (((w as f32) * 0.15).ceil() as usize).max(2);
    let min_y_run = (((h as f32) * 0.15).ceil() as usize).max(2);
    let x_runs = projection_runs(&x_projection, min_x_run).len();
    let y_runs = projection_runs(&y_projection, min_y_run).len();

    match (x_runs, y_runs) {
        // One clear y-band with several character strokes along x: a horizontal line.
        (2.., 1) => Axis::Horizontal,
        // One clear x-band with several character rows along y: a vertical column.
        (1, 2..) => Axis::Vertical,
        // Multiple bands in the cross-axis too: a grid, not a single line (the ambiguous case).
        (2.., 2..) => Axis::Ambiguous,
        // A single run in both axes is a lone glyph or touching text: fall back to the box's own
        // aspect ratio, the only weak signal left. Keep this deliberately conservative so a
        // square-ish SFX blob stays ambiguous rather than setting a confident wrong direction.
        (1, 1) => {
            let (w, h) = (w as f32, h as f32);
            if h >= w * 1.5 {
                Axis::Vertical
            } else if w >= h * 1.5 {
                Axis::Horizontal
            } else {
                Axis::Ambiguous
            }
        }
        _ => Axis::Ambiguous,
    }
}

/// Whether one region's recognition outcome means this page's result is incomplete for a
/// *transient* reason, and so must not be cached as authoritative (issue #101).
///
/// Only an infrastructure failure counts. A success obviously doesn't, and neither does a content
/// rejection — that verdict is reproducible, so dropping the region is the intended final answer.
fn is_infrastructure_failure(outcome: &Result<String, RecognizeHandleError>) -> bool {
    matches!(outcome, Err(RecognizeHandleError::Infrastructure(_)))
}

/// Whether at least half of `inner`'s own area lies inside `outer`. Used to count which raw
/// detector boxes actually contributed to a merged region before deciding to re-recognize that
/// region as one unit.
fn box_mostly_inside(inner: &BoundingBox, outer: &BoundingBox) -> bool {
    let ix0 = inner.x.max(outer.x);
    let iy0 = inner.y.max(outer.y);
    let ix1 = inner.right().min(outer.right());
    let iy1 = inner.bottom().min(outer.bottom());
    if ix1 <= ix0 || iy1 <= iy0 {
        return false;
    }
    let overlap = u64::from(ix1 - ix0) * u64::from(iy1 - iy0);
    (overlap as f64) >= 0.5 * (inner.area() as f64)
}

/// Whether two detector boxes are close enough that they are very likely fragments of one visual
/// text block (overlapping, or separated by a gap well under either box's own size). Used to skip
/// the per-fragment rotated-recognition fallback: re-reading each fragment of an already-fragmented
/// block independently is both expensive and unlikely to help the whole-block union pass, and the
/// extra RPCs are exactly what can push the cold GPU worker past its 60s recognition timeout.
fn boxes_touch(a: &BoundingBox, b: &BoundingBox) -> bool {
    if a.iou(b) > 0.0 {
        return true;
    }
    let (dx, dy) = a.gap(b);
    let min_w = a.w.min(b.w) as f32;
    let min_h = a.h.min(b.h) as f32;
    (dx as f32) <= min_w * 0.5 && (dy as f32) <= min_h * 0.5
}

/// Folds per-region infrastructure-failure flags into one flag per page.
///
/// Per page, not per batch: a batch can cover several pages, and one page's transient RPC failure
/// must not stop a different page in the same batch from being cached.
fn degraded_pages(
    page_count: usize,
    regions: impl IntoIterator<Item = (usize, bool)>,
) -> Vec<bool> {
    let mut degraded = vec![false; page_count];
    for (page_index, infra_failure) in regions {
        if infra_failure {
            degraded[page_index] = true;
        }
    }
    degraded
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
    /// At least one detected line box on this page was dropped because recognition failed for
    /// infrastructure reasons ([`RecognizeHandleError::Infrastructure`]), so `regions` is known to
    /// be incomplete through no fault of the page. Callers must not persist such a result as the
    /// authoritative record — see `translation_pipeline::ensure_detected` (issue #101).
    ///
    /// A content-level rejection does NOT set this: that verdict is reproducible, and dropping
    /// that region is the intended, cacheable outcome.
    pub degraded_by_infrastructure_failure: bool,
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

    /// The recognition handle this engine dispatches to — exposed so callers can run an
    /// additional recognition pass (e.g. `translation_pipeline`'s bubble-guided fallback for a
    /// bubble the detector never produced a text box for) without rebuilding an engine.
    pub fn recognizer(&self) -> Arc<dyn TextRecognizerHandle> {
        Arc::clone(&self.recognizer)
    }

    pub fn with_merge_config(mut self, merge_config: MergeConfig) -> Self {
        self.merge_config = merge_config;
        self
    }
}

/// Minimum connected-component area (in pixels) a detector prior's component must cover before
/// [`recognize_by_prior_components`] treats it as a real text column worth a separate recognition
/// call — rejects single-pixel/junk noise while keeping a genuine thin column.
const MIN_RETRY_COMPONENT_AREA: usize = 24;

/// Connected components (8-connectivity) of a row-major boolean mask — used only to split a
/// detection box whose own recognition failed into the separate text columns its detector prior
/// actually contains. Returns each component as a list of row-major pixel indices.
fn prior_components(prior: &[bool], w: u32, h: u32) -> Vec<Vec<usize>> {
    let (w, h) = (w as usize, h as usize);
    debug_assert_eq!(prior.len(), w * h);
    let mut seen = vec![false; prior.len()];
    let mut components = Vec::new();
    for start in 0..prior.len() {
        if !prior[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut pixels = Vec::new();
        while let Some(index) = stack.pop() {
            pixels.push(index);
            let (x, y) = ((index % w) as i32, (index / w) as i32);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let neighbour = ny as usize * w + nx as usize;
                    if prior[neighbour] && !seen[neighbour] {
                        seen[neighbour] = true;
                        stack.push(neighbour);
                    }
                }
            }
        }
        components.push(pixels);
    }
    components
}

/// Retries a crop whose own recognition failed by splitting its detector prior into connected
/// components and recognizing each component's own tight crop separately, then concatenating the
/// non-empty results in reading order.
///
/// Real reported incident (2026-09-19, issue #101): the DB detector emitted an over-wide box
/// `(156,102,83,248)` covering the right column `種付け人に` *plus* a 9px-wide sliver of the
/// neighbouring left column `任命されました`. `manga-ocr` returned an empty string for that mixed
/// crop, and `run_batch` dropped the line outright — before `merge_lines` ever ran — so the whole
/// right column was never translated or erased. The same prior, split into its two components
/// (a 35px-wide right column and a 9px sliver), gives the recognizer a clean single-column crop
/// for the real text.
///
/// Returns `None` when there is nothing to split, the prior doesn't match the crop, or no
/// component produced non-empty text.
fn recognize_by_prior_components(
    recognizer: &dyn TextRecognizerHandle,
    crop: &RgbImage,
    prior: &[bool],
) -> Option<String> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 || prior.len() != (w * h) as usize {
        return None;
    }
    let mut components: Vec<(u32, u32, u32, u32)> = prior_components(prior, w, h)
        .into_iter()
        .filter_map(|pixels| {
            if pixels.len() < MIN_RETRY_COMPONENT_AREA {
                return None;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
            for &index in &pixels {
                let (x, y) = ((index as u32) % w, (index as u32) / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
            Some((x0, y0, x1 - x0, y1 - y0))
        })
        .collect();
    if components.is_empty() {
        return None;
    }
    // Drop thin slivers of a neighbouring column that an over-wide detection box clipped in (the
    // real incident's left-column component was 9px wide against the right column's 35px): they
    // are not a real text line of their own, and recognizing one could splice a fragment of the
    // neighbouring column into this column's text. Comparable-width components are all kept, so
    // a genuine multi-column box still splits into each of its real columns.
    const MIN_COMPONENT_WIDTH_RATIO: f32 = 0.35;
    let max_component_width = components
        .iter()
        .map(|(_, _, cw, _)| *cw)
        .max()
        .unwrap_or(0);
    components.retain(|(_, _, cw, _)| {
        (*cw as f32) >= MIN_COMPONENT_WIDTH_RATIO * max_component_width as f32
    });
    if components.is_empty() {
        return None;
    }
    // Reading order: a vertical detection box's components are columns (right-to-left); a
    // horizontal box's are left-to-right. `h >= w` is the same orientation proxy
    // `lanrurugi_ocr::merge` uses for its own reading-order pass.
    if h >= w {
        components.sort_by_key(|(x, y, _, _)| (std::cmp::Reverse(*x), *y));
    } else {
        components.sort_by_key(|(x, y, _, _)| (*y, *x));
    }

    const MARGIN: u32 = 2;
    let mut out = String::new();
    for (x, y, cw, ch) in components {
        let x0 = x.saturating_sub(MARGIN);
        let y0 = y.saturating_sub(MARGIN);
        let x1 = (x + cw + MARGIN).min(w);
        let y1 = (y + ch + MARGIN).min(h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let sub_crop = image::imageops::crop_imm(crop, x0, y0, x1 - x0, y1 - y0).to_image();
        if let Ok(text) = recognizer.recognize(&sub_crop) {
            let text = text.trim();
            if !text.is_empty() {
                out.push_str(text);
            }
        }
    }
    if !out.is_empty() {
        return Some(out);
    }

    // The component split can still fail if the detector's own prior merged the column's strokes
    // into one component that includes a neighbouring sliver (or if the component's tight crop
    // sits a little too tight for the recognizer). Fall back to a few increasingly aggressive
    // *geometric* sub-crops of the same box: an inset that trims any neighbouring-column sliver
    // off the edges, then the box's own halves in reading order. Each candidate is tried only
    // after the previous ones produced nothing, so a normal single-column box pays nothing.
    let inset_try = |x0: u32, y0: u32, x1: u32, y1: u32| -> Option<String> {
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let sub_crop = image::imageops::crop_imm(crop, x0, y0, x1 - x0, y1 - y0).to_image();
        recognizer
            .recognize(&sub_crop)
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    for fraction in [0.10f32, 0.18, 0.28] {
        let dx = (w as f32 * fraction) as u32;
        let dy = (h as f32 * fraction) as u32;
        if let Some(text) = inset_try(dx, dy, w.saturating_sub(dx), h.saturating_sub(dy)) {
            return Some(text);
        }
    }
    let mut halves = String::new();
    if h >= w {
        let mid = w / 2;
        let overlap = (w / 10).max(2);
        for (x0, x1) in [
            (mid.saturating_sub(overlap), w),
            (0, (mid + overlap).min(w)),
        ] {
            if let Some(text) = inset_try(x0, 0, x1, h) {
                halves.push_str(&text);
            }
        }
    } else {
        let mid = h / 2;
        let overlap = (h / 10).max(2);
        for (y0, y1) in [
            (0, (mid + overlap).min(h)),
            (mid.saturating_sub(overlap), h),
        ] {
            if let Some(text) = inset_try(0, y0, w, y1) {
                halves.push_str(&text);
            }
        }
    }
    (!halves.is_empty()).then_some(halves)
}

/// One content-failed recognition plus the successful same-page neighbour it should be merged
/// with before retrying.
struct FailureGroup {
    page_index: usize,
    /// The two boxes' union — the crop that gets recognized once more.
    union: BoundingBox,
    /// Indices into the recognition result vector that this group replaces.
    replace: Vec<usize>,
    /// Index of the successful neighbour whose style estimate the merged line inherits.
    style_from: usize,
}

/// Plans which content-failed boxes should be merged with a successful neighbour and retried as
/// one crop.
///
/// Real reported incident (2026-09-19, page 12, issue #101): the skull bubble's right column
/// `なおこの命令は` recognized as an empty string on its own, while its left column
/// `皇帝陛下の勅命です` succeeded — so the right column was dropped and left untranslated. The two
/// columns' union crop recognizes cleanly (`なおこの命令は皇帝陛下の勅命です`) because `manga-ocr`
/// gets a coherent multi-column block instead of one ambiguous column. The same shape recurs for
/// the `種付け人に` / `任命されました` pair.
///
/// Only content failures are grouped; infrastructure failures are the caller's degraded/retry
/// path, not something a different crop can fix.
fn plan_failure_groups(
    page_indices: &[usize],
    boxes: &[BoundingBox],
    content_failed: &[bool],
    infra_failed: &[bool],
) -> Vec<FailureGroup> {
    let mut groups = Vec::new();
    let mut used = vec![false; boxes.len()];
    for i in 0..boxes.len() {
        if !content_failed[i] || infra_failed[i] || used[i] {
            continue;
        }
        let mut best: Option<(usize, f32)> = None;
        for j in 0..boxes.len() {
            if i == j
                || page_indices[i] != page_indices[j]
                || content_failed[j]
                || infra_failed[j]
                || used[j]
            {
                continue;
            }
            let (a, b) = (&boxes[i], &boxes[j]);
            let iou = a.iou(b);
            let (dx, dy) = a.gap(b);
            let min_w = a.w.min(b.w) as f32;
            let min_h = a.h.min(b.h) as f32;
            let near = iou > 0.0 || ((dx as f32) <= min_w * 0.8 && (dy as f32) <= min_h * 0.8);
            if !near {
                continue;
            }
            // Prefer real overlap; among equally-overlapping candidates prefer the closest one.
            let score = iou * 1_000.0 - (dx + dy) as f32;
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((j, score));
            }
        }
        if let Some((j, _)) = best {
            used[i] = true;
            used[j] = true;
            groups.push(FailureGroup {
                page_index: page_indices[i],
                union: boxes[i].union_with(&boxes[j]),
                replace: vec![i, j],
                style_from: j,
            });
        }
    }
    groups
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
        /// The detector's own raw text-probability prior for this box, row-major and matching
        /// `crop`'s dimensions — used by `recognize_by_prior_components` if recognition fails.
        prior: Vec<bool>,
        /// Projection-based reading axis, computed before the recognition dispatch so the
        /// post-recognition alternate pass can run knowing exactly which crops need it.
        axis: Axis,
    }

    /// One raw detector box after recognition, before paragraph-level merging. Carries the
    /// orientation / alternate-candidate metadata through the retry and merge stages so the
    /// final `DetectedTextRegion`s can inherit it.
    struct RecognizedWork {
        page_index: usize,
        bbox: crate::entities::BoundingBox,
        text: String,
        style: crate::style_estimate::StyleEstimate,
        infra_failure: bool,
        writing_direction: Option<WritingDirection>,
        alternate_source_text: Option<String>,
    }

    let mut work: Vec<RegionWork> = Vec::new();
    for (page_index, (page, boxes)) in pages.iter().zip(boxes_per_page.iter()).enumerate() {
        for bbox in boxes {
            if let Some(crop) = crop_region(&page.image, bbox) {
                let prior = detected_per_page[page_index]
                    .1
                    .crop_and_pack(bbox)
                    .to_bool_vec();
                let axis = reading_axis(&crop, &prior);
                work.push(RegionWork {
                    page_index,
                    bbox: *bbox,
                    crop,
                    prior,
                    axis,
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
                degraded_by_infrastructure_failure: false,
            })
            .collect());
    }

    // --- Recognition + style estimation: ONE batch-wide rayon dispatch ------------------------
    let recognize_engine = Arc::clone(&engine);
    // Kept for the content-failure union retry below, which runs after the primary recognition
    // dispatch has already consumed `recognize_engine`.
    let retry_engine = Arc::clone(&engine);

    // Collect ambiguous crops before `work` is consumed. This second recognition pass runs
    // serially (one crop at a time) below: the GPU container is memory-sensitive enough that
    // simply parallelising a second call per region is the documented crash risk.
    let rotate_work: Vec<(usize, RgbImage)> = work
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.axis == Axis::Ambiguous
                && !boxes_per_page[item.page_index]
                    .iter()
                    .any(|other| other != &item.bbox && boxes_touch(other, &item.bbox))
        })
        .map(|(index, item)| (index, item.crop.clone()))
        .collect();

    let recognized = parallel_map(work, move |item| {
        // Recognition failure for one region degrades that region to empty text rather than
        // failing the batch — FR-020's per-page isolation applies within a page too. An
        // infrastructure failure additionally flags the page as degraded, so the caller knows
        // this page's region set is incomplete for a transient reason and must not be cached.
        let outcome = recognize_engine.recognizer.recognize(&item.crop);
        let infra_failure = is_infrastructure_failure(&outcome);
        let mut text = match &outcome {
            Ok(text) => text.clone(),
            Err(e) => {
                tracing::warn!(error = %e, "region recognition failed; skipping region");
                String::new()
            }
        };
        // A content-level failure (empty output, low confidence, or a non-Japanese result) is
        // frequently the box's own mixed-column crop rather than a real "no text here" verdict —
        // retry per detector-prior component before dropping the line (see
        // `recognize_by_prior_components`'s own comment for the real incident this recovers).
        // Infrastructure failures are deliberately excluded: the caller's normal re-detection
        // path is what retries those, and re-calling a down worker here would just fail again.
        if !infra_failure
            && (text.trim().is_empty() || !crate::recognize::looks_like_japanese(&text))
        {
            match recognize_by_prior_components(
                recognize_engine.recognizer.as_ref(),
                &item.crop,
                &item.prior,
            ) {
                Some(recovered) => {
                    tracing::info!(
                        ?item.bbox,
                        "recovered recognition by splitting the detector prior into components"
                    );
                    text = recovered;
                }
                None => tracing::warn!(
                    ?item.bbox,
                    "prior-component retry found no readable crop either; dropping region"
                ),
            }
        }
        let style = estimate(&item.crop, 1);
        RecognizedWork {
            page_index: item.page_index,
            bbox: item.bbox,
            text,
            style,
            infra_failure,
            writing_direction: item.axis.as_writing_direction(),
            alternate_source_text: None,
        }
    })
    .await?;

    // --- Rotated candidate for ambiguous crops: serial, one recognition at a time -------------
    let mut recognized = recognized;
    if !rotate_work.is_empty() {
        let rotate_recognizer = engine.recognizer();
        let rotated = lanrurugi_core::concurrency::run_blocking(move || {
            rotate_work
                .into_iter()
                .map(|(index, crop)| (index, rotate_recognizer.recognize(&rotate_cw(&crop))))
                .collect::<Vec<_>>()
        })
        .await?;
        for (index, outcome) in rotated {
            match outcome {
                Ok(text) => {
                    let text = text.trim();
                    if !text.is_empty() && crate::recognize::looks_like_japanese(text) {
                        let existing = &mut recognized[index];
                        if existing.text.trim() != text {
                            tracing::info!(
                                bbox = ?existing.bbox,
                                original = %existing.text,
                                rotated = %text,
                                "ambiguous reading axis: rotated 90° candidate differs"
                            );
                            existing.alternate_source_text = Some(text.to_string());
                        }
                    }
                }
                Err(e) => tracing::debug!(
                    error = %e,
                    "rotated OCR candidate failed; keeping the original reading"
                ),
            }
        }
    }

    // --- Merge a content-failed box with a successful neighbour and retry the union once ------
    // See `plan_failure_groups`'s own doc comment for the real page-12 incident this recovers.
    {
        let page_indices: Vec<usize> = recognized.iter().map(|r| r.page_index).collect();
        let boxes: Vec<BoundingBox> = recognized.iter().map(|r| r.bbox).collect();
        let infra_failed: Vec<bool> = recognized.iter().map(|r| r.infra_failure).collect();
        let content_failed: Vec<bool> = recognized
            .iter()
            .map(|r| {
                !r.infra_failure
                    && (r.text.trim().is_empty() || !crate::recognize::looks_like_japanese(&r.text))
            })
            .collect();
        let groups = plan_failure_groups(&page_indices, &boxes, &content_failed, &infra_failed);
        if !groups.is_empty() {
            let work: Vec<(usize, RgbImage)> = groups
                .iter()
                .enumerate()
                .filter_map(|(index, group)| {
                    crop_region(&pages[group.page_index].image, &group.union)
                        .map(|crop| (index, crop))
                })
                .collect();
            let retried = parallel_map(work, move |(index, crop)| {
                (index, retry_engine.recognizer.recognize(&crop))
            })
            .await?;

            let mut keep = vec![true; recognized.len()];
            for group in &groups {
                for &index in &group.replace {
                    keep[index] = false;
                }
            }
            let mut recovered = Vec::new();
            for (index, outcome) in retried {
                let group = &groups[index];
                if let Ok(text) = outcome {
                    let text = text.trim();
                    if !text.is_empty() && crate::recognize::looks_like_japanese(text) {
                        tracing::info!(
                            union = ?group.union,
                            "recovered a failed region by recognizing its union with a successful neighbour"
                        );
                        let style_from = &recognized[group.style_from];
                        recovered.push(RecognizedWork {
                            page_index: group.page_index,
                            bbox: group.union,
                            text: text.to_string(),
                            style: style_from.style,
                            infra_failure: false,
                            writing_direction: style_from.writing_direction,
                            alternate_source_text: None,
                        });
                    }
                }
            }
            recognized = recognized
                .into_iter()
                .enumerate()
                .filter(|(index, _)| keep[*index])
                .map(|(_, item)| item)
                .collect();
            recognized.extend(recovered);
        }
    }

    // --- Regroup, merge into paragraph-level regions, reattach styles -------------------------
    let mut lines_per_page: Vec<Vec<DetectedLine>> = vec![Vec::new(); pages.len()];
    let mut styles: Vec<
        Vec<(
            crate::entities::BoundingBox,
            crate::style_estimate::StyleEstimate,
        )>,
    > = vec![Vec::new(); pages.len()];
    let degraded_per_page = degraded_pages(
        pages.len(),
        recognized.iter().map(|r| (r.page_index, r.infra_failure)),
    );

    for rec in recognized {
        if rec.text.trim().is_empty() {
            // Real reported incident (2026-09-18): a detected candidate box for a legitimate
            // second line of text ("トウカ", the second line of a two-line "皇族ガーディアン /
            // トウカ" label) silently vanished with no trace in any log — this branch was
            // previously silent, so there was no way to tell "recognition returned empty for a
            // real box" apart from "the box was never detected at all" without adding ad-hoc
            // diagnostics after the fact. Logging here even though it isn't (yet) proven to be
            // this exact bug's own root cause — it closes a real observability gap either way.
            tracing::warn!(
                ?rec.bbox,
                "recognition produced empty text for a detected box; dropping region"
            );
            continue;
        }
        // A region whose transcription doesn't look like real Japanese text (garbled output from
        // stylised/deformed SFX lettering `manga-ocr` failed to read — see `looks_like_japanese`'s
        // own docs) is dropped here rather than carried forward: better to leave the original
        // artwork untouched than translate garbage and composite a garbled block over legible
        // lettering.
        if !crate::recognize::looks_like_japanese(&rec.text) {
            tracing::warn!(%rec.text, "OCR result doesn't look like Japanese text; dropping region");
            continue;
        }
        lines_per_page[rec.page_index].push(DetectedLine::with_ocr_metadata(
            rec.bbox,
            rec.text,
            rec.alternate_source_text,
            rec.writing_direction,
        ));
        styles[rec.page_index].push((rec.bbox, rec.style));
    }

    let mut raw_masks_per_page: Vec<Option<crate::detect::PageTextProbabilityMap>> =
        detected_per_page
            .into_iter()
            .map(|(_, mask)| Some(mask))
            .collect();

    // Clone page images up front: the final map below consumes `pages`, but the post-merge
    // union-recognition pass needs the original pixel data for regions whose member boxes could
    // not be read independently. A batch is capped at `MAX_BATCH_PAGES` images, so the temporary
    // clone is bounded and short-lived.
    let page_images: Vec<RgbImage> = pages.iter().map(|page| page.image.clone()).collect();

    let mut results: Vec<PageOcrResult> = pages
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
                degraded_by_infrastructure_failure: degraded_per_page[i],
            }
        })
        .collect();

    // --- Whole-region candidate for detector-fragmented text blocks ---------------------------
    // The detector often splits one visual block (especially a multi-column label) into several
    // overlapping boxes. Recognizing each fragment independently can interleave characters even
    // though the union crop reads cleanly. This pass is deliberately serial and only runs for
    // regions built from at least two successful raw boxes, so a normal single-box region pays
    // nothing; a multi-line region only pays when the box set was fragmented enough to be worth
    // re-reading as one unit. Its result becomes `alternate_source_text`, not an automatic
    // replacement — the translation prompt has the LLM choose between the candidates.
    let mut union_work: Vec<(usize, usize, RgbImage, Axis)> = Vec::new();
    for (page_index, result) in results.iter().enumerate() {
        for (region_index, region) in result.regions.iter().enumerate() {
            // Use *all* detected boxes, not just the ones that survived recognition: a
            // fragmented label's rightmost/leftmost column can fail recognition on its own and be
            // dropped before `merge_lines`, which would otherwise leave the union crop one column
            // short (real page-15 label: `スーツ母娘` was recovered only after including the
            // dropped `ーッ` column).
            let fragments: Vec<&BoundingBox> = boxes_per_page[page_index]
                .iter()
                .filter(|bbox| box_mostly_inside(bbox, &region.bounding_box))
                .collect();
            if fragments.len() < 2 {
                continue;
            }
            let union_bbox = fragments
                .iter()
                .fold(region.bounding_box, |acc, bbox| acc.union_with(bbox));
            if let Some(crop) = crop_region(&page_images[page_index], &union_bbox) {
                let axis = reading_axis(&crop, &[]);
                union_work.push((page_index, region_index, crop, axis));
            }
        }
    }

    if !union_work.is_empty() {
        let union_recognizer = engine.recognizer();
        let outcomes = lanrurugi_core::concurrency::run_blocking(move || {
            union_work
                .into_iter()
                .map(|(page_index, region_index, crop, axis)| {
                    (
                        page_index,
                        region_index,
                        axis,
                        union_recognizer.recognize(&crop),
                    )
                })
                .collect::<Vec<_>>()
        })
        .await?;
        for (page_index, region_index, axis, outcome) in outcomes {
            let region = &mut results[page_index].regions[region_index];
            if let Some(direction) = axis.as_writing_direction() {
                region.writing_direction = Some(direction);
            }
            match outcome {
                Ok(text) => {
                    let text = text.trim();
                    if !text.is_empty()
                        && crate::recognize::looks_like_japanese(text)
                        && text != region.source_text.trim()
                    {
                        tracing::info!(
                            bbox = ?region.bounding_box,
                            original = %region.source_text,
                            whole_block = %text,
                            "merged-region union recognition produced a second reading candidate"
                        );
                        region.alternate_source_text = Some(text.to_string());
                    }
                }
                Err(e) => tracing::debug!(
                    error = %e,
                    "merged-region union recognition failed; keeping the per-box text"
                ),
            }
        }
    }

    Ok(results)
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

    fn blank_crop(w: u32, h: u32) -> RgbImage {
        RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]))
    }

    fn fill_dark(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32) {
        for py in y..y + h {
            for px in x..x + w {
                img.put_pixel(px, py, image::Rgb([10, 10, 10]));
            }
        }
    }

    #[test]
    fn reading_axis_sees_a_vertical_column() {
        let mut crop = blank_crop(60, 180);
        fill_dark(&mut crop, 15, 10, 30, 40);
        fill_dark(&mut crop, 15, 70, 30, 40);
        fill_dark(&mut crop, 15, 130, 30, 40);
        assert_eq!(reading_axis(&crop, &[]), Axis::Vertical);
    }

    #[test]
    fn reading_axis_sees_a_horizontal_line() {
        let mut crop = blank_crop(180, 60);
        fill_dark(&mut crop, 10, 15, 40, 30);
        fill_dark(&mut crop, 70, 15, 40, 30);
        fill_dark(&mut crop, 130, 15, 40, 30);
        assert_eq!(reading_axis(&crop, &[]), Axis::Horizontal);
    }

    #[test]
    fn reading_axis_treats_a_grid_as_ambiguous() {
        let mut crop = blank_crop(100, 100);
        for (x, y) in [(10, 10), (60, 10), (10, 60), (60, 60)] {
            fill_dark(&mut crop, x, y, 30, 30);
        }
        assert_eq!(reading_axis(&crop, &[]), Axis::Ambiguous);
    }

    #[test]
    fn rotate_cw_uses_clockwise_pixel_mapping() {
        let mut original = RgbImage::new(2, 1);
        original.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        original.put_pixel(1, 0, image::Rgb([0, 255, 0]));
        let rotated = rotate_cw(&original);
        assert_eq!(rotated.dimensions(), (1, 2));
        assert_eq!(rotated.get_pixel(0, 0), &image::Rgb([255, 0, 0]));
        assert_eq!(rotated.get_pixel(0, 1), &image::Rgb([0, 255, 0]));
    }

    /// Issue #101: the GPU worker RPC failing (timeout / not ready / connection lost / session
    /// died mid-call) means the recognizer never reached a verdict about this crop, so the page's
    /// region set is silently incomplete and `ensure_detected` must not cache it.
    #[test]
    fn an_infrastructure_failure_marks_the_region_degrading() {
        let outcome = Err(RecognizeHandleError::Infrastructure(
            "RPC call to lanrurugi-gpu-worker timed out after 60s".into(),
        ));
        assert!(is_infrastructure_failure(&outcome));
    }

    /// The case that must NOT change: the model ran and rejected the crop on its own merits. That
    /// verdict is reproducible, so the region is dropped and the page stays cacheable — re-running
    /// detection would only burn another inference call to reach the same conclusion.
    #[test]
    fn a_content_rejection_does_not_degrade_the_page() {
        let outcome = Err(RecognizeHandleError::Content(
            "decode confidence 0.057 below minimum 0.5".into(),
        ));
        assert!(
            !is_infrastructure_failure(&outcome),
            "a reproducible content verdict must stay cacheable"
        );
    }

    /// No regression on the happy path — a region that recognized cleanly never blocks caching.
    #[test]
    fn a_successful_recognition_does_not_degrade_the_page() {
        assert!(!is_infrastructure_failure(&Ok("こんにちは".to_string())));
    }

    /// The flag is per page, not per batch: one page hitting an infrastructure failure must not
    /// stop a *different* page in the same batch from being cached. `run_batch` aggregates by
    /// `page_index`, which this mirrors directly — a batch can cover up to `MAX_BATCH_PAGES`.
    #[test]
    fn degradation_is_tracked_per_page_not_across_the_whole_batch() {
        // One region on page 1 hit an RPC failure; pages 0 and 2 recognized cleanly.
        let degraded = degraded_pages(3, vec![(0, false), (1, true), (1, false), (2, false)]);

        assert_eq!(
            degraded,
            vec![false, true, false],
            "only the page that actually hit the failure is held back from caching"
        );
    }

    /// A page is degraded if *any* of its regions hit an infrastructure failure — the other
    /// regions succeeding is exactly the partial-result case issue #101 is about.
    #[test]
    fn one_failed_region_degrades_the_whole_page() {
        assert_eq!(
            degraded_pages(1, vec![(0, false), (0, true), (0, false)]),
            vec![true]
        );
    }

    /// A fully clean batch caches normally — the no-regression case.
    #[test]
    fn a_clean_batch_degrades_no_pages() {
        assert_eq!(
            degraded_pages(2, vec![(0, false), (1, false)]),
            vec![false, false]
        );
    }

    /// A recognizer whose result depends on the crop's own width — enough to model "the mixed
    /// multi-column box fails, but the split single-column sub-crops succeed", and to assert the
    /// order the components are concatenated in.
    struct WidthMapRecognizer {
        large_width: u32,
        full: &'static str,
        right: &'static str,
        left: &'static str,
    }

    impl TextRecognizerHandle for WidthMapRecognizer {
        fn recognize(&self, crop: &RgbImage) -> Result<String, RecognizeHandleError> {
            if crop.width() >= self.large_width {
                Ok(self.full.to_string())
            } else if crop.width() >= 15 {
                Ok(self.right.to_string())
            } else {
                Ok(self.left.to_string())
            }
        }
    }

    /// 80x200 light crop with an 8px wide dark sliver on the left and a 15px wide dark column on
    /// the right — the same shape as the real over-wide `種付け人に` detection box (issue #101).
    fn two_column_crop() -> (RgbImage, Vec<bool>) {
        let (w, h) = (80u32, 200u32);
        let mut crop = RgbImage::from_pixel(w, h, image::Rgb([235, 235, 235]));
        for y in 10..190 {
            for x in 0..8 {
                crop.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
            for x in 60..75 {
                crop.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        let mut prior = vec![false; (w * h) as usize];
        for y in 10..190u32 {
            for x in 0..8u32 {
                prior[(y * w + x) as usize] = true;
            }
            for x in 60..75u32 {
                prior[(y * w + x) as usize] = true;
            }
        }
        (crop, prior)
    }

    #[test]
    fn prior_component_retry_recovers_a_column_from_an_over_wide_box() {
        let (crop, prior) = two_column_crop();
        let recognizer = WidthMapRecognizer {
            large_width: 40,
            full: "",
            right: "種付け人に",
            left: "",
        };
        let recovered = recognize_by_prior_components(&recognizer, &crop, &prior)
            .expect("the right column's own component must be recoverable");
        assert_eq!(recovered, "種付け人に");
    }

    #[test]
    fn prior_component_retry_orders_vertical_columns_right_to_left() {
        let (crop, prior) = two_column_crop();
        let recognizer = WidthMapRecognizer {
            large_width: 40,
            full: "",
            right: "R",
            left: "L",
        };
        let recovered = recognize_by_prior_components(&recognizer, &crop, &prior)
            .expect("both real columns must be recovered");
        assert_eq!(recovered, "RL", "vertical columns read right-to-left");
    }

    #[test]
    fn prior_component_retry_drops_a_thin_sliver_of_a_neighbouring_column() {
        // The real over-wide box's left component was 9px wide against the right column's 35px;
        // recognizing that sliver could splice part of the neighbouring column's text in.
        let (mut crop, mut prior) = two_column_crop();
        // Shrink the left component from 8px to 3px (ratio 3/15 = 0.2 < 0.35).
        for y in 10..190u32 {
            for x in 0..8u32 {
                crop.put_pixel(x, y, image::Rgb([235, 235, 235]));
                prior[(y * 80 + x) as usize] = false;
            }
            for x in 0..3u32 {
                crop.put_pixel(x, y, image::Rgb([20, 20, 20]));
                prior[(y * 80 + x) as usize] = true;
            }
        }
        let recognizer = WidthMapRecognizer {
            large_width: 40,
            full: "",
            right: "R",
            left: "L",
        };
        let recovered = recognize_by_prior_components(&recognizer, &crop, &prior)
            .expect("the real column must still be recovered");
        assert_eq!(recovered, "R", "a thin sliver must not contribute text");
    }

    #[test]
    fn prior_component_retry_returns_none_when_nothing_is_readable() {
        let (crop, prior) = two_column_crop();
        let recognizer = WidthMapRecognizer {
            large_width: 0,
            full: "",
            right: "",
            left: "",
        };
        assert!(recognize_by_prior_components(&recognizer, &crop, &prior).is_none());
    }

    #[test]
    fn prior_component_retry_rejects_a_mismatched_prior() {
        let (crop, _) = two_column_crop();
        let recognizer = WidthMapRecognizer {
            large_width: 0,
            full: "",
            right: "",
            left: "x",
        };
        assert!(recognize_by_prior_components(&recognizer, &crop, &[true, false]).is_none());
    }

    #[test]
    fn failure_group_plans_a_union_with_a_successful_neighbour() {
        let pages = [0usize, 0, 0];
        let boxes = [
            BoundingBox::new(299, 1016, 40, 200),
            BoundingBox::new(267, 1015, 42, 238),
            BoundingBox::new(900, 100, 30, 30),
        ];
        let content_failed = [true, false, false];
        let infra_failed = [false, false, false];
        let groups = plan_failure_groups(&pages, &boxes, &content_failed, &infra_failed);
        assert_eq!(
            groups.len(),
            1,
            "the failed column must be grouped with its neighbour"
        );
        assert_eq!(groups[0].union, BoundingBox::new(267, 1015, 72, 238));
        assert_eq!(groups[0].replace, vec![0, 1]);
        assert_eq!(groups[0].style_from, 1);
    }

    #[test]
    fn failure_group_skips_infrastructure_failures_and_far_boxes() {
        let pages = [0usize, 0];
        let far = [
            BoundingBox::new(10, 10, 20, 20),
            BoundingBox::new(500, 500, 20, 20),
        ];
        assert!(plan_failure_groups(&pages, &far, &[true, false], &[false, false]).is_empty());
        // The failed box is an infrastructure failure — the caller's degraded path owns it, not
        // a union retry.
        assert!(plan_failure_groups(&pages, &far, &[true, false], &[true, false]).is_empty());
    }
}
