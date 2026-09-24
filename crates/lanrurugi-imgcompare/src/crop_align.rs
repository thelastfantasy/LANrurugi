//! Detects when two versions of "the same page" have a different crop margin and/or scan
//! resolution (a real, common scenario — e.g. one scanner leaves a white border around the page,
//! another crops it off; scan resolution itself varies scanner to scanner) — so the magnifier
//! comparison UI can sample the *same underlying content* on both sides instead of the same raw
//! pixel coordinate, which would otherwise drift apart the moment the two images aren't pixel-
//! identical crops of each other.
//!
//! Multi-scale search: for a handful of candidate scale factors, find the translation that
//! maximizes normalized cross-correlation between A and B *at that scale*, sampling B via
//! bilinear interpolation at fractional grid coordinates `(x*scale + shift_x, y*scale + shift_y)`
//! — `scale` participates directly in the per-pixel lookup during correlation, not as an
//! intermediate resize stage. An earlier version resized B to a scale-dependent intermediate size
//! and then back down to a fixed grid for correlation; that composition mathematically cancels
//! `scale` out of the final coordinate correspondence (two resizes into the same fixed target size
//! converge regardless of any intermediate size), so it was never actually testing scale
//! correspondence — confirmed by deriving the coordinate math by hand, not by a passing/failing
//! test alone (the old version's tests happened to pass anyway, riding entirely on resampling-
//! filter noise rather than genuine content alignment, which would not have generalized reliably
//! to real page images). The scale/translation combination with the single best correlation
//! across all candidates is the answer — assumes only scale + translation differ (real
//! crop-margin/resolution differences), not rotation/shear, which this scenario doesn't produce.

use image::{DynamicImage, GrayImage};

/// Height (in px) both images are downsampled to before correlating — width follows each image's
/// OWN aspect ratio (A and B are downsampled independently, not forced to share one aspect ratio,
/// since bilinear sampling during correlation handles any size mismatch directly). Shared by both
/// [`estimate_crop_alignment_with_confidence`] (the coarse/fast algorithm) and
/// [`estimate_precise_crop_alignment_with_confidence`]'s own coarse pass — total search cost scales
/// roughly as this value to the 4th power (`scale-candidate-count × shift-range² × grid-pixels`,
/// and both shift-range and grid-pixels scale with this constant) — 128 measured 1.47s/call in
/// release, genuinely too slow to call more than a handful of times per comparison (a real concern
/// for the rescue path, which can call this once per orphaned page across a whole book, not just
/// the sampled-pairs-only magnifier/display use). 48 measured 38.8ms/call — confirmed live, not
/// assumed.
const CORRELATION_HEIGHT: u32 = 48;

/// Scale factors tried for B relative to A, centered on 1.0 (same size), for the FAST/COARSE
/// [`estimate_crop_alignment_with_confidence`] only — covers a realistic range of scan-resolution/
/// crop-margin differences without an unbounded (and much slower) search. Deliberately kept
/// separate from [`PRECISE_SCALE_CANDIDATES`]'s own wider range (see that constant's own docs for
/// why): the coarse function's own historical score range (this app's rescue-confidence threshold
/// was calibrated against it) shifts if this range changes, so it stays exactly as originally
/// shipped rather than being widened alongside the newer, separate precise algorithm.
const COARSE_SCALE_CANDIDATES: [f64; 7] = [0.85, 0.90, 0.95, 1.0, 1.05, 1.10, 1.15];

/// Scale factors tried for B relative to A, for [`estimate_precise_crop_alignment_with_confidence`]'s
/// own coarse pass only — wider than [`COARSE_SCALE_CANDIDATES`] (same candidate COUNT, coarser 0.1
/// step instead of 0.05 — the refine pass recovers the lost precision) after a real border-margin
/// test case exposed that the narrower range couldn't reach the correct answer at all: this
/// constant is in GRID-space (the coordinate-lookup `scale` — see `SearchPass`'s own docs), which
/// gets multiplied by each image's own real-height ratio to produce the PUBLIC `CropAlignment`'s
/// `scale` — for that real case (A 1500px tall, B 1680px tall, a real border needing an eventual
/// `CropAlignment.scale` of `1057/1151 ≈ 0.919`), the old range's own lowest candidate (0.85) could
/// only ever reach a public scale of `0.85 * (1680/1500) ≈ 0.952` — never low enough, regardless of
/// how well the search or the refine pass did around it.
const PRECISE_SCALE_CANDIDATES: [f64; 7] = [0.7, 0.8, 0.9, 1.0, 1.1, 1.2, 1.3];

/// How far the search looks for the best-matching translation, in units of A-grid pixels —
/// generous enough to find a real crop-margin offset within the downsampled frame without
/// exploding the search space. Shared by both the coarse and precise algorithms' own coarse pass.
const MAX_SHIFT_PX: i32 = (CORRELATION_HEIGHT / 4) as i32;

/// Second-pass resolution for [`estimate_precise_crop_alignment_with_confidence`]'s own refine
/// step — only ever searched over a NARROW neighborhood (see [`REFINE_SHIFT_RADIUS_PX`]/
/// [`REFINE_SCALE_DELTAS`]) around the coarse pass's own best result, not the full frame/scale
/// range, so the much higher per-pixel cost of this resolution is paid for a much smaller search
/// volume — confirmed live: a real crop-margin/border test case measured the refinement pass
/// itself at ~45ms (comparable to the coarse 38.8ms pass, not the ~25x-worse cost a FULL search at
/// this resolution would have per `CORRELATION_HEIGHT`'s own measured 128px/1.47s data point),
/// while correctly recovering border fractions the coarse-only 48px grid had rounded/clamped away
/// (that same real case had `offset+scale` land close enough to exactly 1.0 at 48px resolution
/// that a real border on two of the four edges was clamped to zero — see `computeEdgeBands`'s own
/// frontend-side docs for the concrete, user-visible symptom this caused: a synthetic pad rendered
/// with stripes on only 2 of the 4 real border edges). Not used by the coarse/fast algorithm at
/// all — that one is deliberately single-pass, kept fast for the rescue path's own call-volume.
const REFINE_HEIGHT: u32 = 128;

/// Search radius, in REFINE-grid pixels, around the coarse shift (converted from
/// `CORRELATION_HEIGHT`-grid to `REFINE_HEIGHT`-grid units first) — needs to comfortably cover the
/// coarse grid's own ±0.5-coarse-grid-pixel quantization error, converted to refine-grid units:
/// `0.5 * (REFINE_HEIGHT / CORRELATION_HEIGHT)` ≈ 1.33 at the current 48→128 ratio. Set well above
/// that (not just barely covering it) to also absorb some compounding error from the coarse scale
/// estimate itself being slightly off, not just shift quantization alone.
const REFINE_SHIFT_RADIUS_PX: i32 = 6;

/// Scale deltas tried around the coarse pass's own best scale, at [`REFINE_HEIGHT`] resolution —
/// deliberately a much finer step (0.025 vs [`PRECISE_SCALE_CANDIDATES`]'s own 0.1 grid-space
/// spacing) since this is now a local refinement around an already-good coarse estimate, not a
/// search that needs to cover the full realistic scale range from scratch. The radius (±0.05) is
/// set to at least HALF of `PRECISE_SCALE_CANDIDATES`'s own step — the true answer can fall
/// anywhere between two coarse candidates, so a refine radius smaller than half that spacing could
/// leave a gap neither the coarse nor the refine pass ever actually reaches.
const REFINE_SCALE_DELTAS: [f64; 5] = [-0.05, -0.025, 0.0, 0.025, 0.05];

/// Below this normalized cross-correlation score, the two images are treated as not reliably
/// alignable this way (likely a genuine content difference, not just crop/resolution) — callers
/// fall back to the identity transform rather than trusting a low-confidence guess.
const MIN_CONFIDENCE: f64 = 0.5;

/// A row/column's pixel-value range (max - min) below this is treated as part of a uniform scan
/// border, for [`detect_content_bounds`]'s own direct (correlation-free) border detection —
/// calibrated against real data, not guessed: a real scan border (WITH light scanner-realistic
/// speckle noise added, not perfectly flat) measured a range of ~50-57 (out of 255) at every edge
/// tested; real manga content (line art/screentone) measured 155+ even at its single
/// LOWEST-variance row/column anywhere in the whole test image — a wide (>2x) safety margin either
/// direction. Whole-frame normalized cross-correlation (this module's other, older technique) can
/// converge on a "wrong but higher-scoring" answer on richly self-similar content like manga line
/// art (confirmed live: the TRUE geometric alignment for a real border test case scored only 0.53,
/// while the correlation search's own chosen — geometrically wrong — answer scored 0.96) — a real
/// uniform border is a much less ambiguous signal, detected directly per-edge instead of inferred
/// from a search that can be fooled by repetitive structure.
const BORDER_UNIFORMITY_THRESHOLD: u8 = 90;

/// Caps how far [`detect_content_bounds`]'s inward scan can travel from any one edge, as a
/// fraction of that axis's own length — without this, a genuinely blank/near-blank page (a real,
/// if unusual, page) could scan almost to the image's own center and report a nonsensical "border"
/// covering nearly the whole frame instead of correctly finding no reliable signal.
const MAX_BORDER_SCAN_FRACTION: f64 = 0.4;

/// An affine transform mapping a point in A to the corresponding point in B, in NORMALIZED
/// per-own-dimension coordinates (the universal UV-texture-style convention: x divided by that
/// image's own width, y divided by that image's own height) — deliberately not real pixels or any
/// grid-internal unit, so a caller can apply it directly: `b_u = a_u * scale + offset_x`,
/// `b_v = a_v * scale + offset_y`, then multiply `b_u`/`b_v` by B's own real width/height to get a
/// real pixel coordinate. A single `scale` is shared by both axes (this module's search assumes B,
/// once corrected, shares A's own aspect ratio — a safe assumption for two scans of the same
/// physical page). Deliberately shaped as a general 2D transform (not just a `{scale, offset}`
/// pair) so a future need for e.g. per-axis scale or rotation can extend this without a breaking
/// change to callers already matching on `scale`/`offset_x`/`_y`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CropAlignment {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

impl CropAlignment {
    pub const IDENTITY: CropAlignment = CropAlignment {
        scale: 1.0,
        offset_x: 0.0,
        offset_y: 0.0,
    };
}

/// Downsamples `image` to `target_height`, preserving its OWN aspect ratio (not forced to match
/// any other image) — width follows from `target_height` and the image's own real dimensions.
fn downsample_gray(image: &DynamicImage, target_height: u32) -> GrayImage {
    let (width, height) = (image.width().max(1), image.height().max(1));
    let target_width = (target_height as f64 * width as f64 / height as f64)
        .round()
        .max(1.0) as u32;
    image
        .resize_exact(
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        )
        .to_luma8()
}

/// One image's own detected content region, as fractions (0..1) of its own width/height — see
/// [`detect_content_bounds`].
struct ContentBounds {
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

impl ContentBounds {
    fn has_any_border(&self) -> bool {
        self.top > 0.0 || self.bottom > 0.0 || self.left > 0.0 || self.right > 0.0
    }
}

/// Scans inward from each of `img`'s 4 edges, counting how many CONSECUTIVE rows/columns from
/// that edge have a pixel-value range (max - min) below [`BORDER_UNIFORMITY_THRESHOLD`] — a
/// direct, correlation-free signal for "this edge has an added scan border." All-zero for an
/// image with no detectable uniform border on any edge (the common case: two scans differing only
/// in resolution, not an added border) — bounded by [`MAX_BORDER_SCAN_FRACTION`] so a genuinely
/// blank/near-blank page can't scan almost to the image's own center.
fn detect_content_bounds(img: &GrayImage) -> ContentBounds {
    let (width, height) = img.dimensions();
    let row_range = |y: u32| -> u8 {
        let (mut lo, mut hi) = (255u8, 0u8);
        for x in 0..width {
            let v = img.get_pixel(x, y).0[0];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    };
    let col_range = |x: u32| -> u8 {
        let (mut lo, mut hi) = (255u8, 0u8);
        for y in 0..height {
            let v = img.get_pixel(x, y).0[0];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    };

    let max_vertical_scan = (height as f64 * MAX_BORDER_SCAN_FRACTION) as u32;
    let max_horizontal_scan = (width as f64 * MAX_BORDER_SCAN_FRACTION) as u32;

    let mut top = 0u32;
    while top < max_vertical_scan && row_range(top) < BORDER_UNIFORMITY_THRESHOLD {
        top += 1;
    }
    let mut bottom = 0u32;
    while bottom < max_vertical_scan.min(height.saturating_sub(top))
        && row_range(height - 1 - bottom) < BORDER_UNIFORMITY_THRESHOLD
    {
        bottom += 1;
    }
    let mut left = 0u32;
    while left < max_horizontal_scan && col_range(left) < BORDER_UNIFORMITY_THRESHOLD {
        left += 1;
    }
    let mut right = 0u32;
    while right < max_horizontal_scan.min(width.saturating_sub(left))
        && col_range(width - 1 - right) < BORDER_UNIFORMITY_THRESHOLD
    {
        right += 1;
    }

    ContentBounds {
        top: top as f64 / height as f64,
        bottom: bottom as f64 / height as f64,
        left: left as f64 / width as f64,
        right: right as f64 / width as f64,
    }
}

/// Bilinear-interpolated pixel value at fractional coordinate `(x, y)` — `None` if any of the four
/// surrounding source pixels would fall outside `img`'s own bounds (edge pixels are simply
/// excluded from the correlation rather than clamped/wrapped, matching `correlation_at_scale_shift`'s
/// own "only compare the real overlap" behavior for the identity/integer-shift case).
fn sample_bilinear(img: &GrayImage, x: f64, y: f64) -> Option<f64> {
    let (width, height) = img.dimensions();
    if x < 0.0 || y < 0.0 || width == 0 || height == 0 {
        return None;
    }
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    if x0 + 1 >= width || y0 + 1 >= height {
        return None;
    }
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let p00 = img.get_pixel(x0, y0).0[0] as f64;
    let p10 = img.get_pixel(x0 + 1, y0).0[0] as f64;
    let p01 = img.get_pixel(x0, y0 + 1).0[0] as f64;
    let p11 = img.get_pixel(x0 + 1, y0 + 1).0[0] as f64;
    Some(
        p00 * (1.0 - fx) * (1.0 - fy)
            + p10 * fx * (1.0 - fy)
            + p01 * (1.0 - fx) * fy
            + p11 * fx * fy,
    )
}

/// Normalized cross-correlation between `a` (sampled at its own integer grid coordinates) and `b`
/// (sampled via bilinear interpolation at `(x*scale + shift_x, y*scale + shift_y)` for each `a`
/// coordinate `(x, y)`) — `scale` and `shift` both participate directly in this per-pixel lookup,
/// which is what makes `scale` actually affect the result (see this module's own docs for why an
/// earlier version, routing `scale` through an intermediate resize instead, did not). Only pixels
/// where `b`'s bilinear sample is in-bounds are compared. Returns `None` if there's no usable
/// overlap at all.
fn correlation_at_scale_shift(
    a: &GrayImage,
    b: &GrayImage,
    scale: f64,
    shift_x: f64,
    shift_y: f64,
) -> Option<f64> {
    let (width, height) = a.dimensions();
    let mut sum_a = 0f64;
    let mut sum_b = 0f64;
    let mut sum_ab = 0f64;
    let mut sum_aa = 0f64;
    let mut sum_bb = 0f64;
    let mut count = 0u32;

    for y in 0..height {
        let by = y as f64 * scale + shift_y;
        for x in 0..width {
            let bx = x as f64 * scale + shift_x;
            let Some(bv) = sample_bilinear(b, bx, by) else {
                continue;
            };
            let av = a.get_pixel(x, y).0[0] as f64;
            sum_a += av;
            sum_b += bv;
            sum_ab += av * bv;
            sum_aa += av * av;
            sum_bb += bv * bv;
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }
    let n = count as f64;
    let mean_a = sum_a / n;
    let mean_b = sum_b / n;
    let numerator = sum_ab - n * mean_a * mean_b;
    let denominator = ((sum_aa - n * mean_a * mean_a) * (sum_bb - n * mean_b * mean_b)).sqrt();
    if denominator <= f64::EPSILON {
        // Both patches (near-)flat — no real structure to correlate on, e.g. a blank page.
        return None;
    }
    Some(numerator / denominator)
}

/// A single scale×shift correlation search over `a_grid`/`b_grid` — the coarse/fast algorithm's
/// own single pass, and the precise algorithm's own coarse (full scale-range, centered-at-origin
/// shift window) and refine (narrow scale delta, shift window re-centered on the coarse result)
/// passes, are all the exact same search operation at different resolutions/ranges, so all three
/// just build one of these and call [`SearchPass::run`] rather than duplicating the nested
/// scale/shift loop per pass. Integer shifts are enough at every resolution this module uses —
/// this is a localization search, not sub-pixel registration, and the eventual consumer (a
/// magnifier lens / a synthetic-pad overlay, not a scientific alignment tool) doesn't need finer
/// precision than that.
struct SearchPass<'a> {
    a_grid: &'a GrayImage,
    b_grid: &'a GrayImage,
    /// Scale factors to try, as absolute values (not deltas) — callers building a refine pass
    /// around a coarse result compute `coarse_scale + delta` for each of their own deltas
    /// themselves before constructing this.
    scale_candidates: &'a [f64],
    /// Shift search window center, in THIS pass's own grid pixel units — `(0, 0)` for the coarse
    /// pass (no prior estimate to center on), or the coarse result converted into the refine
    /// pass's own (higher-resolution) grid units.
    shift_center: (i32, i32),
    /// Shift search radius around `shift_center`, in this pass's own grid pixel units.
    shift_radius: i32,
}

/// One [`SearchPass`]'s own best-scoring result.
struct SearchResult {
    scale: f64,
    shift: (i32, i32),
    score: f64,
}

impl SearchPass<'_> {
    fn run(&self) -> SearchResult {
        let mut best = SearchResult {
            scale: self.scale_candidates.first().copied().unwrap_or(1.0),
            shift: (0, 0),
            score: f64::MIN,
        };
        for &scale in self.scale_candidates {
            for shift_y in (self.shift_center.1 - self.shift_radius)
                ..=(self.shift_center.1 + self.shift_radius)
            {
                for shift_x in (self.shift_center.0 - self.shift_radius)
                    ..=(self.shift_center.0 + self.shift_radius)
                {
                    let Some(score) = correlation_at_scale_shift(
                        self.a_grid,
                        self.b_grid,
                        scale,
                        shift_x as f64,
                        shift_y as f64,
                    ) else {
                        continue;
                    };
                    if score > best.score {
                        best = SearchResult {
                            scale,
                            shift: (shift_x, shift_y),
                            score,
                        };
                    }
                }
            }
        }
        best
    }
}

/// Attempts the direct, correlation-free border-detection path described in
/// [`estimate_crop_alignment_with_confidence`]'s own docs. Returns `None` (falls through to the
/// correlation search instead) when neither image has a detectable uniform border, OR when the
/// geometrically-implied alignment doesn't actually correlate well enough to trust (a
/// structurally-plausible-looking uniform region that doesn't correlate is more likely a false
/// positive than a real border).
fn try_border_detected_alignment(
    a: &DynamicImage,
    b: &DynamicImage,
    a_height: u32,
    b_width: u32,
    b_height: u32,
) -> Option<(CropAlignment, f64)> {
    // Full ORIGINAL resolution, not a downsampled grid — unlike the correlation search (whose cost
    // is quadratic-ish in resolution, per `CORRELATION_HEIGHT`'s own docs), a row/column min-max
    // scan is cheap (linear in the pixels actually visited) and stops the moment it hits real
    // content, so border rows/columns — typically a small fraction of the frame — are all that
    // ever gets scanned in the common case. Confirmed live this matters, not just theoretically:
    // detecting against a `REFINE_HEIGHT`-downsampled grid instead measured real quantization
    // error (~0.6 grid px, i.e. `1151px-wide-B / 128grid ≈ 9px` per grid step) large enough to
    // push a real test case's own validation score to 0.501 — a hair above `MIN_CONFIDENCE`,
    // fragile enough that a slightly different real image could easily fall the other side of the
    // threshold and reject a border detection that direction-of-truth still very much has.
    let a_full = a.to_luma8();
    let b_full = b.to_luma8();
    let a_bounds = detect_content_bounds(&a_full);
    let b_bounds = detect_content_bounds(&b_full);
    if !a_bounds.has_any_border() && !b_bounds.has_any_border() {
        return None;
    }

    let a_content_lo_x = a_bounds.left;
    let a_content_hi_x = 1.0 - a_bounds.right;
    let a_content_lo_y = a_bounds.top;
    let a_content_hi_y = 1.0 - a_bounds.bottom;
    let b_content_lo_x = b_bounds.left;
    let b_content_hi_x = 1.0 - b_bounds.right;
    let b_content_lo_y = b_bounds.top;
    let b_content_hi_y = 1.0 - b_bounds.bottom;

    let a_content_width = a_content_hi_x - a_content_lo_x;
    let a_content_height = a_content_hi_y - a_content_lo_y;
    if a_content_width <= 0.0 || a_content_height <= 0.0 {
        return None; // Degenerate (border detection consumed the whole frame) — not trustworthy.
    }

    let scale_x = (b_content_hi_x - b_content_lo_x) / a_content_width;
    let scale_y = (b_content_hi_y - b_content_lo_y) / a_content_height;

    // `CropAlignment` has a single shared scale for both axes (per its own docs, a safe
    // assumption for two GENUINE scans of the same physical page at a uniform DPI difference),
    // but the two axes' own independently-detected scales don't always agree exactly — confirmed
    // live: a real test case (content pasted UNSCALED into a differently-proportioned bordered
    // canvas — a real, if less common, scenario: e.g. inconsistent left/right vs. top/bottom
    // scanner-bed trimming) measured `scale_x=0.918`, `scale_y=0.893`, genuinely different, not
    // just detection noise — their PLAIN AVERAGE (0.906) correlated WORSE (0.487, just under
    // `MIN_CONFIDENCE`) than either axis alone would have. Rather than assume averaging is always
    // right, try all three (`scale_x`, `scale_y`, their average) — still just 3 cheap validation
    // correlations, nowhere near the full search's own cost — and keep whichever one actually
    // correlates best.
    let candidates = [scale_x, scale_y, (scale_x + scale_y) / 2.0];

    let a_refine = downsample_gray(a, REFINE_HEIGHT);
    let b_refine = downsample_gray(b, REFINE_HEIGHT);
    let grid_to_a_px = a_height as f64 / REFINE_HEIGHT as f64;
    let grid_to_b_px = b_height as f64 / REFINE_HEIGHT as f64;

    let mut best: Option<(f64, f64, f64, f64)> = None; // (scale, offset_x, offset_y, score)
    for &scale in &candidates {
        let offset_x = b_content_lo_x - scale * a_content_lo_x;
        let offset_y = b_content_lo_y - scale * a_content_lo_y;
        let grid_scale = scale * grid_to_a_px / grid_to_b_px;
        let grid_shift_x = (offset_x * b_width as f64) / grid_to_b_px;
        let grid_shift_y = (offset_y * b_height as f64) / grid_to_b_px;
        let Some(score) = correlation_at_scale_shift(
            &a_refine,
            &b_refine,
            grid_scale,
            grid_shift_x,
            grid_shift_y,
        ) else {
            continue;
        };
        if best.is_none_or(|(_, _, _, best_score)| score > best_score) {
            best = Some((scale, offset_x, offset_y, score));
        }
    }
    let (scale, offset_x, offset_y, score) = best?;
    if score < MIN_CONFIDENCE {
        return None;
    }

    Some((
        CropAlignment {
            scale,
            offset_x,
            offset_y,
        },
        score,
    ))
}

/// [`estimate_crop_alignment`] plus the raw best-fit normalized cross-correlation score — exposed
/// separately (not just "identity or not") so a caller can use the confidence itself as evidence
/// (e.g. `alignment::align_sequences`'s own rescue path for a pair whose perceptual-hash distance
/// alone looks like a mismatch but is otherwise a strong sequence-position candidate — a wide
/// scan-margin difference can push perceptual hash distance well past its own threshold while the
/// two pages are still clearly the same content once optimally aligned).
///
/// This is the FAST/COARSE algorithm: a single [`SearchPass`] over [`COARSE_SCALE_CANDIDATES`] at
/// [`CORRELATION_HEIGHT`] resolution, no border detection, no refine pass — deliberately kept
/// simple and fast, since it's what `alignment::align_sequences_with_rescue`'s own rescue check
/// calls (potentially once per orphaned page across a whole book) AND what `compare_archives` uses
/// for a sample's FIRST, immediately-displayed `crop_alignment` (see that module's own SSE-phase
/// docs). Deliberately NOT the more accurate [`estimate_precise_crop_alignment_with_confidence`]:
/// that algorithm's own honestly-reported confidence for a real border case (0.53) is LOWER than
/// this coarse search's own (often artificially inflated by self-similar manga line-art/screentone
/// content fooling whole-frame correlation) score for the SAME real case (0.96) — using the precise
/// algorithm's own confidence for the RESCUE decision (which was calibrated against THIS coarse
/// algorithm's own historical score range) wrongly rejected a real match once tried live: the cover
/// page disappeared from comparison results entirely the moment the rescue path was switched over
/// to the more honest, lower-scoring precise algorithm. Matching decisions ("is this the same
/// page") and precise display geometry ("exactly how wide is the border") are different questions
/// with different reliable answers — this function only ever answers the first.
pub fn estimate_crop_alignment_with_confidence(
    a: &DynamicImage,
    b: &DynamicImage,
) -> (CropAlignment, f64) {
    let a_height = a.height().max(1);
    let (b_width, b_height) = (b.width().max(1), b.height().max(1));

    let a_coarse = downsample_gray(a, CORRELATION_HEIGHT);
    let b_coarse = downsample_gray(b, CORRELATION_HEIGHT);
    let coarse = SearchPass {
        a_grid: &a_coarse,
        b_grid: &b_coarse,
        scale_candidates: &COARSE_SCALE_CANDIDATES,
        shift_center: (0, 0),
        shift_radius: MAX_SHIFT_PX,
    }
    .run();

    if coarse.score < MIN_CONFIDENCE {
        return (CropAlignment::IDENTITY, coarse.score);
    }

    // Convert from the grid-internal correspondence (A-grid px -> B-grid px via `grid_scale` +
    // integer `shift`) to the public, real-dimension-normalized contract `CropAlignment` promises.
    // `a_coarse`/`b_coarse` preserve their OWN image's real aspect ratio at `CORRELATION_HEIGHT`,
    // so 1 grid px = `a_height/CORRELATION_HEIGHT` real A px (both axes, uniformly) and likewise
    // `b_height/CORRELATION_HEIGHT` real B px — converting through real pixels and then
    // normalizing by each side's own real width/height (per this type's own UV convention) yields
    // the real a->b scale and offset below.
    let grid_to_a_px = a_height as f64 / CORRELATION_HEIGHT as f64;
    let grid_to_b_px = b_height as f64 / CORRELATION_HEIGHT as f64;
    let real_scale = coarse.scale * grid_to_b_px / grid_to_a_px;

    // A grid pixel (x, y) maps to B grid pixel (x*grid_scale + shift_x, y*grid_scale + shift_y).
    // In real pixels: a_real = (x,y) * grid_to_a_px; b_real = (x*grid_scale + shift_x, ...) *
    // grid_to_b_px = a_real * (grid_scale * grid_to_b_px/grid_to_a_px) + shift * grid_to_b_px
    //             = a_real * real_scale + shift * grid_to_b_px.
    let offset_x_b_px = coarse.shift.0 as f64 * grid_to_b_px;
    let offset_y_b_px = coarse.shift.1 as f64 * grid_to_b_px;

    (
        CropAlignment {
            scale: real_scale,
            offset_x: offset_x_b_px / b_width as f64,
            offset_y: offset_y_b_px / b_height as f64,
        },
        coarse.score,
    )
}

/// The PRECISE algorithm — border-detection-first (see [`try_border_detected_alignment`]), with a
/// wider-range coarse-then-refine correlation search (see [`PRECISE_SCALE_CANDIDATES`]/
/// [`REFINE_HEIGHT`]) as its own fallback when no uniform border is detected. Meaningfully more
/// accurate than [`estimate_crop_alignment_with_confidence`] (see that function's own docs for the
/// real, measured gap) but also meaningfully more expensive — used ONLY to populate a sample's
/// DISPLAY `crop_alignment` with pixel-accurate border geometry once a pair has ALREADY been
/// confirmed a match via the coarse/fast algorithm (`compare_archives`'s own SSE phase 2 — see
/// that module's own docs), never for the rescue/matching decision itself.
pub fn estimate_precise_crop_alignment_with_confidence(
    a: &DynamicImage,
    b: &DynamicImage,
) -> (CropAlignment, f64) {
    let a_height = a.height().max(1);
    let (b_width, b_height) = (b.width().max(1), b.height().max(1));

    // Tried FIRST, before the correlation search below: a direct, correlation-free border
    // detection handles the specific real-world case that search can get confidently wrong (see
    // `BORDER_UNIFORMITY_THRESHOLD`'s own docs for the real measured numbers) — a genuine uniform
    // scan border on one (or both) side(s) is a far less ambiguous signal than "which scale/shift
    // correlates best across the whole frame," which richly self-similar manga content (line art,
    // repeating screentone) can fool. Only trusted if it ALSO clears the same correlation bar the
    // search-based path would (`try_border_detected_alignment`'s own validation step) — a
    // structurally-plausible-looking border that doesn't actually correlate well is more likely a
    // false positive (e.g. a genuinely blank panel near an edge) than a real border, and falls
    // through to the search below instead of being trusted blindly.
    if let Some(result) = try_border_detected_alignment(a, b, a_height, b_width, b_height) {
        return result;
    }

    let a_coarse = downsample_gray(a, CORRELATION_HEIGHT);
    let b_coarse = downsample_gray(b, CORRELATION_HEIGHT);
    let coarse = SearchPass {
        a_grid: &a_coarse,
        b_grid: &b_coarse,
        scale_candidates: &PRECISE_SCALE_CANDIDATES,
        shift_center: (0, 0),
        shift_radius: MAX_SHIFT_PX,
    }
    .run();

    if coarse.score < MIN_CONFIDENCE {
        return (CropAlignment::IDENTITY, coarse.score);
    }

    // Refine: a narrow, high-resolution local search around the coarse result — see
    // `REFINE_HEIGHT`/`REFINE_SHIFT_RADIUS_PX`/`REFINE_SCALE_DELTAS`'s own docs for why this stays
    // cheap despite the higher per-pixel cost (small search VOLUME, not a full re-search). Shift
    // center is the coarse shift converted from `CORRELATION_HEIGHT`-grid to `REFINE_HEIGHT`-grid
    // units first (1 coarse-grid px = `REFINE_HEIGHT/CORRELATION_HEIGHT` refine-grid px).
    let a_refine = downsample_gray(a, REFINE_HEIGHT);
    let b_refine = downsample_gray(b, REFINE_HEIGHT);
    let grid_ratio = REFINE_HEIGHT as f64 / CORRELATION_HEIGHT as f64;
    let refine_scale_candidates: Vec<f64> = REFINE_SCALE_DELTAS
        .iter()
        .map(|delta| coarse.scale + delta)
        .collect();
    let refine = SearchPass {
        a_grid: &a_refine,
        b_grid: &b_refine,
        scale_candidates: &refine_scale_candidates,
        shift_center: (
            (coarse.shift.0 as f64 * grid_ratio).round() as i32,
            (coarse.shift.1 as f64 * grid_ratio).round() as i32,
        ),
        shift_radius: REFINE_SHIFT_RADIUS_PX,
    }
    .run();

    // The refine pass searches the SAME real region the coarse pass already confirmed clears
    // `MIN_CONFIDENCE`, just at finer resolution/granularity — it should generally match or beat
    // the coarse score, but a real (not hypothetical) safety net costs nothing: fall back to the
    // coarse result if the refine pass somehow scored lower (e.g. resampling-filter differences
    // between the two downsample resolutions on genuinely borderline content).
    let (best_grid_scale, best_shift, best_score, grid_height) = if refine.score >= coarse.score {
        (refine.scale, refine.shift, refine.score, REFINE_HEIGHT)
    } else {
        (coarse.scale, coarse.shift, coarse.score, CORRELATION_HEIGHT)
    };

    // Convert from the grid-internal correspondence (A-grid px -> B-grid px via `grid_scale` +
    // integer `shift`) to the public, real-dimension-normalized contract `CropAlignment` promises.
    // `a_coarse`/`a_refine`/`b_coarse`/`b_refine` all preserve their OWN image's real aspect ratio
    // at whichever `grid_height` won above, so 1 grid px = `a_height/grid_height` real A px (both
    // axes, uniformly) and likewise `b_height/grid_height` real B px — converting through real
    // pixels and then normalizing by each side's own real width/height (per this type's own UV
    // convention) yields the real a->b scale and offset below.
    let grid_to_a_px = a_height as f64 / grid_height as f64;
    let grid_to_b_px = b_height as f64 / grid_height as f64;
    let real_scale = best_grid_scale * grid_to_b_px / grid_to_a_px;

    // A grid pixel (x, y) maps to B grid pixel (x*grid_scale + shift_x, y*grid_scale + shift_y).
    // In real pixels: a_real = (x,y) * grid_to_a_px; b_real = (x*grid_scale + shift_x, ...) *
    // grid_to_b_px = a_real * (grid_scale * grid_to_b_px/grid_to_a_px) + shift * grid_to_b_px
    //             = a_real * real_scale + shift * grid_to_b_px.
    let offset_x_b_px = best_shift.0 as f64 * grid_to_b_px;
    let offset_y_b_px = best_shift.1 as f64 * grid_to_b_px;

    (
        CropAlignment {
            scale: real_scale,
            offset_x: offset_x_b_px / b_width as f64,
            offset_y: offset_y_b_px / b_height as f64,
        },
        best_score,
    )
}

/// Estimates the crop/resolution alignment mapping A's pixels onto B's, via the multi-scale
/// search described in this module's own docs. Falls back to [`CropAlignment::IDENTITY`] whenever
/// no candidate scale reaches [`MIN_CONFIDENCE`] — a low correlation at every tried scale means
/// this pair likely isn't a simple crop/resolution difference (could be genuinely different
/// content that the caller's own perceptual-hash alignment only loosely matched), and guessing an
/// unreliable transform would misalign the magnifier worse than not aligning at all.
pub fn estimate_crop_alignment(a: &DynamicImage, b: &DynamicImage) -> CropAlignment {
    estimate_crop_alignment_with_confidence(a, b).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Luma, Rgb};

    /// A deterministic "page-like" test image with structure at TWO scales, like a real manga
    /// page's panel-level layout plus finer line-art/text detail: coarse ~40-55px blocks (panels)
    /// give the correlation something that survives downsampling, while a finer ~5px diagonal
    /// stripe (detail) gives the search enough resolution to actually discriminate between nearby
    /// scale candidates.
    fn page_pattern(width: u32, height: u32) -> DynamicImage {
        let buf: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_fn(width, height, |x, y| {
            let coarse = ((x / 40) % 2 == 0) ^ ((y / 55) % 2 == 0);
            let fine = ((x + y) / 5) % 2 == 0;
            let base: i32 = if coarse { 200 } else { 60 };
            let v = if fine { base + 30 } else { base - 30 };
            Luma([v.clamp(0, 255) as u8])
        });
        DynamicImage::ImageLuma8(buf)
    }

    /// A whole-canvas copy of `page_pattern`'s own content resized to `scale` — simulates two
    /// scans of the same page at genuinely different resolutions (the common real-world case this
    /// module targets), not padding added inside a fixed-size canvas.
    fn page_at_scale(base_width: u32, base_height: u32, scale: f64) -> DynamicImage {
        let content = page_pattern(base_width, base_height);
        let out_w = (base_width as f64 * scale).round() as u32;
        let out_h = (base_height as f64 * scale).round() as u32;
        content.resize_exact(out_w, out_h, image::imageops::FilterType::Triangle)
    }

    #[test]
    fn identical_images_align_at_scale_one_with_zero_offset() {
        let a = page_pattern(400, 560);
        let alignment = estimate_crop_alignment(&a, &a);
        assert!((alignment.scale - 1.0).abs() < 0.02, "got {alignment:?}");
        assert!(alignment.offset_x.abs() < 0.02, "got {alignment:?}");
        assert!(alignment.offset_y.abs() < 0.02, "got {alignment:?}");
    }

    #[test]
    fn detects_a_resolution_difference_as_a_scale_change() {
        // Two whole-canvas scans of the same page at genuinely different resolutions — the common
        // real-world case (scan DPI varies scanner to scanner) — must be detected as a scale
        // change, not left at the default identity. B is 1.15x A's own real pixel dimensions; since
        // `real_scale` is normalized to B's own real height, a correctly-detected alignment should
        // recover something close to 1.15 here.
        let a = page_pattern(400, 560);
        let b = page_at_scale(400, 560, 1.15);
        let alignment = estimate_crop_alignment(&a, &b);
        assert!(
            (alignment.scale - 1.15).abs() < 0.1,
            "expected scale near 1.15 for a 1.15x resolution change, got {alignment:?}"
        );
    }

    /// `page_pattern`'s content pasted onto a slightly larger canvas, offset only toward the
    /// top/left — shifts the content's own center within the canvas, a real translation.
    fn page_with_offset_margin(
        base_width: u32,
        base_height: u32,
        margin_x: u32,
        margin_y: u32,
    ) -> DynamicImage {
        let content = page_pattern(base_width, base_height).to_rgb8();
        let out_w = base_width + margin_x;
        let out_h = base_height + margin_y;
        let mut canvas: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(out_w, out_h, Rgb([255, 255, 255]));
        image::imageops::overlay(&mut canvas, &content, margin_x as i64, margin_y as i64);
        DynamicImage::ImageRgb8(canvas)
    }

    #[test]
    fn detects_an_off_center_crop_as_a_nonzero_offset() {
        // A margin added only on the top/left shifts the remaining content's own center within the
        // canvas — a real translation.
        let a = page_pattern(400, 560);
        let b = page_with_offset_margin(400, 560, 20, 15);
        let alignment = estimate_crop_alignment(&a, &b);
        assert!(
            alignment.offset_x.abs() > 0.01 || alignment.offset_y.abs() > 0.01,
            "expected a real offset for an off-center crop, got {alignment:?}"
        );
    }

    #[test]
    fn falls_back_to_identity_for_genuinely_unrelated_images() {
        let a = page_pattern(400, 560);
        let b = DynamicImage::ImageLuma8(ImageBuffer::from_fn(400, 560, |x, y| {
            let v = ((x / 3 + y / 5) % 2) == 0;
            Luma([if v { 30 } else { 220 }])
        }));
        let alignment = estimate_crop_alignment(&a, &b);
        assert_eq!(alignment, CropAlignment::IDENTITY);
    }

    /// A smoothly-varying (not sharply periodic) test pattern — large-scale sine/cosine gradients
    /// at deliberately non-aliasing-prone, near-irrational-ish frequencies, plus light per-pixel
    /// texture so the correlation's own variance check has real structure to work with. Unlike
    /// `page_pattern`'s own sharp, exactly-periodic checkerboard (fine for THAT function's own
    /// existing tests, whose shifts/scales don't push it into a bad phase), a strictly periodic
    /// pattern downsampled heavily (this module's `REFINE_HEIGHT`, an ~8x reduction from a real
    /// manga-page-scale test image) can land in ANTI-phase at a slightly-off candidate shift —
    /// confirmed live: `page_pattern`'s own checkerboard scored near-ZERO correlation (0.002-0.017)
    /// even at the geometrically EXACT true alignment once downsampled this heavily, an aliasing
    /// artifact of the fixture itself, not a real bug in the border-detection code being tested
    /// (which was separately, successfully validated against real manga page content).
    fn smooth_page_pattern(width: u32, height: u32) -> DynamicImage {
        let buf: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_fn(width, height, |x, y| {
            let fx = x as f64;
            let fy = y as f64;
            let base = 128.0
                + 60.0 * (fx * 0.0137).sin()
                + 60.0 * (fy * 0.0211).cos()
                + 25.0 * ((fx * 0.071 + fy * 0.053).sin());
            let texture = if (x ^ y) % 7 == 0 { 15.0 } else { -15.0 };
            Luma([(base + texture).clamp(0.0, 255.0) as u8])
        });
        DynamicImage::ImageLuma8(buf)
    }

    /// `page_pattern`'s content pasted UNSCALED onto a larger canvas with an INDEPENDENT margin
    /// per edge (not just top/left like `page_with_offset_margin`) — the real shape of scan
    /// borders (`crop_align`'s own real motivating case), including the specific real case that
    /// exposed a genuine gap: independent per-edge margins mean the content's own width:height
    /// ratio, relative to the padded canvas, differs between X and Y — the single shared `scale`
    /// `CropAlignment` exposes can't represent both axes exactly at once.
    fn page_with_border(
        content: &DynamicImage,
        top: u32,
        bottom: u32,
        left: u32,
        right: u32,
    ) -> DynamicImage {
        let content = content.to_rgb8();
        let out_w = content.width() + left + right;
        let out_h = content.height() + top + bottom;
        let mut canvas: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(out_w, out_h, Rgb([255, 255, 255]));
        image::imageops::overlay(&mut canvas, &content, left as i64, top as i64);
        DynamicImage::ImageRgb8(canvas)
    }

    #[test]
    fn detect_content_bounds_finds_nothing_on_a_border_free_image() {
        let img = page_pattern(400, 560).to_luma8();
        let bounds = detect_content_bounds(&img);
        assert!(
            !bounds.has_any_border(),
            "expected no border, got a non-zero bound"
        );
    }

    #[test]
    fn detect_content_bounds_recovers_a_real_uniform_border_on_all_four_edges() {
        let (base_w, base_h) = (400u32, 560u32);
        let (top, bottom, left, right) = (40, 20, 12, 24);
        let content = page_pattern(base_w, base_h);
        let bordered = page_with_border(&content, top, bottom, left, right).to_luma8();
        let bounds = detect_content_bounds(&bordered);
        let (out_w, out_h) = (base_w + left + right, base_h + top + bottom);
        assert!(
            (bounds.top - top as f64 / out_h as f64).abs() < 0.01,
            "got {:?}",
            bounds.top
        );
        assert!(
            (bounds.bottom - bottom as f64 / out_h as f64).abs() < 0.01,
            "got {:?}",
            bounds.bottom
        );
        assert!(
            (bounds.left - left as f64 / out_w as f64).abs() < 0.01,
            "got {:?}",
            bounds.left
        );
        assert!(
            (bounds.right - right as f64 / out_w as f64).abs() < 0.01,
            "got {:?}",
            bounds.right
        );
    }

    #[test]
    fn estimate_precise_crop_alignment_uses_border_detection_for_an_asymmetric_real_shaped_border()
    {
        // The exact shape that exposed a real gap live: independent per-edge margins (not a
        // uniform crop-margin scale) mean X and Y axes genuinely need different scales — the
        // border-detection path must still land close to the true content-region boundary by
        // picking whichever of scale_x/scale_y/their average actually correlates best, not just
        // averaging blindly (confirmed live: blind averaging scored BELOW `MIN_CONFIDENCE` here
        // and silently fell through to the far-less-accurate correlation search instead).
        // Real manga-page-scale dimensions and proportionally realistic margins (not the smaller
        // 400x560 size this module's other tests use) — the correlation validation step needs
        // enough real pixel data at `REFINE_HEIGHT`'s own downsampled resolution to be reliable;
        // a much smaller test image leaves too little signal there, confirmed live.
        let (base_w, base_h) = (1000u32, 1400u32);
        let (top, bottom, left, right) = (110, 50, 30, 60);
        let a = smooth_page_pattern(base_w, base_h);
        let b = page_with_border(&a, top, bottom, left, right);
        let (out_w, out_h) = (base_w + left + right, base_h + top + bottom);

        let (alignment, _) = estimate_precise_crop_alignment_with_confidence(&a, &b);
        let true_offset_x = left as f64 / out_w as f64;
        let true_offset_y = top as f64 / out_h as f64;
        assert!(
            (alignment.offset_x - true_offset_x).abs() < 0.02,
            "got {alignment:?}, expected offset_x near {true_offset_x}"
        );
        assert!(
            (alignment.offset_y - true_offset_y).abs() < 0.02,
            "got {alignment:?}, expected offset_y near {true_offset_y}"
        );
        // Scale must land near EITHER axis's own true value (whichever the validation step picked
        // as better-correlating) — not the identity fallback (1.0) a broken detection would leave
        // it at, and not wildly off in the other direction either.
        let true_scale_x = base_w as f64 / out_w as f64;
        let true_scale_y = base_h as f64 / out_h as f64;
        assert!(
            (alignment.scale - true_scale_x).abs() < 0.03
                || (alignment.scale - true_scale_y).abs() < 0.03,
            "got {alignment:?}, expected scale near either {true_scale_x} or {true_scale_y}"
        );
    }

    #[test]
    fn coarse_estimate_never_uses_border_detection_and_stays_high_confidence() {
        // Regression guard for a real, live bug: once `estimate_crop_alignment_with_confidence`
        // (the RESCUE path's own function) briefly gained the same border-detection-first logic
        // as the precise algorithm, a real border page's honestly-lower confidence there (0.53)
        // fell below `alignment::RESCUE_CONFIDENCE_THRESHOLD` (0.9, calibrated against THIS
        // coarse function's own historically higher — if geometrically imprecise — score), and
        // the cover page silently disappeared from real comparison results entirely. The coarse
        // function must keep producing a HIGH-confidence result for this shape of input (its own
        // establishing correlation-search behavior, unaffected by the newer, separate precise
        // algorithm) — a low score here would silently break the rescue mechanism again.
        let (base_w, base_h) = (1000u32, 1400u32);
        let (top, bottom, left, right) = (110, 50, 30, 60);
        let a = smooth_page_pattern(base_w, base_h);
        let b = page_with_border(&a, top, bottom, left, right);

        let (_, confidence) = estimate_crop_alignment_with_confidence(&a, &b);
        assert!(
            confidence > 0.9,
            "expected the coarse/fast algorithm to keep its historically high confidence for this \
             shape of input, got {confidence} — if this regresses, check whether border detection \
             leaked into `estimate_crop_alignment_with_confidence` again"
        );
    }
}
