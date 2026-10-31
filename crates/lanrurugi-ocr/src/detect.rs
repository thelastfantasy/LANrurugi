//! Text-region **detection** via `oar-ocr`'s PP-OCR text detector (research.md §1).
//!
//! Detection answers only "where is there text on this page"; transcription is [`crate::recognize`]'s
//! job, against a manga-specific model, because PP-OCR's own recognition is trained on regular
//! documents and does poorly on vertical/stylized manga lettering.
//!
//! Note: research.md §1 assumed `oar-ocr` fetches and manages its own detection model. It does not —
//! `TextDetectionAdapterBuilder::build()` takes a `ModelSource` the caller supplies, so the
//! detection model is acquired and discovered alongside the recognition model
//! (`scripts/fetch-ocr-model.sh`, [`crate::model_discovery`]).
//!
//! This no longer goes through `oar-ocr`'s own `TextDetectionAdapter` — it builds and drives the
//! underlying `DBModel` directly (`preprocess` → `infer` → `postprocess`, each a public method).
//! `TextDetectionAdapter::execute` is a thin wrapper around exactly that same three-step call
//! (confirmed by reading its actual source, 2026-09-09) that only ever returns the *derived*
//! boxes/scores, discarding the raw `infer()` output — a real per-pixel text-probability map (the
//! ONNX graph's own final node is a `Sigmoid`, confirmed by reading the graph directly) that this
//! module's own [`TextDetector::detect_batch_with_raw_mask`] now surfaces alongside the boxes —
//! the same one real detection instance either produces both outputs from, or neither; there is no
//! second inference call anywhere in this module. `crate::batch::run_batch` crops and packs each
//! merged region's own slice of it into `crate::entities::DetectedTextRegion::raw_text_mask`
//! (via [`PageTextProbabilityMap::crop_and_pack`]), a coarse "text is definitely somewhere in
//! here" prior `lanrurugi_inpaint::stroke_mask::local_background_stroke_mask`'s own coverage
//! sanity check consumes downstream.

use std::path::Path;

use image::RgbImage;
use oar_ocr::models::detection::db::{
    DBModel, DBModelBuilder, DBPostprocessConfig, DBPreprocessConfig,
};
use oar_ocr::processors::{
    BoundingBox as OarBoundingBox, BoxType, ImageScaleInfo, LimitType, ScoreMode,
};
use thiserror::Error;

use crate::entities::BoundingBox;

#[derive(Debug, Error)]
pub enum DetectionError {
    #[error("failed to load the text-detection model: {0}")]
    ModelLoad(String),
    #[error("text detection failed: {0}")]
    Inference(String),
}

/// A detected box covering more than this fraction of the page's own area is discarded rather
/// than trusted — a real bug (2026-09-15): the DB detector's own probability map occasionally
/// bridges two genuinely unrelated pieces of lettering (a horizontal stamp/caption and a nearby
/// vertical speech-bubble line, in the reported case) into one connected component when the
/// background pixels between them score just high enough to read as "maybe text", producing a
/// single detection box that spans both. `unclip_ratio` (see
/// [`text_detection_postprocess_config`]) makes this worse, not better — it exists to expand a
/// *correct* contour outward to better cover a real line's own edges, so any accidental bridging
/// the raw contour already had gets expanded right along with it. No merge-stage fix downstream of
/// detection can undo this: `crate::merge::merge_lines` only ever sees "one box", never the two
/// (or more) real pieces of lettering DB's own probability map failed to separate, so nothing it
/// does can recover the correct grouping — this filter at the source is the fix. Confirmed against
/// a real page in this codebase's own reported bug: the anomalous box covered ~10% of the page
/// (388x470px against a 1136x1600 page).
///
/// Raised from `0.05` to `0.08` (2026-09-16, a real live incident): a genuine, correctly-detected
/// single text box — "不採用", a large stylised title/stamp printed across a whole sheet of paper —
/// covered ~6.1% of its own real page and was being silently discarded by this exact guard. `0.05`
/// was picked (see the incident above) only against that one bridging-bug box's own ~10%, without
/// ever checking it against a real large-title box's own legitimate footprint — this box is normal
/// in shape (aspect ratio ~1.4, nowhere near [`MAX_BOX_ASPECT_RATIO`]'s own territory), just
/// genuinely large, and got caught by a threshold that was never actually validated against that
/// case. `0.08` keeps clear rejection room below the confirmed-anomalous ~10% box while no longer
/// catching this confirmed-legitimate ~6.1% one. Losing a box entirely (rather than attempting to
/// split it) is still deliberate for whatever this guard *does* reject: a spurious merge silently
/// corrupts translation quality in a way a reader has no way to notice went wrong, while a dropped
/// region is at worst as bad as detection never having run for that spot (FR-019's own "reader
/// still gets to read the page" discipline extends the same way here).
const MAX_BOX_AREA_FRACTION: f64 = 0.08;

/// A detected box whose long side exceeds its short side by more than this ratio is discarded —
/// same reasoning and same real incident as [`MAX_BOX_AREA_FRACTION`]'s own doc comment (that
/// bug's own box was 388x470, a ratio under 1.5, which `MAX_BOX_AREA_FRACTION` alone already
/// catches; this second, independent guard exists for the *other* shape a bridged detection can
/// take — two lines connected by only a thin sliver of misclassified background pixels between
/// them produce a long, thin box far more extreme than any real line of manga lettering, vertical
/// or horizontal, is ever drawn at). `12.0` is well above any legitimate single line's own aspect
/// ratio (a full-height tategaki column on this codebase's own page sizes rarely exceeds ~8:1).
const MAX_BOX_ASPECT_RATIO: f64 = 12.0;

/// Whether `bbox` passes both anomaly guards above for a page of `page_w` x `page_h` pixels — see
/// [`MAX_BOX_AREA_FRACTION`]'s own doc comment for why this exists and why dropping the box
/// outright, not attempting to split it, is the correct response.
fn is_plausible_text_box(bbox: &BoundingBox, page_w: u32, page_h: u32) -> bool {
    let page_area = (page_w as f64) * (page_h as f64);
    if page_area <= 0.0 {
        return true;
    }
    if (bbox.area() as f64) / page_area > MAX_BOX_AREA_FRACTION {
        return false;
    }
    let (w, h) = (bbox.w.max(1) as f64, bbox.h.max(1) as f64);
    (w / h).max(h / w) <= MAX_BOX_ASPECT_RATIO
}

/// `TextDetectionAdapterBuilder`'s own defaults for non-seal text were `box_threshold: 0.6`
/// (confirmed by reading `oar-ocr-core`'s own `domain/adapters/text_detection_adapter.rs` and
/// `domain/adapters/preprocessing.rs::db_preprocess_for_text_type` directly, 2026-09-09) —
/// `DBModelBuilder::new()`'s own bare defaults (`DBPreprocessConfig::default()`/
/// `DBPostprocessConfig::default()`) are *not* the same values (notably `box_threshold` defaults
/// to `0.7` there) and must not be used unmodified, or this module's own detection boxes would
/// silently diverge from what `TextDetectionAdapter` used to produce for the exact same model/
/// input.
///
/// A real live incident (2026-09-15) found two real misses on one page: a small isolated
/// interjection bubble ("また…", a single short line off on its own with no bubble/panel touching
/// the main dialogue box next to it) and a stylised diagonal stamp/caption ("不採用", printed at an
/// angle across a shaded piece of paper rather than the usual horizontal/vertical grid) both went
/// completely undetected — not merged wrong, not garbled, simply never produced as a candidate box
/// at all. Lowering `box_threshold` to `0.5` was tried first and confirmed live *not* to recover
/// either miss (still absent from every detected region on a re-run) — the actual cause turned out
/// to be this function's own `limit_side_len`, not the confidence threshold: `box_threshold` only
/// ever filters candidates the model already scored, and at `960` this page's own long edge (1600px)
/// was downscaled by 1.67x before the model ever saw it, shrinking small/low-contrast lettering's
/// own pixel footprint below what the probability map could resolve at all, regardless of what
/// threshold was applied afterward. `limit_side_len: Some(1600)` (below) fixed both misses on a
/// real re-run. See [`text_detection_postprocess_config`]'s own doc comment for the `box_threshold`
/// value actually kept.
fn text_detection_preprocess_config() -> DBPreprocessConfig {
    DBPreprocessConfig {
        // Raised from `960` — see this function's own doc comment above for the real detection
        // miss this fixed and why `box_threshold` alone couldn't. `zyddnys/manga-image-translator`
        // (`DetectorConfig.detection_size`, confirmed by reading `manga_translator/config.py`
        // directly, 2026-09-15) defaults this same knob to `2048` for the same DB-family detector
        // family — `1600` is a deliberately more conservative middle ground (this codebase's own
        // page heights are typically ~1600px, so this value already means "don't downscale a
        // normal page at all" without matching that reference project's full number, which would
        // roughly double this call's own inference cost for pages that don't need it).
        limit_side_len: Some(1600),
        limit_type: Some(LimitType::Max),
        max_side_limit: Some(4000),
        resize_long: None,
    }
}

/// See [`text_detection_preprocess_config`]'s own doc comment — these are
/// `TextDetectionConfig::default()`'s own values (`oar-ocr-core`'s `domain/tasks/text_detection.rs`),
/// not `DBPostprocessConfig::default()`'s. `box_threshold` was tried lowered to `0.5` first (2026-
/// 09-15, chasing the same missed-detection incident `text_detection_preprocess_config`'s own doc
/// comment covers) and confirmed live *not* to recover either miss — kept at this crate's own
/// `0.6` default; `limit_side_len` (that same doc comment) was the fix that actually worked.
fn text_detection_postprocess_config() -> DBPostprocessConfig {
    DBPostprocessConfig {
        score_threshold: 0.3,
        box_threshold: 0.6,
        unclip_ratio: 1.5,
        max_candidates: 1000,
        use_dilation: false,
        score_mode: ScoreMode::Fast,
        box_type: BoxType::Quad,
    }
}

/// A loaded PP-OCR text detector.
///
/// Construction loads an ONNX session, so this is created once and reused across pages rather than
/// per request — building it per page would pay full model-load cost on every page turn.
pub struct TextDetector {
    model: DBModel,
    box_threshold: f32,
}

impl std::fmt::Debug for TextDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextDetector").finish_non_exhaustive()
    }
}

impl TextDetector {
    /// Loads the detector from an ONNX model file.
    ///
    /// No GPU execution provider is registered — CPU only, per research.md §1's OpenVINO-EP-hang
    /// finding (registering an EP an ORT build lacks can hang inside `session.run()` rather than
    /// failing cleanly). Matches this module's own previous `TextDetectionAdapterBuilder`-based
    /// construction exactly: neither ever explicitly configures an execution provider at all
    /// (`oar-ocr-core`'s own `OrtSessionConfig::execution_providers` field defaults to `None`,
    /// confirmed by reading its source, 2026-09-09), leaving `ort` to its own default provider
    /// selection — safe specifically because this codebase's own deployment targets never have GPU
    /// hardware available for `ort` to select in the first place; the OpenVINO incident this
    /// module's own doc comments reference was about *explicitly* registering a GPU EP an ORT
    /// build didn't actually support, not about `ort`'s own default fallback path.
    pub fn load(model_path: &Path) -> Result<Self, DetectionError> {
        let model = DBModelBuilder::new()
            .preprocess_config(text_detection_preprocess_config())
            .postprocess_config(text_detection_postprocess_config())
            .build(model_path.to_path_buf())
            .map_err(|e| DetectionError::ModelLoad(e.to_string()))?;
        Ok(Self {
            model,
            box_threshold: text_detection_postprocess_config().box_threshold,
        })
    }

    /// Diagnostic-only: same as [`Self::load`], but with `box_threshold` overridden — used to
    /// measure, on a real page, whether a detection miss whose ROI probability mean sits just under
    /// `box_threshold` (2026-09-18 investigation into "トウカ"/"美女3人" — see this module's own
    /// `text_detection_postprocess_config` doc comment for the box_threshold value actually shipped)
    /// would actually be recovered by lowering it, and how many new (possibly spurious) candidate
    /// boxes that same change produces elsewhere on the same page. Not used by any production call
    /// site — `detect_batch_with_raw_mask` still calls `text_detection_postprocess_config()`
    /// directly for its own postprocess call, so this constructor's `box_threshold` only actually
    /// takes effect wherever a caller also threads it through consistently (currently: nowhere in
    /// production, only `examples/diag_probmap.rs`).
    #[doc(hidden)]
    pub fn load_with_box_threshold(
        model_path: &Path,
        box_threshold: f32,
    ) -> Result<Self, DetectionError> {
        let mut postprocess = text_detection_postprocess_config();
        postprocess.box_threshold = box_threshold;
        let model = DBModelBuilder::new()
            .preprocess_config(text_detection_preprocess_config())
            .postprocess_config(postprocess)
            .build(model_path.to_path_buf())
            .map_err(|e| DetectionError::ModelLoad(e.to_string()))?;
        Ok(Self {
            model,
            box_threshold,
        })
    }

    /// Detects text boxes across a batch of pages in one inference call.
    ///
    /// Returns one `Vec<BoundingBox>` per input image, in input order. Batching happens here (the
    /// model takes many images natively) rather than by looping one call per page.
    pub fn detect_batch(
        &self,
        pages: Vec<RgbImage>,
    ) -> Result<Vec<Vec<BoundingBox>>, DetectionError> {
        Ok(self
            .detect_batch_with_raw_mask(pages)?
            .into_iter()
            .map(|(boxes, _mask)| boxes)
            .collect())
    }

    /// Single-page convenience wrapper over [`Self::detect_batch`].
    pub fn detect(&self, page: RgbImage) -> Result<Vec<BoundingBox>, DetectionError> {
        Ok(self
            .detect_batch(vec![page])?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    /// Same as [`Self::detect_batch`], but also returns each page's own raw per-pixel text
    /// probability map (the DB model's own `Sigmoid` output, before thresholding/unclip/NMS turn
    /// it into boxes) — row-major, resized back to that page's own real dimensions, one `f32` in
    /// `[0, 1]` per pixel. This is the same single inference call `detect_batch` itself makes; the
    /// mask is never computed by a second pass.
    ///
    /// Returns the *whole page's* own probability map, not yet cropped to any one region — callers
    /// that want a specific region's own prior (e.g. `crate::batch::run_batch`, to populate each
    /// merged region's own `crate::entities::DetectedTextRegion::raw_text_mask`) crop and
    /// bit-pack a sub-rectangle of it themselves via [`PageTextProbabilityMap::crop_and_pack`].
    ///
    /// Callers that only need boxes should use [`Self::detect_batch`]/[`Self::detect`] instead —
    /// building and resizing the per-page mask images has a real (if modest) cost this module's
    /// own box-only callers shouldn't pay for a value they never use.
    pub fn detect_batch_with_raw_mask(
        &self,
        pages: Vec<RgbImage>,
    ) -> Result<Vec<(Vec<BoundingBox>, PageTextProbabilityMap)>, DetectionError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }

        let page_sizes: Vec<(u32, u32)> = pages.iter().map(|p| p.dimensions()).collect();
        let (batch_tensor, img_shapes) = self
            .model
            .preprocess(pages)
            .map_err(|e| DetectionError::Inference(e.to_string()))?;
        let predictions = self
            .model
            .infer(&batch_tensor)
            .map_err(|e| DetectionError::Inference(e.to_string()))?;
        let output = self.model.postprocess(
            &predictions,
            img_shapes.clone(),
            text_detection_postprocess_config().score_threshold,
            self.box_threshold,
            text_detection_postprocess_config().unclip_ratio,
        );

        let mut result = Vec::with_capacity(page_sizes.len());
        for (i, &(page_w, page_h)) in page_sizes.iter().enumerate() {
            let boxes: Vec<BoundingBox> = output
                .boxes
                .get(i)
                .into_iter()
                .flatten()
                .filter_map(to_axis_aligned)
                .filter(|bbox| {
                    let ok = is_plausible_text_box(bbox, page_w, page_h);
                    if !ok {
                        tracing::warn!(
                            ?bbox,
                            page_w,
                            page_h,
                            "discarding an anomalously large/thin detected text box — see MAX_BOX_AREA_FRACTION's own doc comment"
                        );
                    }
                    ok
                })
                .collect();
            let mask = page_text_probability_map(&predictions, i, &img_shapes[i], page_w, page_h);
            result.push((boxes, mask));
        }
        Ok(result)
    }
}

/// Threshold applied to [`PageTextProbabilityMap`]'s own raw `f32` probabilities when packing a
/// region's own crop into a persisted `crate::entities::RawTextMask` — the *same* value
/// [`DBPostProcess`]'s own `thresh` field defaults to (confirmed by reading `oar-ocr-core`'s own
/// `processors/db_postprocess.rs`, 2026-09-09), i.e. this is the model's own creators' calibrated
/// binarization point for turning this exact probability output into a real/not-real pixel
/// decision, not a value picked independently of the model.
///
/// [`DBPostProcess`]: oar_ocr::processors::DBPostProcess
pub const RAW_TEXT_MASK_THRESHOLD: f32 = 0.3;

/// One page's own raw text-probability map, row-major, exactly `width * height` `f32` values in
/// `[0, 1]` — see [`TextDetector::detect_batch_with_raw_mask`]'s own doc comment. Deliberately
/// *not* the same type as `crate::entities::RawTextMask` (which is region-cropped, bit-packed, and
/// persisted): this one is the whole-page, full-precision working value `crate::batch::run_batch`
/// crops per region and thresholds/packs down from, not something meant to be stored as-is.
pub struct PageTextProbabilityMap {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
}

impl PageTextProbabilityMap {
    pub fn get(&self, x: u32, y: u32) -> f32 {
        self.values[(y * self.width + x) as usize]
    }

    /// Crops `bbox` out of this page-sized map, thresholds each pixel at
    /// [`RAW_TEXT_MASK_THRESHOLD`], and bit-packs the result into a
    /// `crate::entities::RawTextMask` ready to attach to that region's own
    /// `crate::entities::DetectedTextRegion::raw_text_mask`. `bbox` is clamped to this map's own
    /// bounds first — a region whose detection box runs even slightly past the page edge (possible
    /// at the very edge of a scan) must not panic here.
    pub fn crop_and_pack(&self, bbox: &BoundingBox) -> crate::entities::RawTextMask {
        let w = bbox.w.min(self.width.saturating_sub(bbox.x));
        let h = bbox.h.min(self.height.saturating_sub(bbox.y));
        let mut values = Vec::with_capacity((w * h) as usize);
        for ry in 0..h {
            for rx in 0..w {
                values.push(self.get(bbox.x + rx, bbox.y + ry) >= RAW_TEXT_MASK_THRESHOLD);
            }
        }
        crate::entities::RawTextMask::pack(w, h, &values)
    }
}

/// Slices `predictions` (the whole batch's own `[batch, 1, h, w]` Sigmoid output) down to page `i`
/// and resizes it from the model's own working resolution back to that page's real
/// `(page_w, page_h)` — same `ratio_h`/`ratio_w` scale-back `DBPostProcess` itself uses internally
/// to map its polygon coordinates to page space, applied here to the raw probability map instead.
fn page_text_probability_map(
    predictions: &ndarray::Array4<f32>,
    i: usize,
    shape: &ImageScaleInfo,
    page_w: u32,
    page_h: u32,
) -> PageTextProbabilityMap {
    let (_, channels, work_h, work_w) = predictions.dim();
    debug_assert_eq!(
        channels, 1,
        "DB model's own probability output is single-channel"
    );

    let mut work_image = image::GrayImage::new(work_w as u32, work_h as u32);
    for y in 0..work_h {
        for x in 0..work_w {
            let v = predictions[[i, 0, y, x]].clamp(0.0, 1.0);
            work_image.put_pixel(x as u32, y as u32, image::Luma([(v * 255.0).round() as u8]));
        }
    }

    // The model's own working image itself may include right/bottom padding beyond
    // `shape.src_h * shape.ratio_h` (`DetResizeForTest`'s own padding-to-a-multiple behaviour) —
    // crop to the real resized-content area *before* scaling back up, matching what
    // `DBPostProcess` itself does when mapping polygon coordinates back to page space.
    let content_w = ((shape.src_w * shape.ratio_w).round() as u32).min(work_w as u32);
    let content_h = ((shape.src_h * shape.ratio_h).round() as u32).min(work_h as u32);
    let content = if (content_w, content_h) == (work_w as u32, work_h as u32) {
        work_image
    } else {
        image::imageops::crop_imm(&work_image, 0, 0, content_w.max(1), content_h.max(1)).to_image()
    };

    let resized = image::imageops::resize(
        &content,
        page_w.max(1),
        page_h.max(1),
        image::imageops::FilterType::Triangle,
    );
    let values: Vec<f32> = resized
        .pixels()
        .map(|p| f32::from(p.0[0]) / 255.0)
        .collect();
    PageTextProbabilityMap {
        width: page_w,
        height: page_h,
        values,
    }
}

/// Converts `oar-ocr`'s polygon box (four `f32` points, possibly rotated) into the axis-aligned
/// integer box the rest of this crate works in.
///
/// Rotated text is preserved only as its enclosing upright box: this phase renders translated text
/// in upright boxes (research.md §11 keeps layout deliberately simple), so carrying the rotation
/// through would add complexity nothing downstream consumes.
fn to_axis_aligned(bbox: &OarBoundingBox) -> Option<BoundingBox> {
    if bbox.points.is_empty() {
        return None;
    }

    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);

    for p in &bbox.points {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }

    // Negative coordinates can appear when a detection runs past the page edge; clamp to origin.
    let x = min_x.max(0.0).round() as u32;
    let y = min_y.max(0.0).round() as u32;
    let w = (max_x - min_x).round().max(0.0) as u32;
    let h = (max_y - min_y).round().max(0.0) as u32;

    (w > 0 && h > 0).then(|| BoundingBox::new(x, y, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oar_ocr::processors::Point;

    fn poly(points: &[(f32, f32)]) -> OarBoundingBox {
        OarBoundingBox::new(points.iter().map(|&(x, y)| Point::new(x, y)).collect())
    }

    #[test]
    fn axis_aligned_box_from_upright_polygon() {
        let bb = to_axis_aligned(&poly(&[
            (10.0, 20.0),
            (60.0, 20.0),
            (60.0, 50.0),
            (10.0, 50.0),
        ]))
        .expect("upright polygon should convert");
        assert_eq!(bb, BoundingBox::new(10, 20, 50, 30));
    }

    #[test]
    fn rotated_polygon_becomes_its_enclosing_box() {
        let bb = to_axis_aligned(&poly(&[
            (30.0, 10.0),
            (50.0, 30.0),
            (30.0, 50.0),
            (10.0, 30.0),
        ]))
        .expect("rotated polygon should convert");
        assert_eq!(bb, BoundingBox::new(10, 10, 40, 40));
    }

    #[test]
    fn negative_coordinates_are_clamped_to_the_page() {
        let bb = to_axis_aligned(&poly(&[
            (-5.0, -8.0),
            (20.0, -8.0),
            (20.0, 15.0),
            (-5.0, 15.0),
        ]))
        .expect("partially off-page polygon should convert");
        assert_eq!(bb.x, 0);
        assert_eq!(bb.y, 0);
    }

    #[test]
    fn degenerate_polygon_is_rejected() {
        assert!(to_axis_aligned(&poly(&[])).is_none());
        // Zero-area box carries no text.
        assert!(to_axis_aligned(&poly(&[(5.0, 5.0), (5.0, 5.0)])).is_none());
    }

    #[test]
    fn a_normal_paragraph_box_is_plausible() {
        // The largest genuinely correct region from the real 2026-09-15 incident's own page
        // (~2% of a 1136x1600 page) — must never be rejected.
        let bbox = BoundingBox::new(882, 1210, 146, 258);
        assert!(is_plausible_text_box(&bbox, 1136, 1600));
    }

    #[test]
    fn an_anomalously_large_box_is_rejected() {
        // The real bridged-detection bug (2026-09-15): a stamp/caption and an unrelated speech-
        // bubble line connected into one ~10%-of-page detection box.
        let bbox = BoundingBox::new(423, 852, 388, 470);
        assert!(!is_plausible_text_box(&bbox, 1136, 1600));
    }

    #[test]
    fn a_large_genuine_title_box_is_no_longer_wrongly_rejected() {
        // The real over-tightened-threshold bug (2026-09-16): "不採用", a large stylised title
        // printed across a whole sheet of paper, was a single genuine, correctly-shaped detection
        // (aspect ratio ~1.4, nowhere near the anomalous shapes the other guard tests cover) that
        // still tripped the area guard at its old `0.05` because nobody had checked a real large
        // title's own footprint against it before picking that number.
        let bbox = BoundingBox::new(423, 852, 397, 280);
        assert!(is_plausible_text_box(&bbox, 1136, 1600));
    }

    #[test]
    fn an_extremely_thin_sliver_box_is_rejected() {
        // Small enough to pass the area guard alone, but a shape no real line of lettering (manga
        // or otherwise) is ever drawn at — the second, independent guard this is for.
        let bbox = BoundingBox::new(100, 100, 500, 20);
        assert!(!is_plausible_text_box(&bbox, 1136, 1600));
    }

    #[test]
    fn a_normal_vertical_column_is_plausible() {
        let bbox = BoundingBox::new(100, 100, 30, 200);
        assert!(is_plausible_text_box(&bbox, 1136, 1600));
    }
}
