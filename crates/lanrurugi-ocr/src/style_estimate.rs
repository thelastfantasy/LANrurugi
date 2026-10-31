//! Per-region foreground/background colour and boldness estimation (FR-008a, research.md §13).
//!
//! `manga-ocr` has no colour-output head (unlike `manga-image-translator`'s multi-head recogniser,
//! which was studied for reference only), and retraining one is out of scope this phase — so these
//! attributes come from a lightweight heuristic pass over each region's cropped pixels instead.
//! The pass runs inside the same rayon batch as OCR (`batch.rs`), never as its own sequential
//! stage.
//!
//! Each of the three attributes is estimated independently and yields `None` on low confidence: a
//! block whose boldness can't be judged must still get its (better-grounded) colour estimate.
//! Boldness in particular has no working prior art to copy — research.md §13 found even
//! `manga-image-translator` never actually computes its own `bold` field — so it is explicitly a
//! first attempt, which is exactly why it can't drag the colour estimates down with it.

use crate::entities::Rgb;
use image::{GenericImageView, RgbImage};

/// What the heuristic produced for one region. Every field is independently optional.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StyleEstimate {
    pub fg_color: Option<Rgb>,
    pub bg_color: Option<Rgb>,
    pub is_bold: Option<bool>,
}

/// Below this luminance separation the two clusters aren't distinguishable enough to call one of
/// them "the text" — colour is reported as unknown rather than guessed.
const MIN_LUMINANCE_SEPARATION: f32 = 0.15;

/// Above this per-channel standard deviation (0-255 scale), the "background" cluster isn't
/// actually a flat colour — reported live, 2026-09-07: a region whose real backdrop is manga
/// artwork (skin, hair, screentone) rather than a solid speech-bubble fill still passes the
/// fg/bg luminance-separation check above (the text ink is still darker/lighter than its
/// surroundings on average), but the "background" side of that split is really a grab-bag of
/// differently-coloured artwork pixels averaging to some in-between colour that matches none of
/// them — painting a flat rect that colour creates a visible seam against the real, varied
/// artwork right outside the box. This is deliberately a *separate* gate from
/// `MIN_LUMINANCE_SEPARATION`: that one asks "is there a text/background split at all," this one
/// asks "is the background side of that split actually uniform enough to flat-fill." A real
/// bubble's interior is close to solid, so its variance stays low even though this samples on a
/// stride (see `estimate`'s own doc comment on why a few thousand pixels is enough).
///
/// Lowered from `18.0` (2026-09-09): `bg_color` being `Some` is also what
/// `lanrurugi_translate::composite::composite_page` uses to decide *not* to draw a safety-margin
/// outline stroke around the translated text (the reasoning there: a confidently-uniform backdrop
/// doesn't need one, since plain text already reads clearly against a flat colour) — a real
/// concern raised live the same day: if this gate is too permissive, a genuinely non-uniform
/// backdrop can slip through as "uniform enough," and the resulting flat-fill *and* the missing
/// outline compound into text that's hard or impossible to read against the real, still-varied
/// artwork underneath — a correctness problem, not just a cosmetic seam, since unreadable text
/// defeats the entire point of the translation feature. `12.0` is more conservative in the
/// direction that actually matters (false negatives here just mean *falling back* to the
/// already-safe outline-stroke path, never *losing* a safety margin that a genuinely busy backdrop
/// needed).
const MAX_BG_STDDEV: f32 = 12.0;

/// A region with fewer usable pixels than this can't support a stable estimate.
const MIN_SAMPLE_PIXELS: usize = 32;

/// Stroke-width-to-glyph-height above this reads as bold. Manga lettering sits well below this for
/// regular weight; the threshold is deliberately conservative so ambiguous cases fall to `None`.
const BOLD_RATIO_THRESHOLD: f32 = 0.185;
/// Ratios within this band of the threshold are too close to call.
const BOLD_RATIO_UNCERTAINTY: f32 = 0.025;

/// Two-means clustering over 1-D luminance. Full KMeans over RGB is unnecessary here: a text region
/// is essentially bimodal (glyph vs. bubble), and clustering on luminance converges reliably in a
/// handful of passes with no random seeding, then the mean RGB of each side is taken.
///
/// Returns `(fg, bg, separation, bg_stddev)` — `bg_stddev` is the background cluster's own
/// per-channel standard deviation (see `MAX_BG_STDDEV`'s doc comment for why callers need this
/// separately from `separation`).
fn two_means_luminance(pixels: &[Rgb]) -> Option<(Rgb, Rgb, f32, f32)> {
    if pixels.len() < MIN_SAMPLE_PIXELS {
        return None;
    }

    let lums: Vec<f32> = pixels.iter().map(|p| p.luminance()).collect();
    let (mut lo, mut hi) = lums
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)));

    if (hi - lo).abs() < f32::EPSILON {
        return None; // Uniform region — nothing to separate.
    }

    for _ in 0..12 {
        let mut lo_sum = 0.0;
        let mut lo_n = 0usize;
        let mut hi_sum = 0.0;
        let mut hi_n = 0usize;

        for &l in &lums {
            if (l - lo).abs() <= (l - hi).abs() {
                lo_sum += l;
                lo_n += 1;
            } else {
                hi_sum += l;
                hi_n += 1;
            }
        }

        if lo_n == 0 || hi_n == 0 {
            return None;
        }

        let (new_lo, new_hi) = (lo_sum / lo_n as f32, hi_sum / hi_n as f32);
        if (new_lo - lo).abs() < 1e-4 && (new_hi - hi).abs() < 1e-4 {
            lo = new_lo;
            hi = new_hi;
            break;
        }
        lo = new_lo;
        hi = new_hi;
    }

    // Mean RGB of each cluster's members.
    let mut lo_acc = [0u64; 3];
    let mut lo_n = 0u64;
    let mut hi_acc = [0u64; 3];
    let mut hi_n = 0u64;

    for (p, &l) in pixels.iter().zip(lums.iter()) {
        let (acc, n) = if (l - lo).abs() <= (l - hi).abs() {
            (&mut lo_acc, &mut lo_n)
        } else {
            (&mut hi_acc, &mut hi_n)
        };
        acc[0] += u64::from(p.r);
        acc[1] += u64::from(p.g);
        acc[2] += u64::from(p.b);
        *n += 1;
    }

    if lo_n == 0 || hi_n == 0 {
        return None;
    }

    let mean = |acc: [u64; 3], n: u64| {
        Rgb::new((acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8)
    };

    let dark = mean(lo_acc, lo_n);
    let light = mean(hi_acc, hi_n);
    let separation = hi - lo;

    // Which cluster is the glyph: the minority one. Text occupies far less area than its bubble.
    let bg_is_lo = lo_n > hi_n;
    let (fg, bg) = if bg_is_lo {
        (light, dark)
    } else {
        (dark, light)
    };

    // Per-channel standard deviation of the *background* cluster's own members — see
    // `MAX_BG_STDDEV`'s doc comment for why this, not just the fg/bg separation above, is what
    // actually tells a solid bubble apart from a region whose "background" is varied artwork.
    let (mut sq_r, mut sq_g, mut sq_b) = (0f64, 0f64, 0f64);
    let mut bg_count = 0u64;
    for (p, &l) in pixels.iter().zip(lums.iter()) {
        let is_bg_member = if bg_is_lo {
            (l - lo).abs() <= (l - hi).abs()
        } else {
            (l - hi).abs() <= (l - lo).abs()
        };
        if !is_bg_member {
            continue;
        }
        sq_r += (f64::from(p.r) - f64::from(bg.r)).powi(2);
        sq_g += (f64::from(p.g) - f64::from(bg.g)).powi(2);
        sq_b += (f64::from(p.b) - f64::from(bg.b)).powi(2);
        bg_count += 1;
    }
    let bg_stddev = if bg_count == 0 {
        f32::MAX
    } else {
        let variance = (sq_r + sq_g + sq_b) / (3.0 * bg_count as f64);
        variance.sqrt() as f32
    };

    Some((fg, bg, separation, bg_stddev))
}

/// Fraction of pixels belonging to the glyph cluster — the input to the boldness ratio.
fn foreground_coverage(pixels: &[Rgb], fg: Rgb, bg: Rgb) -> f32 {
    let (fg_l, bg_l) = (fg.luminance(), bg.luminance());
    let mid = (fg_l + bg_l) / 2.0;
    let fg_is_darker = fg_l < bg_l;

    let count = pixels
        .iter()
        .filter(|p| {
            let l = p.luminance();
            if fg_is_darker {
                l < mid
            } else {
                l > mid
            }
        })
        .count();

    count as f32 / pixels.len() as f32
}

/// Estimates all three attributes for one already-cropped region image.
///
/// `line_count` is how many detected lines merged into this region — needed to convert coverage
/// into a per-glyph stroke ratio, since a tall region holding six lines has proportionally more ink
/// than a one-line region of the same height at the same weight.
pub fn estimate(crop: &RgbImage, line_count: usize) -> StyleEstimate {
    let (w, h) = crop.dimensions();
    if w == 0 || h == 0 {
        return StyleEstimate::default();
    }

    // Sample on a stride for large crops: a few thousand pixels are plenty to characterise a
    // bimodal region, and this keeps the pass cheap enough to sit inside the OCR batch.
    let total = (w as usize) * (h as usize);
    let stride = ((total / 4096).max(1) as u32).max(1);

    let mut pixels = Vec::with_capacity((total / stride as usize) + 1);
    for y in (0..h).step_by(stride as usize) {
        for x in (0..w).step_by(stride as usize) {
            let p = crop.get_pixel(x, y).0;
            pixels.push(Rgb::new(p[0], p[1], p[2]));
        }
    }

    let Some((fg, bg, separation, bg_stddev)) = two_means_luminance(&pixels) else {
        return StyleEstimate::default();
    };

    // Low separation means the "two clusters" are really one — don't claim a colour.
    let colors_confident = separation >= MIN_LUMINANCE_SEPARATION;
    // Separately, `bg_color` additionally requires the background cluster to actually be a flat
    // colour — see `MAX_BG_STDDEV`'s doc comment. `fg_color`/`is_bold` don't need this: the glyph
    // side of the split stays meaningful even when the backdrop behind it is busy artwork.
    let bg_uniform = bg_stddev <= MAX_BG_STDDEV;

    let is_bold = if colors_confident {
        estimate_boldness(&pixels, fg, bg, h, line_count)
    } else {
        // Without a trustworthy fg/bg split there's no meaningful coverage figure either.
        None
    };

    StyleEstimate {
        fg_color: colors_confident.then_some(fg),
        bg_color: (colors_confident && bg_uniform).then_some(bg),
        is_bold,
    }
}

/// Stroke-width-to-glyph-height heuristic (research.md §13).
///
/// Coverage (ink fraction) rises both with stroke weight and with how many glyph rows are packed
/// into the region, so it is normalised by the estimated per-line height before comparison.
fn estimate_boldness(
    pixels: &[Rgb],
    fg: Rgb,
    bg: Rgb,
    region_height: u32,
    line_count: usize,
) -> Option<bool> {
    let coverage = foreground_coverage(pixels, fg, bg);

    // Implausible coverage means the cluster split didn't actually isolate glyphs.
    if !(0.02..=0.75).contains(&coverage) {
        return None;
    }

    let lines = line_count.max(1) as f32;
    let line_height = region_height as f32 / lines;
    if line_height < 6.0 {
        return None; // Too small to measure a stroke against.
    }

    // Coverage per line of text approximates ink-per-glyph-box; the constant folds in the typical
    // ratio of glyph bounding box to line box for CJK lettering.
    let ratio = coverage / lines.sqrt();

    if (ratio - BOLD_RATIO_THRESHOLD).abs() < BOLD_RATIO_UNCERTAINTY {
        None // Too close to the boundary to claim either way.
    } else {
        Some(ratio > BOLD_RATIO_THRESHOLD)
    }
}

/// Crops `page` to `bbox`, clamped to the page bounds. Returns `None` if the box lies outside the
/// image or has zero area after clamping.
pub fn crop_region(page: &RgbImage, bbox: &crate::entities::BoundingBox) -> Option<RgbImage> {
    let (pw, ph) = page.dimensions();
    if bbox.x >= pw || bbox.y >= ph {
        return None;
    }
    let w = bbox.w.min(pw - bbox.x);
    let h = bbox.h.min(ph - bbox.y);
    if w == 0 || h == 0 {
        return None;
    }
    Some(page.view(bbox.x, bbox.y, w, h).to_image())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb as ImageRgb;

    /// A white region with black "glyph" rows covering `ink_rows` of every 10 rows.
    fn synthetic(w: u32, h: u32, ink_rows: u32) -> RgbImage {
        let mut img = RgbImage::from_pixel(w, h, ImageRgb([255, 255, 255]));
        for y in 0..h {
            if y % 10 < ink_rows {
                for x in 0..w {
                    img.put_pixel(x, y, ImageRgb([0, 0, 0]));
                }
            }
        }
        img
    }

    #[test]
    fn uniform_region_yields_no_estimates() {
        let img = RgbImage::from_pixel(64, 64, ImageRgb([128, 128, 128]));
        let est = estimate(&img, 1);
        assert_eq!(est.fg_color, None);
        assert_eq!(est.bg_color, None);
        assert_eq!(est.is_bold, None);
    }

    #[test]
    fn black_text_on_white_is_detected() {
        let img = synthetic(80, 80, 2);
        let est = estimate(&img, 1);
        let fg = est.fg_color.expect("foreground should be estimated");
        let bg = est.bg_color.expect("background should be estimated");
        assert!(
            fg.luminance() < bg.luminance(),
            "text must be the darker cluster"
        );
        assert!(bg.luminance() > 0.8, "bubble should read as white");
    }

    #[test]
    fn white_text_on_black_inverts_correctly() {
        // Minority cluster is the glyph regardless of which side is darker.
        let mut img = RgbImage::from_pixel(80, 80, ImageRgb([0, 0, 0]));
        for y in 0..80 {
            if y % 10 < 2 {
                for x in 0..80 {
                    img.put_pixel(x, y, ImageRgb([255, 255, 255]));
                }
            }
        }
        let est = estimate(&img, 1);
        let fg = est.fg_color.expect("foreground should be estimated");
        let bg = est.bg_color.expect("background should be estimated");
        assert!(fg.luminance() > bg.luminance(), "white text on black");
    }

    #[test]
    fn tiny_region_is_not_estimated() {
        let img = synthetic(3, 3, 1);
        assert_eq!(estimate(&img, 1).fg_color, None);
    }

    #[test]
    fn heavier_ink_reads_as_bolder_than_lighter_ink() {
        // Not asserting an absolute bold/not-bold verdict (the threshold is a first attempt with
        // no external validation) — only that the heuristic is monotonic in stroke weight, which
        // is the property the threshold is calibrated against.
        let light = synthetic(120, 120, 1);
        let heavy = synthetic(120, 120, 6);

        let light_px = sample(&light);
        let heavy_px = sample(&heavy);
        let (lfg, lbg, _, _) = two_means_luminance(&light_px).unwrap();
        let (hfg, hbg, _, _) = two_means_luminance(&heavy_px).unwrap();

        assert!(
            foreground_coverage(&heavy_px, hfg, hbg) > foreground_coverage(&light_px, lfg, lbg),
            "heavier strokes must yield higher ink coverage"
        );
    }

    fn sample(img: &RgbImage) -> Vec<Rgb> {
        img.pixels()
            .map(|p| Rgb::new(p.0[0], p.0[1], p.0[2]))
            .collect()
    }

    #[test]
    fn crop_outside_page_returns_none() {
        let page = RgbImage::from_pixel(50, 50, ImageRgb([255, 255, 255]));
        let bbox = crate::entities::BoundingBox::new(100, 100, 10, 10);
        assert!(crop_region(&page, &bbox).is_none());
    }

    #[test]
    fn crop_is_clamped_to_page_bounds() {
        let page = RgbImage::from_pixel(50, 50, ImageRgb([255, 255, 255]));
        let bbox = crate::entities::BoundingBox::new(40, 40, 100, 100);
        let crop = crop_region(&page, &bbox).expect("overlapping crop should succeed");
        assert_eq!(crop.dimensions(), (10, 10));
    }
}
