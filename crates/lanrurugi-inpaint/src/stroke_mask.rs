//! Glyph-stroke-level text masks, built from per-region foreground/background colour estimates
//! rather than a dedicated segmentation model — the same technique Google Lens/Translate and
//! `manga-image-translator` both use (`mask_dilation_offset` there is this module's own
//! [`DILATION_RADIUS`]): classify each pixel by colour distance to the known text colour, dilate
//! slightly to catch anti-aliased edges, done. No extra ONNX inference, so no extra latency —
//! deliberately chosen over a dedicated stroke-segmentation model for exactly that reason
//! (confirmed live, 2026-09-08: the user's own priority was "as fast as Google's", ruling out
//! adding another per-region model call after the LaMa session-pool/rayon work already done this
//! session to fix a *different* latency problem).
//!
//! [`stroke_mask`] only ever narrows what a region contributes to the page-wide mask
//! [`crate::Inpainter::erase_page`] treats as "erase this" — it composes with (via intersection) a
//! bubble-shape mask exactly the way
//! `lanrurugi_translate::composite::best_matching_bubble_mask`'s own mask already does, so a region
//! with both a matched bubble and a confident colour estimate gets the tightest of the two: erase
//! only pixels that are both inside the real bubble outline *and* actually glyph-coloured, leaving
//! everything else — including the bubble's own fill color right next to the glyph — completely
//! untouched rather than regenerated.
//!
//! When no confident `fg`/`bg` colour pair exists at all for [`stroke_mask`] to use,
//! [`local_background_stroke_mask`] is the real fallback — [`otsu_stroke_mask`] was tried first
//! (a single global luminance threshold) and confirmed live, 2026-09-08, to fail on a real
//! multi-tone glyph (a coloured fill plus a contrasting outline stroke) over a textured backdrop,
//! since a whole-crop luminance histogram in that case has no single clean bimodal split at all.
//! `local_background_stroke_mask` classifies each pixel by its own distance from a *locally*
//! estimated background colour instead of one global threshold, which remains valid regardless of
//! how many distinct ink tones a glyph has.

use crate::densecrf::DenseCrf2d;
use image::{GrayImage, Luma, Rgb, RgbImage};
use imageproc::filter::median_filter;
use palette::{IntoColor, Lab, Srgb};

/// A pixel is classified as text-stroke if it's closer (by this squared-distance-in-0..1-space
/// threshold) to `fg` than the midpoint between `fg` and `bg` — see [`stroke_mask`]'s own doc
/// comment. `0.5` in normalized terms is a plain nearest-cluster classification (matches the same
/// midpoint idea `style_estimate::foreground_coverage` already uses on luminance alone); this
/// works in full RGB space instead of collapsing to luminance first, so a colored glyph on a
/// similarly-bright but differently-hued background still classifies correctly.
const FG_DISTANCE_FRACTION: f32 = 0.5;

/// How many pixels to grow the raw classification by on every side for the mask actually fed to
/// LaMa (the *model* mask — see [`PASTE_DILATION_RADIUS`] for the separate, smaller radius used
/// for the mask that controls which pixels get replaced in the final page). Matches the
/// qualitative role of `manga-image-translator`'s own `mask_dilation_offset` parameter (their
/// default: `20`, at their own typical resolutions). A real reported incident (2026-09-09): this
/// constant's own earlier value of `2` — picked without validating against that reference default
/// at all, and back when the model mask and paste mask were still the same value — produced a
/// visible glyph-shaped ghost of low-contrast noise on an otherwise-flat bubble backdrop after a
/// real whole-page LaMa erase, confirmed by a direct pixel-level comparison against the original
/// page. LaMa (and generative inpainting models generally) needs a real transition band around a
/// hole to blend its own reconstruction smoothly into genuinely flat surrounding context — a mask
/// that hugs the glyph's own stroke too tightly starves the model of that band, and the result is
/// a faint stroke-shaped texture artifact rather than a clean flat fill.
///
/// Raising this alone (still shared with the paste mask at the time) fixed that ghost, but
/// introduced a second real incident the same day: a real page had one OCR region whose own
/// bounding box was inaccurate enough to already include a manga panel's own border line, and
/// growing *both* masks by the same `8px` let that panel border get folded into the pixels
/// actually replaced with LaMa's reconstruction — a real straight line warped/blurred in the
/// output, confirmed by a direct pixel-level comparison. Splitting the two radii apart (this one
/// generous, [`PASTE_DILATION_RADIUS`] small) fixes both incidents at once: LaMa still gets the
/// wide transition band it needs to blend cleanly, but only a narrow ring around the glyph's own
/// real shape is ever actually replaced in the page the user sees — an inaccurate bbox can still
/// pull in a little of a panel border, but nowhere near as much as the full model radius would.
const DILATION_RADIUS: i32 = 8;

/// How many pixels to grow the raw classification by on every side for the *paste* mask — the one
/// that actually controls which pixels of the final page get replaced with LaMa's reconstruction
/// (see [`DILATION_RADIUS`]'s own doc comment for the incident this split fixes, and
/// [`crate::Inpainter::erase_page`]'s own doc comment for how the two masks are threaded through
/// to the model call). Deliberately small — this only needs to catch a few pixels of
/// anti-aliasing at the glyph's own edge, not give the model a wide reconstruction context (that's
/// the model mask's job). `2` matches this module's own original, pre-incident dilation radius,
/// which was never the source of either bug above (both were about the *model* mask being either
/// too small to give LaMa a real transition band, or too large once it was widened without a
/// matching paste-mask split).
const PASTE_DILATION_RADIUS: i32 = 2;

/// The minimum fraction of a crop's own pixels [`local_background_stroke_mask`]'s raw
/// classification must cover before that result is trusted outright — below this, the function
/// tries two independent fallbacks in order, stopping as soon as coverage clears this bar again:
/// first OR-ing in the caller's own `coarse_prior` (the detection model's own independently
/// computed "text is here" signal — added 2026-09-10, no extra inference cost since it's already
/// computed at detection time), then [`kmeans_stroke_mask`] if that alone still isn't enough,
/// keeping whichever of the (up to three) candidate results covers the most of the crop.
///
/// A real reported incident (2026-09-09): a two-character crop with a busy, non-flat backdrop (a
/// wooden door's own dark decorative line pattern) had the window median for several of its own
/// real glyph pixels dragged toward that decoration's colour rather than the door's true flat
/// colour, so those pixels' own distance from their "local background" never rose above the
/// threshold — the function returned `Some` (not `None`, which would have triggered
/// `lanrurugi_translate::composite::combined_erase_mask`'s own whole-rectangle fallback), but with
/// roughly half a real two-character glyph silently missing from `raw` — confirmed live: one
/// character fully detected, the other almost entirely absent, discovered only by a direct
/// pixel-level comparison against the original page, not by any signal this function itself
/// raised.
///
/// `0.08` is a real, measured value from that actual incident, not a guess: the real crop's own
/// raw coverage was `6.2%` (982/15865 pixels) with one full character silently missing, so `8%`
/// sits just above that real failure point while staying below the multi-tone synthetic test's
/// own `12.75%` raw coverage
/// (`local_background_finds_a_multi_tone_outlined_glyph_where_otsu_fails`, a real thin-outline
/// case that must *not* trigger this fallback) — verified this doesn't regress that test.
/// Deliberately not lower: a genuinely thin single-stroke glyph, or an outline-only character, can
/// legitimately cover only a few percent of its own tight crop, and this check is only meant to
/// catch the pathological case where the window estimate has clearly broken down for a large
/// fraction of the crop, not every partial miss. Re-run against the real incident's own crop after
/// the fallback was added: raw coverage rose from `6.2%` to k-means's own `13.7%`
/// (2176/15865 pixels), with both characters now visibly detected in a direct visual check —
/// confirms the fallback actually fires and actually helps for the real incident it exists for,
/// not just in theory.
const MIN_PLAUSIBLE_RAW_COVERAGE: f32 = 0.08;

/// The three masks every function in this module builds together, row-major and
/// `crop.width() * crop.height()`-long: [`Self::raw`] (undilated, [`sample_stroke_colour`]'s own
/// input), [`Self::paste`] (small dilation — the pixels actually replaced in the final page), and
/// [`Self::model`] (large dilation — the wider hole actually fed to LaMa, giving it a real
/// transition band to blend into; see [`DILATION_RADIUS`]'s own doc comment for why these two
/// dilations were split apart at all). `paste` is always a subset of `model` (same raw shape, just
/// grown by a smaller radius), which is what lets [`crate::Inpainter::erase_page`] feed `model` to
/// the network while only ever actually overwriting `paste`'s own pixels in the page it returns.
pub struct StrokeMask {
    pub raw: Vec<bool>,
    pub paste: Vec<bool>,
    pub model: Vec<bool>,
}

/// Builds a row-major, `crop.width() * crop.height()`-long stroke mask for one already-cropped
/// region image, given that region's own estimated foreground (glyph) and background (backdrop)
/// colours (`lanrurugi_ocr::style_estimate::estimate`'s own output, threaded through by the
/// caller). Returns `None` when `fg` and `bg` are identical (division by zero in the distance
/// normalisation below) — a real region always has some separation or `style_estimate` itself
/// wouldn't have reported both colours.
///
/// Returns the *un-dilated* raw classification alongside the two dilated masks — see
/// [`StrokeMask`]'s own doc comment. [`sample_stroke_colour`] needs `raw` specifically: a dilated
/// mask's extra ring of pixels (added to catch anti-aliased glyph edges, see [`DILATION_RADIUS`]'s
/// own doc comment) are largely *background* pixels by construction, and averaging them into a
/// colour sample would pull the result back toward the very background colour this function
/// exists to tell apart from the glyph — reported live, 2026-09-08: text rendered pure white
/// instead of its real ink colour, traced back to `style_estimate`'s own two-means clustering
/// misreading a pixel-heavy background pattern as the glyph cluster on a region with no real
/// bubble backdrop.
pub fn stroke_mask(crop: &RgbImage, fg: Rgb<u8>, bg: Rgb<u8>) -> Option<StrokeMask> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let to_f32 = |c: Rgb<u8>| [f32::from(c.0[0]), f32::from(c.0[1]), f32::from(c.0[2])];
    let (fg_f, bg_f) = (to_f32(fg), to_f32(bg));
    let span_sq: f32 = fg_f
        .iter()
        .zip(bg_f.iter())
        .map(|(a, b)| (a - b).powi(2))
        .sum();
    if span_sq < f32::EPSILON {
        return None;
    }

    let mut raw = vec![false; (w * h) as usize];
    for (i, px) in crop.pixels().enumerate() {
        let p = to_f32(*px);
        let dist_fg_sq: f32 = p
            .iter()
            .zip(fg_f.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum();
        // Fraction of the way from fg to bg, projected onto the fg-bg axis in squared-distance
        // terms — closer to `fg` than `FG_DISTANCE_FRACTION` of the total span counts as stroke.
        raw[i] = dist_fg_sq < span_sq * FG_DISTANCE_FRACTION * FG_DISTANCE_FRACTION;
    }

    let paste = dilate(&raw, w, h, PASTE_DILATION_RADIUS);
    let model = dilate(&raw, w, h, DILATION_RADIUS);
    Some(StrokeMask { raw, paste, model })
}

/// Averages `crop`'s own pixels wherever `raw_mask` (the *un-dilated* classification from
/// [`stroke_mask`] — never the dilated one, see that function's own doc comment) is `true` — the
/// real ink colour, read directly off the actual glyph pixels rather than re-derived from
/// `style_estimate`'s own statistical cluster split, which a busy/patterned backdrop can fool
/// (see [`stroke_mask`]'s own doc comment for the real incident this fixes). Returns `None` if no
/// pixel was classified as stroke at all.
pub fn sample_stroke_colour(crop: &RgbImage, raw_mask: &[bool]) -> Option<Rgb<u8>> {
    let stroke_pixels: Vec<Rgb<u8>> = crop
        .pixels()
        .zip(raw_mask.iter())
        .filter(|(_, &is_stroke)| is_stroke)
        .map(|(px, _)| *px)
        .collect();
    if stroke_pixels.is_empty() {
        return None;
    }

    // Averaging every `raw`-classified pixel unweighted pulls the result toward whatever
    // brighter, non-ink pixels the mask's own boundary happened to include — real reported
    // incident (2026-09-16): `densecrf_stroke_mask`'s own `raw` classification (no confident
    // fg/bg colour prior to anchor against, only the detector's coarse mask + Gaussian/bilateral
    // smoothing — see that function's own doc comment) draws its boundary a little wide around a
    // glyph's real dark strokes on several real regions, including enough lighter transition/
    // outline-adjacent pixels that the plain mean came out a visibly grey `rgb(56,71,80)` instead
    // of near-black ink — legible, but a real colour-fidelity regression from the source. Ink is
    // reliably the *darkest* pixels within any stroke mask built by this crate (every stroke-mask
    // constructor here classifies a pixel as "stroke" for being unusually different from its own
    // background, and manga ink is overwhelmingly dark relative to its backdrop — the founding
    // assumption `otsu_stroke_mask`'s own "darker cluster is the glyph" rule already relies on
    // elsewhere in this file) — so instead of averaging every classified pixel, only the darkest
    // half by luminance is averaged, discarding exactly the lighter fringe pixels most likely to
    // be background bleed-through rather than real ink.
    let luminance = |px: &Rgb<u8>| {
        0.299 * f32::from(px.0[0]) + 0.587 * f32::from(px.0[1]) + 0.114 * f32::from(px.0[2])
    };
    let mut by_luminance = stroke_pixels;
    by_luminance.sort_by(|a, b| luminance(a).partial_cmp(&luminance(b)).unwrap());
    let keep = by_luminance.len().div_ceil(2).max(1);

    let mut sum = [0u64; 3];
    for px in &by_luminance[..keep] {
        sum[0] += u64::from(px.0[0]);
        sum[1] += u64::from(px.0[1]);
        sum[2] += u64::from(px.0[2]);
    }
    let count = keep as u64;
    Some(Rgb([
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ]))
}

/// Same purpose as [`stroke_mask`] (a precise, non-rectangular text-shape mask), but for a region
/// where no confident `fg`/`bg` colour estimate exists at all (`style_estimate::estimate` itself
/// returned `None` — typically a busy/textured backdrop with no real bubble, where a two-colour
/// classifier has nothing to anchor against). Falls back to Otsu's method: an automatic threshold
/// that needs no colour priors, computed purely from the crop's own luminance histogram by finding
/// the split point that maximises between-class variance — the textbook technique for "this image
/// region is bimodal (ink vs. paper), find the two clusters without being told their colours."
///
/// This is a deliberately lightweight stand-in for what a real dedicated text-segmentation model
/// would do (`zyddnys/manga-image-translator`'s own `mask_refinement` module runs a full DenseCRF
/// refinement pass seeded by its detector's own pixel-level segmentation output, confirmed by
/// reading its actual source, 2026-09-08 — a materially different architecture this codebase does
/// not have an equivalent input for, since our OCR detector only ever produces bounding boxes, not
/// per-pixel masks). Otsu gets a real, precise glyph shape without needing that missing model or
/// any new Python-side dependency, at the cost of being less robust on genuinely low-contrast or
/// multi-tone glyphs than a learned segmentation model would be.
///
/// Returns `None` for a flat/near-flat crop (no real bimodal split to find — e.g. a solid colour
/// background with no visible glyph at all, which can happen at a region's own padded edge).
pub fn otsu_stroke_mask(crop: &RgbImage) -> Option<StrokeMask> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let luminance = |px: &Rgb<u8>| {
        // Rec. 601 luma weights — same convention `style_estimate`'s own luminance split uses,
        // kept consistent rather than inventing a second weighting scheme in this crate.
        (0.299 * f32::from(px.0[0]) + 0.587 * f32::from(px.0[1]) + 0.114 * f32::from(px.0[2]))
            .round() as u32
    };

    let mut histogram = [0u32; 256];
    for px in crop.pixels() {
        histogram[luminance(px).min(255) as usize] += 1;
    }

    let total = (w * h) as f64;
    let sum_all: f64 = histogram
        .iter()
        .enumerate()
        .map(|(i, &c)| (i as f64) * f64::from(c))
        .sum();

    let mut sum_below = 0f64;
    let mut weight_below = 0f64;
    let mut best_threshold = 0usize;
    let mut best_variance = 0f64;
    for (t, &count) in histogram.iter().enumerate() {
        weight_below += f64::from(count);
        if weight_below <= 0.0 {
            continue;
        }
        let weight_above = total - weight_below;
        if weight_above <= 0.0 {
            break;
        }
        sum_below += (t as f64) * f64::from(count);
        let mean_below = sum_below / weight_below;
        let mean_above = (sum_all - sum_below) / weight_above;
        let between_variance = weight_below * weight_above * (mean_below - mean_above).powi(2);
        if between_variance > best_variance {
            best_variance = between_variance;
            best_threshold = t;
        }
    }
    // No real separation found (a flat/near-flat crop) — every pixel landed in one histogram bin,
    // so `best_variance` never rose above zero and there is no bimodal split to report.
    if best_variance <= 0.0 {
        return None;
    }

    // Ink is conventionally the darker cluster in manga lettering (near-black strokes on a
    // lighter backdrop) — same assumption `DEFAULT_FG`'s own near-black default makes elsewhere in
    // this codebase. A pixel at or below the threshold counts as stroke.
    let raw: Vec<bool> = crop
        .pixels()
        .map(|px| luminance(px) <= best_threshold as u32)
        .collect();

    let paste = dilate(&raw, w, h, PASTE_DILATION_RADIUS);
    let model = dilate(&raw, w, h, DILATION_RADIUS);
    Some(StrokeMask { raw, paste, model })
}

/// Median filter radius used to estimate each pixel's own *local* background colour in
/// [`local_background_stroke_mask`] — must be comfortably larger than a typical glyph stroke
/// width (a few px in a typical manga crop) so the window genuinely captures the surrounding
/// backdrop rather than being dominated by the stroke's own colour; `9` (a 19x19 window) is a
/// deliberately generous starting point, not tuned against a real evaluation set. Cheap to keep
/// generous: [`imageproc::filter::median_filter`] answers a windowed *median* in `O(radius)` per
/// pixel via a sliding histogram (not a fresh sort per pixel), and the [`SummedAreaTable`]-backed
/// Pass 2 refinement answers its own windowed mean query in `O(1)` regardless of window size — so
/// this radius no longer trades off against runtime the way a naive per-pixel sort would.
const LOCAL_BACKGROUND_RADIUS: i32 = 9;

/// Also used as the radius of the small "self-exclusion" window subtracted out in Pass 2 (see
/// [`local_background_stroke_mask`]) — deliberately smaller than [`LOCAL_BACKGROUND_RADIUS`] so
/// the background estimate still draws from a real neighbourhood, just not from pixels close
/// enough to the query pixel itself to have plausibly been misclassified as background *because*
/// they're actually part of the same stroke.
const SELF_EXCLUSION_RADIUS: i32 = 3;

/// Same purpose as [`stroke_mask`]/[`otsu_stroke_mask`] (a precise, non-rectangular text-shape
/// mask), for the case [`otsu_stroke_mask`] itself was found to handle poorly: a glyph with more
/// than one real ink tone (a coloured fill plus a contrasting outline stroke — common in manga
/// SFX/caption lettering) sitting on a textured, non-uniform backdrop. A pure luminance histogram
/// over the whole crop sees several real peaks in that case (ink fill, outline, backdrop
/// midtones, backdrop highlights all landing at different brightness levels) with no single
/// threshold that cleanly separates "any ink tone" from "backdrop" — confirmed live, 2026-09-08:
/// a real reported region (a blue-filled, white-outlined "タタ" SFX glyph over a lightly textured
/// wall) produced a continuous, clearly non-bimodal luminance histogram, and `otsu_stroke_mask`
/// left the original lettering almost entirely unerased.
///
/// Takes a different approach entirely, per a second-opinion review from an external analysis of
/// this exact failure (2026-09-08): instead of one global threshold, estimate each pixel's own
/// *local* background colour (a large-radius median filter — robust to a thin stroke sitting
/// within the window without being pulled toward the stroke's own colour, unlike a mean filter)
/// and classify by how far that pixel's own colour is from its own local backdrop estimate, in
/// CIE Lab space (perceptually more uniform than raw RGB — a colour difference is treated
/// consistently regardless of which axis it falls on, so a saturated blue ink and a bright white
/// outline can both register as "far from a beige wall" even though they'd sit at very different
/// points along a pure RGB or luminance axis). This sidesteps the single-tone assumption
/// entirely: an ink pixel of *any* colour or brightness still stands out against its own
/// immediate surroundings, which a whole-crop global threshold can never see.
///
/// Connected-component filtering (dropping components implausibly large relative to the crop, a
/// simple proxy for "this is backdrop texture bleeding into the raw threshold, not a real glyph
/// stroke") removes the texture false-positives a naive per-pixel threshold would otherwise pick
/// up from a genuinely non-uniform backdrop.
///
/// Returns `None` when no pixel registers a meaningful local colour distance at all (a flat crop
/// with nothing to separate from its own surroundings).
///
/// `coarse_prior`, when given, must be exactly `crop.width() * crop.height()` long, row-major,
/// already cropped to this same region (`lanrurugi_ocr::entities::RawTextMask::to_bool_vec`'s own
/// output shape, via `DetectedTextRegion::raw_text_mask` — see [`MIN_PLAUSIBLE_RAW_COVERAGE`]'s
/// own doc comment for what it's used for and why it's checked before, not instead of,
/// [`kmeans_stroke_mask`]). A mismatched length is silently ignored (treated the same as `None`)
/// rather than panicking — a caller threading a stale/wrongly-sized prior through is a real
/// possibility (region bboxes can legitimately be re-cropped/clamped between detection and this
/// call) and "prior unavailable, fall through to the next fallback" is the correct degrade, not a
/// crash.
pub fn local_background_stroke_mask(
    crop: &RgbImage,
    coarse_prior: Option<&[bool]>,
) -> Option<StrokeMask> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let coarse_prior = coarse_prior.filter(|p| p.len() == (w * h) as usize);

    let to_lab = |px: &Rgb<u8>| -> Lab {
        Srgb::new(px.0[0], px.0[1], px.0[2])
            .into_format::<f32>()
            .into_color()
    };
    let l: Vec<f32> = crop.pixels().map(|px| to_lab(px).l).collect();
    let a: Vec<f32> = crop.pixels().map(|px| to_lab(px).a).collect();
    let b: Vec<f32> = crop.pixels().map(|px| to_lab(px).b).collect();

    // Pass 1: coarse classification via each pixel's distance from its own windowed *median* Lab
    // colour — a real sliding-window median (via `imageproc::filter::median_filter`'s own
    // histogram-based `O(radius)`-per-pixel algorithm, not a fresh sort per pixel), not the
    // windowed *mean* an earlier revision of this function used. A real reported incident
    // (2026-09-08): a mean is not robust to a thin stroke occupying a small minority of the
    // window's pixels — the stroke's own colour gets diluted into the average rather than
    // standing apart from it, so a 1px-wide outline occupying under 10% of a 19x19 window failed
    // to register as different from its own local "background" estimate at all. A median doesn't
    // have that failure mode: as long as a stroke covers under half the window, the window's
    // median is still a real background sample, untouched by the stroke's own colour — recovering
    // the same robustness the original literal-median implementation had, without its `O(window
    // size · log window size)`-per-pixel sort (see that revision's own doc comment on this
    // function for the exact incident: 3+ GB RSS, minutes of `D`-state, from ~1.5 million
    // short-lived heap allocations and ~4.5 billion sort comparisons across one real page's ~28
    // detected regions).
    //
    // `median_filter` only accepts `Pixel<Subpixel = u8>` images, so each Lab channel is quantised
    // to `u8` first (`L` from its native `0..100` range, `a`/`b` from their native `-128..127`
    // range) and filtered independently as its own `GrayImage` — the filter is already per-channel
    // independent, so there's no need to pack the three channels into one `Rgb<u8>` image first.
    let quantise_l = |v: f32| (v.clamp(0.0, 100.0) * 2.55).round() as u8;
    let quantise_ab = |v: f32| ((v.clamp(-128.0, 127.0) + 128.0).round()) as u8;
    let l_img = GrayImage::from_fn(w, h, |x, y| Luma([quantise_l(l[(y * w + x) as usize])]));
    let a_img = GrayImage::from_fn(w, h, |x, y| Luma([quantise_ab(a[(y * w + x) as usize])]));
    let b_img = GrayImage::from_fn(w, h, |x, y| Luma([quantise_ab(b[(y * w + x) as usize])]));
    let radius = LOCAL_BACKGROUND_RADIUS as u32;
    let l_med_img = median_filter(&l_img, radius, radius);
    let a_med_img = median_filter(&a_img, radius, radius);
    let b_med_img = median_filter(&b_img, radius, radius);
    let dequantise_l = |v: u8| f32::from(v) / 2.55;
    let dequantise_ab = |v: u8| f32::from(v) - 128.0;

    let mut coarse_distance = vec![0f32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let bg_l = dequantise_l(l_med_img.get_pixel(x, y).0[0]);
            let bg_a = dequantise_ab(a_med_img.get_pixel(x, y).0[0]);
            let bg_b = dequantise_ab(b_med_img.get_pixel(x, y).0[0]);
            coarse_distance[i] =
                ((l[i] - bg_l).powi(2) + (a[i] - bg_a).powi(2) + (b[i] - bg_b).powi(2)).sqrt();
        }
    }
    let Some(coarse_threshold) = background_cluster_threshold(&coarse_distance) else {
        // No real separation found at all (a flat/near-flat crop) — nothing to erase.
        return None;
    };
    let maybe_ink: Vec<bool> = coarse_distance
        .iter()
        .map(|&d| d > coarse_threshold)
        .collect();

    // Pass 2: re-estimate the local background using only pixels the coarse pass did *not* flag
    // as possible ink, refining Pass 1's median-based estimate further — build a summed-area table
    // over only the background-pixel subset (and a matching background-pixel-count table so the
    // windowed mean divides by the right denominator, since a window may contain a different
    // number of background pixels depending how much of it the coarse ink mask covers).
    //
    // A query pixel that Pass 1 *itself* misclassified as background (a false negative — e.g. a
    // thin stroke pixel wrongly marked `maybe_ink = false`) would otherwise count itself as one of
    // its own background samples here, pulling its own background estimate toward its own colour
    // and driving its distance toward zero — confirmed live, 2026-09-08, as the second half of the
    // original mean-based failure (the white outline pixel's `refined` distance came out as
    // exactly `0`, i.e. it was comparing itself to a "background" average that included itself).
    // Subtracting out a small `SELF_EXCLUSION_RADIUS`-sized window centred on the query pixel
    // (still an `O(1)`-per-pixel summed-area-table query — one extra rectangle subtraction) means
    // the background estimate can never include the query pixel or its immediate few neighbours,
    // regardless of what Pass 1 decided about them.
    let bg_mask: Vec<bool> = maybe_ink.iter().map(|&ink| !ink).collect();
    let l_bg: Vec<f32> = l
        .iter()
        .zip(&bg_mask)
        .map(|(&v, &bg)| if bg { v } else { 0.0 })
        .collect();
    let a_bg: Vec<f32> = a
        .iter()
        .zip(&bg_mask)
        .map(|(&v, &bg)| if bg { v } else { 0.0 })
        .collect();
    let b_bg: Vec<f32> = b
        .iter()
        .zip(&bg_mask)
        .map(|(&v, &bg)| if bg { v } else { 0.0 })
        .collect();
    let bg_count: Vec<f32> = bg_mask
        .iter()
        .map(|&bg| if bg { 1.0 } else { 0.0 })
        .collect();

    let sat_l_bg = SummedAreaTable::build(&l_bg, w, h);
    let sat_a_bg = SummedAreaTable::build(&a_bg, w, h);
    let sat_b_bg = SummedAreaTable::build(&b_bg, w, h);
    let sat_bg_count = SummedAreaTable::build(&bg_count, w, h);

    let (wi, hi) = (w as i32, h as i32);
    let mut refined_distance = vec![0f32; (w * h) as usize];
    for y in 0..hi {
        for x in 0..wi {
            let (x0, y0, x1, y1) = window_bounds(x, y, wi, hi, LOCAL_BACKGROUND_RADIUS);
            let (ex0, ey0, ex1, ey1) = window_bounds(x, y, wi, hi, SELF_EXCLUSION_RADIUS);
            let count = sat_bg_count.window_sum(x0, y0, x1, y1)
                - sat_bg_count.window_sum(ex0, ey0, ex1, ey1);
            let i = (y * wi + x) as usize;
            // No background-classified pixel anywhere in the outer window minus the self-exclusion
            // window (a fully ink-covered patch, or the coarse pass's own edge case) — fall back
            // to the coarse pass's own distance for this pixel rather than dividing by zero.
            let d = if count > 0.0 {
                let bg_l = (sat_l_bg.window_sum(x0, y0, x1, y1)
                    - sat_l_bg.window_sum(ex0, ey0, ex1, ey1))
                    / count;
                let bg_a = (sat_a_bg.window_sum(x0, y0, x1, y1)
                    - sat_a_bg.window_sum(ex0, ey0, ex1, ey1))
                    / count;
                let bg_b = (sat_b_bg.window_sum(x0, y0, x1, y1)
                    - sat_b_bg.window_sum(ex0, ey0, ex1, ey1))
                    / count;
                let (here_l, here_a, here_b) = (l[i], a[i], b[i]);
                ((here_l - bg_l).powi(2) + (here_a - bg_a).powi(2) + (here_b - bg_b).powi(2)).sqrt()
            } else {
                coarse_distance[i]
            };
            refined_distance[i] = d;
        }
    }

    let threshold = background_cluster_threshold(&refined_distance)?;
    let mut raw: Vec<bool> = refined_distance.iter().map(|&d| d > threshold).collect();

    // Connected-component area filter: a real glyph stroke is a small, compact blob; a swathe of
    // backdrop texture that happened to register above the distance threshold tends to form a
    // much larger connected region (texture noise is spatially diffuse, ink strokes are not) —
    // drop any component covering more than this fraction of the crop's own area outright, on the
    // theory that no single real glyph stroke plausibly covers that much of its own region.
    const MAX_COMPONENT_AREA_FRACTION: f32 = 0.5;
    let max_component_area = ((w * h) as f32 * MAX_COMPONENT_AREA_FRACTION) as usize;
    drop_oversized_components(&mut raw, w, h, max_component_area);

    // Coverage sanity check — see [`MIN_PLAUSIBLE_RAW_COVERAGE`]'s own doc comment for the real
    // incident this exists for and the fallbacks it triggers, tried in order: the detection
    // model's own coarse prior first (a real, independently-computed signal about where text
    // actually is — cheap to consult, already computed at detection time, no extra inference
    // here), then k-means only if that still isn't enough.
    let raw_coverage = raw.iter().filter(|&&m| m).count() as f32 / raw.len() as f32;
    if raw_coverage < MIN_PLAUSIBLE_RAW_COVERAGE {
        if let Some(prior) = coarse_prior {
            for (r, &p) in raw.iter_mut().zip(prior) {
                *r |= p;
            }
        }
        let supplemented_coverage = raw.iter().filter(|&&m| m).count() as f32 / raw.len() as f32;
        if supplemented_coverage < MIN_PLAUSIBLE_RAW_COVERAGE {
            if let Some(kmeans_raw) = kmeans_stroke_mask(crop) {
                let kmeans_coverage =
                    kmeans_raw.iter().filter(|&&m| m).count() as f32 / kmeans_raw.len() as f32;
                if kmeans_coverage > supplemented_coverage {
                    raw = kmeans_raw;
                }
            }
        }
    }

    let paste = dilate(&raw, w, h, PASTE_DILATION_RADIUS);
    let model = dilate(&raw, w, h, DILATION_RADIUS);
    Some(StrokeMask { raw, paste, model })
}

/// Refines a coarse, per-pixel "text is here" prior into a precise glyph-shape mask via a dense
/// conditional random field ([`crate::densecrf::DenseCrf2d`]) — the same architecture
/// `zyddnys/manga-image-translator`'s own `mask_refinement/text_mask_utils.py::refine_mask` uses,
/// with identical hyperparameters (`sxy=1, compat=3` Gaussian; `sxy=23, srgb=7, compat=20`
/// bilateral; 5 mean-field iterations), ported line-for-line rather than re-tuned — see
/// [`crate::densecrf`]'s own module doc comment for why matching a known-correct reference exactly
/// is the actual correctness argument for numerical code like this, not an independent guess at
/// "reasonable" hyperparameters.
///
/// This exists specifically for the region this crate previously had no trustworthy mask for at
/// all: no confident `fg`/`bg` colour pair (`style_estimate::estimate` returned `None`, so
/// [`stroke_mask`] can't run), which `combined_erase_mask` in `lanrurugi-translate` used to treat
/// as "skip this region's erase entirely" after `local_background_stroke_mask` was found to
/// produce visible smudging on exactly these inputs (see this file's own history for that
/// incident). `coarse_prior` — the detection model's own already-computed, already-thresholded
/// per-pixel text mask (`lanrurugi_ocr::entities::RawTextMask`, via
/// `DetectedTextRegion::raw_text_mask`) — is a real, independently-produced signal about where
/// text actually is, not a guess; DenseCRF's whole job here is pulling that coarse box-shaped
/// signal (a detector's own receptive field rarely traces a glyph's real edge precisely) into
/// alignment with the crop's actual local colour/position structure, exactly the refinement
/// `refine_mask` performs on its own detector's coarse output.
///
/// `coarse_prior` must be exactly `crop.width() * crop.height()` long, row-major (same contract as
/// [`local_background_stroke_mask`]'s own `coarse_prior` parameter). Returns `None` when the prior
/// doesn't match the crop's dimensions, or when the crop is empty — there is nothing for DenseCRF
/// to refine without a real starting point, and it is not this function's job to invent one (that
/// remains [`local_background_stroke_mask`]/[`kmeans_stroke_mask`]'s territory when a caller still
/// wants a from-scratch guess instead of a refinement).
pub fn densecrf_stroke_mask(crop: &RgbImage, coarse_prior: &[bool]) -> Option<StrokeMask> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 || coarse_prior.len() != (w * h) as usize {
        return None;
    }

    // The detection model's own raw mask only ever marks a glyph's dark strokes (that's what it
    // was trained to find) — it has no notion of a white/light outline stroke drawn around those
    // strokes, common in manga SFX/caption lettering sitting directly over illustrated backdrops
    // (real reported incident, 2026-09-16: "帰ってくる家間違えた?"'s own white outline, ~4-8px wide
    // on a real page, was left almost entirely unerased — its pixels never had a chance to be
    // classified as text at all, since the unary below gives them essentially zero prior
    // probability, and the bilateral pairwise term's colour-similarity smoothing can't rescue them
    // either: a white outline against a light backdrop has too little colour contrast for that
    // term to pull it toward the (dark) stroke cluster). Dilating the prior *before* it becomes
    // the unary — rather than only dilating the final `raw` classification afterward, which
    // `StrokeMask::model`/`StrokeMask::paste` already do for anti-aliasing margins — gives DenseCRF
    // a starting hypothesis that already covers the outline band, letting the pairwise terms then
    // refine that wider hypothesis against the crop's real edges instead of trying to recover
    // pixels the unary alone had already all but ruled out.
    const PRIOR_DILATION_RADIUS: i32 = 4;
    /// Extra margin (px) added to each component's DenseCRF *search window* beyond its own
    /// bounding box — see the per-component loop below for why this is deliberately a window
    /// expansion, never a prior (unary) dilation.
    const CRF_SEARCH_MARGIN: i32 = 8;
    let dilated_prior = dilate(coarse_prior, w, h, PRIOR_DILATION_RADIUS);

    // The reference project's own `complete_mask` (`text_mask_utils.py`) never runs its DenseCRF
    // equivalent (`refine_mask`) over a whole region in one shot — it first splits the *coarse*
    // mask into connected components, drops any component that isn't plausibly a real textline
    // (compared against the detector's own textline polygons there; this crate has no such
    // polygon, only the coarse prior itself, so the same fixed-fraction area cap
    // [`local_background_stroke_mask`] already uses stands in for that comparison), and only
    // *then* re-crops each surviving component to its own tight bounding box and runs DenseCRF on
    // that small crop alone. Real reported incident (2026-09-16) that this reordering fixes: an
    // earlier revision ran DenseCRF once over the *whole* OCR bbox first and filtered by area
    // afterward — on a busy/textured backdrop (the page's own patterned question-mark artwork),
    // the bilateral pairwise term's colour/position smoothing (which has no notion of what text
    // *looks like*, only "nearby similar-coloured pixels probably share a label") pulled the
    // *entire* crop into one single connected blob of misclassified "text", so the after-the-fact
    // area filter had only one giant component to judge and dropped the whole region outright —
    // zero erase for a region that, cropped correctly per-component first, refines cleanly.
    // Running DenseCRF per pre-filtered component instead keeps that failure local: a busy
    // backdrop can still confuse the pairwise terms *within* one component's own small crop, but
    // it can no longer swallow the rest of the region's genuinely separate text components along
    // with it, since those never shared a DenseCRF call (or even a connected-component grouping)
    // with the confused one in the first place.
    // The reference implementation (`mask_refinement/text_mask_utils.py::complete_mask`) has no
    // "component covers too much of the crop" rule at all — it validates each connected component
    // against the *detected text-lines* (`area1 >= area2`), and its own dilation is adaptive
    // (`dilate_size = max((int(text_size * 0.3) // 2) * 2 + 1, 3)`, plus a `0.1 * text_size` region
    // extension before refining). A previous revision here instead dropped any component whose
    // 4px-dilated extent covered more than **50%** of the *tight* region crop. Measured against a
    // real page (issue #103) that drops ordinary, clean, single-line labels — e.g. the "トウカ"
    // label's own dilated prior covers 75.5% of its crop, the "皇族ガーディアン" line 65.8% — so
    // `densecrf_stroke_mask` returned `None`, the caller treated the region as "no precise mask",
    // and skipped erase *and* redraw, leaving the original Japanese on the page. That is exactly
    // the reported "检测漏检"-looking symptom, even though the detector had emitted the box.
    //
    // The one guard kept is the reference's own "component bigger than its text-line" rule, which
    // in this crate's per-region crop (the crop *is* the text-line) means "covers essentially the
    // whole crop" — kept only to reject a near-total blob, and applied to the *refined* output so
    // a legitimate dense prior is never dropped before DenseCRF gets a chance to run.
    const NEAR_TOTAL_COVERAGE_FRACTION: f32 = 0.98;
    let max_component_area = ((w * h) as f32 * NEAR_TOTAL_COVERAGE_FRACTION) as usize;
    let components: Vec<Vec<usize>> = connected_components(&dilated_prior, w, h);
    if components.is_empty() {
        return None;
    }

    let mut raw = vec![false; (w * h) as usize];
    for component in &components {
        // Tight bounding box around just this component (same "re-crop before refining" step the
        // reference's own per-component `refine_mask` call does).
        let (mut x0, mut y0, mut x1, mut y1) = (w as i32, h as i32, 0i32, 0i32);
        for &i in component {
            let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }

        // Widen the DenseCRF *search window* by `CRF_SEARCH_MARGIN` on every side (clamped to the
        // crop) while leaving the unary prior exactly `component`. The detection model's coarse
        // prior systematically stops short of the real glyph at its ends/edges by several px
        // (measured ~8px on a real label's own strokes); the previous window *was* the component's
        // own bounding box, so those pixels were never presented to DenseCRF at all and could not
        // be recovered no matter what it decided — not a tuning problem, a structural one.
        // Widening only the window — never the prior/unary — lets the bilateral colour/position
        // term pull a colour-similar stroke end back into the text label, while a
        // differently-coloured illustrated backdrop stays background. It does NOT introduce the
        // unconditional over-reach a larger *prior dilation* causes (the real regression that made
        // `PRIOR_DILATION_RADIUS=6` unsafe on illustration backdrops).
        let (wx0, wy0) = (
            (x0 - CRF_SEARCH_MARGIN).max(0),
            (y0 - CRF_SEARCH_MARGIN).max(0),
        );
        let (wx1, wy1) = (
            (x1 + CRF_SEARCH_MARGIN).min(w as i32),
            (y1 + CRF_SEARCH_MARGIN).min(h as i32),
        );
        let (cw, ch) = ((wx1 - wx0) as u32, (wy1 - wy0) as u32);
        if cw == 0 || ch == 0 {
            continue;
        }

        let sub_crop = image::imageops::crop_imm(crop, wx0 as u32, wy0 as u32, cw, ch).to_image();
        let mut sub_prior = vec![false; (cw * ch) as usize];
        for &i in component {
            let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
            let (sx, sy) = (x - wx0, y - wy0);
            sub_prior[(sy as u32 * cw + sx as u32) as usize] = true;
        }

        let sub_raw = densecrf_classify(&sub_crop, &sub_prior, cw, ch);
        // Busy-backdrop / blob guard, on the *refined* output (see `MAX_COMPONENT_AREA_FRACTION`'s
        // own comment above for why it must not be applied to the coarse prior).
        let refined_count = sub_raw.iter().filter(|b| **b).count();
        if refined_count > max_component_area {
            tracing::debug!(
                refined_count,
                max_component_area,
                "densecrf_stroke_mask: refined component still covers essentially the whole crop; dropping it"
            );
            continue;
        }
        for (sub_i, &is_text) in sub_raw.iter().enumerate() {
            if !is_text {
                continue;
            }
            let (sx, sy) = ((sub_i as u32 % cw) as i32, (sub_i as u32 / cw) as i32);
            let (x, y) = (wx0 + sx, wy0 + sy);
            raw[(y as u32 * w + x as u32) as usize] = true;
        }
    }

    // Every component's own refinement was rejected as a blob: treat the whole region as "no
    // precise mask" (returns `None`) rather than handing back an empty `Some` mask, so the caller
    // skips erase *and* redraw instead of drawing over unerased text.
    if !raw.iter().any(|b| *b) {
        return None;
    }

    // The detector prior alone is not a complete text signal, and DenseCRF cannot fix that by
    // itself: its per-component window is the prior's own component bbox plus `CRF_SEARCH_MARGIN`,
    // so anything the prior never marked more than that margin away is structurally unreachable.
    // Measured against a real page (2026-09-18, issue #101, region `そんな三皇女様を…`):
    // `raw_text_mask` covered 24.9% of the crop, the refined CRF mask covered only 54.7% of the
    // pixels that stayed as visible original strokes in the final render, and 46% of those
    // residual strokes were >8px away from any prior pixel; across five other real regions the
    // same figure was 61-86%. Meanwhile [`local_background_stroke_mask`] — already in this module
    // — covered 100% of those same residual pixels on that region (it classifies each pixel by
    // distance from its own local median backdrop, so it does not need the detector's kernel to
    // have marked the stroke) but as a *standalone* mask it was previously rejected for smudging
    // busy backdrops.
    //
    // The fix is therefore a bounded supplement, not a replacement: take `local_background`'s own
    // ink classification only where it is within `LOCAL_SUPPLEMENT_GROW_RADIUS` of the already-
    // refined CRF mask. That closes anti-aliased stroke edges (which `model` fed to LaMa but
    // `paste` never overwrote — `StrokeMask`'s own `paste`/`model` split leaves the 2-8px ring
    // original) and whole nearby strokes, while a far-away backdrop false positive from the
    // local-background estimate still cannot enter the mask on its own. Re-running DenseCRF on
    // the *union* seed instead was measured on the same region and is explicitly rejected: the
    // union's dense unary lets the Potts pairwise term (`compat=20`) smear to 74% of the crop,
    // i.e. an over-erase blob, whereas this bounded supplement measured 25.7% (vs. 20.4% for the
    // CRF alone) while covering 93.5% of those same residual strokes.
    const LOCAL_SUPPLEMENT_GROW_RADIUS: i32 = 24;
    if let Some(local) = local_background_stroke_mask(crop, Some(coarse_prior)) {
        let nearby = dilate(&raw, w, h, LOCAL_SUPPLEMENT_GROW_RADIUS);
        for i in 0..raw.len() {
            if !raw[i] && local.raw[i] && nearby[i] {
                raw[i] = true;
            }
        }
    }

    let paste = dilate(&raw, w, h, PASTE_DILATION_RADIUS);
    let model = dilate(&raw, w, h, DILATION_RADIUS);
    Some(StrokeMask { raw, paste, model })
}

/// The actual DenseCRF call [`densecrf_stroke_mask`] now makes once per pre-filtered connected
/// component's own tight crop, rather than once over a whole region — factored out because that
/// call site needs exactly this (crop, prior) -> raw classification step with no dilation/
/// component-filtering/mask-assembly wrapped around it (all of that is already handled once at
/// the whole-region level by its own caller).
fn densecrf_classify(crop: &RgbImage, prior: &[bool], w: u32, h: u32) -> Vec<bool> {
    const CLIP: f32 = 1e-5;
    let n = (w * h) as usize;
    // `mask_softmax = [1 - rawmask, rawmask]` in the original, both channels clipped to `[CLIP,
    // 1]` by `unary_from_softmax` — reproduced directly here (pixel-major, 2 labels: 0 =
    // background, 1 = text) rather than going through a literal softmax array first, since the
    // source values are already hard 0/1.
    let mut probs = vec![0f32; n * 2];
    for (i, &is_text) in prior.iter().enumerate() {
        let (bg_p, text_p) = if is_text { (CLIP, 1.0) } else { (1.0, CLIP) };
        probs[i * 2] = bg_p;
        probs[i * 2 + 1] = text_p;
    }

    let mut crf = DenseCrf2d::new(w as usize, h as usize, 2);
    crf.set_unary_energy_from_softmax(&probs);
    crf.add_pairwise_gaussian(1.0, 1.0, 3.0);
    let rgb: Vec<u8> = crop.pixels().flat_map(|px| px.0).collect();
    crf.add_pairwise_bilateral(23.0, 23.0, 7.0, 7.0, 7.0, &rgb, 20.0);
    let q = crf.inference(5);

    // `argmax` over the 2 labels per pixel — label 1 (text) wins when its probability exceeds
    // label 0's, matching `np.argmax(Q, axis=0)` in the original exactly.
    (0..n).map(|i| q[i * 2 + 1] > q[i * 2]).collect()
}

/// A from-scratch fallback stroke classification for when [`local_background_stroke_mask`]'s own
/// coverage sanity check trips (see that function's own doc comment on the check itself and the
/// real incident it exists for) — global k-means colour clustering (`k` = 2) over the whole crop's
/// own Lab pixels, immune to [`local_background_stroke_mask`]'s specific failure mode (a *local*
/// window's background estimate getting dragged toward a nearby real decorative feature) since
/// this never estimates a local background at all: every pixel is classified purely by which of
/// the crop's own two global colour clusters it's closer to.
///
/// The smaller of the two clusters (by pixel count) is treated as the glyph — real manga lettering
/// is a minority of its own tightly-cropped OCR region by construction (the crop always includes
/// real surrounding backdrop), so whichever cluster covers less of the crop is the more plausible
/// "ink" candidate; the majority cluster is background. This is a coarser signal than
/// [`local_background_stroke_mask`]'s own per-pixel local classification (no shape/texture
/// awareness, no connected-component filtering here), which is why it's only ever a fallback for
/// when that finer approach measurably failed, not a routine first choice.
///
/// Returns `None` when k-means itself can't be run at all (an empty crop) — a real region always
/// has enough pixels to cluster, so this should never trip in practice for a real caller.
fn kmeans_stroke_mask(crop: &RgbImage) -> Option<Vec<bool>> {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let lab: Vec<Lab> = crop
        .pixels()
        .map(|px| {
            Srgb::new(px.0[0], px.0[1], px.0[2])
                .into_format::<f32>()
                .into_color()
        })
        .collect();

    const K: usize = 2;
    const MAX_ITER: usize = 20;
    const CONVERGE: f32 = 5.0;
    const SEED: u64 = 0;
    let result = kmeans_colors::get_kmeans(K, MAX_ITER, CONVERGE, false, &lab, SEED);
    if result.centroids.len() < K {
        // Degenerate input (e.g. every pixel identical) collapsed to fewer than `K` real clusters
        // — no meaningful glyph/background split to report.
        return None;
    }

    let mut counts = [0usize; K];
    for &idx in &result.indices {
        counts[idx as usize] += 1;
    }
    let glyph_cluster = if counts[0] <= counts[1] { 0u8 } else { 1u8 };
    Some(
        result
            .indices
            .iter()
            .map(|&idx| idx == glyph_cluster)
            .collect(),
    )
}

/// Clamps a `LOCAL_BACKGROUND_RADIUS`-sized window centred on `(x, y)` to the image bounds,
/// returning `(x0, y0, x1, y1)` as an inclusive-exclusive rectangle ready for
/// [`SummedAreaTable::window_sum`].
fn window_bounds(x: i32, y: i32, w: i32, h: i32, radius: i32) -> (i32, i32, i32, i32) {
    (
        (x - radius).max(0),
        (y - radius).max(0),
        (x + radius + 1).min(w),
        (y + radius + 1).min(h),
    )
}

/// Multiplier on the background cluster's own median absolute deviation (MAD), converted to an
/// equivalent-to-standard-deviation scale via the usual `1.4826` consistency constant, used by
/// [`background_cluster_threshold`] — `6` (so effectively ~6 "robust standard deviations") is
/// deliberately generous, favouring under-classifying a genuinely ambiguous pixel as background
/// over eating into real backdrop right next to a glyph.
const BACKGROUND_MAD_MULTIPLIER: f32 = 6.0 * 1.4826;

/// A hard floor on [`background_cluster_threshold`]'s own spread term, added on top of (not
/// instead of) the MAD-based term — needed because a *perfectly* flat background (real crops with
/// zero local noise, and every synthetic test in this module) drives MAD itself to exactly `0`,
/// which would otherwise flag any nonzero distance at all as ink. `1.0` matches
/// [`otsu_threshold`]'s own "no real separation" cutoff used elsewhere in this file, i.e. distances
/// under `1.0` are already treated as noise-level everywhere else in this module.
const BACKGROUND_DISTANCE_FLOOR: f32 = 1.0;

/// Otsu's own threshold-selection maths (bucketed histogram, maximise between-class variance —
/// same algorithm [`otsu_stroke_mask`] uses on raw luminance), applied here to a *distance* map
/// instead: distance from local background is a genuinely bimodal-*or-more* quantity (either a
/// pixel is part of an ink stroke and stands out from its neighbourhood, or it's backdrop and
/// doesn't) even when raw colour/luminance is not, because a multi-tone glyph's every tone still
/// stands out from its own local surroundings the same way. Returns `None` when every value is
/// (near-)identical — no real separation to find.
///
/// Splits `distance` into two groups at the Otsu-optimal point and returns `(threshold,
/// background_group)`, where `background_group` is whichever side of the split has more members
/// (this module's crops are always mostly backdrop by area, so the majority side is always
/// background, never ink) — [`background_cluster_threshold`] uses that group's own mean/stddev
/// instead of this raw split point directly, since a raw Otsu split is only reliable for a
/// genuinely two-cluster distribution (see that function's own doc comment for why a three-cluster
/// case, e.g. background + a thin ink outline + a solid ink fill, needs the extra step).
fn otsu_threshold(distance: &[f32]) -> Option<(f32, Vec<f32>)> {
    let max_distance = distance.iter().copied().fold(0f32, f32::max);
    if max_distance < 1.0 {
        return None;
    }

    const DISTANCE_BUCKETS: usize = 256;
    let mut histogram = [0u32; DISTANCE_BUCKETS];
    let bucket_of = |d: f32| {
        ((d / max_distance) * ((DISTANCE_BUCKETS - 1) as f32))
            .round()
            .clamp(0.0, (DISTANCE_BUCKETS - 1) as f32) as usize
    };
    for &d in distance {
        histogram[bucket_of(d)] += 1;
    }
    let total = distance.len() as f64;
    let sum_all: f64 = histogram
        .iter()
        .enumerate()
        .map(|(i, &c)| (i as f64) * f64::from(c))
        .sum();
    let mut sum_below = 0f64;
    let mut weight_below = 0f64;
    let mut best_bucket = 0usize;
    let mut best_variance = 0f64;
    for (bucket, &count) in histogram.iter().enumerate() {
        weight_below += f64::from(count);
        if weight_below <= 0.0 {
            continue;
        }
        let weight_above = total - weight_below;
        if weight_above <= 0.0 {
            break;
        }
        sum_below += (bucket as f64) * f64::from(count);
        let mean_below = sum_below / weight_below;
        let mean_above = (sum_all - sum_below) / weight_above;
        let between_variance = weight_below * weight_above * (mean_below - mean_above).powi(2);
        if between_variance > best_variance {
            best_variance = between_variance;
            best_bucket = bucket;
        }
    }
    if best_variance <= 0.0 {
        return None;
    }
    let split = (best_bucket as f32 / (DISTANCE_BUCKETS - 1) as f32) * max_distance;
    let (below, above): (Vec<f32>, Vec<f32>) = distance.iter().copied().partition(|&d| d <= split);
    let background_group = if below.len() >= above.len() {
        below
    } else {
        above
    };
    Some((split, background_group))
}

/// The real threshold [`local_background_stroke_mask`] classifies ink against — deliberately not
/// [`otsu_threshold`]'s own raw split point. A pure Otsu split maximises *between-class* variance
/// for exactly two classes; applied directly to a genuinely three-cluster distance distribution
/// (background, a thin ink outline, a solid ink fill — the real reported case, 2026-09-08, a
/// blue-filled white-outlined glyph over textured wallpaper) it reliably separates the single most
/// extreme cluster (the solid fill, furthest from background) from *everything else*, including
/// the outline, rather than separating background from both ink clusters — confirmed both by a
/// standalone maths check (population-weighted between-class variance is provably higher for the
/// fill-vs-rest split than the background-vs-rest split whenever the outline cluster is small
/// relative to the other two) and by this function's own real debug output on the failing test:
/// `refined_threshold` landed at `22.44`, a hair above the outline's own real distance (`~22.26`,
/// consistent across all 60 real outline pixels checked) and far below the fill's (`~97`) — Otsu
/// was doing exactly what it's designed to do, just not what this module needs from it.
///
/// Instead: take [`otsu_threshold`]'s split only to identify which pixels are the dominant
/// background cluster (always the majority side, since these crops are mostly backdrop by area),
/// then classify ink as "far enough above that cluster's own centre" using the cluster's *median*
/// and *median absolute deviation* rather than its mean/standard deviation — a real reported
/// incident (2026-09-08, after switching to mean/stddev first): the outline cluster sits close
/// enough to Otsu's own split point that roughly half of it lands on the "background" side of the
/// split too, and a plain mean/stddev has no resistance to that kind of contamination (the outline
/// pixels that leaked in drag the mean and inflate the stddev enough to raise the final threshold
/// *above* the outline's own real distance, the opposite of the intended effect). A median and MAD
/// stay correct under up to 50% contamination — exactly the property needed here, since the
/// "background" side of an Otsu split on a three-cluster distribution can genuinely contain that
/// much contamination from a mis-split smaller cluster.
fn background_cluster_threshold(distance: &[f32]) -> Option<f32> {
    let (_, mut background) = otsu_threshold(distance)?;
    if background.is_empty() {
        return None;
    }
    background.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = background[background.len() / 2];
    let mut deviations: Vec<f32> = background.iter().map(|&d| (d - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = deviations[deviations.len() / 2];
    Some(median + (BACKGROUND_MAD_MULTIPLIER * mad).max(BACKGROUND_DISTANCE_FLOOR))
}

/// A 2D summed-area table (integral image): after `O(w·h)` construction, answers "sum of values
/// in any axis-aligned rectangle" in `O(1)`, which is what lets
/// [`local_background_stroke_mask`] compute a windowed *mean* per pixel in linear total time
/// instead of resorting a fresh window at every pixel. Standard technique (the same one behind
/// Viola-Jones-style box filters and Bradley's adaptive thresholding), not a novel structure —
/// picked here specifically because the window size this crate needs
/// ([`LOCAL_BACKGROUND_RADIUS`]) no longer affects per-pixel query cost at all once built.
struct SummedAreaTable {
    /// `(w+1) x (h+1)` — one extra row/column of leading zeros so `window_sum` never needs a
    /// special case for a window touching the image's own top/left edge.
    sums: Vec<f64>,
    w: i32,
}

impl SummedAreaTable {
    fn build(values: &[f32], w: u32, h: u32) -> Self {
        let (wi, hi) = (w as i32, h as i32);
        let stride = wi + 1;
        let mut sums = vec![0f64; (stride * (hi + 1)) as usize];
        for y in 0..hi {
            let mut row_sum = 0f64;
            for x in 0..wi {
                row_sum += f64::from(values[(y * wi + x) as usize]);
                let above = sums[((y * stride) + x + 1) as usize];
                sums[(((y + 1) * stride) + x + 1) as usize] = above + row_sum;
            }
        }
        Self { sums, w: stride }
    }

    /// Sum of `values` over `[x0, x1) x [y0, y1)` (half-open, matching [`window_bounds`]'s own
    /// convention) — the standard inclusion-exclusion identity over four corner lookups.
    fn window_sum(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> f32 {
        let at = |x: i32, y: i32| self.sums[(y * self.w + x) as usize];
        (at(x1, y1) - at(x0, y1) - at(x1, y0) + at(x0, y0)) as f32
    }
}

/// Finds every 4-connected component in `mask`, returning each as its own row-major pixel-index
/// list — the enumeration [`drop_oversized_components`] itself only needs a pass/fail decision
/// per component for, but [`densecrf_stroke_mask`]'s own per-component DenseCRF re-crop (see that
/// function's own doc comment) needs the actual pixel membership and bounding box of each
/// surviving component, not just a yes/no.
fn connected_components(mask: &[bool], w: u32, h: u32) -> Vec<Vec<usize>> {
    let (wi, hi) = (w as i32, h as i32);
    let mut visited = vec![false; mask.len()];
    let mut components = Vec::new();
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        let mut component = vec![start];
        visited[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i as i32) % wi, (i as i32) / wi);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || nx >= wi || ny < 0 || ny >= hi {
                    continue;
                }
                let ni = (ny * wi + nx) as usize;
                if mask[ni] && !visited[ni] {
                    visited[ni] = true;
                    stack.push(ni);
                    component.push(ni);
                }
            }
        }
        components.push(component);
    }
    components
}

/// Zeroes out every connected component (4-connected) in `mask` whose own pixel count exceeds
/// `max_area` — flood-fill based, no external dependency needed for this crate's own modest crop
/// sizes (a region crop is typically well under a few hundred pixels per side).
fn drop_oversized_components(mask: &mut [bool], w: u32, h: u32, max_area: usize) {
    let (wi, hi) = (w as i32, h as i32);
    let mut visited = vec![false; mask.len()];
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || visited[start] {
            continue;
        }
        let mut component = vec![start];
        visited[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i as i32) % wi, (i as i32) / wi);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || nx >= wi || ny < 0 || ny >= hi {
                    continue;
                }
                let ni = (ny * wi + nx) as usize;
                if mask[ni] && !visited[ni] {
                    visited[ni] = true;
                    stack.push(ni);
                    component.push(ni);
                }
            }
        }
        if component.len() > max_area {
            for i in component {
                mask[i] = false;
            }
        }
    }
}

/// Grows a `true` region outward by `radius` pixels on every side (a plain square structuring
/// element — real-time cost matters more here than a circular kernel's slightly better isotropy,
/// and `DILATION_RADIUS` is small enough the difference is invisible at this scale).
fn dilate(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    if radius <= 0 {
        return mask.to_vec();
    }
    let (wi, hi) = (w as i32, h as i32);
    let mut out = vec![false; mask.len()];
    for y in 0..hi {
        for x in 0..wi {
            if mask[(y * wi + x) as usize] {
                continue;
            }
            let mut hit = false;
            'search: for dy in -radius..=radius {
                let ny = y + dy;
                if ny < 0 || ny >= hi {
                    continue;
                }
                for dx in -radius..=radius {
                    let nx = x + dx;
                    if nx < 0 || nx >= wi {
                        continue;
                    }
                    if mask[(ny * wi + nx) as usize] {
                        hit = true;
                        break 'search;
                    }
                }
            }
            out[(y * wi + x) as usize] = hit;
        }
        for x in 0..wi {
            let idx = (y * wi + x) as usize;
            out[idx] = out[idx] || mask[idx];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, colour: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb(colour))
    }

    #[test]
    fn identical_fg_bg_returns_none() {
        let crop = solid(10, 10, [128, 128, 128]);
        assert!(stroke_mask(&crop, Rgb([128, 128, 128]), Rgb([128, 128, 128])).is_none());
    }

    #[test]
    fn a_pixel_near_fg_is_classified_as_stroke() {
        let mut crop = solid(10, 10, [255, 255, 255]);
        crop.put_pixel(5, 5, Rgb([0, 0, 0]));
        let mask = stroke_mask(&crop, Rgb([0, 0, 0]), Rgb([255, 255, 255])).unwrap();
        assert!(
            mask.raw[(5 * 10 + 5) as usize],
            "the black pixel itself must be stroke in the raw (undilated) mask"
        );
        assert!(
            mask.paste[(5 * 10 + 5) as usize],
            "the black pixel itself must be stroke in the paste mask too"
        );
        assert!(
            mask.model[(5 * 10 + 5) as usize],
            "the black pixel itself must be stroke in the model mask too"
        );
    }

    #[test]
    fn a_pixel_far_from_the_glyph_stays_background_after_dilation() {
        let mut crop = solid(20, 20, [255, 255, 255]);
        crop.put_pixel(10, 10, Rgb([0, 0, 0]));
        let mask = stroke_mask(&crop, Rgb([0, 0, 0]), Rgb([255, 255, 255])).unwrap();
        // Far corner (index 0), well outside either dilation radius of the single glyph pixel.
        assert!(!mask.paste[0]);
        assert!(!mask.model[0]);
    }

    #[test]
    fn sample_stroke_colour_averages_only_the_raw_stroke_pixels() {
        // Two glyph pixels of the *same* shade of "black" plus a lot of white background — the
        // sample must average only the two glyph pixels, not the background, when there's no
        // darkest-half selection to complicate the expected result.
        let mut crop = solid(10, 10, [255, 255, 255]);
        crop.put_pixel(3, 3, Rgb([10, 10, 10]));
        crop.put_pixel(4, 3, Rgb([10, 10, 10]));
        let mask = stroke_mask(&crop, Rgb([10, 10, 10]), Rgb([255, 255, 255])).unwrap();
        let sampled = sample_stroke_colour(&crop, &mask.raw).expect("some stroke pixels exist");
        assert_eq!(sampled, Rgb([10, 10, 10]));
    }

    #[test]
    fn sample_stroke_colour_keeps_only_the_darkest_half() {
        // Real reported incident (2026-09-16): a stroke mask built with no confident colour prior
        // (`densecrf_stroke_mask`) can draw its boundary a little wide around a glyph's real dark
        // strokes, pulling in enough lighter transition/outline-adjacent pixels that a plain mean
        // comes out visibly grey rather than near-black. Four "stroke" pixels of increasing
        // brightness — the sample must average only the darker half (10 and 30), discarding the
        // two brighter ones (50, 70) as likely background bleed-through rather than real ink.
        let mut crop = solid(10, 10, [255, 255, 255]);
        crop.put_pixel(3, 3, Rgb([10, 10, 10]));
        crop.put_pixel(4, 3, Rgb([30, 30, 30]));
        crop.put_pixel(5, 3, Rgb([50, 50, 50]));
        crop.put_pixel(6, 3, Rgb([70, 70, 70]));
        let raw_mask: Vec<bool> = crop.pixels().map(|px| px.0 != [255, 255, 255]).collect();
        let sampled = sample_stroke_colour(&crop, &raw_mask).expect("some stroke pixels exist");
        assert_eq!(sampled, Rgb([20, 20, 20]));
    }

    #[test]
    fn sample_stroke_colour_with_no_stroke_pixels_returns_none() {
        let mask = vec![false; 100];
        let crop = solid(10, 10, [255, 255, 255]);
        assert!(sample_stroke_colour(&crop, &mask).is_none());
    }

    #[test]
    fn dilation_does_not_pollute_the_colour_sample() {
        // A single glyph pixel surrounded by a *differently coloured* background — sampling off
        // the model mask (which also marks a wide background ring around the glyph as "stroke",
        // by design, to give LaMa a real transition band) would pull the average toward that
        // background colour; sampling off the raw mask must not.
        let mut crop = solid(20, 20, [200, 100, 50]);
        crop.put_pixel(10, 10, Rgb([0, 0, 0]));
        let mask = stroke_mask(&crop, Rgb([0, 0, 0]), Rgb([200, 100, 50])).unwrap();
        assert_eq!(
            sample_stroke_colour(&crop, &mask.raw),
            Some(Rgb([0, 0, 0])),
            "the raw mask's own sample must be the pure glyph colour"
        );
        assert_ne!(
            sample_stroke_colour(&crop, &mask.model),
            Some(Rgb([0, 0, 0])),
            "the model mask's sample would be pulled toward the surrounding background"
        );
    }

    #[test]
    fn dilation_grows_a_single_pixel_into_its_neighbourhood() {
        let mask = vec![false; 25];
        let mut mask = mask;
        mask[12] = true; // centre of a 5x5 grid (2,2)
        let dilated = dilate(&mask, 5, 5, 1);
        // The 3x3 neighbourhood around (2,2) should now all be true.
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let idx = ((2 + dy) * 5 + (2 + dx)) as usize;
                assert!(
                    dilated[idx],
                    "expected ({},{}) to be dilated true",
                    2 + dx,
                    2 + dy
                );
            }
        }
        // Just outside that neighbourhood must stay false.
        assert!(!dilated[0]);
    }

    #[test]
    fn zero_radius_is_a_no_op() {
        let mut mask = vec![false; 9];
        mask[4] = true;
        assert_eq!(dilate(&mask, 3, 3, 0), mask);
    }

    #[test]
    fn empty_crop_returns_none() {
        let crop = RgbImage::new(0, 0);
        assert!(stroke_mask(&crop, Rgb([0, 0, 0]), Rgb([255, 255, 255])).is_none());
    }

    #[test]
    fn otsu_finds_a_dark_glyph_on_a_light_background_with_no_colour_priors() {
        let mut crop = solid(20, 20, [230, 230, 230]);
        for y in 8..12 {
            for x in 8..12 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let mask = otsu_stroke_mask(&crop).expect("a real bimodal split exists");
        assert!(
            mask.raw[(9 * 20 + 9) as usize],
            "the dark glyph block must classify as stroke"
        );
        assert!(
            !mask.raw[0],
            "a far corner background pixel must not classify as stroke"
        );
        assert!(
            mask.paste[(9 * 20 + 9) as usize],
            "the paste mask must still cover the glyph itself"
        );
    }

    #[test]
    fn otsu_returns_none_for_a_perfectly_flat_crop() {
        let crop = solid(10, 10, [128, 128, 128]);
        assert!(
            otsu_stroke_mask(&crop).is_none(),
            "a crop with no real luminance variation has no bimodal split to find"
        );
    }

    #[test]
    fn otsu_empty_crop_returns_none() {
        let crop = RgbImage::new(0, 0);
        assert!(otsu_stroke_mask(&crop).is_none());
    }

    #[test]
    fn otsu_classifies_the_darker_cluster_as_stroke_even_on_a_dark_background() {
        // Mid-grey background, near-black glyph — Otsu must still find the split even when
        // neither cluster is anywhere near pure white/black.
        let mut crop = solid(20, 20, [150, 150, 150]);
        for y in 5..15 {
            for x in 5..15 {
                crop.put_pixel(x, y, Rgb([40, 40, 40]));
            }
        }
        let mask = otsu_stroke_mask(&crop).expect("bimodal split exists");
        assert!(
            mask.raw[(10 * 20 + 10) as usize],
            "the darker cluster is the glyph"
        );
        assert!(
            !mask.raw[0],
            "the lighter background must not classify as stroke"
        );
    }

    #[test]
    fn local_background_finds_a_solid_glyph_on_a_uniform_backdrop() {
        let mut crop = solid(40, 40, [220, 210, 200]);
        for y in 15..25 {
            for x in 15..25 {
                crop.put_pixel(x, y, Rgb([30, 30, 30]));
            }
        }
        let mask = local_background_stroke_mask(&crop, None).expect("a real local contrast exists");
        assert!(
            mask.raw[(20 * 40 + 20) as usize],
            "the glyph pixel must classify as stroke"
        );
        assert!(
            !mask.raw[0],
            "a far corner background pixel must not classify as stroke"
        );
        assert!(mask.paste[(20 * 40 + 20) as usize]);
    }

    #[test]
    fn local_background_empty_crop_returns_none() {
        let crop = RgbImage::new(0, 0);
        assert!(local_background_stroke_mask(&crop, None).is_none());
    }

    #[test]
    fn local_background_returns_none_for_a_perfectly_flat_crop() {
        let crop = solid(20, 20, [128, 128, 128]);
        assert!(
            local_background_stroke_mask(&crop, None).is_none(),
            "a crop with no local contrast anywhere has nothing to erase"
        );
    }

    #[test]
    fn local_background_finds_a_multi_tone_outlined_glyph_where_otsu_fails() {
        // Reproduces the real reported bug (2026-09-08): a blue-filled, white-outlined glyph over
        // a lightly textured backdrop — three real tones (blue ink, white outline, background),
        // not the two Otsu's own whole-crop luminance threshold assumes. Build a synthetic crop
        // with the same structure: a mid-tone textured background (alternating two close shades,
        // simulating wall texture), a white outline ring, and a blue-ish fill inside it.
        let (w, h) = (40, 40);
        let mut crop = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // Textured backdrop: alternates between two close beige tones — real texture, not
                // a flat colour, but nowhere near as different as the glyph's own ink tones.
                let base = if (x + y) % 2 == 0 { 210 } else { 200 };
                crop.put_pixel(x, y, Rgb([base, base - 10, base - 30]));
            }
        }
        // White outline ring.
        for y in 12..28 {
            for x in 12..28 {
                let on_ring = y == 12 || y == 27 || x == 12 || x == 27;
                if on_ring {
                    crop.put_pixel(x, y, Rgb([250, 250, 250]));
                }
            }
        }
        // Blue-ish fill inside the ring.
        for y in 14..26 {
            for x in 14..26 {
                crop.put_pixel(x, y, Rgb([40, 60, 180]));
            }
        }

        let mask = local_background_stroke_mask(&crop, None).expect("real local contrast exists");
        assert!(
            mask.raw[(20 * w + 20) as usize],
            "the blue fill's own centre pixel must classify as stroke"
        );
        assert!(
            mask.raw[(12 * w + 20) as usize],
            "the white outline's own pixel must also classify as stroke, not just the fill"
        );
        assert!(
            !mask.raw[(2 * w + 2) as usize],
            "a background-texture pixel far from the glyph must not classify as stroke"
        );
    }

    #[test]
    fn kmeans_stroke_mask_separates_a_minority_ink_cluster_from_a_majority_background() {
        // Plain two-cluster crop: mostly beige backdrop, a smaller cyan-ish glyph block — no local
        // contamination to defeat, just checking `kmeans_stroke_mask` itself gets the basic split
        // right (the smaller cluster by pixel count is the glyph).
        let mut crop = solid(30, 30, [210, 200, 190]);
        for y in 10..20 {
            for x in 10..20 {
                crop.put_pixel(x, y, Rgb([30, 150, 160]));
            }
        }
        let raw = kmeans_stroke_mask(&crop).expect("two real clusters exist");
        assert!(
            raw[(15 * 30 + 15) as usize],
            "the glyph block's own centre must be ink"
        );
        assert!(!raw[0], "a far corner background pixel must not be ink");
    }

    #[test]
    fn local_background_falls_back_to_kmeans_when_raw_coverage_is_implausibly_low() {
        // Directly exercises the fallback wiring itself (not `local_background_stroke_mask` as a
        // whole, which has no way to deterministically force its own coarse/refined pipeline into
        // the low-coverage state the real incident hit — see this constant's own doc comment for
        // the real measured numbers from that incident instead). Simulates exactly what
        // `local_background_stroke_mask` itself does once its raw coverage check trips: given a
        // `raw` result deliberately far below `MIN_PLAUSIBLE_RAW_COVERAGE`, `kmeans_stroke_mask`
        // must find *more* of a real two-cluster crop, confirming the fallback path itself (invoked
        // the same way the real function invokes it) actually improves on an implausibly sparse
        // result rather than being unreachable/inert code.
        let mut crop = solid(30, 30, [210, 200, 190]);
        for y in 5..25 {
            for x in 5..25 {
                crop.put_pixel(x, y, Rgb([30, 150, 160]));
            }
        }
        // A deliberately sparse stand-in for what a broken local pass produced in the real
        // incident — far below `MIN_PLAUSIBLE_RAW_COVERAGE`, even though the crop's own real glyph
        // block is large.
        let mut raw = vec![false; (30 * 30) as usize];
        raw[(15 * 30 + 15) as usize] = true;
        let sparse_coverage = raw.iter().filter(|&&m| m).count() as f32 / raw.len() as f32;
        assert!(
            sparse_coverage < MIN_PLAUSIBLE_RAW_COVERAGE,
            "test setup: the stand-in sparse result must itself be below the real threshold"
        );

        let kmeans_raw = kmeans_stroke_mask(&crop).expect("two real clusters exist");
        let kmeans_coverage =
            kmeans_raw.iter().filter(|&&m| m).count() as f32 / kmeans_raw.len() as f32;
        assert!(
            kmeans_coverage > sparse_coverage,
            "k-means must cover more of this real two-cluster crop than the deliberately sparse \
             stand-in result — otherwise the fallback would never actually help in practice"
        );
    }

    #[test]
    fn local_background_prefers_the_coarse_prior_over_kmeans_when_it_alone_clears_the_bar() {
        // End-to-end through the real `local_background_stroke_mask` entry point (not the fallback
        // functions directly, unlike the k-means-only tests above) — confirms `coarse_prior` is
        // consulted *before* k-means runs at all, per the priority order this feature was built
        // for (2026-09-10): the detection model's own independently-computed prior is free (no
        // extra inference), so it should win over falling all the way through to k-means whenever
        // it alone is enough to clear `MIN_PLAUSIBLE_RAW_COVERAGE`.
        //
        // The glyph block here is deliberately small relative to the crop (60x60=3600 total,
        // 12x12=144 glyph ≈ 4%, comfortably under `MIN_PLAUSIBLE_RAW_COVERAGE`'s `8%`) so the
        // sanity check actually trips on this crop's own baseline result — unlike a larger,
        // already-well-covered block, which would never reach the prior-consulting code path at
        // all and make this test pass for the wrong reason (verified live, 2026-09-10: an earlier
        // version of this test used a block large enough that baseline coverage already cleared
        // the bar, so the prior was silently never consulted and the test still failed — this
        // smaller block was sized specifically to make the sanity check genuinely trigger).
        let mut crop = solid(60, 60, [210, 200, 190]);
        for y in 24..36 {
            for x in 24..36 {
                crop.put_pixel(x, y, Rgb([30, 150, 160]));
            }
        }
        let without_prior =
            local_background_stroke_mask(&crop, None).expect("real local contrast exists");
        let without_prior_coverage = without_prior.raw.iter().filter(|&&v| v).count() as f32
            / without_prior.raw.len() as f32;
        assert!(
            without_prior_coverage < MIN_PLAUSIBLE_RAW_COVERAGE,
            "test setup: this crop's own baseline coverage ({without_prior_coverage}) must itself \
             be below the threshold, or the sanity check never triggers and this test can't tell \
             the prior path was actually exercised"
        );

        // A prior covering the entire real glyph block plus a little more — plausible output shape
        // for a real detection-model probability map thresholded down to a boolean mask.
        let mut prior = vec![false; (60 * 60) as usize];
        for y in 23..37 {
            for x in 23..37 {
                prior[(y * 60 + x) as usize] = true;
            }
        }
        let with_prior =
            local_background_stroke_mask(&crop, Some(&prior)).expect("real local contrast exists");
        let with_prior_coverage =
            with_prior.raw.iter().filter(|&&v| v).count() as f32 / with_prior.raw.len() as f32;

        assert!(
            with_prior_coverage > without_prior_coverage,
            "supplying a real coarse prior on a crop whose own baseline coverage trips the sanity \
             check must raise the final raw mask's own coverage"
        );
        // The specific behaviour this test exists for: every pixel the prior marked `true` must
        // survive into the final `raw` result (the sanity-check block only ever ORs the prior in,
        // never masks anything back out).
        for (i, &p) in prior.iter().enumerate() {
            if p {
                assert!(
                    with_prior.raw[i],
                    "pixel {i} was marked by the coarse prior but missing from the final raw mask"
                );
            }
        }
    }

    #[test]
    fn local_background_drops_an_oversized_texture_component() {
        // A large connected blob covering most of the crop must be dropped even if it registers
        // above the distance threshold — real glyph strokes are compact, not crop-spanning.
        let mut mask = vec![false; 100]; // 10x10
        for y in 0..8u32 {
            for x in 0..8u32 {
                mask[(y * 10 + x) as usize] = true;
            }
        }
        drop_oversized_components(&mut mask, 10, 10, 20);
        assert!(
            !mask.iter().any(|&m| m),
            "the oversized (64px, over the 20px cap) component must be dropped entirely"
        );
    }

    #[test]
    fn drop_oversized_components_keeps_a_small_component() {
        let mut mask = vec![false; 100];
        mask[45] = true;
        mask[46] = true;
        drop_oversized_components(&mut mask, 10, 10, 20);
        assert!(
            mask[45] && mask[46],
            "a small component under the cap must survive"
        );
    }

    #[test]
    fn densecrf_refines_a_coarse_box_prior_to_a_dark_glyph_shape() {
        // A light background with a dark glyph block, but the coarse prior is a wider box than
        // the real glyph — same shape a text detector's own bounding box would produce (it always
        // over-covers the real glyph edges to some degree). DenseCRF should shrink toward the real
        // dark pixels' own boundary rather than keeping the whole prior box.
        //
        // The coarse prior is deliberately one single connected component here (a solid box, no
        // gaps) — `densecrf_stroke_mask` now re-crops to a tight bounding box around each
        // pre-filtered component (plus a small margin) before ever calling DenseCRF, rather than
        // running DenseCRF over the whole region once (see that function's own doc comment on why:
        // a busy backdrop could otherwise pull one runaway DenseCRF call into misclassifying the
        // entire region as one giant blob). That means only pixels within the prior box's own
        // (slightly padded) extent are ever part of this call's own crop at all — a pixel well
        // outside the prior box's own extent (like `(2,2)`, checked below) never gets a chance to
        // be classified as text in the first place, which is a stronger and more direct guarantee
        // than "the bilateral term pulled it back toward background" ever was.
        let mut crop = solid(40, 40, [230, 230, 230]);
        for y in 15..25 {
            for x in 15..25 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let mut coarse_prior = vec![false; 40 * 40];
        for y in 10..30u32 {
            for x in 10..30u32 {
                coarse_prior[(y * 40 + x) as usize] = true;
            }
        }
        let mask =
            densecrf_stroke_mask(&crop, &coarse_prior).expect("a real crop and matching prior");
        assert!(
            mask.raw[(20 * 40 + 20) as usize],
            "the real glyph centre must still classify as text"
        );
        assert!(
            !mask.raw[0],
            "a far corner, well outside both the prior box and the real glyph, must not classify \
             as text"
        );
        assert!(
            !mask.raw[(2 * 40 + 2) as usize],
            "a background pixel well outside the coarse prior box's own (padded) extent must \
             never be classified as text at all, since it was never part of this component's own \
             re-cropped DenseCRF call"
        );
    }

    #[test]
    fn densecrf_supplements_a_whole_stroke_the_prior_never_marked() {
        // Real-page defect (2026-09-18, issue #101): the detector's `raw_text_mask` can miss an
        // entire column/line inside the region's own bbox. DenseCRF cannot recover it — its
        // per-component window is the prior's own bbox plus `CRF_SEARCH_MARGIN`, so a stroke this
        // far away is structurally unreachable — while `local_background_stroke_mask` still sees
        // it (local contrast, no detector prior needed). The bounded supplement must pull it in.
        let mut crop = solid(64, 64, [235, 235, 235]);
        for y in 10..54 {
            for x in 10..18 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
            for x in 32..40 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        // Detector prior covers only the first stroke; the second is 14px beyond it.
        let mut coarse_prior = vec![false; 64 * 64];
        for y in 10..54u32 {
            for x in 10..18u32 {
                coarse_prior[(y * 64 + x) as usize] = true;
            }
        }

        let mask = densecrf_stroke_mask(&crop, &coarse_prior).expect("a real crop and prior");
        assert!(
            mask.raw[(30 * 64 + 35) as usize],
            "the second stroke, which the coarse prior never marked and the CRF window cannot \
             reach, must be recovered by the bounded local-background supplement"
        );
        assert!(
            !mask.raw[(5 * 64 + 5) as usize],
            "background far from any real stroke must stay background"
        );
    }

    #[test]
    fn densecrf_empty_crop_returns_none() {
        let crop = RgbImage::new(0, 0);
        assert!(densecrf_stroke_mask(&crop, &[]).is_none());
    }

    #[test]
    fn densecrf_mismatched_prior_length_returns_none() {
        let crop = solid(10, 10, [128, 128, 128]);
        assert!(densecrf_stroke_mask(&crop, &[true, false]).is_none());
    }

    #[test]
    fn densecrf_drops_a_near_total_blob_refinement() {
        // The one guard kept (the reference's own "component bigger than its text-line" rule,
        // https://github.com/zyddnys/manga-image-translator `complete_mask`) rejects a refinement
        // that still covers essentially the *whole* crop. Only a near-total blob trips it; a merely
        // dense prior over real text (see the next test) must NOT be dropped.
        let crop = solid(40, 40, [200, 200, 200]);
        let mut coarse_prior = vec![false; 40 * 40];
        for y in 0..40u32 {
            for x in 0..40u32 {
                coarse_prior[(y * 40 + x) as usize] = true;
            }
        }
        assert!(
            densecrf_stroke_mask(&crop, &coarse_prior).is_none(),
            "a refinement still covering essentially the whole crop must be dropped"
        );
    }

    #[test]
    fn densecrf_keeps_a_legitimate_dense_prior_covering_most_of_its_crop() {
        // Issue #103: on a real page a clean single-line label's own 4px-dilated prior covers
        // 65-76% of its tight crop; the old 50%-of-crop cap dropped those regions entirely, so
        // they got no precise mask and were skipped (no erase, no redraw), leaving the original
        // text on the page. A dense prior over real text must now be refined, not dropped.
        let mut crop = solid(200, 200, [235, 235, 235]);
        for y in 40..160u32 {
            for x in 90..110u32 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let mut coarse_prior = vec![false; 200 * 200];
        for y in 20..180u32 {
            for x in 20..180u32 {
                coarse_prior[(y * 200 + x) as usize] = true;
            }
        }
        let mask = densecrf_stroke_mask(&crop, &coarse_prior).expect(
            "a legitimate dense prior covering most of its crop must be refined, not dropped",
        );
        assert!(
            mask.raw[(100 * 200 + 100) as usize],
            "the real dark stroke must be in the refined mask"
        );
    }

    #[test]
    fn densecrf_recovers_a_glyph_end_the_coarse_prior_under_covers() {
        // Real reported bug (2026-09-18): the detection model's coarse prior stops a few px short
        // of the real glyph at its ends/edges ("笔画末端/边角"), leaving a few px of real stroke
        // unerased. The previous per-component crop was the *prior's own* bounding box (padded
        // only by `PRIOR_DILATION_RADIUS`), so those px were never presented to DenseCRF at all
        // and could not be recovered no matter what it decided. `CRF_SEARCH_MARGIN` widens the
        // search window (never the unary) so the bilateral colour term can pull a colour-similar
        // stroke end back in.
        let mut crop = solid(40, 60, [235, 235, 235]);
        // A tall dark glyph spanning y=10..50.
        for y in 10..50 {
            for x in 15..25 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        // The coarse prior only covers the glyph's *middle*, y=20..40 — 10px short at each end.
        let mut coarse_prior = vec![false; 40 * 60];
        for y in 20..40u32 {
            for x in 15..25u32 {
                coarse_prior[(y * 40 + x) as usize] = true;
            }
        }

        let mask =
            densecrf_stroke_mask(&crop, &coarse_prior).expect("a real crop and matching prior");

        // The old window (prior bbox, dilated by PRIOR_DILATION_RADIUS) reached only y=16..44 —
        // y=12 and y=48 were never classified at all. They are the glyph's own dark pixels, so the
        // widened window must now recover them.
        assert!(
            mask.raw[(12 * 40 + 20) as usize],
            "the glyph's own top end (prior under-covered by 10px) must be recovered"
        );
        assert!(
            mask.raw[(48 * 40 + 20) as usize],
            "the glyph's own bottom end (prior under-covered by 10px) must be recovered"
        );
        // ...but a light background pixel inside the widened window must NOT be pulled in: the
        // wider window must not become a blanket over-erase of whatever it now sees.
        assert!(
            !mask.raw[(10 * 40 + 8) as usize],
            "background well left of the glyph, now inside the widened window, must stay background"
        );
    }

    #[test]
    fn densecrf_widened_window_does_not_swallow_a_colour_dissimilar_backdrop() {
        // The widened search window must not become a blanket over-erase: a busy backdrop whose
        // colours differ from the glyph's own ink must stay background. This is the exact
        // distinction the earlier *isotropic dilation* attempt could not make (it grew into
        // illustration content unconditionally); here the wider window only lets the bilateral
        // colour term pull in pixels that actually look like the strokes.
        let mut crop = solid(52, 52, [235, 235, 235]);
        for y in 5..47u32 {
            for x in 5..47u32 {
                let c = if (x / 3 + y / 3) % 2 == 0 {
                    Rgb([200, 40, 40])
                } else {
                    Rgb([40, 160, 40])
                };
                crop.put_pixel(x, y, c);
            }
        }
        // A dark glyph whose real extent (y=14..38) is under-covered by the prior (y=22..30).
        for y in 14..38u32 {
            for x in 22..30u32 {
                crop.put_pixel(x, y, Rgb([20, 20, 20]));
            }
        }
        let mut coarse_prior = vec![false; 52 * 52];
        for y in 22..30u32 {
            for x in 22..30u32 {
                coarse_prior[(y * 52 + x) as usize] = true;
            }
        }
        let mask =
            densecrf_stroke_mask(&crop, &coarse_prior).expect("a real crop and matching prior");
        assert!(
            mask.raw[(16 * 52 + 25) as usize],
            "the glyph's own dark end must be recovered even on a busy backdrop"
        );
        assert!(
            !mask.raw[(12 * 52 + 12) as usize],
            "a colour-dissimilar decoration pixel inside the widened window must stay background"
        );
    }
}
