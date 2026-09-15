//! Server-side compositing: drawing translated text over the original page image
//! (T029/T034, research.md §6/§16).
//!
//! This is a **re-derivable** step over already-persisted data, not the point where translation
//! happens: it reads `translated_text` and the resolved `font`/colour/boldness off the
//! `DetectedTextRegion` records and renders them. Losing the rendered image costs one re-run of
//! this function; nothing needs OCR or a billed LLM call again (research.md §16).
//!
//! Used for the **cloud-backend path only**. The locally-hosted path composites in the browser
//! instead, because the translated text only ever exists there (constitution Principle V) and
//! round-tripping it to the server purely to draw it would be pointless.
//!
//! Scope note: FR-005 only required translated text *positioned over* the original region, not the
//! original lettering removed — that was true when this module had no inpainting at all (a flat
//! background box behind the text was the only legibility mechanism). `lanrurugi-inpaint` now
//! erases the original lettering for real when its model is installed (optional, degrades
//! gracefully — see that crate's own docs), with `bubble_segment`'s precise mask used instead of a
//! plain rectangle when a matching detected bubble is available.
//!
//! Vertical CJK layout (research.md §11 update): a region whose original (source) text is
//! Han/Hiragana/Katakana and whose box is tall-and-narrow renders top-to-bottom, right-to-left —
//! matching how those source bubbles were actually lettered. Everything else stays horizontal.
//! Shaping/line-breaking go through `harfrust`/`icu_segmenter`; `ab_glyph` still does the actual
//! rasterization (outline → coverage → alpha blend), unchanged from before.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgb as ImageRgb, RgbImage};
use lanrurugi_ocr::bubble_segment::DetectedBubble;
use lanrurugi_ocr::entities::{BoundingBox, DetectedTextRegion, Rgb};

/// Fallback text colour when the heuristic couldn't estimate one (FR-008a) — near-black, matching
/// the overwhelmingly common case for manga dialogue.
const DEFAULT_FG: Rgb = Rgb {
    r: 20,
    g: 20,
    b: 20,
};

/// Bounds on rendered text size, in pixels. `MIN_FONT_PX` was `9.0` until a real reported incident
/// (2026-09-09): a Japanese-to-Chinese translation into a vertical bubble rendered visibly smaller
/// than the original Japanese lettering, traced to `fit_text_vertical`/`fit_text` having no notion
/// of the *source* text's own rendered size at all — the fitted size is purely "largest size that
/// makes the *translation* fit the box," and a longer translation than its source (very common:
/// Chinese and Japanese aren't the same length per idea, and this codebase always translates *out
/// of* Japanese, never into a language shorter than it) drives that fitted size down with no floor
/// tied to what actually looks legible. Recording and reusing the source's own real rendered size
/// isn't implemented (`DetectedTextRegion` carries no such field, and estimating one from the OCR
/// crop would be a real design effort of its own) — raising the floor is the lightweight mitigation
/// available now: `12.0` keeps a badly-shrunk case at least readable, and `layout_box`'s own
/// bubble-interior preference plus [`LAYOUT_OVERFLOW_FRACTION`] reduce how often the shrink search
/// even needs to go that low in the first place.
const MIN_FONT_PX: f32 = 12.0;
const MAX_FONT_PX: f32 = 72.0;

/// How far a region's own text layout box may extend past its OCR-detected bounding box on each
/// side, as a fraction of that box's own width/height — see [`layout_box`]'s own doc comment for
/// why this exists (a tight OCR crop under-covers a bubble's own real usable interior even more
/// than [`MIN_FONT_PX`]'s own floor alone can compensate for, especially when no matched bubble is
/// available to lay out against instead). Deliberately modest: a few points of the region's own
/// size, not free rein to spill into surrounding artwork or a neighbouring bubble.
const LAYOUT_OVERFLOW_FRACTION: f32 = 0.15;

/// Fraction of the region's width kept clear on each side.
const HORIZONTAL_PADDING: f32 = 0.04;

/// Multiplier applied to `ScaleFont::height()` (ascent + descent + the font's own internal
/// line-gap) to get the actual line/column pitch used for both `fit_text`'s shrink-to-fit sizing
/// and the real draw pass. `height()` alone reads noticeably looser than typical manga lettering
/// once there's more than one line/column — CJK fonts commonly ship a generous built-in line-gap
/// meant for running prose, not the tight balloon text this renders (reported live, 2026-09-06,
/// compared against real translation-software output). Tightening only affects spacing *between*
/// lines/columns, not glyph size itself.
const LINE_SPACING_FACTOR: f32 = 0.85;

/// A box at least this much taller than it is wide, whose *source* text is CJK, is treated as a
/// vertically-lettered bubble. Manga's horizontal bubbles are typically wider than tall or close
/// to square; the vertical ones this feature now handles are reliably narrow-and-tall — there is
/// no bubble-shape signal available yet (research.md's future inpainting work would add one), so
/// the box aspect ratio is the only signal available this phase.
const VERTICAL_ASPECT_THRESHOLD: f32 = 1.2;

#[derive(Debug, thiserror::Error)]
pub enum CompositeError {
    #[error("no usable font available for compositing")]
    NoFont,
    #[error("failed to encode the composited page: {0}")]
    Encode(String),
}

/// A font the compositor can draw with, resolved from a golden-set font id.
///
/// Holds the raw font bytes rather than a parsed `ab_glyph::FontRef` — rasterization
/// (`ab_glyph::FontRef`) and shaping (`harfrust::FontRef`, a distinct type from an unrelated crate)
/// each need their own zero-copy parse of the same underlying bytes, so the bytes are what this
/// type actually owns/borrows.
pub struct FontSet<'a> {
    /// Used when a region names no font, or names one not present here.
    pub fallback: &'a [u8],
    /// Golden-set id → font file bytes.
    pub by_id: Vec<(String, &'a [u8])>,
}

impl<'a> FontSet<'a> {
    pub fn new(fallback: &'a [u8]) -> Self {
        Self {
            fallback,
            by_id: Vec::new(),
        }
    }

    pub fn with_font(mut self, id: impl Into<String>, font: &'a [u8]) -> Self {
        self.by_id.push((id.into(), font));
        self
    }

    /// Resolves a region's font bytes, falling back rather than failing — a missing font must
    /// never block rendering the translation itself.
    fn resolve(&self, font_id: Option<&str>) -> &'a [u8] {
        font_id
            .and_then(|id| self.by_id.iter().find(|(known, _)| known == id))
            .map(|(_, bytes)| *bytes)
            .unwrap_or(self.fallback)
    }
}

/// The two font views `draw_region` needs for one glyph run: `ab_glyph` for rasterizing outlines,
/// `harfrust` for shaping (advances/offsets, including vertical CJK layout). Bundled together
/// because every call site that resolves one needs the other.
struct ResolvedFont<'a> {
    raster: FontRef<'a>,
    shaper_data: harfrust::ShaperData,
    hb_font: harfrust::FontRef<'a>,
}

impl<'a> ResolvedFont<'a> {
    fn load(bytes: &'a [u8]) -> Option<Self> {
        let raster = FontRef::try_from_slice(bytes).ok()?;
        // `from_index(_, 0)` rather than `FontRef::new` — this project's bundled CJK faces are
        // `.ttc` collections (e.g. Noto Sans/Serif CJK), and `harfrust::FontRef::new` only accepts
        // a single font, not a collection container; `from_index` handles both, taking the first
        // face in a collection (matching `ab_glyph`'s own default). Confirmed live: using `new`
        // here made every region's `ResolvedFont::load` silently fail on this container's actual
        // fonts, so nothing was ever drawn despite the page reporting a successful composite.
        let hb_font = harfrust::FontRef::from_index(bytes, 0).ok()?;
        let shaper_data = harfrust::ShaperData::new(&hb_font);
        Some(Self {
            raster,
            shaper_data,
            hb_font,
        })
    }

    fn shaper(&self) -> harfrust::Shaper<'_> {
        self.shaper_data
            .shaper(&self.hb_font)
            .instance(None)
            .build()
    }
}

/// Draws every translated region onto `page`.
///
/// Regions without a translation are skipped silently: a page may be partially translated (one
/// block's failure must not affect the others, FR-020).
///
/// `inpainter` is `None` when `lanrurugi-inpaint`'s model isn't installed (the common case until a
/// deployment runs `scripts/fetch-inpaint-model.sh` — see that crate's own README) — every call
/// site degrades gracefully to the pre-existing flat-fill/outline-stroke compositing rather than
/// failing, matching this feature's established "missing model = feature unavailable, not an
/// error" convention (FR-019).
///
/// `bubbles`, similarly optional (`scripts/fetch-bubble-seg-model.sh`'s model), are matched to
/// each `region` by bounding-box overlap (`best_matching_bubble_mask`) so `inpainter` erases the
/// bubble's own real outline instead of the OCR text region's plain rectangle where a confident
/// match exists — falling back to the rectangle otherwise (no match, or `bubbles` itself absent).
/// Everything [`prepare_page_erase`] computes that [`finish_composite_page`] needs afterward —
/// carries the mask-building pass's own results across the caller's async
/// [`lanrurugi_inpaint::Inpainter::erase_page`]-equivalent call (GPU EP integration plan v9, issue
/// #103: that call now goes through a subprocess RPC client the caller owns, not a type this crate
/// depends on — see this module's own top-level doc comment update).
pub struct PageErasePlan<'a> {
    pristine: RgbImage,
    page_model_mask: Vec<bool>,
    page_paste_mask: Vec<bool>,
    translatable: Vec<&'a DetectedTextRegion>,
    sampled_colours: Vec<Option<Rgb>>,
    has_precise_mask: Vec<bool>,
    matched_bubble_bbox: Vec<Option<BoundingBox>>,
}

impl PageErasePlan<'_> {
    /// The whole-page image and the two page-sized masks a real
    /// [`lanrurugi_inpaint::Inpainter::erase_page`] call (or its RPC-forwarded equivalent) needs —
    /// `(page, model_mask, paste_mask)`, matching that method's own argument order.
    pub fn erase_request(&self) -> (&RgbImage, &[bool], &[bool]) {
        (&self.pristine, &self.page_model_mask, &self.page_paste_mask)
    }

    /// Whether any region on this page actually needs the flat-fill-only fallback path — `true`
    /// when there's simply nothing to erase (no translatable region had a precise mask), in which
    /// case the caller can skip the erase RPC call entirely rather than paying a round-trip for a
    /// no-op.
    pub fn nothing_to_erase(&self) -> bool {
        self.has_precise_mask.iter().all(|&precise| !precise)
    }
}

/// First half of page compositing (GPU EP integration plan v9 split — see [`PageErasePlan`]'s own
/// doc comment): builds the page-wide erase mask from every translatable region's own precise
/// (bubble or stroke) mask, entirely synchronous/CPU-bound, no model call. The caller is
/// responsible for taking [`PageErasePlan::erase_request`], running a real erase against it
/// (`lanrurugi_inpaint::Inpainter::erase_page` directly, or forwarded over RPC to
/// `lanrurugi-gpu-worker` — this crate has no opinion on which), and passing the result (or `None`
/// on failure/no inpainter available) to [`finish_composite_page`].
pub fn prepare_page_erase<'a>(
    page: &RgbImage,
    regions: &'a [DetectedTextRegion],
    bubbles: Option<&[DetectedBubble]>,
) -> PageErasePlan<'a> {
    // Whole-page single-pass erase, not one `Inpainter` call per region — matches how real
    // production manga translators actually do this (`zyddnys/manga-image-translator`'s own
    // `dispatch_inpainting(inpainter, ctx.img_rgb, ctx.mask, ...)`, confirmed by reading its
    // actual source, 2026-09-08: one page-sized mask, one model call). An earlier version of this
    // function ran the model once per region instead, which hit two separate real bugs in
    // practice that a whole-page pass has no equivalent failure mode for at all:
    // - Cross-region context pollution: a region's own padded context crop could read pixels an
    //   earlier region in the same pass had already erased/redrawn instead of real original
    //   artwork (there is no "earlier region" to pollute from when the whole page erases in one
    //   shot — see `lanrurugi_inpaint::Inpainter::erase_page`'s own doc comment).
    // - Irregular-small-hole degradation: a single region's own stroke-shaped hole, relative to
    //   only that region's own small padded crop, is far more irregular (proportionally) than the
    //   same hole shape is relative to the entire page — reported live, 2026-09-08: an isolated
    //   caption's own per-region LaMa output degraded to a flat pale colour instead of real
    //   background detail, traced all the way down to the model being asked to fill a *plain
    //   rectangular* hole per region regardless of the mask's own precise shape (the mask only
    //   ever restricted paste-back, never what the model itself actually saw). A whole-page mask
    //   is fed to the model directly with its own real shape intact, and the model has the
    //   entire page's worth of genuine surrounding context to reconstruct from.
    let translatable: Vec<&DetectedTextRegion> = regions
        .iter()
        .filter(|r| {
            r.translated_text
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty())
        })
        .collect();

    let pristine = page.clone();
    let (pw, ph) = pristine.dimensions();
    let mut page_model_mask = vec![false; (pw * ph) as usize];
    let mut page_paste_mask = vec![false; (pw * ph) as usize];
    let mut sampled_colours: Vec<Option<Rgb>> = Vec::with_capacity(translatable.len());
    // Tracks, per region, whether a precise (bubble or stroke) mask was actually available — see
    // `merge_region_mask_into_page`'s own doc comment for why a region's plain bounding box alone
    // is never trustworthy enough to erase or flat-fill blind. Shared between the erase-mask pass
    // below and `flat_fill_fallback`, which needs the same "no precise mask, don't touch it" rule.
    let mut has_precise_mask: Vec<bool> = Vec::with_capacity(translatable.len());
    // The region's own matched bubble bbox (if any), threaded through to the draw pass below so
    // text can be laid out against the real bubble's own (usually larger) interior instead of the
    // tighter OCR text-detection box — see `layout_box`'s own doc comment for why. Computed here,
    // in the same IoU match `combined_erase_mask` itself needs, rather than re-matching a second
    // time in the draw loop.
    let mut matched_bubble_bbox: Vec<Option<BoundingBox>> = Vec::with_capacity(translatable.len());
    for &region in &translatable {
        let matched_bubble =
            bubbles.and_then(|bubbles| best_matching_bubble(&region.bounding_box, bubbles));
        matched_bubble_bbox.push(matched_bubble.map(|b| b.bbox));
        let bubble_mask = matched_bubble.map(|b| crop_bubble_mask(&region.bounding_box, b));
        let (region_erase, sampled_fg) = combined_erase_mask(&pristine, region, bubble_mask);
        sampled_colours.push(sampled_fg);
        has_precise_mask.push(region_erase.is_some());
        merge_region_mask_into_page(
            &mut page_model_mask,
            pw,
            ph,
            &region.bounding_box,
            region_erase.as_ref().map(|e| e.model.as_slice()),
        );
        merge_region_mask_into_page(
            &mut page_paste_mask,
            pw,
            ph,
            &region.bounding_box,
            region_erase.as_ref().map(|e| e.paste.as_slice()),
        );
    }

    PageErasePlan {
        pristine,
        page_model_mask,
        page_paste_mask,
        translatable,
        sampled_colours,
        has_precise_mask,
        matched_bubble_bbox,
    }
}

/// Second half of page compositing (see [`PageErasePlan`]'s own doc comment) — takes the erase
/// result the caller obtained from `plan.erase_request()` (`Some(erased_page)` on a successful
/// erase, `None` if there's no inpainter available or the erase call itself failed — both fall
/// back to flat-fill backdrops, matching this function's pre-split behavior exactly) and finishes
/// compositing: draws every translatable region's own translated text over `page`.
pub fn finish_composite_page(
    page: &mut RgbImage,
    plan: PageErasePlan<'_>,
    fonts: &FontSet<'_>,
    erased: Option<RgbImage>,
) -> Result<(), CompositeError> {
    let PageErasePlan {
        pristine: _,
        page_model_mask: _,
        page_paste_mask: _,
        translatable,
        sampled_colours,
        has_precise_mask,
        matched_bubble_bbox,
    } = plan;
    let (pw, ph) = page.dimensions();

    let flat_fill_fallback = |page: &mut RgbImage| {
        for (region, &precise) in translatable.iter().zip(has_precise_mask.iter()) {
            if !precise {
                continue;
            }
            if let Some(bg) = region.bg_color {
                let bbox = &region.bounding_box;
                let w = bbox.w.min(pw.saturating_sub(bbox.x));
                let h = bbox.h.min(ph.saturating_sub(bbox.y));
                if w > 0 && h > 0 {
                    fill_rect(page, bbox.x, bbox.y, w, h, bg);
                }
            }
        }
    };
    match erased {
        Some(erased_page) => *page = erased_page,
        // No inpainter available, or the erase call itself failed (the caller is responsible for
        // logging why — this function only sees the binary "did we get an erased page back or
        // not", matching FR-019's "missing model = feature unavailable, not an error" convention).
        None => flat_fill_fallback(page),
    }
    let mut to_draw = Vec::with_capacity(translatable.len());
    for ((&region, sampled_fg), matched_bubble) in translatable
        .iter()
        .zip(sampled_colours.iter())
        .zip(matched_bubble_bbox.iter())
    {
        let text = region.translated_text.as_deref().expect("filtered above");

        // Real ink colour sampled directly off the glyph pixels (`combined_erase_mask`'s own doc
        // comment) beats `style_estimate`'s statistical cluster split when both are available —
        // falls back to that estimate, then to a near-black default, exactly as before when no
        // stroke mask could be built at all (no confident fg/bg split, or an empty stroke result).
        let fg = sampled_fg.or(region.fg_color).unwrap_or(DEFAULT_FG);
        let bold = region.is_bold.unwrap_or(false);
        let Some(font) = ResolvedFont::load(fonts.resolve(region.font.as_deref())) else {
            continue;
        };

        // Checked against `text` (the actual translated string about to be drawn), not
        // `region.source_text` (the original Japanese) — a real bug (2026-09-09): the earlier
        // version checked the source text's own script, so a Japanese-to-English translation in a
        // tall/narrow bubble still rendered the *English* translation vertically, because the
        // *original* Japanese happened to be CJK. Whether to lay text out vertically is a property
        // of what's actually being drawn, not of what it was translated from — English (or any
        // Latin-script target) must always render horizontally regardless of source script, and a
        // CJK target (e.g. translating into Chinese) should still be eligible for vertical layout
        // even from a non-CJK source in principle, though in practice this codebase only ever
        // translates *from* Japanese manga, so that direction doesn't currently arise.
        let vertical = is_vertical_bubble(&region.bounding_box, text);
        let layout = layout_box(&region.bounding_box, matched_bubble.as_ref());
        to_draw.push((
            region,
            text,
            font,
            fg,
            bold,
            vertical,
            layout,
            matched_bubble.is_some(),
        ));
    }

    for (region, text, font, fg, bold, vertical, layout, has_matched_bubble) in &to_draw {
        // Skipping the outline-stroke safety margin requires *both* a confidently-uniform backdrop
        // estimate *and* a real matched bubble — not `bg_color: Some` alone. Tightened from
        // `bg_color`-alone (2026-09-09): `style_estimate::estimate`'s own uniformity gate
        // (`MAX_BG_STDDEV`) is a statistical threshold on a strided pixel sample, not a guarantee —
        // a genuinely non-uniform backdrop (a gradient, or a low-contrast complex texture) can
        // still slip under that threshold, and text with no outline at all over a backdrop that
        // turns out not to be flat can become effectively unreadable rather than merely
        // less-polished — a correctness failure, not a cosmetic one, since illegible text defeats
        // the point of translating it at all. A real matched bubble is a second, independent signal
        // that the backdrop really is a flat interior (the bubble segmentation model's own job,
        // not a colour-statistics heuristic) — requiring both before skipping the safety margin
        // means a `bg_color` false-positive with no bubble backing it up still gets the outline,
        // and only the doubly-confident case (uniform colour *and* a real bubble shape) draws plain
        // text. See `draw_region`'s own historical doc comment for why the outline exists at all
        // (still true: LaMa's reconstruction quality isn't guaranteed perfectly flat even after a
        // successful erase, on top of this).
        let outline = match (region.bg_color, has_matched_bubble) {
            (Some(_), true) => None,
            _ => Some(if fg.luminance() < 0.5 {
                Rgb {
                    r: 255,
                    g: 255,
                    b: 255,
                }
            } else {
                Rgb { r: 0, g: 0, b: 0 }
            }),
        };
        let (pw, ph) = page.dimensions();
        if layout.x >= pw || layout.y >= ph || layout.w == 0 || layout.h == 0 {
            continue;
        }
        let w = layout.w.min(pw - layout.x);
        let h = layout.h.min(ph - layout.y);
        if *vertical {
            draw_region_vertical(page, layout, w, h, text, font, *fg, *bold, outline);
        } else {
            draw_region_horizontal(page, layout, w, h, text, font, *fg, *bold, outline);
        }
    }
    Ok(())
}

/// Ors one region's own mask (or, if it has none, its whole rectangle — the same "no mask means
/// erase the entire box" convention `combined_erase_mask` itself uses) into `page_mask`, a
/// `page_w * page_h`-long row-major buffer shared across every region on the page — this is what
/// [`composite_page`] builds up before its single [`lanrurugi_inpaint::Inpainter::erase_page`]
/// call, one region at a time. Silently clips/no-ops on an out-of-bounds or zero-sized region, so
/// a single malformed region still can't fail the whole page's compositing.
fn merge_region_mask_into_page(
    page_mask: &mut [bool],
    page_w: u32,
    page_h: u32,
    bbox: &BoundingBox,
    region_mask: Option<&[bool]>,
) {
    if bbox.x >= page_w || bbox.y >= page_h || bbox.w == 0 || bbox.h == 0 {
        return;
    }
    let w = bbox.w.min(page_w - bbox.x);
    let h = bbox.h.min(page_h - bbox.y);
    // No precise mask (no confidently-matched bubble outline, no confidently-estimated glyph
    // stroke colour) means this region's own bounding box is the *only* signal available for
    // "where the real text actually is" — and that signal alone isn't trustworthy enough to erase
    // blind. A real reported bug (2026-09-08): an OCR text-region merge defect (unrelated,
    // tracked separately in `lanrurugi-ocr`) produced a wildly oversized bbox for one small "タ"
    // SFX glyph that ended up also covering an entirely unrelated background character's head —
    // erasing that whole rectangle destroyed real artwork that was never text at all. Skipping
    // the region entirely here (leaving the original pixels, including the original untranslated
    // lettering, untouched) is strictly safer than either erasing or flat-filling the box: the
    // translated text still gets drawn on top of it afterward (composite_page's own draw pass),
    // just without a clean backdrop underneath — a visibly busier result, not a destroyed one.
    let Some(region_mask) = region_mask.filter(|m| m.len() == (w * h) as usize) else {
        return;
    };

    for ry in 0..h {
        for rx in 0..w {
            if region_mask[(ry * w + rx) as usize] {
                let i = ((bbox.y + ry) * page_w + (bbox.x + rx)) as usize;
                page_mask[i] = true;
            }
        }
    }
}

/// Minimum IoU (intersection-over-union) between an OCR text region's own box and a detected
/// bubble's box for [`best_matching_bubble`] to treat them as the same bubble — see that
/// function's own doc comment.
const MIN_BUBBLE_MATCH_IOU: f32 = 0.2;

/// Combines a bubble-shape mask (if any) with a glyph-colour-based stroke mask (if the region's
/// `fg_color`/`bg_color` were confidently estimated) into the [`RegionErase`] pair this region
/// contributes to the page-wide erase — see `lanrurugi_inpaint::stroke_mask`'s own module doc for
/// why this exists at all (matching Google Lens/Translate and `manga-image-translator`'s own
/// approach of only ever erasing glyph-coloured pixels, not the whole rectangular text box, so
/// real background/artwork right next to the lettering is never regenerated at all).
///
/// Four cases, from most to least precise (applied independently to both the `model` and `paste`
/// masks — `lanrurugi_inpaint::stroke_mask::StrokeMask`'s own doc comment covers why there are two
/// at all):
/// - Both available: intersection — a pixel must be inside the real bubble outline *and* actually
///   glyph-coloured to be erased. Tighter than either alone; a bubble's own fill colour right next
///   to the glyph, or a stroke-coloured pixel that happens to fall outside the bubble's real
///   outline (its own OCR box padding, typically), is left untouched either way.
/// - Only the stroke mask: the common case for isolated SFX/caption lettering directly on artwork
///   with no bubble of its own — still narrows erasure to just the glyph pixels rather than the
///   whole OCR box.
/// - Only the bubble mask: `fg_color`/`bg_color` weren't confident enough (`style_estimate`'s own
///   gates) to build a stroke mask, but a real bubble shape is still known — falls back to the
///   pre-stroke-mask behaviour for that region. Both `model` and `paste` end up as the same bubble
///   mask in this case (no stroke-mask dilation to distinguish them by).
/// - Neither: `None`, meaning "erase the whole rectangle" — `merge_region_mask_into_page`'s own
///   behaviour when `region_mask` is absent.
///
/// Also returns a real ink colour sampled directly off the glyph pixels the stroke mask found
/// (`lanrurugi_inpaint::stroke_mask::sample_stroke_colour`), when a stroke mask was built at all —
/// this is deliberately preferred over `region.fg_color` (`style_estimate`'s own statistical
/// cluster split) at the actual text-drawing call site, since a busy/patterned backdrop can fool
/// that clustering into misreading which cluster is glyph vs. background — investigated live,
/// 2026-09-08, on exactly this kind of region (isolated caption lettering directly over a
/// door/wall pattern, no real bubble backdrop): the two colours turned out to agree closely in
/// the end, but the stroke-mask sample is still the more principled source going forward.
fn combined_erase_mask(
    pristine: &RgbImage,
    region: &DetectedTextRegion,
    bubble_mask: Option<Vec<bool>>,
) -> (Option<RegionErase>, Option<Rgb>) {
    let stroke = region.fg_color.zip(region.bg_color).and_then(|(fg, bg)| {
        let crop = crop_region(pristine, &region.bounding_box)?;
        let mask = lanrurugi_inpaint::stroke_mask::stroke_mask(
            &crop,
            ImageRgb([fg.r, fg.g, fg.b]),
            ImageRgb([bg.r, bg.g, bg.b]),
        )?;
        let sampled =
            lanrurugi_inpaint::stroke_mask::sample_stroke_colour(&crop, &mask.raw).map(|c| Rgb {
                r: c.0[0],
                g: c.0[1],
                b: c.0[2],
            });
        Some((mask, sampled))
    });
    // No confident fg/bg colour pair to seed `stroke_mask` with at all (`style_estimate` itself
    // couldn't split the region's own crop into two clean clusters — a busy/textured backdrop
    // with no real bubble is the common case). `local_background_stroke_mask` finds a precise,
    // non-rectangular glyph shape with no colour priors, still not falling straight through to
    // "no mask" (which `merge_region_mask_into_page` correctly treats as "skip this region
    // entirely," but skipping is only correct when detection/translation themselves failed — a
    // region OCR read and the LLM translated successfully still deserves *some* attempt at
    // precise erasure before giving up, not an automatic pass just because the colour-pair fast
    // path didn't apply). `otsu_stroke_mask` (single global luminance threshold) was tried first
    // here and confirmed live, 2026-09-08, to fail on a real multi-tone glyph (a coloured fill
    // plus a contrasting outline stroke) over a textured backdrop — its whole-crop luminance
    // histogram has no clean bimodal split in that case. `local_background_stroke_mask`'s own
    // doc comment covers why classifying by distance from a *locally* estimated background
    // sidesteps that failure mode entirely.
    let stroke = stroke.or_else(|| {
        let crop = crop_region(pristine, &region.bounding_box)?;
        // `region.raw_text_mask` (the detection model's own independently-computed "text is here"
        // prior, `lanrurugi_ocr::entities::RawTextMask`'s own doc comment covers where it comes
        // from) feeds `local_background_stroke_mask`'s own coverage sanity check — see that
        // function's own doc comment on `coarse_prior` for the exact contract (must match this
        // crop's own dimensions, silently ignored otherwise rather than panicking).
        let coarse_prior = region.raw_text_mask.as_ref().map(|m| m.to_bool_vec());
        let mask = lanrurugi_inpaint::stroke_mask::local_background_stroke_mask(
            &crop,
            coarse_prior.as_deref(),
        )?;
        let sampled =
            lanrurugi_inpaint::stroke_mask::sample_stroke_colour(&crop, &mask.raw).map(|c| Rgb {
                r: c.0[0],
                g: c.0[1],
                b: c.0[2],
            });
        Some((mask, sampled))
    });
    let (stroke_mask, sampled_fg) = match stroke {
        Some((mask, colour)) => (Some(mask), colour),
        None => (None, None),
    };

    // Both the model and paste masks are intersected with the bubble mask independently — same
    // "tightest of the two" rule this function's own doc comment describes, just applied twice
    // instead of once now that there are two dilations to carry through
    // (`lanrurugi_inpaint::stroke_mask::StrokeMask`'s own doc comment covers why the split exists
    // at all).
    let intersect = |bubble: &[bool], stroke: &[bool]| -> Vec<bool> {
        bubble.iter().zip(stroke).map(|(&b, &s)| b && s).collect()
    };
    let region_erase = match (bubble_mask, stroke_mask) {
        (Some(bubble), Some(stroke)) if bubble.len() == stroke.model.len() => Some(RegionErase {
            model: intersect(&bubble, &stroke.model),
            paste: intersect(&bubble, &stroke.paste),
        }),
        (Some(bubble), _) => Some(RegionErase {
            paste: bubble.clone(),
            model: bubble,
        }),
        (None, Some(stroke)) => Some(RegionErase {
            model: stroke.model,
            paste: stroke.paste,
        }),
        (None, None) => None,
    };
    (region_erase, sampled_fg)
}

/// A region's own contribution to the page-wide erase — see [`lanrurugi_inpaint::stroke_mask::StrokeMask`]'s
/// own doc comment for why `model` (fed to LaMa) and `paste` (actually overwritten in the output)
/// are two different masks rather than one.
struct RegionErase {
    model: Vec<bool>,
    paste: Vec<bool>,
}

/// Crops `page` to `bbox`, clamped to the page bounds — same clamping rule
/// `merge_region_mask_into_page` itself applies, so the crop this hands to `stroke_mask` always
/// matches the `w`/`h` that call site ends up using for the mask-length check.
fn crop_region(page: &RgbImage, bbox: &BoundingBox) -> Option<RgbImage> {
    let (pw, ph) = page.dimensions();
    if bbox.x >= pw || bbox.y >= ph || bbox.w == 0 || bbox.h == 0 {
        return None;
    }
    let w = bbox.w.min(pw - bbox.x);
    let h = bbox.h.min(ph - bbox.y);
    Some(image::imageops::crop_imm(page, bbox.x, bbox.y, w, h).to_image())
}

/// Finds the detected bubble that best overlaps `region` (by IoU, requiring at least
/// [`MIN_BUBBLE_MATCH_IOU`]) — split out from the actual mask-cropping step
/// ([`crop_bubble_mask`]) so the matched bubble's own `bbox` can be reused for text layout
/// (`layout_box`'s own doc comment) without a second IoU pass.
fn best_matching_bubble<'a>(
    region: &BoundingBox,
    bubbles: &'a [DetectedBubble],
) -> Option<&'a DetectedBubble> {
    bubbles
        .iter()
        .map(|b| (b, region.iou(&b.bbox)))
        .filter(|(_, iou)| *iou >= MIN_BUBBLE_MATCH_IOU)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(b, _)| b)
}

/// Crops `bubble`'s own mask (in the *page's* coordinate frame) into `region`'s own coordinate
/// frame — row-major, exactly `region.w * region.h` long, ready to feed into
/// `merge_region_mask_into_page`.
///
/// A bubble's own detected box is rarely pixel-identical to the OCR text region's box inside it
/// (the bubble is usually somewhat larger — real padding around the lettering) — this crop handles
/// that offset directly rather than requiring the two boxes to already agree. Pixels `region`
/// covers that fall outside `bubble`'s own box (should be rare given the IoU requirement, but
/// possible right at an edge) default to `false` — no mask data exists there, so nothing gets
/// erased for that sliver rather than guessing.
fn crop_bubble_mask(region: &BoundingBox, bubble: &DetectedBubble) -> Vec<bool> {
    let mut cropped = vec![false; (region.w * region.h) as usize];
    for ry in 0..region.h {
        for rx in 0..region.w {
            let page_x = region.x + rx;
            let page_y = region.y + ry;
            if page_x < bubble.bbox.x || page_y < bubble.bbox.y {
                continue;
            }
            let (bx, by) = (page_x - bubble.bbox.x, page_y - bubble.bbox.y);
            if bx >= bubble.bbox.w || by >= bubble.bbox.h {
                continue;
            }
            cropped[(ry * region.w + rx) as usize] =
                bubble.mask[(by * bubble.bbox.w + bx) as usize];
        }
    }
    cropped
}

/// Whether `bbox` should be treated as a top-to-bottom, right-to-left bubble (research.md §11
/// update). Both signals are required: aspect ratio alone would misfire on a square/wide CJK
/// bubble that was actually set horizontally, and CJK-script alone would misfire on wide
/// word-balloon shapes. `text` must be the string about to actually be drawn (the translation),
/// not the original source lettering — vertical layout is a property of the target script, not
/// the source's (see this function's own call site in `composite_page` for the real incident this
/// distinction fixed: a Japanese-to-English translation was rendered vertically because the
/// *original* Japanese happened to be CJK, even though the English output plainly should not be).
fn is_vertical_bubble(bbox: &BoundingBox, text: &str) -> bool {
    if bbox.w == 0 {
        return false;
    }
    let tall_and_narrow = bbox.h as f32 >= bbox.w as f32 * VERTICAL_ASPECT_THRESHOLD;
    tall_and_narrow && text.chars().any(is_cjk_ideographic_or_kana)
}

/// Han ideographs and the two Japanese kana blocks — the scripts manga is actually lettered in
/// vertically. Deliberately narrow (not a general "is this CJK" check): Hangul manhwa are
/// conventionally lettered horizontally even when translated from vertical Japanese source, so
/// including it here would vertically-render text that was never vertical to begin with.
fn is_cjk_ideographic_or_kana(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309F   // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

/// The real box `fit_text`/`fit_text_vertical` shrink-fit the translated text into, and the box
/// `draw_region_horizontal`/`draw_region_vertical` actually draw against — deliberately not always
/// `region`'s own bounding box. See [`MIN_FONT_PX`]'s own doc comment for the real incident this
/// exists to mitigate (a longer translation than its source rendering visibly smaller than the
/// original lettering, since `fit_text*` has no notion of the source's own real size at all).
///
/// Prefers `matched_bubble`'s own box when a bubble was confidently matched to this region (the
/// bubble's own interior is close to the *real* space manga lettering actually gets set into — an
/// OCR text-detection box is deliberately tight around just the glyph pixels themselves, not the
/// bubble's own real usable area around them, so laying out against the bubble instead gives the
/// translation meaningfully more room before the shrink-to-fit search needs to reach for a smaller
/// size at all). Falls back to `region`'s own box, grown by [`LAYOUT_OVERFLOW_FRACTION`] on every
/// side, when no bubble was matched (an isolated SFX/caption with no bubble of its own, the common
/// case a matched-bubble box can never help) — modest permission to spill a little past the tight
/// OCR crop rather than none at all.
fn layout_box(region: &BoundingBox, matched_bubble: Option<&BoundingBox>) -> BoundingBox {
    if let Some(bubble) = matched_bubble {
        return *bubble;
    }
    let grow_w = (region.w as f32 * LAYOUT_OVERFLOW_FRACTION).round() as u32;
    let grow_h = (region.h as f32 * LAYOUT_OVERFLOW_FRACTION).round() as u32;
    BoundingBox::new(
        region.x.saturating_sub(grow_w),
        region.y.saturating_sub(grow_h),
        region.w + grow_w * 2,
        region.h + grow_h * 2,
    )
}

#[allow(clippy::too_many_arguments)]
fn draw_region_horizontal(
    page: &mut RgbImage,
    bbox: &BoundingBox,
    w: u32,
    h: u32,
    text: &str,
    font: &ResolvedFont<'_>,
    fg: Rgb,
    bold: bool,
    outline: Option<Rgb>,
) {
    let Some((font_px, lines)) = fit_text(text, font, w, h) else {
        return;
    };

    let upem = font.raster.units_per_em().unwrap_or(1000.0);
    let px_scale = PxScale::from(font_px);
    let scaled = font.raster.as_scaled(px_scale);
    let line_height = scaled.height() * LINE_SPACING_FACTOR;
    let total_height = line_height * lines.len() as f32;
    // Vertically centre the block within the region.
    let mut baseline_y = bbox.y as f32 + (h as f32 - total_height).max(0.0) / 2.0 + scaled.ascent();

    for line in &lines {
        let shaped = shape_line(font, line, harfrust::Direction::LeftToRight, font_px, upem);
        // Horizontally centre each line — matches how manga dialogue is typically set.
        let mut pen_x = bbox.x as f32 + (w as f32 - shaped.advance).max(0.0) / 2.0;
        let pen_y = baseline_y;

        for glyph in &shaped.glyphs {
            draw_glyph(
                page,
                font,
                glyph.glyph_id,
                px_scale,
                pen_x + glyph.x_offset,
                pen_y - glyph.y_offset,
                fg,
                bold,
                outline,
            );
            pen_x += glyph.x_advance;
        }
        baseline_y += line_height;
    }
}

/// Top-to-bottom, right-to-left layout — columns fill from the region's right edge leftward,
/// matching how the manga's own vertical dialogue reads (research.md §11 update).
#[allow(clippy::too_many_arguments)]
fn draw_region_vertical(
    page: &mut RgbImage,
    bbox: &BoundingBox,
    w: u32,
    h: u32,
    text: &str,
    font: &ResolvedFont<'_>,
    fg: Rgb,
    bold: bool,
    outline: Option<Rgb>,
) {
    let Some((font_px, columns)) = fit_text_vertical(text, font, w, h) else {
        return;
    };

    let upem = font.raster.units_per_em().unwrap_or(1000.0);
    let px_scale = PxScale::from(font_px);
    let scaled = font.raster.as_scaled(px_scale);
    let column_width = scaled.height() * LINE_SPACING_FACTOR;
    let total_width = column_width * columns.len() as f32;
    // Horizontally centre the block of columns within the region, then lay columns out
    // right-to-left starting from that block's right edge.
    let mut pen_x = bbox.x as f32 + (w as f32 + total_width).max(0.0) / 2.0 - column_width;

    for column in &columns {
        let shaped = shape_line(
            font,
            column,
            harfrust::Direction::TopToBottom,
            font_px,
            upem,
        );
        // Vertically centre each column — mirrors how a horizontal line is centred.
        let mut pen_y = bbox.y as f32 + (h as f32 - shaped.advance).max(0.0) / 2.0;
        // Columns are drawn glyph-top-anchored (unlike the horizontal path's baseline), since
        // vertical metrics/baselines are far less standardised across CJK fonts than horizontal
        // ascent/descent — anchoring to the glyph's own top keeps columns visually aligned.
        let column_pen_x = pen_x + column_width / 2.0;

        for glyph in &shaped.glyphs {
            draw_glyph(
                page,
                font,
                glyph.glyph_id,
                px_scale,
                column_pen_x + glyph.x_offset
                    - scaled.h_advance(ab_glyph::GlyphId(glyph.glyph_id as u16)) / 2.0,
                pen_y - glyph.y_offset + scaled.ascent(),
                fg,
                bold,
                outline,
            );
            pen_y += glyph.y_advance;
        }
        pen_x -= column_width;
    }
}

/// Offsets a cheap 8-direction outline stroke samples at — same technique subtitle rendering
/// commonly uses in place of a true stroked-path fill, and simple enough to build on the same
/// per-pixel `coverage` callback `ab_glyph::OutlineCurveBuilder` already drives everything else
/// in this file through. Deliberately a fixed 1px ring rather than scaling with font size: this
/// only needs to read as "text has a contrasting edge," not reproduce a specific stroke width.
const OUTLINE_STROKE_OFFSETS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Rasterizes and alpha-blends one glyph at `(pen_x, pen_y)` — the shared last step for both the
/// horizontal and vertical layout paths.
///
/// `outline` is `Some` only when `draw_region` had no confidently-uniform background to flat-fill
/// (see that function's own doc comment) — in that case a contrasting stroke is drawn around the
/// glyph before its normal fill, since there's no backing plate behind it to provide contrast.
#[allow(clippy::too_many_arguments)]
fn draw_glyph(
    page: &mut RgbImage,
    font: &ResolvedFont<'_>,
    glyph_id: u32,
    scale: PxScale,
    pen_x: f32,
    pen_y: f32,
    fg: Rgb,
    bold: bool,
    outline: Option<Rgb>,
) {
    let glyph = ab_glyph::GlyphId(glyph_id as u16)
        .with_scale_and_position(scale, ab_glyph::point(pen_x, pen_y));

    let Some(glyph_outline) = font.raster.outline_glyph(glyph) else {
        return;
    };
    let bounds = glyph_outline.px_bounds();

    if let Some(stroke_color) = outline {
        glyph_outline.draw(|gx, gy, coverage| {
            if coverage <= 0.01 {
                return;
            }
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            for (dx, dy) in OUTLINE_STROKE_OFFSETS {
                blend_pixel(page, px + dx, py + dy, stroke_color, coverage);
            }
        });
    }

    glyph_outline.draw(|gx, gy, coverage| {
        if coverage <= 0.01 {
            return;
        }
        let px = bounds.min.x as i32 + gx as i32;
        let py = bounds.min.y as i32 + gy as i32;
        blend_pixel(page, px, py, fg, coverage);

        // Faux-bold: a second pass offset by one pixel. A real bold face would be better, but the
        // golden set carries style buckets rather than per-weight font files, and this keeps an
        // estimated-bold block visibly heavier.
        if bold {
            blend_pixel(page, px + 1, py, fg, coverage);
        }
    });
}

/// One shaped run's positioned glyphs plus its total advance along the layout direction (line
/// width for horizontal, column height for vertical).
struct ShapedGlyph {
    glyph_id: u32,
    x_advance: f32,
    y_advance: f32,
    x_offset: f32,
    y_offset: f32,
}

struct ShapedLine {
    glyphs: Vec<ShapedGlyph>,
    advance: f32,
}

/// Shapes one line/column of text with `harfrust`, converting its font-unit output to pixels.
fn shape_line(
    font: &ResolvedFont<'_>,
    text: &str,
    direction: harfrust::Direction,
    font_px: f32,
    units_per_em: f32,
) -> ShapedLine {
    if text.is_empty() {
        return ShapedLine {
            glyphs: Vec::new(),
            advance: 0.0,
        };
    }

    let scale = font_px / units_per_em;

    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    buffer.set_direction(direction);
    // `guess_segment_properties`'s script guess drives HarfBuzz's cluster/shaping-engine
    // selection independently of `direction` — confirmed live: leaving its guess in place for
    // vertical CJK runs produced a shaper that still clustered/ordered glyphs as if horizontal,
    // scrambling column reading order even though `direction` itself was correctly `TopToBottom`.
    // Explicitly overriding to Han (which also covers hiragana/katakana here — this project has
    // no distinct vertical shaping rules per CJK sub-script) is what actually selects the
    // vertical-aware behaviour.
    let is_vertical_direction = matches!(
        direction,
        harfrust::Direction::TopToBottom | harfrust::Direction::BottomToTop
    );
    if is_vertical_direction {
        if let Some(han) = harfrust::Script::from_iso15924_tag(harfrust::Tag::new(b"Hani")) {
            buffer.set_script(han);
        }
    }

    let shaper = font.shaper();
    let output = shaper.shape(buffer, harfrust::ShapeOptions::new());

    let mut glyphs = Vec::with_capacity(output.glyph_infos().len());
    let mut advance = 0.0;
    for (info, pos) in output.glyph_infos().iter().zip(output.glyph_positions()) {
        let x_advance = pos.x_advance as f32 * scale;
        // HarfBuzz's font-unit space has +y pointing up (mathematical convention) regardless of
        // direction, so a `TopToBottom` run's pen genuinely moves in -y as it advances down the
        // column. This project's image coordinate space has +y pointing down (standard raster
        // convention), and every consumer of `ShapedGlyph`/`ShapedLine::advance` here (`measure`
        // during line-fitting, `pen_y +=` during drawing) wants "distance moved so far", i.e. a
        // value that's non-negative and directly addable to a raster y-coordinate. Negating here,
        // once, at the one place this unit crosses from HarfBuzz's convention into this module's,
        // is simpler than teaching every call site about the sign flip.
        //
        // Confirmed live: without this, `y_advance` came back consistently negative (e.g. -504 for
        // a 7-character run at 72px), so every "does this fit" comparison against a positive
        // `max_extent` trivially passed — `wrap_by_break_opportunities` never broke a single
        // vertical column, and `draw_region_vertical`'s `pen_y += glyph.y_advance` walked upward
        // instead of downward, stacking every glyph near the column's top.
        let y_advance = -(pos.y_advance as f32) * scale;
        glyphs.push(ShapedGlyph {
            glyph_id: info.glyph_id,
            x_advance,
            y_advance,
            x_offset: pos.x_offset as f32 * scale,
            y_offset: pos.y_offset as f32 * scale,
        });
        advance += if direction == harfrust::Direction::TopToBottom
            || direction == harfrust::Direction::BottomToTop
        {
            y_advance
        } else {
            x_advance
        };
    }

    ShapedLine { glyphs, advance }
}

/// Finds the largest font size at which `text` wraps into lines that fit the region.
///
/// Shrink-to-fit rather than overflow: an overflowing translation would cover neighbouring
/// artwork, which is worse than slightly smaller text.
fn fit_text(
    text: &str,
    font: &ResolvedFont<'_>,
    box_w: u32,
    box_h: u32,
) -> Option<(f32, Vec<String>)> {
    let usable_w = (box_w as f32) * (1.0 - 2.0 * HORIZONTAL_PADDING);
    if usable_w <= 0.0 {
        return None;
    }
    let upem = font.raster.units_per_em().unwrap_or(1000.0);

    let start = (box_h as f32 * 0.5).clamp(MIN_FONT_PX, MAX_FONT_PX);
    let mut size = start;

    while size >= MIN_FONT_PX {
        let lines = wrap_text(text, font, size, upem, usable_w);
        let line_height = font.raster.as_scaled(PxScale::from(size)).height() * LINE_SPACING_FACTOR;
        let total_height = line_height * lines.len() as f32;

        let all_fit = lines.iter().all(|l| {
            shape_line(font, l, harfrust::Direction::LeftToRight, size, upem).advance <= usable_w
        });
        if total_height <= box_h as f32 && all_fit {
            return Some((size, lines));
        }
        size -= 1.0;
    }

    // Even at the minimum size it doesn't fit; render at the floor rather than nothing at all.
    Some((
        MIN_FONT_PX,
        wrap_text(text, font, MIN_FONT_PX, upem, usable_w),
    ))
}

/// Finds the largest font size at which `text` wraps into columns that fit the region, laid out
/// top-to-bottom.
fn fit_text_vertical(
    text: &str,
    font: &ResolvedFont<'_>,
    box_w: u32,
    box_h: u32,
) -> Option<(f32, Vec<String>)> {
    let usable_h = box_h as f32 * (1.0 - 2.0 * HORIZONTAL_PADDING);
    if usable_h <= 0.0 {
        return None;
    }
    let upem = font.raster.units_per_em().unwrap_or(1000.0);

    let start = (box_w as f32 * 0.5).clamp(MIN_FONT_PX, MAX_FONT_PX);
    let mut size = start;

    while size >= MIN_FONT_PX {
        let columns = wrap_text_vertical(text, font, size, upem, usable_h);
        let column_width =
            font.raster.as_scaled(PxScale::from(size)).height() * LINE_SPACING_FACTOR;
        let total_width = column_width * columns.len() as f32;

        let all_fit = columns.iter().all(|c| {
            shape_line(font, c, harfrust::Direction::TopToBottom, size, upem).advance <= usable_h
        });
        if total_width <= box_w as f32 && all_fit {
            return Some((size, columns));
        }
        size -= 1.0;
    }

    Some((
        MIN_FONT_PX,
        wrap_text_vertical(text, font, MIN_FONT_PX, upem, usable_h),
    ))
}

/// Greedy line wrap using Unicode UAX#14 break opportunities (`icu_segmenter`), so CJK text wraps
/// correctly without whitespace — the previous `split_whitespace`-based wrap could never break a
/// CJK line at all (research.md §11 update).
fn wrap_text(
    text: &str,
    font: &ResolvedFont<'_>,
    font_px: f32,
    upem: f32,
    max_w: f32,
) -> Vec<String> {
    wrap_by_break_opportunities(text, max_w, |segment| {
        shape_line(
            font,
            segment,
            harfrust::Direction::LeftToRight,
            font_px,
            upem,
        )
        .advance
    })
}

/// Same greedy wrap, but the measured dimension is a column's height (vertical layout) rather
/// than a line's width.
fn wrap_text_vertical(
    text: &str,
    font: &ResolvedFont<'_>,
    font_px: f32,
    upem: f32,
    max_h: f32,
) -> Vec<String> {
    wrap_by_break_opportunities(text, max_h, |segment| {
        shape_line(
            font,
            segment,
            harfrust::Direction::TopToBottom,
            font_px,
            upem,
        )
        .advance
    })
}

/// Greedily packs `text` into segments no longer (by `measure`) than `max_extent`, breaking only
/// at Unicode line-break opportunities — never mid-grapheme. A single break-opportunity-free run
/// that's itself over-long is hard-split character by character, same fallback as before.
fn wrap_by_break_opportunities(
    text: &str,
    max_extent: f32,
    measure: impl Fn(&str) -> f32,
) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let breaks: Vec<usize> = icu_segmenter::LineSegmenter::new_auto(Default::default())
        .segment_str(text)
        .collect();

    let mut lines: Vec<String> = Vec::new();
    let mut line_start = 0usize;
    let mut candidate_start = 0usize;

    for &brk in &breaks {
        if brk == 0 {
            continue;
        }
        let candidate_end = brk.min(text.len());
        let candidate = &text[line_start..candidate_end];

        if measure(candidate) <= max_extent || candidate_start == line_start {
            candidate_start = candidate_end;
            continue;
        }

        // The segment up to (but not including) this break opportunity is the longest that still
        // fits; commit it and start the next line from there.
        lines.push(text[line_start..candidate_start].to_string());
        line_start = candidate_start;
        candidate_start = candidate_end;

        // Even a single break-to-break run may itself be over-long (one very wide character, or a
        // long run with no earlier break opportunity) — hard-split it by grapheme cluster.
        if measure(&text[line_start..candidate_end]) > max_extent {
            let mut chunk_start = line_start;
            let mut probe = line_start;
            for (offset, ch) in text[line_start..candidate_end].char_indices() {
                let next = line_start + offset + ch.len_utf8();
                if measure(&text[chunk_start..next]) > max_extent && next > probe + ch.len_utf8() {
                    lines.push(text[chunk_start..probe].to_string());
                    chunk_start = probe;
                }
                probe = next;
            }
            line_start = chunk_start;
            candidate_start = candidate_end;
        }
    }

    if line_start < text.len() {
        lines.push(text[line_start..].to_string());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn fill_rect(page: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: Rgb) {
    let px = ImageRgb([color.r, color.g, color.b]);
    for yy in y..(y + h) {
        for xx in x..(x + w) {
            page.put_pixel(xx, yy, px);
        }
    }
}

/// Alpha-blends one glyph pixel, ignoring anything outside the image.
fn blend_pixel(page: &mut RgbImage, x: i32, y: i32, color: Rgb, coverage: f32) {
    let (w, h) = page.dimensions();
    if x < 0 || y < 0 || x as u32 >= w || y as u32 >= h {
        return;
    }
    let a = coverage.clamp(0.0, 1.0);
    let existing = page.get_pixel(x as u32, y as u32).0;
    let blend = |dst: u8, src: u8| -> u8 {
        (f32::from(dst) * (1.0 - a) + f32::from(src) * a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    page.put_pixel(
        x as u32,
        y as u32,
        ImageRgb([
            blend(existing[0], color.r),
            blend(existing[1], color.g),
            blend(existing[2], color.b),
        ]),
    );
}

/// Encodes a composited page as WebP — the same format (and therefore the same quota accounting)
/// the reader's resize cache already uses (research.md §18).
pub fn encode_webp(page: &RgbImage, quality: f32) -> Result<Vec<u8>, CompositeError> {
    let encoder = webp::Encoder::from_rgb(page.as_raw(), page.width(), page.height());
    Ok(encoder.encode(quality).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::ids::ArchiveId;
    use lanrurugi_ocr::entities::PageNumber;

    /// The container image bundles CJK fonts; tests that need a real font are skipped when none is
    /// found rather than failing on a machine without one.
    fn load_test_font() -> Option<&'static [u8]> {
        const CANDIDATES: [&str; 4] = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ];
        for path in CANDIDATES {
            if let Ok(bytes) = std::fs::read(path) {
                return Some(Box::leak(bytes.into_boxed_slice()));
            }
        }
        None
    }

    /// Test-only equivalent of the pre-split `composite_page(page, regions, fonts, inpainter,
    /// bubbles)` — none of this module's tests actually exercise a real `Inpainter`/RPC-forwarded
    /// erase (GPU EP integration plan v9 moved that behind a subprocess this crate has no
    /// dependency on), so `erased` is always `None` here, matching every one of these tests'
    /// pre-split `inpainter: None` argument exactly.
    fn composite_page_for_test(
        page: &mut RgbImage,
        regions: &[DetectedTextRegion],
        fonts: &FontSet<'_>,
        bubbles: Option<&[DetectedBubble]>,
    ) -> Result<(), CompositeError> {
        let plan = prepare_page_erase(page, regions, bubbles);
        finish_composite_page(page, plan, fonts, None)
    }

    fn region(text: Option<&str>, bbox: BoundingBox) -> DetectedTextRegion {
        let mut r = DetectedTextRegion::new(
            ArchiveId::from("a"),
            PageNumber(1),
            bbox,
            "元の文".into(),
            false,
        );
        r.translated_text = text.map(String::from);
        r
    }

    #[test]
    fn untranslated_regions_are_skipped_without_error() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        let mut page = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        let before = page.clone();

        composite_page_for_test(
            &mut page,
            &[region(None, BoundingBox::new(10, 10, 100, 40))],
            &fonts,
            None,
        )
        .unwrap();

        assert_eq!(
            page, before,
            "a region with no translation must not be drawn"
        );
    }

    #[test]
    fn bg_color_alone_without_a_matched_bubble_still_gets_the_outline() {
        // Real concern raised live (2026-09-09, see `outline`'s own doc comment at its call site):
        // `bg_color: Some` on its own (a colour-statistics heuristic, not a shape guarantee) must
        // no longer be enough to skip the safety-margin outline — a real matched bubble is also
        // required. Confirmed here by compositing the exact same region/text twice, differing only
        // in whether a matched bubble was supplied, and checking the two outputs differ (the
        // `bg_color`-with-bubble case draws bare fg-coloured glyphs; the `bg_color`-without-bubble
        // case additionally draws a contrasting outline ring around them, so the two rasterised
        // results cannot be pixel-identical for any real glyph).
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        let bbox = BoundingBox::new(10, 10, 150, 60);

        let mut r = region(Some("Hello"), bbox);
        r.bg_color = Some(Rgb {
            r: 250,
            g: 250,
            b: 250,
        });
        r.fg_color = Some(Rgb {
            r: 20,
            g: 20,
            b: 20,
        });

        let mut page_no_bubble = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        composite_page_for_test(&mut page_no_bubble, &[r.clone()], &fonts, None).unwrap();

        let bubble = DetectedBubble {
            bbox,
            confidence: 1.0,
            mask: vec![true; (bbox.w * bbox.h) as usize],
        };
        let mut page_with_bubble = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        composite_page_for_test(
            &mut page_with_bubble,
            &[r],
            &fonts,
            Some(std::slice::from_ref(&bubble)),
        )
        .unwrap();

        assert_ne!(
            page_no_bubble, page_with_bubble,
            "bg_color alone (no matched bubble) must draw an outline the bubble-matched case \
             doesn't, so the two rasterised pages must differ"
        );
    }

    #[test]
    fn a_translated_region_changes_the_page() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        let mut page = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        let before = page.clone();

        composite_page_for_test(
            &mut page,
            &[region(Some("Hello"), BoundingBox::new(10, 10, 150, 60))],
            &fonts,
            None,
        )
        .unwrap();

        assert_ne!(page, before);
    }

    #[test]
    fn a_region_outside_the_page_is_ignored() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        let mut page = RgbImage::from_pixel(50, 50, ImageRgb([120, 120, 120]));
        let before = page.clone();

        composite_page_for_test(
            &mut page,
            &[region(Some("Hello"), BoundingBox::new(500, 500, 100, 40))],
            &fonts,
            None,
        )
        .unwrap();

        assert_eq!(page, before);
    }

    #[test]
    fn merge_region_mask_erases_nothing_when_no_mask_given() {
        // `None` means no precise mask was available at all — the function's own doc comment
        // covers why blind whole-rectangle erasure was deliberately removed (a real bug,
        // 2026-09-08: an oversized bbox erased part of an unrelated character's head). This test
        // used to assert the old whole-rectangle-erase behaviour; updated to match the documented,
        // intentional "skip this region" behaviour instead.
        let mut page_mask = vec![false; 10 * 10];
        merge_region_mask_into_page(&mut page_mask, 10, 10, &BoundingBox::new(2, 2, 3, 3), None);
        assert!(
            page_mask.iter().all(|&b| !b),
            "no precise mask means nothing should be marked for erasure"
        );
    }

    #[test]
    fn merge_region_mask_only_marks_masked_pixels_when_a_mask_is_given() {
        let mut page_mask = vec![false; 10 * 10];
        // 3x3 region, only the centre pixel masked true.
        let region_mask = vec![
            false, false, false, //
            false, true, false, //
            false, false, false,
        ];
        merge_region_mask_into_page(
            &mut page_mask,
            10,
            10,
            &BoundingBox::new(2, 2, 3, 3),
            Some(&region_mask),
        );
        assert!(
            page_mask[(3 * 10 + 3) as usize],
            "the centre pixel (3,3) must be marked"
        );
        assert!(
            !page_mask[(2 * 10 + 2) as usize],
            "a corner pixel the region mask excluded must not be marked"
        );
    }

    #[test]
    fn merge_region_mask_never_clears_a_pixel_another_region_already_set() {
        // Two overlapping regions on the same page — a later region's own `false` mask entries
        // must never un-mark a pixel an earlier region already erased. Uses a real (non-`None`)
        // mask for the first region since `None` now erases nothing at all (see
        // `merge_region_mask_erases_nothing_when_no_mask_given`) — this test is specifically about
        // a *second* region's all-false mask not clobbering a pixel a *first* region did mark.
        let mut page_mask = vec![false; 10 * 10];
        let first_mask = vec![true; 25]; // all-true: this region erases its whole 5x5 box.
        merge_region_mask_into_page(
            &mut page_mask,
            10,
            10,
            &BoundingBox::new(0, 0, 5, 5),
            Some(&first_mask),
        );
        let second_mask = vec![false; 25]; // all-false: this region itself erases nothing.
        merge_region_mask_into_page(
            &mut page_mask,
            10,
            10,
            &BoundingBox::new(0, 0, 5, 5),
            Some(&second_mask),
        );
        assert!(
            page_mask[(2 * 10 + 2) as usize],
            "a pixel the first region marked must survive a second, non-overlapping-in-effect region"
        );
    }

    #[test]
    fn merge_region_mask_ignores_an_out_of_bounds_region() {
        let mut page_mask = vec![false; 10 * 10];
        merge_region_mask_into_page(
            &mut page_mask,
            10,
            10,
            &BoundingBox::new(20, 20, 5, 5),
            None,
        );
        assert!(page_mask.iter().all(|&b| !b));
    }

    #[test]
    fn wrapping_splits_long_text_across_lines() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let font = ResolvedFont::load(bytes).unwrap();
        let upem = font.raster.units_per_em().unwrap_or(1000.0);

        let lines = wrap_text(
            "the quick brown fox jumps over the lazy dog",
            &font,
            16.0,
            upem,
            80.0,
        );
        assert!(lines.len() > 1, "long text must wrap");
        assert!(lines.iter().all(|l| {
            shape_line(&font, l, harfrust::Direction::LeftToRight, 16.0, upem).advance <= 80.0 + 1.0
        }));
    }

    #[test]
    fn an_unbreakably_long_word_is_hard_broken() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let font = ResolvedFont::load(bytes).unwrap();
        let upem = font.raster.units_per_em().unwrap_or(1000.0);

        let lines = wrap_text(
            "supercalifragilisticexpialidocious",
            &font,
            16.0,
            upem,
            40.0,
        );
        assert!(lines.len() > 1, "an over-long single word must be broken");
    }

    #[test]
    fn cjk_text_wraps_without_whitespace() {
        // The prior `split_whitespace`-based wrap could never break this at all — a real
        // regression this feature fixes (research.md §11 update).
        let Some(bytes) = load_test_font() else {
            return;
        };
        let font = ResolvedFont::load(bytes).unwrap();
        let upem = font.raster.units_per_em().unwrap_or(1000.0);

        let lines = wrap_text(
            "これは長い日本語の文章です続く続く続く",
            &font,
            16.0,
            upem,
            60.0,
        );
        assert!(
            lines.len() > 1,
            "long CJK text with no spaces must still wrap"
        );
    }

    #[test]
    fn font_resolution_falls_back_rather_than_failing() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        // An unknown font id must still resolve to something drawable.
        assert!(ResolvedFont::load(fonts.resolve(Some("no-such-font"))).is_some());
        assert!(ResolvedFont::load(fonts.resolve(None)).is_some());
    }

    #[test]
    fn blending_ignores_out_of_bounds_pixels() {
        let mut page = RgbImage::from_pixel(10, 10, ImageRgb([0, 0, 0]));
        blend_pixel(&mut page, -5, -5, Rgb::new(255, 255, 255), 1.0);
        blend_pixel(&mut page, 100, 100, Rgb::new(255, 255, 255), 1.0);
        assert_eq!(page.get_pixel(0, 0).0, [0, 0, 0]);
    }

    #[test]
    fn layout_box_prefers_the_matched_bubble_over_the_ocr_region() {
        // Real incident this fixes (2026-09-09, see `MIN_FONT_PX`'s own doc comment): a longer
        // translation than its source rendered visibly smaller because the layout box was always
        // the tight OCR text-detection box, never the bubble's own (larger) real interior.
        let region = BoundingBox::new(50, 50, 30, 30);
        let bubble = BoundingBox::new(20, 20, 100, 100);
        assert_eq!(layout_box(&region, Some(&bubble)), bubble);
    }

    #[test]
    fn layout_box_grows_the_region_a_little_when_no_bubble_matched() {
        // The common case a matched bubble can never help — an isolated SFX/caption with no bubble
        // of its own — still gets a modest overflow allowance rather than being stuck with the
        // OCR box's own exact (often too-tight) size.
        let region = BoundingBox::new(50, 50, 100, 40);
        let layout = layout_box(&region, None);
        assert!(
            layout.w > region.w,
            "width must grow past the tight OCR box"
        );
        assert!(
            layout.h > region.h,
            "height must grow past the tight OCR box"
        );
        assert!(
            layout.x < region.x,
            "growth must extend left, not just right"
        );
        assert!(layout.y < region.y, "growth must extend up, not just down");
    }

    #[test]
    fn a_tall_narrow_box_with_cjk_text_is_vertical() {
        assert!(is_vertical_bubble(
            &BoundingBox::new(0, 0, 50, 200),
            "なにかありました?"
        ));
    }

    #[test]
    fn a_tall_narrow_box_with_latin_text_stays_horizontal() {
        // Aspect ratio alone isn't enough — a tall/narrow box whose text isn't CJK (e.g. an
        // English translation, or a vertically-stacked English sound effect) must not be forced
        // into vertical layout, even if the box shape alone would suggest it.
        assert!(!is_vertical_bubble(
            &BoundingBox::new(0, 0, 50, 200),
            "BOOM"
        ));
    }

    #[test]
    fn a_wide_box_with_cjk_text_stays_horizontal() {
        // CJK script alone isn't enough either — a wide horizontal dialogue bubble is common even
        // for CJK text.
        assert!(!is_vertical_bubble(
            &BoundingBox::new(0, 0, 300, 80),
            "なにかありました?"
        ));
    }

    #[test]
    fn vertical_check_uses_the_translated_text_not_the_source() {
        // The real bug (2026-09-09): `composite_page` used to pass `region.source_text` (always
        // Japanese, since this codebase only ever translates *from* Japanese manga) instead of the
        // actual string about to be drawn — a CJK source rendered vertically even when translated
        // into English, since the check never looked at the English output at all. This test
        // exercises `is_vertical_bubble` directly with a Latin translation of what would be a
        // tall/narrow CJK source box, confirming the function itself is correct; the real fix is
        // at the `composite_page` call site (passing `text`, not `region.source_text`) — this test
        // guards the primitive so a future regression can't silently swap the argument back.
        assert!(
            !is_vertical_bubble(&BoundingBox::new(0, 0, 50, 200), "Are you okay?"),
            "an English translation must stay horizontal even in a tall/narrow box, regardless \
             of what script the original Japanese source text used"
        );
    }

    #[test]
    fn a_vertical_region_is_still_drawn() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        let mut page = RgbImage::from_pixel(200, 300, ImageRgb([120, 120, 120]));
        let before = page.clone();

        // Tall/narrow box + CJK *translated* text triggers the vertical layout path
        // (`draw_region_vertical`) — `source_text` is set too, for realism, but must not be what
        // drives the vertical/horizontal decision (see `vertical_check_uses_the_translated_text_not_the_source`).
        let mut r = region(Some("有什么事吗"), BoundingBox::new(20, 20, 40, 250));
        r.source_text = "なにかありました?".into();

        composite_page_for_test(&mut page, &[r], &fonts, None).unwrap();

        assert_ne!(
            page, before,
            "a vertical-layout region must still render something"
        );
    }
}
