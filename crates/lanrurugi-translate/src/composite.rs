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
use lanrurugi_ocr::entities::{BoundingBox, DetectedTextRegion, Rgb, WritingDirection};

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

/// Extra layout room (per side, as a fraction of the region's own size) granted along the axis
/// text actually grows along when *no* bubble was matched — see [`effective_draw_bbox`]'s own doc
/// comment. Real reported incident (2026-09-19, page 12): when bubble segmentation missed a
/// region's bubble entirely, the layout was confined to the tight OCR box (+15%), so a short
/// translation was rendered at a small size and sat at the bottom-left of the real (much larger)
/// speech bubble. This grows the growth axis up to 1.6x, hard-clamped so it never overlaps another
/// translated region's own bounding box (the earlier real bug this clamp exists for: a region's
/// grown draw box spilling onto a neighbour).
const NO_BUBBLE_LAYOUT_GROWTH_FRACTION: f32 = 0.30;

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

/// Fraction of one vertical grid cell (`vertical_pitch`) kept clear at the top and bottom of every
/// column laid out against a bubble's own silhouette, so glyphs never sit right on the balloon's
/// own outline. See [`place_columns_in_shape`].
const VERTICAL_SHAPE_PADDING_FRACTION: f32 = 0.30;

/// Fraction of one horizontal line's pitch kept clear at the left and right of every line laid out
/// against a bubble's own silhouette — the horizontal counterpart of
/// [`VERTICAL_SHAPE_PADDING_FRACTION`], see [`place_lines_in_shape`].
const HORIZONTAL_SHAPE_PADDING_FRACTION: f32 = 0.15;

/// A box at least this much taller than it is wide, whose *source* text is CJK, is treated as a
/// vertically-lettered bubble. Manga's horizontal bubbles are typically wider than tall or close
/// to square; the vertical ones this feature now handles are reliably narrow-and-tall — there is
/// no bubble-shape signal available yet (research.md's future inpainting work would add one), so
/// the box aspect ratio is the only signal available this phase.
const VERTICAL_ASPECT_THRESHOLD: f32 = 1.2;

/// A vertical translation of at most this many characters is kept in one column even when
/// splitting it would allow a larger font — see [`fit_text_vertical`].
///
/// Real reported incident (2026-09-18): "おかえりなさいませ" (bbox `93x199`) translated to the
/// 4-character "欢迎回来" rendered as a 2x2 block that reads "回欢来迎". `fit_text_vertical`
/// searches downward from [`MAX_FONT_PX`] and returns the *first* size that fits, so it accepted a
/// large size whose column only held two glyphs (two columns of two, total width still inside the
/// box) and never reached the smaller size where all four fit one column. Right-to-left column
/// order then interleaves the reading order — for a short phrase a reader takes in as one unit,
/// that reads as scrambled text rather than as two columns.
///
/// Bounded deliberately rather than applied to every length: a long translation genuinely has to
/// wrap, and forcing one column there would shrink it far past what the box can show legibly
/// (measured: a 30-character string in a `166x236` box drops from 27px/4 columns to 14px/2
/// columns). `8` is the point where a single column still lands at a reasonable size for the
/// tall-narrow bubbles this path actually sees, and multi-column layout for anything longer is
/// both expected and read correctly.
const SHORT_VERTICAL_MAX_CHARS: usize = 8;

/// The minimum character count every column must have in a multi-column layout before that
/// layout is trusted over the single-column preference above. Real reported incident
/// (2026-09-18): the first attempt at reconciling "avoid scrambled reading order" with "avoid a
/// severe font-size penalty" compared font sizes between the single- and multi-column layouts —
/// but that broke the original regression test, because "欢迎回来" (4 chars) also splits into a
/// *larger*-font multi-column layout (2 columns of 2), the exact shape that reads scrambled. The
/// real distinguishing factor is column length, not font size: a 2-character column is too short
/// to read as a coherent fragment on its own (the original "回欢来迎" incident), while a
/// 3-character column reads as ordinary vertical text. `3` requires every column in the
/// multi-column candidate to clear that bar before it's preferred over a single, smaller-font
/// column.
const MIN_COLUMN_CHARS_FOR_MULTI_COLUMN: usize = 3;

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
    /// Per region: when `Some((mask, colour))`, this region is erased by *flat-filling* `mask`
    /// (region-frame) with `colour` in [`finish_composite_page`] instead of going through the
    /// whole-page LaMa call — used for a fully-translated matched speech bubble, whose interior is
    /// a flat backdrop. Real measured incident (2026-09-20, page 13): LaMa fed a bubble-sized hole
    /// bled the neighbouring dark artwork into the hole, leaving 40% of the original dark pixels
    /// still dark; a flat fill cannot do that. Such a region's pixels are deliberately *excluded*
    /// from `page_model_mask`/`page_paste_mask`.
    flat_fills: Vec<Option<(Vec<bool>, Rgb)>>,
    /// Per region: the matched bubble's own silhouette (see [`BubbleShape`]), for the draw pass to
    /// lay lettering out inside the real bubble shape instead of its bounding rectangle.
    bubble_shapes: Vec<Option<BubbleShape>>,
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
        // `true` when nothing actually needs the whole-page LaMa call — every region either has no
        // precise mask or is handled by a flat fill (`flat_fills`), whose pixels were excluded from
        // these page masks.
        !self.page_model_mask.iter().any(|&m| m)
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
    // below and `finish_composite_page`'s own `backdrop_handled` guard, which needs the same
    // "no precise mask, don't touch it" rule.
    let mut has_precise_mask: Vec<bool> = Vec::with_capacity(translatable.len());
    // The region's own matched bubble bbox (if any), threaded through to the draw pass below so
    // text can be laid out against the real bubble's own (usually larger) interior instead of the
    // tighter OCR text-detection box — see `layout_box`'s own doc comment for why. Computed here,
    // in the same IoU match `combined_erase_mask` itself needs, rather than re-matching a second
    // time in the draw loop.
    let mut matched_bubble_bbox: Vec<Option<BoundingBox>> = Vec::with_capacity(translatable.len());
    let mut flat_fills: Vec<Option<(Vec<bool>, Rgb)>> = Vec::with_capacity(translatable.len());
    let mut bubble_shapes: Vec<Option<BubbleShape>> = Vec::with_capacity(translatable.len());
    // Growth-strip candidates collected during the main pass below, applied only after every
    // region's own `bounding_box` is known (see the loop after this one for why).
    let mut growth_strips: Vec<BoundingBox> = Vec::new();
    // Every *other* region's own bbox, so a no-bubble layout box can never grow onto a neighbour
    // (see `effective_draw_bbox`'s own doc comment).
    let other_region_bboxes: Vec<BoundingBox> =
        translatable.iter().map(|r| r.bounding_box).collect();
    // Per detected bubble: is every dark pixel inside it covered by a *translated* region? Only
    // then is wiping the bubble's whole interior safe (see `bubble_is_fully_translated`).
    let bubble_safe: Vec<bool> = bubbles
        .map(|bubbles| {
            bubbles
                .iter()
                .map(|bubble| bubble_is_fully_translated(&pristine, bubble, &translatable))
                .collect()
        })
        .unwrap_or_default();
    for &region in &translatable {
        let matched_bubble_index =
            bubbles.and_then(|bubbles| best_matching_bubble(&region.bounding_box, bubbles));
        let matched_bubble =
            matched_bubble_index.and_then(|index| bubbles.and_then(|bs| bs.get(index)));
        // Prefer the balloon's real interior as read off the page itself; the segmentation mask is
        // only the fallback (see `bubble_interior_from_page` for the measured reasons).
        let page_interior = matched_bubble.and_then(|b| {
            bubble_interior_from_page(&pristine, &region.bounding_box, &b.bbox, &b.mask)
        });
        // `bubble_is_fully_translated` is also asked about the *page-derived* interior, when there is
        // one: the segmentation mask is not the balloon (see `bubble_interior_from_page`), and a
        // degenerate one made this check answer "not fully translated" for a balloon whose text
        // actually was translated — which sent that region down the `bubble ∩ stroke` path, whose
        // DenseCRF prior under-covers, so the original Japanese stayed on the page. Real report
        // (2026-09-20): the「は？ちょ待…」balloon kept its Japanese next to the translation.
        let interior_fully_translated = page_interior.as_ref().is_some_and(|(mask, frame)| {
            bubble_is_fully_translated(
                &pristine,
                &DetectedBubble {
                    bbox: *frame,
                    confidence: 1.0,
                    mask: mask.clone(),
                },
                &translatable,
            )
        });
        // NOTE: `interior_fully_translated` is deliberately NOT OR-ed in here. Doing that (tried
        // 2026-09-20) made the「くすくす」SFX balloon qualify for a whole-interior flat fill and wiped
        // the hand-drawn pink lettering — the region's own text there is a *misrecognition*
        // ("それ"), so "every dark pixel is covered by a translated region" was technically true
        // while the region's translation had no business being drawn over that artwork. Erasing must
        // stay bound to the segmentation model's own confidence that this really is a balloon with
        // translatable text; the user's rule for this whole area is simply: 不翻译的地方就不要抠.
        // Safe to trust the page-derived interior here *now*: the earlier attempt at this made the
        // pink「くすくす」SFX balloon qualify for a whole-interior fill and wiped the artwork, but
        // `coloured_ink_gate` above now drops exactly that class of region before it can be drawn or
        // erased. What this buys is the「は？ちょ待…」balloon, whose degenerate segmentation mask made
        // the box-based check answer "not fully translated" and sent it down `bubble ∩ stroke`,
        // whose DenseCRF prior under-covers — leaving its Japanese on the page next to the
        // translation.
        let prefer_bubble_only = matched_bubble_index.is_some()
            && (matched_bubble_index
                .and_then(|index| bubble_safe.get(index).copied())
                .unwrap_or(false)
                || interior_fully_translated);
        matched_bubble_bbox.push(matched_bubble.map(|b| b.bbox));
        let bubble_mask = match &page_interior {
            Some((mask, frame)) => {
                Some(crop_page_mask_to_region(mask, frame, &region.bounding_box))
            }
            None => matched_bubble.map(|b| crop_bubble_mask(&region.bounding_box, b)),
        };
        bubble_shapes.push(match page_interior {
            Some((mask, frame)) => Some(BubbleShape { mask, frame }),
            None => bubble_mask.as_ref().map(|mask| BubbleShape {
                mask: mask.clone(),
                frame: region.bounding_box,
            }),
        });
        let (region_erase, sampled_fg) =
            combined_erase_mask(&pristine, region, bubble_mask, prefer_bubble_only);
        // 不翻译的地方就不要抠: strongly coloured ink means this is stylised/hand-drawn artwork we
        // cannot reliably read — drop the region entirely (no erase, and via `backdrop_handled` no
        // drawn translation either) rather than wiping art and drawing a pink-on-pink guess into it.
        let region_erase =
            if region_erase.is_some() && coloured_ink_gate(&pristine, &region.bounding_box) {
                None
            } else {
                region_erase
            };
        sampled_colours.push(sampled_fg);
        has_precise_mask.push(region_erase.is_some());

        // A fully-translated matched bubble is flat-filled with its own background colour rather
        // than sent through LaMa (see `flat_fills`'s own field comment); its pixels are then
        // deliberately not merged into the whole-page erase masks.
        let flat_fill = if prefer_bubble_only {
            region_erase.as_ref().map(|erase| {
                (
                    erase.paste.clone(),
                    sample_bubble_background(&pristine, matched_bubble),
                )
            })
        } else {
            None
        };
        if flat_fill.is_none() {
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
        flat_fills.push(flat_fill);

        // The erase mask above only ever covers `region.bounding_box` (the tight OCR crop) — but
        // when no bubble is matched, `finish_composite_page`'s draw pass lays text out against
        // `effective_draw_bbox`'s own result, which can be wider (cross-axis) than that tight crop
        // for a multi-column vertical (or multi-line horizontal) layout — still comfortably inside
        // the grown box, which is exactly why `fit_text_vertical` picked that layout at all. Real
        // reported incident (2026-09-18, "回错家了吗？" rendered as two vertical columns): the
        // newly-added column's own backdrop pixels were never erased or filled, so the original
        // artwork/question-mark pattern showed straight through behind that column's glyphs.
        // `effective_draw_bbox` (not the raw, un-clamped `layout_box`) is used here specifically so
        // this erase-side extent always matches the draw-side extent exactly — see that function's
        // own doc comment for the real incident (drifting apart on the clamped axis) that requiring
        // a single shared computation fixes. Only meaningful when this region actually got a
        // precise mask at all (`region_erase.is_some()` — `finish_composite_page`'s
        // `backdrop_handled` guard skips drawing entirely otherwise, so there would be nothing to
        // backfill for), and only for the no-bubble case: a matched bubble's own bbox *is*
        // `layout_box`'s return value already (see `layout_box`'s own doc comment), so
        // `region.bounding_box` there already covers the same geometry `region_erase` was built
        // against — nothing extra to add.
        if matched_bubble.is_none() && region_erase.is_some() {
            let text = region.translated_text.as_deref().unwrap_or_default();
            let vertical = is_vertical_bubble(&region.bounding_box, text, region.writing_direction);
            growth_strips.push(effective_draw_bbox(
                &region.bounding_box,
                None,
                vertical,
                &other_region_bboxes,
            ));
        }
    }

    // Applying `growth_strips` now, after every region's own `bounding_box` is known, rather than
    // inline in the loop above — real reported incident (2026-09-18): densely-packed short
    // captions ("诶？"/"陌生的", each with no precise mask of their own, guarded off from drawing
    // entirely by `finish_composite_page`'s `backdrop_handled` check) sit close enough to a
    // neighbouring region ("美女赖着不走的强盗……？", no bubble, `region_erase: Some`) that this
    // neighbour's own 15%-grown `layout_box` overlapped their bounding boxes outright (real
    // coordinates: grown box x:[147,265] y:[1124,1557] vs. "诶？"'s own box x:[245,283]
    // y:[1179,1257] — a real, non-trivial overlap). Marking that overlap into the shared page mask
    // erased and redrew translated text across a region that was supposed to be left completely
    // untouched, producing the exact original+translation overlap this module's own
    // `backdrop_handled` guard exists to prevent. Skip any growth-strip pixel that falls inside
    // another translatable region's own (tight, un-grown) `bounding_box` — that pixel belongs to
    // that other region's own erase/draw decision, never to a neighbour's cosmetic growth margin.
    for strip in &growth_strips {
        mark_bbox_in_page_mask_excluding(&mut page_model_mask, pw, ph, strip, &translatable);
        mark_bbox_in_page_mask_excluding(&mut page_paste_mask, pw, ph, strip, &translatable);
    }

    PageErasePlan {
        pristine,
        page_model_mask,
        page_paste_mask,
        translatable,
        sampled_colours,
        has_precise_mask,
        matched_bubble_bbox,
        flat_fills,
        bubble_shapes,
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
        flat_fills,
        bubble_shapes,
    } = plan;

    // Whether erase and text-draw actually happen together for a given region — never
    // independently. Real reported incident (2026-09-17, screenshot comparison): the draw loop below
    // used to run unconditionally over every translated region regardless of whether its own
    // backdrop had actually been erased, so a region the erase never touched got its Japanese
    // original left intact *and* had the Chinese translation drawn directly on top of it, producing
    // visibly overlapping original+translated text. Erasing and drawing must be all-or-nothing per
    // region: if this region's own backdrop was not actually handled, skip drawing its translation
    // too and leave the original page pixels alone — a silently-untranslated region (already an
    // accepted degradation elsewhere in this pipeline, e.g. missing detections) is far less
    // confusing than original-and-translation text visibly stacked on each other.
    //
    // A failed/absent whole-page erase (`erased: None`) therefore handles *no* region at all.
    // Earlier versions instead flat-filled `region.bounding_box` with `region.bg_color` and treated
    // that as handled; that was removed (2026-09-17) after it produced visible hard-edged rectangle
    // blocks on a real page. Two independent reasons it can't be rescued by narrowing it: the fill
    // covered exactly the same region set this guard already skips, so it never prevented any
    // overlap the guard doesn't prevent on its own; and it filled the *OCR text box*, never the
    // matched bubble box, so its edges land on bubble interior rather than any real shape boundary —
    // wrong geometry even when `bg_color` is exactly right. (`bg_color` itself is only a k-means
    // cluster mean gated on a stddev threshold *inside* that crop, and says nothing about the pixels
    // just outside it, where the rectangle's edges actually fall.) The upstream reference
    // implementation (`manga-image-translator`, `manga_translator.py`) has no such step either: on
    // inpainting failure it renders onto the untouched original.
    //
    // Real reported incident (2026-09-17, this exact "おかえりなさいませ" caption, confirmed via a
    // real triggered translation + a `tracing::warn!` diagnostic dump of `combined_erase_mask`'s own
    // per-region result): the first version of this fix treated `erased: Some(_)` (a real whole-page
    // LaMa pass that *did* run and *did* succeed) as "every region is handled" — but a successful
    // whole-page erase only actually repaints the pixels its own `page_model_mask`/`page_paste_mask`
    // marked `true`, which are built strictly from each region's own `has_precise_mask` result
    // (`prepare_page_erase`'s loop, above). A region whose own mask-building failed
    // (`densecrf_stroke_mask` returned `None` — confirmed live for this exact region: a short 2-4
    // character caption/label whose own connected component, relative to its own small crop, gets
    // large enough to trip `MAX_COMPONENT_AREA_FRACTION`'s oversized-component filter) contributes
    // zero pixels to either page mask, so LaMa never touches that region's own pixels regardless of
    // whether the *rest* of the page erased successfully — its original text survives untouched
    // inside an otherwise-successfully-erased page. `handled` must therefore check this region's own
    // `has_precise_mask`, in both the `erased: Some` and `erased: None` cases — not just assume every
    // region benefited from a page-wide success it may have contributed nothing to.
    let erased_present = erased.is_some();
    let backdrop_handled: Vec<bool> = translatable
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if erased_present {
                has_precise_mask[index]
            } else {
                // No whole-page erase at all: only regions handled by a flat fill (which needs no
                // LaMa) are safe to draw over.
                flat_fills[index].is_some()
            }
        })
        .collect();
    // No inpainter available, or the erase call itself failed (the caller is responsible for
    // logging why — this function only sees the binary "did we get an erased page back or not",
    // matching FR-019's "missing model = feature unavailable, not an error" convention). In that
    // case `page` is simply left as the original: every region is then skipped by the
    // `backdrop_handled` guard below, so nothing is drawn over an unerased backdrop.
    if let Some(erased_page) = erased {
        *page = erased_page;
    }
    // Apply every flat-filled region (fully-translated matched bubbles): paint the region's own
    // erase mask with its sampled bubble background colour. This replaces both the original
    // lettering and LaMa's own reconstruction for those pixels — see `flat_fills`'s own field
    // comment for the measured LaMa-bleed incident this fixes.
    for (region, fill) in translatable.iter().zip(flat_fills.iter()) {
        let Some((mask, colour)) = fill else {
            continue;
        };
        let (pw, ph) = page.dimensions();
        let bbox = &region.bounding_box;
        if bbox.x >= pw || bbox.y >= ph {
            continue;
        }
        let w = bbox.w.min(pw - bbox.x);
        let h = bbox.h.min(ph - bbox.y);
        if w == 0 || h == 0 || mask.len() != (w * h) as usize {
            continue;
        }
        for ry in 0..h {
            for rx in 0..w {
                if mask[(ry * w + rx) as usize] {
                    page.put_pixel(
                        bbox.x + rx,
                        bbox.y + ry,
                        ImageRgb([colour.r, colour.g, colour.b]),
                    );
                }
            }
        }
    }
    // Every region's own bbox, for `effective_draw_bbox`'s neighbour clamp (see its own doc
    // comment) when a region has no matched bubble.
    let other_region_bboxes: Vec<BoundingBox> =
        translatable.iter().map(|r| r.bounding_box).collect();
    let mut to_draw = Vec::with_capacity(translatable.len());
    for ((((&region, sampled_fg), matched_bubble), &handled), bubble_shape) in translatable
        .iter()
        .zip(sampled_colours.iter())
        .zip(matched_bubble_bbox.iter())
        .zip(backdrop_handled.iter())
        .zip(bubble_shapes.iter())
    {
        if !handled {
            continue;
        }
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
        let vertical = is_vertical_bubble(&region.bounding_box, text, region.writing_direction);
        let layout = effective_draw_bbox(
            &region.bounding_box,
            matched_bubble.as_ref(),
            vertical,
            &other_region_bboxes,
        );
        to_draw.push((
            region,
            text,
            font,
            fg,
            bold,
            vertical,
            layout,
            matched_bubble.is_some(),
            bubble_shape.as_ref(),
        ));
    }

    for (region, text, font, fg, bold, vertical, layout, has_matched_bubble, bubble_shape) in
        &to_draw
    {
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
        // `layout` is already `effective_draw_bbox`'s own result — the no-bubble clamp-back-to-
        // `region.bounding_box` (see that function's own doc comment for the real incident it
        // fixes) already happened when this tuple was built, above. Only the page-edge clamp is
        // still needed here, since that depends on `page`'s own dimensions, not on the region.
        let w = layout.w.min(pw - layout.x);
        let h = layout.h.min(ph - layout.y);

        if *vertical {
            draw_region_vertical(
                page,
                layout,
                w,
                h,
                text,
                font,
                *fg,
                *bold,
                outline,
                bubble_shape.as_ref().copied(),
            );
        } else {
            draw_region_horizontal(
                page,
                layout,
                w,
                h,
                text,
                font,
                *fg,
                *bold,
                outline,
                bubble_shape.as_ref().copied(),
            );
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

/// Marks every pixel inside `bbox` `true` in `page_mask`, *except* pixels that fall inside any
/// other region's own (tight, un-grown) `bounding_box` in `all_regions` — see this function's own
/// call site (`prepare_page_erase`'s `growth_strips` pass) for the real incident this guard exists
/// for: a growth strip is a cosmetic margin belonging to the region that produced it, and must
/// never steal pixels that are actually inside a *different* region's own detection box, since
/// that other region's own erase/draw eligibility (and `finish_composite_page`'s
/// `backdrop_handled` guard) was decided independently and may have chosen "leave this untouched"
/// — a growth strip overriding that decision behind its back is exactly the bug this excludes.
/// Same out-of-bounds/zero-size clipping as `merge_region_mask_into_page`.
fn mark_bbox_in_page_mask_excluding(
    page_mask: &mut [bool],
    page_w: u32,
    page_h: u32,
    bbox: &BoundingBox,
    all_regions: &[&DetectedTextRegion],
) {
    if bbox.x >= page_w || bbox.y >= page_h || bbox.w == 0 || bbox.h == 0 {
        return;
    }
    let w = bbox.w.min(page_w - bbox.x);
    let h = bbox.h.min(page_h - bbox.y);
    for ry in 0..h {
        for rx in 0..w {
            let (px, py) = (bbox.x + rx, bbox.y + ry);
            // Pixels inside *any* region's own tight bounding box are skipped here, including the
            // region that produced this growth strip itself — harmless for that case: `bbox` only
            // ever grows outward from its own region's box (see `layout_box`), so the region's own
            // interior was already correctly handled by `merge_region_mask_into_page` above: this
            // loop only ever needed to add the pixels *outside* every region's own tight box in the
            // first place.
            let inside_any_region = all_regions.iter().any(|r| {
                let b = &r.bounding_box;
                px >= b.x && px < b.x + b.w && py >= b.y && py < b.y + b.h
            });
            if inside_any_region {
                continue;
            }
            let i = (py * page_w + px) as usize;
            page_mask[i] = true;
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
/// Three cases, from most to least precise (applied independently to both the `model` and `paste`
/// masks — `lanrurugi_inpaint::stroke_mask::StrokeMask`'s own doc comment covers why there are two
/// at all):
/// - Both available: intersection — a pixel must be inside the real bubble outline *and* actually
///   glyph-coloured to be erased. Tighter than either alone; a bubble's own fill colour right next
///   to the glyph, or a stroke-coloured pixel that happens to fall outside the bubble's real
///   outline (its own OCR box padding, typically), is left untouched either way.
/// - Only the stroke mask: isolated SFX/caption lettering directly on artwork with no bubble of
///   its own, but `fg_color`/`bg_color` *were* confidently estimated — still narrows erasure to
///   just the glyph pixels rather than the whole OCR box.
/// - Only the bubble mask: `fg_color`/`bg_color` weren't confident enough (`style_estimate`'s own
///   gates) to build a stroke mask, but a real bubble shape is still known (a *measured* boundary,
///   not a guess) — falls back to the pre-stroke-mask behaviour for that region. Both `model` and
///   `paste` end up as the same bubble mask in this case (no stroke-mask dilation to distinguish
///   them by).
/// - Neither: `None`, meaning "erase nothing for this region" — `merge_region_mask_into_page`'s
///   own behaviour when `region_mask` is absent. This is now genuinely rare rather than the common
///   case it once was for a no-bubble, no-colour-pair region: see this function's own inline
///   comment on the `lanrurugi_inpaint::stroke_mask::densecrf_stroke_mask` fallback that now
///   covers most of what used to fall through to here.
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
    prefer_bubble_only: bool,
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
    // with no real bubble is the common case). This used to fall back to
    // `local_background_stroke_mask` (a precise, non-rectangular glyph shape built with no colour
    // priors at all, just local Lab-space contrast) — a real reported incident (2026-09-16) found
    // that fallback's own mask quality genuinely isn't trustworthy on exactly the inputs that
    // reach it (isolated SFX/caption lettering with no bubble and no clean colour split, e.g.
    // "チラ"): the erased result showed visible seams/smudging that looked *worse* than simply
    // leaving the original lettering in place, so it was removed outright rather than kept as a
    // guess-based fallback (per "擦除效果不好就保留原文" — an uncertain erase must never be
    // attempted just because *some* attempt is possible).
    //
    // `densecrf_stroke_mask` (2026-09-16) replaces that gap with a real, principled refinement
    // instead of a from-scratch guess: `region.raw_text_mask` — the detection model's own
    // already-computed, already-thresholded per-pixel text mask, not a heuristic invented for this
    // fallback — seeds a dense CRF (`lanrurugi_inpaint::densecrf`, a line-for-line port of
    // `zyddnys/manga-image-translator`'s own mask-refinement architecture) that pulls the coarse
    // box-shaped detector signal into alignment with the crop's real local colour/position
    // structure. This is a measured starting point refined by a real algorithm, not a guess dressed
    // up as one — unlike `local_background_stroke_mask`, it has an actual detection-model signal
    // behind every pixel it marks, which is exactly what that removed fallback lacked.
    let (stroke_mask, sampled_fg) = match stroke {
        Some((mask, colour)) => (Some(mask), colour),
        None => {
            let densecrf_mask = region.raw_text_mask.as_ref().and_then(|raw_mask| {
                let crop = crop_region(pristine, &region.bounding_box)?;
                lanrurugi_inpaint::stroke_mask::densecrf_stroke_mask(&crop, &raw_mask.to_bool_vec())
            });
            match densecrf_mask {
                Some(mask) => {
                    let sampled = lanrurugi_inpaint::stroke_mask::sample_stroke_colour(
                        &crop_region(pristine, &region.bounding_box).expect(
                            "densecrf_mask above already required a successful crop_region call",
                        ),
                        &mask.raw,
                    )
                    .map(|c| Rgb {
                        r: c.0[0],
                        g: c.0[1],
                        b: c.0[2],
                    });
                    (Some(mask), sampled)
                }
                None => (None, None),
            }
        }
    };

    // A confidently matched bubble erases its *whole detected interior*, not `bubble ∩ stroke`:
    // the bubble segmentation model's mask is a real, independently-detected speech-bubble shape,
    // and the stroke mask routinely under-covers the original glyphs inside it (real reported
    // incident 2026-09-20, page 13 left-middle bubble: `bubble ∩ stroke.paste` covered only 13097
    // of the bubble's 29618 pixels, leaving 81.6% of the original Japanese dark pixels untouched
    // under the translation — visible as doubled/overlapping lettering). The previous
    // `bubble ∩ stroke` intersection existed to avoid erasing artwork a "bubble" happens to
    // enclose, but the under-coverage it causes on real speech bubbles is the far more visible
    // failure; LaMa reconstructs the bubble interior (flat backdrop) cleanly from the surrounding
    // page context.
    // TEMPORARY diagnostic (issue #103): which erase path each region took and how many mask
    // pixels it ended up with, so a residual/untranslated region can be traced to "no mask at
    // all" vs "bubble-only" vs "DenseCRF under-coverage". Delete once the investigation closes.
    let dbg_has_fgbg = region.fg_color.is_some() && region.bg_color.is_some();
    let dbg_bubble_px = bubble_mask
        .as_ref()
        .map(|b| b.iter().filter(|x| **x).count())
        .unwrap_or(0);
    let dbg_stroke = stroke_mask.as_ref().map(|s| {
        (
            s.raw.iter().filter(|x| **x).count(),
            s.model.iter().filter(|x| **x).count(),
            s.paste.iter().filter(|x| **x).count(),
        )
    });

    let region_erase = match (bubble_mask, stroke_mask) {
        // Whole-bubble wipe: only when `bubble_is_fully_translated` confirmed there is no
        // untranslated lettering left inside this bubble. The bubble segmentation mask is the
        // authority for *where* the bubble is; the region's detected stroke mask may only contribute
        // within a small halo of that mask (`whole_bubble_erase_mask`), never across the region's whole
        // bounding box.
        //
        // History, because this arm has already flipped once: an earlier version unioned the *entire*
        // region's stroke mask with the bubble mask, on the reading that a bubble-mask-only wipe left
        // original Japanese in place (measured: the bubble mask covered "only 10.1%" of the region's
        // dark pixels). That measurement was wrong — it counted every dark pixel in the region's OCR
        // box, and on this page (2026-09-20, page 13, left-middle bubble) that box reaches ~180px above
        // the bubble, over the character's dark clothing; the bubble mask actually covers every dark
        // pixel *inside the bubble's own outline*. What really left lettering behind was LaMa bleeding
        // neighbouring dark artwork into the hole (bubble-mask-only + LaMa left 367 of the bubble's
        // 2434 original dark pixels still dark — all of them already inside the paste mask), which the
        // flat fill below — not a wider mask — is the fix for. The whole-region union made things
        // visibly worse instead: paste grew to 45340px from a 29618px bubble, painting a solid
        // backdrop-coloured blob over the clothing above it.
        (Some(bubble), stroke) if prefer_bubble_only => Some(whole_bubble_erase_mask(
            &bubble,
            stroke
                .as_ref()
                .map(|s| (s.model.as_slice(), s.paste.as_slice())),
            region.bounding_box.w,
            region.bounding_box.h,
        )),
        // Bubble is *not* fully translated: erase only the pixels that actually look like the
        // detected text (`bubble ∩ stroke`), so a missed line's original survives untouched rather
        // than being wiped and replaced by nothing.
        (Some(bubble), Some(stroke)) if bubble.len() == stroke.model.len() => Some(RegionErase {
            model: bubble
                .iter()
                .zip(&stroke.model)
                .map(|(&b, &s)| b && s)
                .collect(),
            paste: bubble
                .iter()
                .zip(&stroke.paste)
                .map(|(&b, &s)| b && s)
                .collect(),
        }),
        // Not fully translated and no stroke mask to constrain with: leave the bubble untouched.
        (Some(_), _) => None,
        (None, Some(stroke)) => Some(RegionErase {
            model: stroke.model,
            paste: stroke.paste,
        }),
        (None, None) => None,
    };
    tracing::info!(
        x = region.bounding_box.x,
        y = region.bounding_box.y,
        w = region.bounding_box.w,
        h = region.bounding_box.h,
        text = %region.source_text.chars().take(18).collect::<String>(),
        has_fgbg = dbg_has_fgbg,
        bubble_px = dbg_bubble_px,
        stroke = ?dbg_stroke,
        erase = ?region_erase.as_ref().map(|e| (
            e.model.iter().filter(|x| **x).count(),
            e.paste.iter().filter(|x| **x).count(),
        )),
        "erase path"
    );
    (region_erase, sampled_fg)
}

/// How far outside a matched bubble's own segmentation mask the region's detected stroke mask may
/// still contribute to a whole-bubble wipe — see [`whole_bubble_erase_mask`].
///
/// The segmentation mask is occasionally a couple of pixels short of the antialiased glyph edges it
/// wraps (`crop_bubble_mask` also clips at the region's own box); a small halo lets the stroke mask
/// close those gaps without letting it reach neighbouring artwork, which the whole-region union this
/// replaces did (real measured incident, 2026-09-20 page 13: the halo-free union painted the bubble
/// backdrop over the character's clothing ~180px above the bubble).
const BUBBLE_ERASE_HALO_RADIUS: i32 = 4;

/// Erase mask for a matched speech bubble whose own text `bubble_is_fully_translated` already
/// confirmed is fully translated and drawn.
///
/// The bubble's own segmentation mask is the authority for *where* the bubble is; `stroke` (the
/// region's detected glyph mask, `(model, paste)` as in [`RegionErase`]) is only allowed to add
/// pixels within [`BUBBLE_ERASE_HALO_RADIUS`] of that mask — covering glyph edges the segmentation
/// mask misses, and small holes inside it, but never the artwork outside the bubble.
fn whole_bubble_erase_mask(
    bubble: &[bool],
    stroke: Option<(&[bool], &[bool])>,
    w: u32,
    h: u32,
) -> RegionErase {
    let halo = dilate_mask(bubble, w, h, BUBBLE_ERASE_HALO_RADIUS);
    match stroke {
        Some((model, paste)) if model.len() == bubble.len() && paste.len() == bubble.len() => {
            RegionErase {
                model: bubble
                    .iter()
                    .zip(&halo)
                    .zip(model)
                    .map(|((&b, &near), &s)| b || (near && s))
                    .collect(),
                paste: bubble
                    .iter()
                    .zip(&halo)
                    .zip(paste)
                    .map(|((&b, &near), &s)| b || (near && s))
                    .collect(),
            }
        }
        // No usable stroke mask (`None`, or a length that disagrees with the bubble crop — the
        // caller's job to keep them in step, but never worth an out-of-bounds guess here): the
        // bubble mask alone is still a complete, trustworthy erase.
        _ => RegionErase {
            model: bubble.to_vec(),
            paste: bubble.to_vec(),
        },
    }
}

/// Grows every `true` pixel of a row-major `w * h` mask outwards by `radius` pixels (square/
/// Chebyshev kernel), clamped to the mask's own bounds. Separable — one horizontal pass, one
/// vertical pass — so cost is `O(w * h * radius)` rather than `O(w * h * radius^2)`.
fn dilate_mask(mask: &[bool], w: u32, h: u32, radius: i32) -> Vec<bool> {
    if radius <= 0 || w == 0 || h == 0 || mask.len() != (w as usize) * (h as usize) {
        return mask.to_vec();
    }
    let (w, h, r) = (w as i32, h as i32, radius);
    let mut horizontal = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            horizontal[(y * w + x) as usize] = (-r..=r).any(|dx| {
                let nx = x + dx;
                nx >= 0 && nx < w && mask[(y * w + nx) as usize]
            });
        }
    }
    let mut out = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = (-r..=r).any(|dy| {
                let ny = y + dy;
                ny >= 0 && ny < h && horizontal[(ny * w + x) as usize]
            });
        }
    }
    out
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
fn best_matching_bubble(region: &BoundingBox, bubbles: &[DetectedBubble]) -> Option<usize> {
    bubbles
        .iter()
        .enumerate()
        .map(|(index, b)| (index, region.iou(&b.bbox)))
        .filter(|(_, iou)| *iou >= MIN_BUBBLE_MATCH_IOU)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(index, _)| index)
}

/// Samples the dominant light colour of a matched bubble's interior from the pristine page — the
/// fill colour [`PageErasePlan::flat_fills`] paints over a fully-translated bubble's own erase mask.
/// Uses the mean of the bubble mask's light pixels; falls back to white when the mask has no light
/// pixels at all (a bubble mask that is itself misplaced).
fn sample_bubble_background(page: &RgbImage, bubble: Option<&DetectedBubble>) -> Rgb {
    let Some(bubble) = bubble else {
        return Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
    };
    let (pw, ph) = page.dimensions();
    let (x1, y1) = (
        (bubble.bbox.x + bubble.bbox.w).min(pw),
        (bubble.bbox.y + bubble.bbox.h).min(ph),
    );
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for y in bubble.bbox.y..y1 {
        for x in bubble.bbox.x..x1 {
            let (bx, by) = (x - bubble.bbox.x, y - bubble.bbox.y);
            if !bubble.mask[(by * bubble.bbox.w + bx) as usize] {
                continue;
            }
            let p = page.get_pixel(x, y).0;
            let luminance =
                0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
            if luminance < 180.0 {
                continue;
            }
            sum[0] += u64::from(p[0]);
            sum[1] += u64::from(p[1]);
            sum[2] += u64::from(p[2]);
            count += 1;
        }
    }
    if count == 0 {
        return Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
    }
    Rgb {
        r: (sum[0] / count) as u8,
        g: (sum[1] / count) as u8,
        b: (sum[2] / count) as u8,
    }
}

/// Whether every dark (lettering) pixel inside `bubble`'s own mask is already covered by at least
/// one *translated* region's bounding box — i.e. wiping the bubble's whole interior is safe because
/// there is no untranslated lettering left inside it.
///
/// This is the precondition for the "a speech bubble is erased whole, no stroke-edge detection
/// needed" policy: without it, a bubble that contains a line the detector/recognizer missed would
/// have that untranslated original wiped and replaced by nothing. `90%` of the bubble's dark pixels
/// must be inside some translated region's box; a bubble with no dark pixels at all is trivially
/// safe.
fn bubble_is_fully_translated(
    page: &RgbImage,
    bubble: &DetectedBubble,
    translated_regions: &[&DetectedTextRegion],
) -> bool {
    let (pw, ph) = page.dimensions();
    let (x0, y0) = (bubble.bbox.x, bubble.bbox.y);
    let (x1, y1) = (
        (bubble.bbox.x + bubble.bbox.w).min(pw),
        (bubble.bbox.y + bubble.bbox.h).min(ph),
    );
    let mut total = 0u64;
    let mut covered = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let (bx, by) = (x - x0, y - y0);
            if !bubble.mask[(by * bubble.bbox.w + bx) as usize] {
                continue;
            }
            let p = page.get_pixel(x, y).0;
            let luminance =
                0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
            if luminance >= 120.0 {
                continue;
            }
            total += 1;
            // NOTE: this is still a *rectangle* test (the region's tight OCR box), and that is a
            // known, deliberate compromise — not an oversight. Tightening it to the detector's own
            // per-pixel `raw_text_mask` was tried (2026-09-20) and measurably broke a balloon that
            // used to erase correctly: that prior only covers part of the glyphs, so the 90%
            // coverage rule failed, the balloon fell back to `bubble ∩ stroke`, and its original
            // Japanese stayed on the page (measured: 554 → 2828 residual dark pixels). Doing this
            // properly means judging coverage against each region's *real erase mask*, which
            // `prepare_page_erase` cannot do in its current single pass (the mask depends on the very
            // `prefer_bubble_only` decision being computed here) — that needs a two-pass rework, not
            // a different pixel predicate.
            let in_region = translated_regions.iter().any(|region| {
                x >= region.bounding_box.x
                    && x < region.bounding_box.x + region.bounding_box.w
                    && y >= region.bounding_box.y
                    && y < region.bounding_box.y + region.bounding_box.h
            });
            if in_region {
                covered += 1;
            }
        }
    }
    total == 0 || covered * 10 >= total * 9
}

/// Minimum luminance for a pixel to count as part of a balloon's flat interior.
const INTERIOR_MIN_LUMINANCE: f32 = 216.0;
/// Maximum channel spread for a pixel to count as flat (not coloured artwork or a tinted page
/// background) — see [`bubble_interior_from_page`].
///
/// Both this and [`INTERIOR_MIN_LUMINANCE`] are deliberately tight. First real run of the
/// page-derived interior (2026-09-20, same page 13): the looser starting values (205/40) let the
/// flood fill walk straight out of a balloon into the pale *lavender* page background above it —
/// that background measures (211,204,237), i.e. luminance 209 and channel spread 33 — and the flat
/// fill then painted a white patch over the character's shoulder. A balloon interior is very close
/// to neutral white; a tinted background is not.
const INTERIOR_MAX_CHROMA: f32 = 18.0;
/// How far the segmentation mask is dilated before it is used as the flood fill's reachability
/// bound — see [`bubble_interior_from_page`].
const INTERIOR_BOUND_DILATION: i32 = 12;
/// Share of a matched bubble's own box its segmentation mask must cover before that mask is trusted
/// as the flood fill's bound at all — see [`bubble_interior_from_page`].
const INTERIOR_BOUND_MIN_COVERAGE: f32 = 0.25;
/// Share of the matched bubble's own box the page-derived interior must cover before it is trusted
/// instead of the segmentation mask — see [`bubble_interior_from_page`].
const INTERIOR_MIN_COVERAGE_OF_BUBBLE: f32 = 0.25;

/// Chroma (max−min channel spread) at or above which a region's own ink counts as *coloured*
/// rather than the near-neutral black of ordinary balloon lettering — see [`coloured_ink_gate`].
const COLOURED_INK_MIN_CHROMA: f32 = 45.0;
/// Minimum number of strongly-coloured ink pixels before a region is considered artwork at all
/// (avoids gating on a handful of JPEG/WebP chroma artefacts).
const COLOURED_INK_MIN_PIXELS: usize = 24;
/// Minimum share of a region's own non-light pixels that must be strongly coloured before the
/// whole region is left untouched. Real page-13 blue-SFX crop: ~19% coloured; its adjacent black
/// dialogue: 0%; ordinary dialogue on coloured artwork can land around 10%, which stays below this
/// gate so the region is still handled normally.
const COLOURED_INK_MIN_FRACTION: f32 = 0.15;

/// Whether a region's own lettering looks like stylised/hand-drawn coloured artwork rather than
/// ordinary balloon dialogue, in which case this module **must not erase it**.
///
/// Real reported incident (2026-09-20, page 13): a pink hand-drawn「くすくす」giggle SFX was
/// detected as a text region, misrecognised as「それ」and "translated" to「那个」. The erase path then
/// wiped the hand-drawn lettering (DenseCRF quite correctly found the pink strokes to be ink) and the
/// replacement text was drawn in the *sampled ink colour* — pink on pale pink, i.e. invisible — so
/// the page lost its artwork and gained nothing. A recognition-confidence gate does not catch this
/// (the model decoded「それ」confidently); the signal that does is that the ink is strongly coloured,
/// which ordinary balloon dialogue never is.
///
/// The rule this implements is the user's own, stated plainly: **不翻译的地方就不要抠** — if we
/// cannot tell that this is ordinary dialogue, leave the original completely untouched. Returning
/// `true` makes the caller drop the region's erase mask entirely, which (through
/// `finish_composite_page`'s own all-or-nothing `backdrop_handled` guard) also stops its translation
/// from being drawn.
fn coloured_ink_gate(page: &RgbImage, bbox: &BoundingBox) -> bool {
    let (pw, ph) = page.dimensions();
    let x1 = (bbox.x + bbox.w).min(pw);
    let y1 = (bbox.y + bbox.h).min(ph);
    let mut ink_pixels = 0usize;
    let mut coloured_pixels = 0usize;
    for y in bbox.y..y1 {
        for x in bbox.x..x1 {
            let p = page.get_pixel(x, y).0;
            let luminance =
                0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
            // `200`, not the old `150`: hand-drawn blue/pink SFX are often lighter than ordinary
            // black lettering, and the whole point of this gate is not to miss them.
            if luminance < 200.0 {
                ink_pixels += 1;
                let max = p[0].max(p[1]).max(p[2]);
                let min = p[0].min(p[1]).min(p[2]);
                if f32::from(max - min) >= COLOURED_INK_MIN_CHROMA {
                    coloured_pixels += 1;
                }
            }
        }
    }
    // Too few ink pixels to judge (a nearly empty crop, a blank region): don't gate on noise.
    if ink_pixels < 24 {
        return false;
    }
    coloured_pixels >= COLOURED_INK_MIN_PIXELS
        && (coloured_pixels as f32) >= COLOURED_INK_MIN_FRACTION * (ink_pixels as f32)
}

/// Estimates a balloon's real flat interior **directly from the page image**, by flood-filling the
/// near-white pixels connected to the region being laid out.
///
/// This exists because the bubble segmentation model's mask turned out to be unusable as balloon
/// *geometry* on real pages (measured 2026-09-20, page 13, two different bubbles in the same page):
///
/// * one matched "bubble" mask was a **filled 96x202 rectangle** — 19392 pixels, exactly its own
///   bounding box — so the shape carried no balloon outline at all, and laying text out inside it
///   gave a font capped by the box's height with the whole width wasted;
/// * another matched mask was **5897 pixels in a 94x203 box**, its longest run only 47px at the
///   column the text was drawn in — it covered a strip at the top of the balloon and nothing else,
///   so the text was positioned against a run that has nothing to do with the balloon and spilled
///   out of it.
///
/// Both of those masks still covered the *lettering* well enough to erase it, which is why the
/// masks kept being used — but they are not the balloon's shape. The page itself, however, is: a
/// speech balloon's interior is the large near-white connected component its own text sits in.
///
/// Returns `(mask, frame)` in page coordinates, or `None` when nothing convincing was found (the
/// caller then falls back to the segmentation mask, i.e. this is strictly an improvement, never a
/// new failure mode).
fn bubble_interior_from_page(
    page: &RgbImage,
    region: &BoundingBox,
    bubble: &BoundingBox,
    seg: &[bool],
) -> Option<(Vec<bool>, BoundingBox)> {
    let (pw, ph) = page.dimensions();
    // Bounded to the region's own box unioned with the matched bubble's box: wide enough to hold the
    // whole balloon, tight enough that the flood fill cannot escape across the page.
    let x0 = region.x.min(bubble.x).min(pw);
    let y0 = region.y.min(bubble.y).min(ph);
    let x1 = (region.x + region.w).max(bubble.x + bubble.w).min(pw);
    let y1 = (region.y + region.h).max(bubble.y + bubble.h).min(ph);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let flat = |x: u32, y: u32| {
        let p = page.get_pixel(x, y).0;
        let luminance = 0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
        let max = p[0].max(p[1]).max(p[2]);
        let min = p[0].min(p[1]).min(p[2]);
        luminance >= INTERIOR_MIN_LUMINANCE && f32::from(max - min) <= INTERIOR_MAX_CHROMA
    };

    let mut mask = vec![false; (w * h) as usize];
    let mut stack: Vec<(u32, u32)> = Vec::new();
    // Seed from every near-white pixel inside the region's own box — that is where the detected
    // text (and therefore the balloon it sits in) actually is.
    for y in region.y..(region.y + region.h).min(ph) {
        for x in region.x..(region.x + region.w).min(pw) {
            if flat(x, y) {
                let index = ((y - y0) * w + (x - x0)) as usize;
                if !mask[index] {
                    mask[index] = true;
                    stack.push((x, y));
                }
            }
        }
    }
    if stack.is_empty() {
        return None;
    }
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < x0 as i32 || ny < y0 as i32 || nx >= x1 as i32 || ny >= y1 as i32 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            if !flat(nx, ny) {
                continue;
            }
            let index = ((ny - y0) * w + (nx - x0)) as usize;
            if mask[index] {
                continue;
            }
            mask[index] = true;
            stack.push((nx, ny));
        }
    }

    // Reachability bound (real regression found 2026-09-20, first run of this estimate): an interior
    // that starts inside a balloon can still walk out through a gap in the balloon's own outline
    // into a neighbouring light area — it escaped onto the character's shoulder and the flat fill
    // whitened it. The segmentation mask is a superset of the balloon often enough to serve as that
    // bound, but *only* when it is informative: a degenerate one (a real mask covered just 47px of a
    // 203px-tall balloon) would clip a perfectly good interior down to nothing, so it is applied only
    // when it covers a plausible share of its own box.
    let bubble_area = (bubble.w * bubble.h) as f32;
    let seg_px = seg.iter().filter(|&&m| m).count() as f32;
    if seg_px >= bubble_area * INTERIOR_BOUND_MIN_COVERAGE
        && seg.len() == (bubble.w * bubble.h) as usize
    {
        let bound = dilate_mask(seg, bubble.w, bubble.h, INTERIOR_BOUND_DILATION);
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x0 + x, y0 + y);
                let inside = px >= bubble.x
                    && py >= bubble.y
                    && px < bubble.x + bubble.w
                    && py < bubble.y + bubble.h;
                let allowed =
                    inside && bound[((py - bubble.y) * bubble.w + (px - bubble.x)) as usize];
                if !allowed {
                    mask[(y * w + x) as usize] = false;
                }
            }
        }
    }

    // Keep only the largest connected component. Seeding from *every* near-white pixel in the
    // region's box lets one fill produce several disconnected blobs at once — the balloon itself
    // plus any other white patch the box happens to contain. Real measured incident (2026-09-20,
    // page 13,「は？ちょ待…」): a second component only 54px tall, one column-pitch to the left of the
    // balloon's own columns, was treated as a layout column that could hold exactly one character —
    // the balanced fill put「等」there, and because that stub's run starts ~110px below its
    // neighbours' the shared-top interval came out empty and the whole line fell back to per-column
    // centring, leaving「等」stranded at the bottom under a large blank (the user-reported symptom).
    {
        let (wi, hi) = (w as usize, h as usize);
        let mut seen = vec![false; mask.len()];
        let mut best: Vec<usize> = Vec::new();
        for start in 0..mask.len() {
            if !mask[start] || seen[start] {
                continue;
            }
            let mut component: Vec<usize> = Vec::new();
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(index) = stack.pop() {
                component.push(index);
                let (x, y) = ((index % wi) as i32, (index / wi) as i32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= wi as i32 || ny >= hi as i32 {
                        continue;
                    }
                    let neighbour = ny as usize * wi + nx as usize;
                    if mask[neighbour] && !seen[neighbour] {
                        seen[neighbour] = true;
                        stack.push(neighbour);
                    }
                }
            }
            if component.len() > best.len() {
                best = component;
            }
        }
        let mut kept = vec![false; mask.len()];
        for index in best {
            kept[index] = true;
        }
        mask = kept;
    }

    // Fill the balloon's own interior. The flood fill above marks only the *near-white background*
    // between the glyphs; erasing exactly that leaves the glyphs' own pixels behind, which is the
    // black speckle reported on real pages. The rule the user stated for this whole area is that the
    // balloon is erased as one piece — its interior minus the outer outline — and that the cleaned
    // interior *is* the text's layout box. One scanline pass per row, from the first to the last
    // marked pixel, closes every glyph-shaped hole inside the blob while leaving its outer silhouette
    // (which is outside the first/last marked pixel of each row) alone.
    for y in 0..h as usize {
        let row = y * w as usize;
        let Some(first) = (0..w as usize).find(|&x| mask[row + x]) else {
            continue;
        };
        let last = (0..w as usize)
            .rev()
            .find(|&x| mask[row + x])
            .unwrap_or(first);
        for x in first..=last {
            mask[row + x] = true;
        }
    }

    // …but a stylised *coloured* element that merely overlaps the balloon must survive: the row fill
    // above would otherwise swallow it along with the glyph-shaped holes, and it did — real report
    // (2026-09-20, page 13): the blue hand-drawn「は！？」crossing that balloon's edge disappeared
    // entirely after this fill was added. Ordinary balloon lettering is near-neutral black; strongly
    // coloured ink is artwork we cannot translate, so it is removed from the shape (not from the
    // erase mask, which would punch holes and leave speckle — see the note in `prepare_page_erase`).
    for y in 0..h {
        for x in 0..w {
            let index = (y * w + x) as usize;
            if !mask[index] {
                continue;
            }
            let p = page.get_pixel(x0 + x, y0 + y).0;
            let luminance =
                0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2]);
            if luminance >= 200.0 {
                continue;
            }
            let max = p[0].max(p[1]).max(p[2]);
            let min = p[0].min(p[1]).min(p[2]);
            if (max - min) as f32 >= 45.0 {
                mask[index] = false;
            }
        }
    }

    let filled = mask.iter().filter(|&&m| m).count() as f32;
    if bubble_area <= 0.0 || filled < bubble_area * INTERIOR_MIN_COVERAGE_OF_BUBBLE {
        return None;
    }
    Some((mask, BoundingBox::new(x0, y0, w, h)))
}

/// Re-crops a page-frame mask ([`bubble_interior_from_page`]'s own output) into `region`'s frame —
/// the frame every erase mask this module builds is expressed in.
fn crop_page_mask_to_region(mask: &[bool], frame: &BoundingBox, region: &BoundingBox) -> Vec<bool> {
    let mut cropped = vec![false; (region.w * region.h) as usize];
    for ry in 0..region.h {
        for rx in 0..region.w {
            let (px, py) = (region.x + rx, region.y + ry);
            if px < frame.x || py < frame.y {
                continue;
            }
            let (fx, fy) = (px - frame.x, py - frame.y);
            if fx >= frame.w || fy >= frame.h {
                continue;
            }
            cropped[(ry * region.w + rx) as usize] = mask[(fy * frame.w + fx) as usize];
        }
    }
    cropped
}

/// A matched bubble's own silhouette, cropped into its region's frame (the same crop
/// [`crop_bubble_mask`] already produces for the erase path) — what the draw pass needs to lay
/// lettering out against the real bubble *shape* instead of its bounding rectangle.
///
/// Real reported incident (2026-09-20, page 13; user-reported "现在很多嵌字都跑出气泡了"): a speech
/// bubble is an ellipse, so a rectangle is the wrong layout target for every one of them. Vertical
/// lettering padded to the bounding rectangle's own full height sticks straight out of the top and
/// bottom of the balloon — a tall column centred on the rectangle is taller than the ellipse's own
/// height at that x as soon as it is off the vertical centre line, and the wider the block of
/// columns, the worse it is. The bubble segmentation mask the erase path already crops holds
/// exactly the shape information the draw path was missing.
struct BubbleShape {
    /// Row-major, `frame.w * frame.h` — same convention as [`crop_bubble_mask`]'s own return value.
    mask: Vec<bool>,
    /// The box `mask` is cropped to (the region's own bounding box).
    frame: BoundingBox,
}

impl BubbleShape {
    /// The bubble's own horizontal extent in page coordinates, as `(x_first, x_last)` inclusive —
    /// `None` for an empty mask.
    fn x_extent(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.frame.w as usize, self.frame.h as usize);
        let mut first: Option<usize> = None;
        let mut last: Option<usize> = None;
        for y in 0..h {
            for x in 0..w {
                if self.mask[y * w + x] {
                    first = Some(first.map_or(x, |m| m.min(x)));
                    last = Some(last.map_or(x, |m| m.max(x)));
                }
            }
        }
        match (first, last) {
            (Some(a), Some(b)) => Some((self.frame.x + a as u32, self.frame.x + b as u32)),
            _ => None,
        }
    }

    /// The bubble's own vertical extent in page coordinates, as `(y_first, y_last)` inclusive —
    /// `None` for an empty mask. The horizontal layout's counterpart of [`Self::x_extent`].
    fn y_extent(&self) -> Option<(u32, u32)> {
        let (w, h) = (self.frame.w as usize, self.frame.h as usize);
        let mut first: Option<usize> = None;
        let mut last: Option<usize> = None;
        for y in 0..h {
            for x in 0..w {
                if self.mask[y * w + x] {
                    first = Some(first.map_or(y, |m| m.min(y)));
                    last = Some(last.map_or(y, |m| m.max(y)));
                }
            }
        }
        match (first, last) {
            (Some(a), Some(b)) => Some((self.frame.y + a as u32, self.frame.y + b as u32)),
            _ => None,
        }
    }

    /// The bubble's own longest contiguous horizontal run at page-y `y`, as `(x_start, len)` in page
    /// coordinates — `None` when the bubble has no pixel in that row at all. Horizontal lettering's
    /// counterpart of [`Self::column_run`].
    fn row_run(&self, y: u32) -> Option<(u32, u32)> {
        if y < self.frame.y || y >= self.frame.y + self.frame.h {
            return None;
        }
        let (w, h) = (self.frame.w as usize, self.frame.h as usize);
        let row = (y - self.frame.y) as usize;
        let mut best: Option<(u32, u32)> = None;
        let mut x = 0u32;
        while (x as usize) < w {
            if self.mask[row * w + x as usize] {
                let start = x;
                while (x as usize) < w && self.mask[row * w + x as usize] {
                    x += 1;
                }
                let len = x - start;
                if best.is_none_or(|(_, best_len)| len > best_len) {
                    best = Some((start, len));
                }
            } else {
                x += 1;
            }
        }
        let _ = h;
        best.map(|(start, len)| (self.frame.x + start, len))
    }

    /// The bubble's own longest contiguous vertical run at page-x `x`, as `(y_start, len)` in page
    /// coordinates — `None` when the bubble has no pixel in that column at all.
    ///
    /// The *longest* run is deliberate: a column near the silhouette's own left or right edge may
    /// clip the shape only at its very top or bottom, and the run the lettering actually sits in is
    /// the long one in the middle.
    fn column_run(&self, x: u32) -> Option<(u32, u32)> {
        if x < self.frame.x || x >= self.frame.x + self.frame.w {
            return None;
        }
        let (w, h) = (self.frame.w as usize, self.frame.h as usize);
        let column = (x - self.frame.x) as usize;
        let mut best: Option<(u32, u32)> = None;
        let mut y = 0u32;
        while (y as usize) < h {
            if self.mask[y as usize * w + column] {
                let start = y;
                while (y as usize) < h && self.mask[y as usize * w + column] {
                    y += 1;
                }
                let len = y - start;
                if best.is_none_or(|(_, best_len)| len > best_len) {
                    best = Some((start, len));
                }
            } else {
                y += 1;
            }
        }
        best.map(|(start, len)| (self.frame.y + start, len))
    }
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
fn is_vertical_bubble(
    bbox: &BoundingBox,
    text: &str,
    writing_direction: Option<WritingDirection>,
) -> bool {
    // OCR measured this direction from the original lettering; it outranks every shape/script
    // guess below. This is the whole point of persisting `writing_direction`: a 2-row horizontal
    // label and a 2-column vertical label can have nearly identical bounding boxes, so the old
    // aspect-ratio + target-script heuristic would misrender one of them.
    if let Some(direction) = writing_direction {
        return direction == WritingDirection::Vertical;
    }
    // Legacy fallback for older persisted regions (or genuinely ambiguous detector output):
    // tall-and-narrow plus a CJK target script. Kept only as a fallback now.
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
#[cfg(test)]
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

/// The real box actually drawn into — `layout_box`'s own grown box, clamped back along whichever
/// axis text actually grows along (see `layout_box`'s own doc comment on why the growth exists,
/// and this function's own history below on why clamping the *extent* alone isn't enough). Single
/// source of truth for both the draw pass (`finish_composite_page`) and the erase-mask pass
/// (`prepare_page_erase`'s `growth_strips`) — having each independently re-derive "what will
/// actually get drawn" is exactly how they drifted apart in the first place (see below).
///
/// Real reported incident (2026-09-18, two rounds): round one clamped only `h`/`w` while leaving
/// `draw_origin` at `layout_box`'s own grown (shifted-up/left) corner — a vertical region's
/// clamped draw box became `y:[1124,1457]` (grown `layout.y=1124` + clamped `h=333`) instead of
/// the intended `y:[1174,1507]` (the *original* `region.bounding_box`'s own extent), a 50px shift
/// that let the drawn text spill into a completely unrelated neighbouring region's own bounding
/// box ("诶？", no precise mask of its own, never erased for this) — producing the exact
/// original+translation overlap `backdrop_handled` exists to prevent, just relocated onto a
/// neighbour instead of this region itself. Round two's own fix (moving the origin back to
/// `region.bounding_box`'s own corner on the clamped axis) then still needed a second, separate
/// pass at `prepare_page_erase`'s own growth-strip logic, which had computed its own erase-only
/// extent (`layout_box(region, None)` — the *whole* grown box, before any clamp) independently of
/// what the draw pass' own clamp would actually use — two independently-maintained
/// "what does this region actually occupy" computations that had already drifted out of sync once
/// is exactly the bug class this shared function exists to close off for good.
fn effective_draw_bbox(
    region: &BoundingBox,
    matched_bubble: Option<&BoundingBox>,
    vertical: bool,
    other_regions: &[BoundingBox],
) -> BoundingBox {
    if let Some(bubble) = matched_bubble {
        return *bubble;
    }

    // Cross axis (the one text does *not* grow along): the same modest `LAYOUT_OVERFLOW_FRACTION`
    // `layout_box` already applied. Growth axis: more room, because the real bubble the region sits
    // inside is usually much larger than the tight OCR crop — but never at the cost of overlapping
    // another translated region's own bounding box (see this function's own doc comment for the real
    // neighbour-overlap incident this guards against). Conservative on purpose: if the grown box so
    // much as touches a neighbour, fall back to the region's own box exactly as before, rather than
    // trying to trim one edge and risking a new off-by-one overlap.
    let cross_pad = ((if vertical { region.w } else { region.h }) as f32 * LAYOUT_OVERFLOW_FRACTION)
        .round() as u32;
    let grow_pad = ((if vertical { region.h } else { region.w }) as f32
        * NO_BUBBLE_LAYOUT_GROWTH_FRACTION)
        .round() as u32;
    let grown = if vertical {
        BoundingBox::new(
            region.x.saturating_sub(cross_pad),
            region.y.saturating_sub(grow_pad),
            region.w + cross_pad * 2,
            region.h + grow_pad * 2,
        )
    } else {
        BoundingBox::new(
            region.x.saturating_sub(grow_pad),
            region.y.saturating_sub(cross_pad),
            region.w + grow_pad * 2,
            region.h + cross_pad * 2,
        )
    };

    for other in other_regions {
        if other == region {
            continue;
        }
        let overlaps = other.x < grown.x + grown.w
            && grown.x < other.x + other.w
            && other.y < grown.y + grown.h
            && grown.y < other.y + other.h;
        if overlaps {
            return *region;
        }
    }
    grown
}

/// One line of a shape-aware horizontal layout: its own centre line and left edge in page
/// coordinates, plus the width of the bubble row it sits in.
#[derive(Clone)]
struct PlacedLine {
    y_centre: f32,
    x_start: f32,
    x_width: f32,
    text: String,
}

/// Horizontal counterpart of [`fit_vertical_in_shape`] — see that function and [`BubbleShape`] for
/// why a bubble's own silhouette, not its bounding rectangle, is the right layout target.
///
/// Same downward font-size search; each line is measured and centred against the balloon's own
/// horizontal extent at that line's own y, so a line crossing the widest part of the balloon can be
/// long while one near its top or bottom is short instead of sticking out of the sides.
fn fit_horizontal_in_shape(
    text: &str,
    font: &ResolvedFont<'_>,
    shape: &BubbleShape,
    upem: f32,
) -> Option<(f32, Vec<PlacedLine>)> {
    let (y_first, y_last) = shape.y_extent()?;
    let shape_h = (y_last - y_first + 1) as f32;
    let centre_y = (y_first as f32 + y_last as f32 + 1.0) / 2.0;

    let mut size = MAX_FONT_PX;
    while size >= MIN_FONT_PX {
        let line_pitch = font.raster.as_scaled(PxScale::from(size)).height() * LINE_SPACING_FACTOR;
        if line_pitch > 0.0 {
            let max_lines = (shape_h / line_pitch).floor() as usize;
            for line_count in 1..=max_lines {
                if let Some(lines) = place_lines_in_shape(
                    text, font, upem, size, shape, line_pitch, line_count, centre_y,
                ) {
                    return Some((size, lines));
                }
            }
        }
        size -= 1.0;
    }
    None
}

/// Wraps `text` into `line_count` lines, each measured against the bubble's own horizontal run at
/// that line's own y, and centred on it. Returns `None` when the text does not fit.
///
/// Unlike the vertical path this cannot split by character count — Latin and CJK advances differ —
/// so each line is filled with the longest prefix whose *shaped* advance fits its own row, using the
/// same Unicode line-break rule as every other wrap in this module. When the text turns out to need
/// fewer lines than requested, the block is re-laid-out for the count actually used rather than left
/// hanging off-centre inside the balloon (the same real page-13 problem the vertical path's balanced
/// fill solves).
#[allow(clippy::too_many_arguments)]
fn place_lines_in_shape(
    text: &str,
    font: &ResolvedFont<'_>,
    upem: f32,
    font_px: f32,
    shape: &BubbleShape,
    line_pitch: f32,
    line_count: usize,
    centre_y: f32,
) -> Option<Vec<PlacedLine>> {
    if line_count == 0 || line_pitch <= 0.0 || text.is_empty() {
        return None;
    }
    let mut count = line_count;
    loop {
        let block_top = centre_y - (count as f32 * line_pitch) / 2.0;
        let mut remaining = text;
        let mut placed: Vec<PlacedLine> = Vec::with_capacity(count);
        for index in 0..count {
            if remaining.is_empty() {
                break;
            }
            let y_centre = block_top + line_pitch * (index as f32 + 0.5);
            let (x_start, run_len) = shape.row_run(y_centre.round().max(0.0) as u32)?;
            let usable = run_len as f32 - 2.0 * HORIZONTAL_SHAPE_PADDING_FRACTION * line_pitch;
            if usable <= 0.0 {
                return None;
            }
            let first = wrap_by_break_opportunities(remaining, usable, |segment| {
                shape_line(
                    font,
                    segment,
                    harfrust::Direction::LeftToRight,
                    font_px,
                    upem,
                )
                .advance
            })
            .into_iter()
            .next()
            .unwrap_or_default();
            if first.is_empty() {
                return None;
            }
            placed.push(PlacedLine {
                y_centre,
                x_start: x_start as f32,
                x_width: run_len as f32,
                text: first.clone(),
            });
            remaining = &remaining[first.len()..];
        }
        if remaining.is_empty() && !placed.is_empty() {
            if placed.len() == count {
                return Some(placed);
            }
            // Fewer lines than asked for were needed: retry centred on that many, so each line gets
            // the width of the row it will actually occupy.
            count = placed.len();
            continue;
        }
        return None;
    }
}

/// Lays out and draws a region's translated text horizontally.
///
/// Mirrors [`draw_region_vertical`]: against the bubble's own silhouette when one is known
/// (per-line width from the balloon's real extent at that line's y), falling back to the region's
/// rectangular box otherwise or when no size fits the silhouette.
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
    shape: Option<&BubbleShape>,
) {
    let upem = font.raster.units_per_em().unwrap_or(1000.0);

    if let Some(shape) = shape {
        if let Some((font_px, lines)) = fit_horizontal_in_shape(text, font, shape, upem) {
            let px_scale = PxScale::from(font_px);
            let scaled = font.raster.as_scaled(px_scale);
            let line_pitch = scaled.height() * LINE_SPACING_FACTOR;
            for line in &lines {
                let shaped = shape_line(
                    font,
                    &line.text,
                    harfrust::Direction::LeftToRight,
                    font_px,
                    upem,
                );
                let mut pen_x = line.x_start + (line.x_width - shaped.advance).max(0.0) / 2.0;
                // Same baseline convention as the rectangular path below: the line's own cell top,
                // plus the font's ascent.
                let pen_y = line.y_centre - line_pitch / 2.0 + scaled.ascent();
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
            }
            return;
        }
    }

    let Some((font_px, lines)) = fit_text(text, font, w, h) else {
        return;
    };

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

/// The vertical grid pitch for `font_px`: exactly one em per cell.
///
/// Real manga lettering is set on a one-em square grid — that is why vertical CJK balloons look the
/// way they do — so this deliberately does *not* derive the cell from the font's own metrics.
/// Measured on the real page-13 bubble (2026-09-20): `ab_glyph`'s `height()` reports this CJK face's
/// OS/2 typo metrics (1.0em), and scaling that by the horizontal path's [`LINE_SPACING_FACTOR`]
/// (0.85) gave a 0.85em cell — *tighter* than the 0.86-0.88em ink of the glyphs it holds, so
/// adjacent characters touched and the grid stopped reading as lettering at all. A CJK em is the
/// cell; leading between lines of running prose is a different question and keeps using
/// [`LINE_SPACING_FACTOR`].
fn vertical_pitch(_font: &ResolvedFont<'_>, font_px: f32) -> f32 {
    font_px
}

/// One column of a shape-aware vertical layout: its centre line and top edge in page coordinates,
/// plus the text it holds. Neither comes from a bounding rectangle — both are derived from the
/// bubble's own silhouette (see [`fit_vertical_in_shape`]).
#[derive(Clone)]
struct PlacedColumn {
    x_centre: f32,
    y_top: f32,
    text: String,
}

/// Draws one column's glyphs as a uniform grid of `pitch`-tall cells starting at `y_top`, centred
/// on `x_centre`. Glyphs are drawn top-anchored (unlike the horizontal path's baseline), since
/// vertical metrics/baselines are far less standardised across CJK fonts than horizontal
/// ascent/descent — anchoring to the glyph's own top keeps columns visually aligned.
#[allow(clippy::too_many_arguments)]
fn draw_vertical_column(
    page: &mut RgbImage,
    font: &ResolvedFont<'_>,
    px_scale: PxScale,
    upem: f32,
    font_px: f32,
    x_centre: f32,
    y_top: f32,
    text: &str,
    pitch: f32,
    fg: Rgb,
    bold: bool,
    outline: Option<Rgb>,
) {
    let scaled = font.raster.as_scaled(px_scale);
    let shaped = shape_line(font, text, harfrust::Direction::TopToBottom, font_px, upem);
    let mut cell_top = y_top;
    for glyph in &shaped.glyphs {
        // Centre the glyph's own *ink* in its grid cell, rather than placing it from the shaper's
        // per-glyph offsets plus the font's ascent/descent. Both of those are wrong for vertical
        // CJK balloon text, and measurably so on the real page-13 bubble (2026-09-20):
        //
        // * `y_offset` + ascent/descent put every column ~a third of a cell too low, because CJK
        //   fonts ship ascent/descent sized for running prose (plus a line gap) rather than for one
        //   glyph's own ink.
        // * the previous `x_offset - h_advance/2` double-counted the shaper's own horizontal
        //   centring: a vertical run already carries `x_offset ≈ -h_advance/2`, so subtracting half
        //   an advance again moved every column half a glyph to the left — the user-reported
        //   "嵌字偏左下" (measured: intended column centre x=176.8, drawn ink centre x=152).
        //
        // Centring the outline's own ink box fixes both axes by construction, and makes the grid
        // self-consistent: every glyph fills its cell the same way regardless of what the shaper
        // chose to report.
        let (left, right, above, below) = glyph_ink_box(font, glyph.glyph_id, font_px).unwrap_or((
            0.0,
            0.0,
            scaled.ascent(),
            scaled.descent().abs(),
        ));
        let pen_x = x_centre - (left + right) / 2.0;
        let baseline = cell_top + pitch / 2.0 + (above - below) / 2.0;
        draw_glyph(
            page,
            font,
            glyph.glyph_id,
            px_scale,
            pen_x,
            baseline,
            fg,
            bold,
            outline,
        );
        cell_top += pitch;
    }
}

/// One glyph's own ink box at `font_px`, relative to the glyph's origin, as
/// `(left, right, above_baseline, below_baseline)` in page pixels: the ink occupies
/// `x ∈ [origin_x + left, origin_x + right]` and `y ∈ [baseline - above, baseline + below]`.
///
/// Read straight off the glyph's own outline rather than derived from the font's ascent/descent (or
/// the shaper's offsets) — see `draw_vertical_column`'s own comment for the two real placement bugs
/// that approximation caused.
fn glyph_ink_box(
    font: &ResolvedFont<'_>,
    glyph_id: u32,
    font_px: f32,
) -> Option<(f32, f32, f32, f32)> {
    let upem = font.raster.units_per_em().unwrap_or(1000.0);
    let bounds = font
        .raster
        .outline(ab_glyph::GlyphId(glyph_id as u16))?
        .bounds;
    let scale = font_px / upem;
    Some((
        bounds.min.x * scale,
        bounds.max.x * scale,
        bounds.max.y * scale,
        -bounds.min.y * scale,
    ))
}

/// Lays out and draws a region's translated text vertically.
///
/// Two layouts, in order of preference:
/// 1. **Against the bubble's own silhouette** ([`BubbleShape`], when the region matched one) — every
///    column measured and centred against the balloon's real extent at that column's own x, so a
///    column near the widest part of the balloon is tall and one near its edge is short. This is
///    what stops lettering from running out of the top and bottom of an elliptical balloon.
/// 2. **Against the region's rectangular box** — the older path, kept for regions with no matched
///    bubble and as the fallback when a silhouette genuinely cannot hold the text at any legible
///    size.
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
    shape: Option<&BubbleShape>,
) {
    let upem = font.raster.units_per_em().unwrap_or(1000.0);

    if let Some(shape) = shape {
        if let Some((font_px, columns)) = fit_vertical_in_shape(text, font, shape, upem) {
            let px_scale = PxScale::from(font_px);
            let pitch = vertical_pitch(font, font_px);
            for column in &columns {
                draw_vertical_column(
                    page,
                    font,
                    px_scale,
                    upem,
                    font_px,
                    column.x_centre,
                    column.y_top,
                    &column.text,
                    pitch,
                    fg,
                    bold,
                    outline,
                );
            }
            return;
        }
    }

    let Some((font_px, columns)) = fit_text_vertical(text, font, w, h) else {
        return;
    };
    let px_scale = PxScale::from(font_px);
    let pitch = vertical_pitch(font, font_px);
    let total_width = pitch * columns.len() as f32;
    // Horizontally centre the block of columns within the region, then lay columns out
    // right-to-left starting from that block's right edge.
    let block_right = bbox.x as f32 + (w as f32 + total_width).max(0.0) / 2.0;

    for (index, column) in columns.iter().enumerate() {
        let x_centre = block_right - pitch * (index as f32 + 0.5);
        // Vertically centre each column — mirrors how a horizontal line is centred.
        let height = pitch * column.chars().count() as f32;
        let y_top = bbox.y as f32 + (h as f32 - height).max(0.0) / 2.0;
        draw_vertical_column(
            page, font, px_scale, upem, font_px, x_centre, y_top, column, pitch, fg, bold, outline,
        );
    }
}

/// Shape-aware vertical lettering — see [`draw_region_vertical`]'s own doc comment for why this
/// exists. Searches font size downward exactly like [`fit_text_vertical`], and reuses that
/// function's short-text single-column preference (a 2x2 split of a four-character phrase reads
/// scrambled — see [`SHORT_VERTICAL_MAX_CHARS`]).
///
/// The one thing it does differently is that a column's capacity is its own real extent inside the
/// balloon (`BubbleShape::column_run`) rather than a height shared by every column.
fn fit_vertical_in_shape(
    text: &str,
    font: &ResolvedFont<'_>,
    shape: &BubbleShape,
    _upem: f32,
) -> Option<(f32, Vec<PlacedColumn>)> {
    let (x_first, x_last) = shape.x_extent()?;
    let shape_w = (x_last - x_first + 1) as f32;
    let centre_x = (x_first as f32 + x_last as f32 + 1.0) / 2.0;
    let prefer_single_column = text.chars().count() <= SHORT_VERTICAL_MAX_CHARS;

    let mut single_column: Option<(f32, Vec<PlacedColumn>)> = None;
    let mut multi_column: Option<(f32, Vec<PlacedColumn>)> = None;

    let mut size = MAX_FONT_PX;
    while size >= MIN_FONT_PX {
        let pitch = vertical_pitch(font, size);
        if pitch > 0.0 {
            let max_columns = (shape_w / pitch).floor() as usize;
            // Fewest columns first: for one font size, fewer columns also means the block sits
            // nearer the middle of the balloon, where its own columns are taller.
            let mut fitted = None;
            for column_count in 1..=max_columns {
                if let Some(columns) =
                    place_columns_in_shape(text, shape, pitch, column_count, centre_x)
                {
                    fitted = Some((column_count, columns));
                    break;
                }
            }
            if let Some((column_count, columns)) = fitted {
                // Same shape as `fit_text_vertical`'s own search: a long translation has to wrap,
                // so the first fit found (largest size) wins outright; only a *short* one is
                // weighed between a single column and a multi-column split.
                if !prefer_single_column {
                    return Some((size, columns));
                }
                if column_count == 1 {
                    single_column = Some((size, columns));
                    break;
                }
                multi_column.get_or_insert((size, columns));
            }
        }
        size -= 1.0;
    }

    // Same rule and same real incident as `fit_text_vertical`'s own comparison: a multi-column
    // layout only beats a single column when every one of its columns holds enough characters to
    // read as a coherent fragment on its own (a column of two reads as scrambled).
    if let (Some(_), Some((multi_size, multi_columns))) = (&single_column, &multi_column) {
        let min_column_chars = multi_columns
            .iter()
            .map(|c| c.text.chars().count())
            .min()
            .unwrap_or(0);
        if min_column_chars >= MIN_COLUMN_CHARS_FOR_MULTI_COLUMN {
            return Some((*multi_size, multi_columns.clone()));
        }
    }
    if let Some(result) = single_column {
        return Some(result);
    }
    if let Some(result) = multi_column {
        return Some(result);
    }
    None
}

/// Packs `text` into exactly `column_count` columns, filling them right-to-left (the same reading
/// order [`draw_region_vertical`] lays its own columns out in), each column taking as much of the
/// remaining text as the bubble's own vertical extent at that column's x allows.
///
/// The split is *balanced* rather than greedy-to-capacity. Real page-13 case (2026-09-20): a 12
/// character translation in a balloon whose three centred columns can hold 6/6/4 characters came out
/// 6/6/0 under a greedy fill — the text needed only two columns' worth of room but the block was
/// laid out for three, so it sat visibly off-centre to the right of the balloon. Splitting 4/4/4
/// both fills the balloon and reads better.
///
/// Returns `None` when the text does not fit — the caller then tries a smaller font, or more
/// columns, or falls back to the rectangular layout rather than drawing outside the balloon.
fn place_columns_in_shape(
    text: &str,
    shape: &BubbleShape,
    pitch: f32,
    column_count: usize,
    centre_x: f32,
) -> Option<Vec<PlacedColumn>> {
    if column_count == 0 || pitch <= 0.0 || text.is_empty() {
        return None;
    }
    let block_left = centre_x - (column_count as f32 * pitch) / 2.0;
    let padding = pitch * VERTICAL_SHAPE_PADDING_FRACTION;

    // Measure every column up front: the balanced fill below has to know how much room the columns
    // *after* this one still have, to avoid leaving them more text than they can take.
    let mut geometry: Vec<(f32, u32, u32, usize)> = Vec::with_capacity(column_count);
    for index in 0..column_count {
        // Column 0 is the *rightmost* — right-to-left reading order.
        let x_centre = block_left + pitch * (column_count as f32 - 0.5 - index as f32);
        let x = x_centre.round().max(0.0) as u32;
        let (y_start, run_len) = shape.column_run(x)?;
        let usable = run_len as f32 - 2.0 * padding;
        if usable < pitch {
            return None;
        }
        let capacity = (usable / pitch).floor() as usize;
        if capacity == 0 {
            return None;
        }
        geometry.push((x_centre, y_start, run_len, capacity));
    }

    // Reject a candidate column layout in which any column's own run is far shorter than the
    // longest one: such a column is a narrow neck or a speck in the shape, not a text column. Real
    // measured incident (2026-09-20, page 13,「は？ちょ待…」): the page-derived interior had a
    // 54px-tall neck sitting *between* the balloon's two real columns (runs 108px and 164px), and a
    // 3-column layout allocated「等」to it. Because that neck's run also *starts* ~110px below its
    // neighbours', the block's shared-top interval came out empty, it fell back to per-column
    // centring, and the glyph was stranded at the bottom under a large blank. Rejecting the layout
    // makes the caller try fewer columns, which drops the neck entirely.
    //
    // Deliberately a *layout-only* rule: it never touches the erase mask. The first attempt at this
    // fix (a morphological opening on the shape) cut the neck but also destroyed the thin near-white
    // web *between glyph strokes* on other balloons, so their erasure stopped working and the
    // translation was drawn straight over the original Japanese.
    const MIN_COLUMN_RUN_FRACTION: f32 = 0.5;
    let longest_run = geometry.iter().map(|g| g.2).max().unwrap_or(0) as f32;
    if longest_run > 0.0
        && geometry
            .iter()
            .any(|g| (g.2 as f32) < longest_run * MIN_COLUMN_RUN_FRACTION)
    {
        return None;
    }

    let text_chars = text.chars().count();
    if geometry.iter().map(|g| g.3).sum::<usize>() < text_chars {
        return None;
    }

    let mut remaining = text;
    let mut placed = Vec::with_capacity(column_count);
    for (index, &(x_centre, y_start, run_len, capacity)) in geometry.iter().enumerate() {
        let rest_capacity: usize = geometry[index + 1..].iter().map(|g| g.3).sum();
        let left = remaining.chars().count();
        // Even share of what is left, never more than this column can take, and never so little
        // that the columns after it are left with more than they can hold.
        let want = left
            .div_ceil(column_count - index)
            .max(left.saturating_sub(rest_capacity))
            .min(capacity);
        let take = take_prefix_fitting(remaining, want);
        let chars = take.chars().count();
        if chars == 0 || chars > capacity {
            return None;
        }
        placed.push(PlacedColumn {
            x_centre,
            y_top: y_start as f32 + (run_len as f32 - chars as f32 * pitch) / 2.0,
            text: take.to_string(),
        });
        remaining = &remaining[take.len()..];
    }

    if !remaining.is_empty() {
        return None;
    }

    // Vertical CJK sets every column from a *shared* top edge — a shorter column simply ends higher
    // — but the columns' own runs inside a balloon are not aligned with each other (real page-13
    // balloon: a slanted quadrilateral, the left column's run starting ~60px lower than the right
    // column's). The shared top therefore has to satisfy every column at once:
    //
    //     lower = max_i(start_i)   <=   T   <=   min_i(end_i - h_i) = upper
    //
    // and belongs in the middle of that feasible interval. An earlier attempt took the
    // *intersection* of the runs instead, which is the same thing only when every column is the same
    // height — on that slanted balloon it collapsed to a short strip near the bottom and pushed the
    // whole block down under a large blank (reported against the「は？ちょ待…」balloon). When the
    // interval is empty the shape genuinely cannot carry a common top, so fall back to centring each
    // column in its own run.
    let lower = geometry.iter().map(|g| g.1 as f32).fold(0.0f32, f32::max);
    let upper = geometry
        .iter()
        .zip(placed.iter())
        .map(|(g, column)| (g.1 + g.2) as f32 - column.text.chars().count() as f32 * pitch)
        .fold(f32::INFINITY, f32::min);
    if lower <= upper {
        let top = lower + (upper - lower) / 2.0;
        for column in &mut placed {
            column.y_top = top;
        }
    }

    Some(placed)
}

/// The longest prefix of `text` that fits `max_chars` characters, preferring to stop at a real
/// Unicode line-break opportunity — so a column never splits a word, and never starts with closing
/// punctuation — and hard-splitting by character only when there is no earlier opportunity at all,
/// the same rule [`wrap_by_break_opportunities`] already applies when it packs a whole string.
fn take_prefix_fitting(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    if text.chars().count() <= max_chars {
        return text;
    }
    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());

    let segmenter = icu_segmenter::LineSegmenter::new_auto(Default::default());
    let mut cut = 0usize;
    for boundary in segmenter.segment_str(text) {
        if boundary == 0 {
            continue;
        }
        if boundary > byte_limit {
            break;
        }
        cut = boundary;
    }
    if cut == 0 {
        // No break opportunity inside the limit: this is one unbreakable run, so take the hard
        // split rather than returning an empty column (which would spin the caller's loop).
        cut = byte_limit;
    }
    &text[..cut]
}

/// Fraction of the glyph's own font size used as the outline stroke's radius (in pixels) — same
/// technique subtitle rendering commonly uses in place of a true stroked-path fill, built on the
/// same per-pixel `coverage` callback `ab_glyph::OutlineCurveBuilder` already drives everything
/// else in this file through. Scales with font size rather than a fixed radius: a real reported
/// incident (2026-09-17, screenshot comparison against the actual rendered page) found the
/// previous fixed-1px ring effectively invisible once font size grew past roughly 20px (this
/// module's real range runs up to `MAX_FONT_PX = 72`) — a 1px stroke reads as "barely thicker
/// anti-aliasing," not a contrasting edge, at those sizes. `manga-image-translator`'s own renderer
/// scales its stroke width with font size for the same reason (its `stroke_width_multiplier`,
/// typically ~5% of font size) rather than using a fixed pixel width.
const OUTLINE_STROKE_FRACTION: f32 = 0.06;

/// Floor on the outline radius computed from [`OUTLINE_STROKE_FRACTION`], so even the smallest
/// rendered text still gets a visible ring rather than rounding down to 0px.
const MIN_OUTLINE_RADIUS_PX: i32 = 1;

/// Every integer-pixel offset within `radius` of the origin (a filled disc, not just its own
/// boundary ring) — necessary because drawing only the *outer* ring at a given radius leaves gaps
/// between the glyph's own fill and that ring once the radius exceeds a couple of pixels: the
/// previous implementation drew exactly one ring at a fixed 1px offset, which is exactly why
/// widening that single ring's own radius alone (without filling the pixels in between) would
/// have produced a hollow halo instead of a solid contrasting edge.
fn outline_stroke_offsets(radius: i32) -> Vec<(i32, i32)> {
    let mut offsets = Vec::new();
    let r_sq = radius * radius;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            if dx * dx + dy * dy <= r_sq {
                offsets.push((dx, dy));
            }
        }
    }
    offsets
}

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
        let radius =
            ((scale.y * OUTLINE_STROKE_FRACTION).round() as i32).max(MIN_OUTLINE_RADIUS_PX);
        let offsets = outline_stroke_offsets(radius);
        glyph_outline.draw(|gx, gy, coverage| {
            if coverage <= 0.01 {
                return;
            }
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            for &(dx, dy) in &offsets {
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
        // Confirmed live: without this, the vertical advance came back consistently negative
        // (e.g. -504 for a 7-character run at 72px), so every "does this fit" comparison against a
        // positive `max_extent` trivially passed and `wrap_by_break_opportunities` never broke a
        // single vertical column.
        //
        // Note this value feeds `ShapedLine::advance` (what the *rectangular* fit search
        // measures); the glyphs inside a column are actually drawn on the uniform `vertical_pitch`
        // grid instead — see that function's own doc comment for the measured reason.
        let y_advance = -(pos.y_advance as f32) * scale;
        glyphs.push(ShapedGlyph {
            glyph_id: info.glyph_id,
            x_advance,
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

    // Always search downward from `MAX_FONT_PX`, never from a box-derived guess (`box_h * 0.5`,
    // as an earlier revision did) — real reported incident (2026-09-16): that guess is a poor
    // predictor of the actual best-fit size for a short/wide box (e.g. a two-line label whose
    // true best-fit size is ~65px comfortably fits `box_h=88`, but `box_h * 0.5 = 44px` was
    // picked as the *starting* size and the loop below only ever shrinks from its start, so it
    // could never discover that a larger size also fits), leaving text rendered visibly smaller
    // than the box allows with a large, asymmetric empty margin — this is what was reported as
    // translated text looking "shifted" within its own box, though the text itself was correctly
    // centred at whatever (too-small) size `fit_text` had settled on; the actual bug was the
    // size search never climbing back up.
    let mut size = MAX_FONT_PX;

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
/// top-to-bottom — except that a translation short enough to fit a *single* column is never split
/// into several just to win a larger font size (see [`SHORT_VERTICAL_MAX_CHARS`]).
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
    // Same fix and same reasoning as `fit_text`'s own doc comment on this exact line — always
    // search downward from `MAX_FONT_PX`, never from a box-derived guess that the loop below can
    // never climb back up past.
    let mut size = MAX_FONT_PX;

    // Only a short translation gets the single-column preference below; a genuinely long one has
    // to wrap no matter what, and forcing it into one column would shrink it drastically.
    let prefer_single_column = text.chars().count() <= SHORT_VERTICAL_MAX_CHARS;
    let mut multi_column_fallback: Option<(f32, Vec<String>)> = None;
    let mut single_column_result: Option<(f32, Vec<String>)> = None;

    while size >= MIN_FONT_PX {
        let pitch = vertical_pitch(font, size);
        let columns = wrap_text_vertical_grid(text, usable_h, pitch);
        let total_width = pitch * columns.len() as f32;

        // Measured against the same uniform grid `draw_region_vertical` actually draws on — not
        // the shaper's own per-glyph advance, which is much looser than that grid (see
        // `vertical_pitch`'s own doc comment) and would report a column as over-long that the draw
        // pass then fits comfortably.
        let all_fit = columns
            .iter()
            .all(|c| c.chars().count() as f32 * pitch <= usable_h);
        if total_width <= box_w as f32 && all_fit {
            if !prefer_single_column {
                return Some((size, columns));
            }
            if columns.len() == 1 {
                // Found the largest size that fits in one column — but don't return it yet: a
                // multi-column fallback may already be sitting on a meaningfully larger size (see
                // below), and a short text with a *severe* single-column font-size penalty reads
                // worse than a short multi-column split does. Keep searching for a moment longer
                // only if no multi-column candidate has been seen yet; otherwise the two are
                // already comparable and we can decide immediately.
                single_column_result = Some((size, columns));
                break;
            }
            // A short text that still wrapped: remember the *first* (largest-size) multi-column
            // candidate seen — this is the best multi-column reference point to compare the
            // eventual single-column size against.
            multi_column_fallback.get_or_insert((size, columns));
        }
        size -= 1.0;
    }

    if let (Some(_), Some((multi_size, multi_cols))) =
        (&single_column_result, &multi_column_fallback)
    {
        // Real reported incident (2026-09-18), found *after* a first attempt at this fix (compare
        // font-size loss between the two layouts) broke the original "欢迎回来" regression test —
        // font-size loss is the wrong axis entirely. "欢迎回来" (4 chars) splits into 2 columns of
        // 2 at a *larger* size than its single-column layout, which by pure size comparison looks
        // just like "回错家了吗？" (6 chars) splitting into 2 columns of 3 — but the two read
        // completely differently: a column of only 2 characters reads as a disorienting fragment
        // (this is the original "回欢来迎" scrambled-reading incident), while a column of 3+
        // reads as ordinary vertical CJK text (top-to-bottom within the column, right-to-left
        // across columns — the same convention real vertical typesetting already uses). The axis
        // that actually matters is *how few characters land in the thinnest column*, not how many
        // pixels of font size get traded away. Only reject the single-column layout in favour of
        // the multi-column one when every column in that multi-column layout has enough
        // characters to read as a coherent fragment on its own.
        let min_column_chars = multi_cols
            .iter()
            .map(|c| c.chars().count())
            .min()
            .unwrap_or(0);
        if min_column_chars >= MIN_COLUMN_CHARS_FOR_MULTI_COLUMN {
            return Some((*multi_size, multi_cols.clone()));
        }
    }
    if let Some(result) = single_column_result {
        return Some(result);
    }
    if let Some(fallback) = multi_column_fallback {
        return Some(fallback);
    }

    Some((
        MIN_FONT_PX,
        wrap_text_vertical_grid(text, usable_h, vertical_pitch(font, MIN_FONT_PX)),
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

/// Greedy vertical wrap, measuring a column by the uniform grid pitch the draw pass actually uses
/// ([`vertical_pitch`]) rather than the shaper's own per-glyph advance — the two disagree by a wide
/// margin, and a wrap computed against the wrong one either overflows the box or leaves empty cells
/// at the bottom of every column.
fn wrap_text_vertical_grid(text: &str, max_h: f32, pitch: f32) -> Vec<String> {
    wrap_by_break_opportunities(text, max_h, |segment| {
        segment.chars().count() as f32 * pitch
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

        if measure(candidate) <= max_extent {
            candidate_start = candidate_end;
            continue;
        }
        if candidate_start == line_start {
            // No earlier break opportunity accepted yet — this whole break-to-break run (which may
            // be the entire remaining text, e.g. one unbreakable word with no internal break
            // opportunities at all) is itself over-long. Real bug found 2026-09-17 (previously
            // masked by the container test image having no CJK fonts installed at all, so every
            // test needing a real font silently short-circuited via `load_test_font`'s own `None`
            // path instead of ever actually running this code): the old code unconditionally
            // `continue`d here without ever hard-splitting, so a single unbreakable long word (e.g.
            // "supercalifragilisticexpialidocious") was never broken at all — it fell through to
            // the final `lines.push(text[line_start..])` below as one single over-long line. Fall
            // through to the same hard-split-by-grapheme logic the normal case already uses,
            // instead of skipping it.
        } else {
            // The segment up to (but not including) this break opportunity is the longest that
            // still fits; commit it and start the next line from there.
            lines.push(text[line_start..candidate_start].to_string());
            line_start = candidate_start;
        }
        candidate_start = candidate_end;

        // Even a single break-to-break run may itself be over-long (one very wide character, or a
        // long run with no earlier break opportunity) — hard-split it by grapheme cluster.
        if measure(&text[line_start..candidate_end]) > max_extent {
            let mut chunk_start = line_start;
            let mut probe = line_start;
            for (offset, ch) in text[line_start..candidate_end].char_indices() {
                let next = line_start + offset + ch.len_utf8();
                // Real bug found 2026-09-17 (masked until now by the test container having no CJK
                // fonts installed, so `measure` was never actually exercised with real widths): the
                // old second condition `next > probe + ch.len_utf8()` is algebraically always false
                // for the very first character that pushes a chunk over `max_extent` (`next` and
                // `probe + ch.len_utf8()` are the same value in that case — `probe` is always
                // `chunk_start`'s or the previous char's own end), so the chunk boundary the first
                // `if` clause detects was never actually committed; it silently fell through to
                // `probe = next` and kept accumulating past `max_extent` indefinitely. The intended
                // guard was "don't split before we've accepted at least one character into this
                // chunk" (so a single character wider than `max_extent` on its own still gets its
                // own line rather than an infinite empty split) — `chunk_start != probe` expresses
                // that directly instead of the always-false arithmetic comparison.
                if measure(&text[chunk_start..next]) > max_extent && chunk_start != probe {
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
    /// bubbles)`. No test here exercises a real `Inpainter`/RPC-forwarded erase (GPU EP integration
    /// plan v9 moved that behind a subprocess this crate has no dependency on), so the erase is
    /// simulated as a no-op "success" (a plain clone of the page): it reports the whole-page erase
    /// call as having succeeded without itself changing any pixels, which is what these drawing
    /// tests need — since 2026-09-17 a *failed* erase (`erased: None`) draws nothing at all, so
    /// passing `None` here would make every drawing assertion below pass vacuously.
    /// `erase_failed_for_test` is the helper for tests that specifically want the failure path.
    fn composite_page_for_test(
        page: &mut RgbImage,
        regions: &[DetectedTextRegion],
        fonts: &FontSet<'_>,
        bubbles: Option<&[DetectedBubble]>,
    ) -> Result<(), CompositeError> {
        let plan = prepare_page_erase(page, regions, bubbles);
        let erased = page.clone();
        finish_composite_page(page, plan, fonts, Some(erased))
    }

    /// Composites with the whole-page erase reported as failed/unavailable (`erased: None`).
    fn erase_failed_for_test(
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

    /// A short vertical translation stays in one column instead of being split into several
    /// just to win a larger font size — see `SHORT_VERTICAL_MAX_CHARS` for the real incident
    /// (a 4-character translation rendered as a 2x2 block that read scrambled).
    #[test]
    fn a_short_vertical_translation_is_not_split_into_columns() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        let font = ResolvedFont::load(fonts.resolve(None)).unwrap();

        // The real reported region: OCR bbox 93x199 grown by `LAYOUT_OVERFLOW_FRACTION` to
        // width 121, with the height clamped back to the original 199 (`composite_page`'s own
        // no-matched-bubble clamp), translated text "欢迎回来".
        let (size, columns) = fit_text_vertical("欢迎回来", &font, 121, 199)
            .expect("a non-empty box always yields a layout");
        assert_eq!(
            columns.len(),
            1,
            "a 4-character translation must stay in one column (got {columns:?} at {size}px)"
        );
        assert_eq!(columns[0], "欢迎回来");
        assert!(size >= MIN_FONT_PX);
    }

    /// The single-column preference above is bounded: a translation too long for one column
    /// still wraps rather than shrinking drastically to avoid wrapping.
    #[test]
    fn a_long_vertical_translation_still_wraps_into_columns() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        let font = ResolvedFont::load(fonts.resolve(None)).unwrap();

        let long = "不过刚才他好像说了奥尔玛这个名字对吧";
        assert!(long.chars().count() > SHORT_VERTICAL_MAX_CHARS);
        let (_, columns) = fit_text_vertical(long, &font, 166, 236)
            .expect("a non-empty box always yields a layout");
        assert!(
            columns.len() > 1,
            "a long translation must still wrap into columns, got {columns:?}"
        );
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
    fn nothing_is_drawn_or_filled_anywhere_when_the_whole_page_erase_fails() {
        // Real reported incident (2026-09-17, screenshot comparison): erase and text-draw must
        // never happen independently per region. The draw loop used to run unconditionally
        // regardless of whether a region's own backdrop had actually been handled, so a region
        // under a failed whole-page erase got its original Japanese left completely untouched *and*
        // the translated text drawn directly on top of it — original and translation visibly
        // overlapping.
        //
        // Second reported incident on the same page (2026-09-17): the `erased: None` path used to
        // flat-fill `bounding_box` with `bg_color` and count that as "handled", which rendered a
        // hard-edged rectangle block of flat colour with the translation on top. Both regions below
        // therefore assert the same thing — a failed whole-page erase must leave the page
        // byte-for-byte pristine — but they cover the two distinct sub-cases: `bg_color: None`
        // (never filled even before, the overlap bug) and `bg_color: Some` + a full colour pair so
        // `has_precise_mask` is true (the region the removed flat-fill *did* paint a rectangle on).
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);

        let no_bg = region(Some("こんにちは"), BoundingBox::new(10, 10, 150, 60));

        let mut with_bg = region(Some("こんにちは"), BoundingBox::new(10, 100, 150, 60));
        with_bg.bg_color = Some(Rgb {
            r: 250,
            g: 250,
            b: 250,
        });
        with_bg.fg_color = Some(Rgb {
            r: 20,
            g: 20,
            b: 20,
        });

        let mut page = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        let before = page.clone();
        erase_failed_for_test(&mut page, &[no_bg, with_bg], &fonts, None).unwrap();

        assert_eq!(
            page, before,
            "a failed whole-page erase must leave the page completely untouched — no translated \
             text drawn over an un-erased backdrop, and no flat-colour rectangle painted either"
        );
    }

    #[test]
    fn a_region_with_a_precise_mask_is_still_drawn_when_the_whole_page_erase_succeeds() {
        // Counterpart to the failed-erase test above: removing the flat-fill fallback must not have
        // turned the guard into "never draw anything". A region with a full colour pair (so
        // `combined_erase_mask`'s stroke branch builds a real mask and `has_precise_mask` is true)
        // under a successful whole-page erase is exactly the case that *should* still render, and
        // this asserts the page actually changes.
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        let mut r = region(Some("こんにちは"), BoundingBox::new(10, 10, 150, 60));
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

        let mut page = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 120]));
        let before = page.clone();
        composite_page_for_test(&mut page, &[r], &fonts, None).unwrap();

        assert_ne!(
            page, before,
            "a region with a precise erase mask under a successful whole-page erase must still \
             have its translation drawn"
        );
    }

    #[test]
    fn a_region_with_no_precise_mask_is_skipped_even_when_the_whole_page_erase_succeeds() {
        // Real reported incident (2026-09-17, this exact case — found only after the first version
        // of the fix above still reproduced a real user-triggered "おかえりなさいませ" screenshot: a
        // successful whole-page LaMa erase (`erased: Some(_)`) does NOT mean every region's own
        // backdrop was actually touched — `page_model_mask`/`page_paste_mask` (what LaMa actually
        // repaints) are built strictly from each region's own `has_precise_mask` result
        // (`prepare_page_erase`'s loop). A region whose own `combined_erase_mask` returns `None`
        // (confirmed live: a short caption with no `bg_color`/`fg_color` colour pair and no
        // `raw_text_mask` for DenseCRF to fall back on) contributes zero pixels to either page mask,
        // so the whole-page erase's own overall success is irrelevant to *this* region — it must
        // still be skipped, exactly like the `erased: None` case above, rather than assumed handled
        // just because the page as a whole erased successfully.
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        // `region()`'s own defaults: `bg_color`/`fg_color`/`raw_text_mask` all `None` — guarantees
        // `combined_erase_mask` returns `(None, None)` (its `stroke` branch needs both colours;
        // its DenseCRF fallback needs `raw_text_mask`), so `has_precise_mask` is `false` for this
        // region regardless of whether the whole-page erase itself succeeds.
        let r = region(Some("こんにちは"), BoundingBox::new(10, 10, 150, 60));

        let mut page = RgbImage::from_pixel(200, 200, ImageRgb([120, 120, 130]));
        let before = page.clone();
        let plan = prepare_page_erase(&page, std::slice::from_ref(&r), None);
        // A real whole-page erase would ordinarily change pixels; a plain clone (no-op "success")
        // is enough here since the point under test is purely whether this *specific* region gets
        // skipped, not whether the erase itself visibly changed the page.
        let erased = page.clone();
        finish_composite_page(&mut page, plan, &fonts, Some(erased)).unwrap();

        assert_eq!(
            page, before,
            "a region with no precise erase mask of its own must not have translated text drawn \
             over it, even when the whole-page erase call succeeded overall"
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

        // `bg_color`/`fg_color`: Some (not `region()`'s own `None` defaults) so `has_precise_mask`
        // is true (`combined_erase_mask`'s `stroke` branch needs both to build a mask at all) and
        // `backdrop_handled` lets this region through under the simulated successful erase
        // `composite_page_for_test` supplies — this test is about the *drawing* logic, not the
        // erase-skip guard (`nothing_is_drawn_or_filled_anywhere_when_the_whole_page_erase_fails`
        // covers that), so it needs a region the guard actually lets through.
        let mut r = region(Some("Hello"), BoundingBox::new(10, 10, 150, 60));
        r.bg_color = Some(Rgb {
            r: 200,
            g: 200,
            b: 200,
        });
        r.fg_color = Some(Rgb {
            r: 20,
            g: 20,
            b: 20,
        });

        composite_page_for_test(&mut page, &[r], &fonts, None).unwrap();

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
            "なにかありました?",
            None,
        ));
    }

    #[test]
    fn a_tall_narrow_box_with_latin_text_stays_horizontal() {
        // Aspect ratio alone isn't enough — a tall/narrow box whose text isn't CJK (e.g. an
        // English translation, or a vertically-stacked English sound effect) must not be forced
        // into vertical layout, even if the box shape alone would suggest it.
        assert!(!is_vertical_bubble(
            &BoundingBox::new(0, 0, 50, 200),
            "BOOM",
            None,
        ));
    }

    #[test]
    fn a_wide_box_with_cjk_text_stays_horizontal() {
        // CJK script alone isn't enough either — a wide horizontal dialogue bubble is common even
        // for CJK text.
        assert!(!is_vertical_bubble(
            &BoundingBox::new(0, 0, 300, 80),
            "なにかありました?",
            None,
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
            !is_vertical_bubble(&BoundingBox::new(0, 0, 50, 200), "Are you okay?", None),
            "an English translation must stay horizontal even in a tall/narrow box, regardless \
             of what script the original Japanese source text used"
        );
    }

    #[test]
    fn persisted_vertical_direction_overrides_a_wide_box_and_latin_text() {
        // Principle: original vertical -> translation vertical. The measured direction is not a
        // guess, so it wins even where the legacy aspect/script heuristic would refuse.
        assert!(is_vertical_bubble(
            &BoundingBox::new(0, 0, 300, 80),
            "Hello",
            Some(WritingDirection::Vertical),
        ));
    }

    #[test]
    fn persisted_horizontal_direction_overrides_a_tall_cjk_box() {
        // The 2-row horizontal label from page 15 has a tall-ish merged bbox; without this it
        // would be forced into vertical columns because the legacy fallback only looks at shape.
        assert!(!is_vertical_bubble(
            &BoundingBox::new(0, 0, 90, 130),
            "戰士母娘",
            Some(WritingDirection::Horizontal),
        ));
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
        // `bg_color`/`fg_color`: Some (not `region()`'s own `None` defaults) so `has_precise_mask`
        // is true and `backdrop_handled` lets this region through under the simulated successful
        // erase — see `a_translated_region_changes_the_page`'s own comment on why both, not just
        // `bg_color`, are needed.
        let mut r = region(Some("有什么事吗"), BoundingBox::new(20, 20, 40, 250));
        r.source_text = "なにかありました?".into();
        r.bg_color = Some(Rgb {
            r: 200,
            g: 200,
            b: 200,
        });
        r.fg_color = Some(Rgb {
            r: 20,
            g: 20,
            b: 20,
        });

        composite_page_for_test(&mut page, &[r], &fonts, None).unwrap();

        assert_ne!(
            page, before,
            "a vertical-layout region must still render something"
        );
    }

    #[test]
    fn a_bubble_is_safe_to_wipe_only_when_all_its_dark_pixels_are_translated() {
        // 60x60 white page with a dark 30x30 lettering block at (15,15), inside a 50x50 bubble
        // mask at (5,5).
        let mut page = RgbImage::from_pixel(60, 60, ImageRgb([250, 250, 250]));
        for y in 15..45 {
            for x in 15..45 {
                page.put_pixel(x, y, ImageRgb([20, 20, 20]));
            }
        }
        let bubble = DetectedBubble {
            bbox: BoundingBox::new(5, 5, 50, 50),
            confidence: 0.9,
            mask: vec![true; 50 * 50],
        };

        // A translated region covering the whole dark block → whole-bubble wipe is safe.
        let covering = region(Some("译文"), BoundingBox::new(15, 15, 30, 30));
        assert!(bubble_is_fully_translated(&page, &bubble, &[&covering]));

        // A region covering only half the block → not safe (there is untranslated lettering left).
        let partial = region(Some("译文"), BoundingBox::new(15, 15, 15, 30));
        assert!(!bubble_is_fully_translated(&page, &bubble, &[&partial]));

        // A bubble with no dark pixels at all is trivially safe.
        let empty = RgbImage::from_pixel(60, 60, ImageRgb([250, 250, 250]));
        assert!(bubble_is_fully_translated(&empty, &bubble, &[]));
    }

    #[test]
    fn a_whole_bubble_wipe_never_reaches_artwork_outside_the_bubble() {
        // Region-frame 100x20 mask: the "bubble" is the 20x20 block at the left, and the region's
        // own detected stroke mask fires across the *entire* frame — the shape of the real
        // page-13 region, whose OCR box reaches ~180px above its bubble onto the character's dark
        // clothing, which the DenseCRF stroke mask legitimately also marks as ink. Unioning that
        // whole-region stroke mask (what this test used to be impossible to write against, because
        // the code did exactly that) painted the bubble's flat backdrop over that clothing.
        let (w, h) = (100u32, 20u32);
        let mut bubble = vec![false; (w * h) as usize];
        for y in 0..h {
            for x in 0..20u32 {
                bubble[(y * w + x) as usize] = true;
            }
        }
        let stroke = vec![true; (w * h) as usize];

        let erase = whole_bubble_erase_mask(&bubble, Some((&stroke, &stroke)), w, h);

        assert_eq!(erase.model, erase.paste);
        // The bubble's own pixels are erased …
        assert!(erase.paste[(10 * w) as usize]);
        // … a stroke pixel just outside it is too, via the halo (`BUBBLE_ERASE_HALO_RADIUS` = 4, so
        // the bubble's own x=19 edge reaches x=23) …
        assert!(erase.paste[(10 * w + 22) as usize]);
        // … but a stroke pixel far from the bubble never is, however confident the stroke mask is.
        assert!(
            !erase.paste[(10 * w + 90) as usize],
            "a whole-region stroke mask must not be unioned into a whole-bubble wipe"
        );
        let row = 10 * w as usize;
        assert_eq!(
            erase.paste[row..row + w as usize]
                .iter()
                .filter(|&&m| m)
                .count(),
            20 + BUBBLE_ERASE_HALO_RADIUS as usize
        );
    }

    #[test]
    fn a_whole_bubble_wipe_without_a_usable_stroke_mask_still_erases_the_bubble() {
        let (w, h) = (10u32, 10u32);
        let bubble = vec![true; (w * h) as usize];
        let mismatched = vec![true; 7];

        for stroke in [None, Some((mismatched.as_slice(), mismatched.as_slice()))] {
            let erase = whole_bubble_erase_mask(&bubble, stroke, w, h);
            assert_eq!(erase.paste, bubble);
            assert_eq!(erase.model, bubble);
        }
    }

    /// A steeply tapered diamond silhouette centred in a `w * w` frame — a column through the
    /// middle is much taller than one near an edge, which makes the per-column capacity difference
    /// easy to assert on.
    fn diamond_shape(w: u32) -> BubbleShape {
        let (w, centre) = (w as i32, w as i32 / 2);
        let mut mask = vec![false; (w * w) as usize];
        for x in 0..w {
            let half = centre - 2 * (x - centre).abs();
            for y in (centre - half).max(0)..(centre + half).min(w) {
                mask[(y * w + x) as usize] = true;
            }
        }
        BubbleShape {
            mask,
            frame: BoundingBox::new(0, 0, w as u32, w as u32),
        }
    }

    /// An ellipse inscribed in a `w * w` frame — a real speech balloon's own proportions, and what
    /// the shape-aware layout is actually for.
    fn ellipse_shape(w: u32) -> BubbleShape {
        let r = w as f32 / 2.0;
        let mut mask = vec![false; (w * w) as usize];
        for x in 0..w {
            let dx = x as f32 + 0.5 - r;
            let half = (r * r - dx * dx).max(0.0).sqrt();
            for y in ((r - half).floor().max(0.0) as u32)..((r + half).ceil().min(w as f32) as u32)
            {
                mask[(y * w + x) as usize] = true;
            }
        }
        BubbleShape {
            mask,
            frame: BoundingBox::new(0, 0, w, w),
        }
    }

    #[test]
    fn a_candidate_layout_is_rejected_when_one_column_is_much_shorter_than_the_rest() {
        // A steeply tapered diamond: the centre column is 60px tall, either edge column only 20px.
        let shape = diamond_shape(60);

        // A single column down the middle still holds five cells …
        let one = place_columns_in_shape("あいうえお", &shape, 10.0, 1, 30.0).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].text, "あいうえお");

        // … but a three-column candidate is rejected outright, because its two edge columns are
        // 20px against the centre's 60px — far below `MIN_COLUMN_RUN_FRACTION`. That is the point of
        // the rule: such a "column" is a narrow neck or a speck in the shape, not a text column, and
        // allocating real text to it is what stranded「等」at the bottom of a balloon under a large
        // blank on the real page this rule comes from (2026-09-20).
        assert!(place_columns_in_shape("あいうえおかき", &shape, 10.0, 3, 30.0).is_none());
        // Eight characters obviously do not fit a single centre column either.
        assert!(place_columns_in_shape("あいうえおかきく", &shape, 10.0, 1, 30.0).is_none());
    }

    #[test]
    fn shape_aware_lettering_never_leaves_the_bubble() {
        let Some(bytes) = load_test_font() else {
            return;
        };
        let fonts = FontSet::new(bytes);
        let font = ResolvedFont::load(fonts.resolve(None)).unwrap();
        let upem = font.raster.units_per_em().unwrap_or(1000.0);
        let shape = ellipse_shape(80);

        // Longer than one centre column can hold, so several columns (of different heights) are
        // needed — exactly the case that used to stick out of the balloon's top and bottom.
        let text = "而且在就任此职之际";
        let (size, columns) = fit_vertical_in_shape(text, &font, &shape, upem)
            .expect("a real balloon silhouette must be able to hold this text at some size");
        let pitch = vertical_pitch(&font, size);

        let reassembled: String = columns.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(reassembled, text, "columns must read back in order");

        for column in &columns {
            let (y_start, run_len) = shape
                .column_run(column.x_centre.round() as u32)
                .expect("every placed column must sit on a real bubble column");
            let height = column.text.chars().count() as f32 * pitch;
            assert!(
                column.y_top >= y_start as f32 - 0.5
                    && column.y_top + height <= y_start as f32 + run_len as f32 + 0.5,
                "column {:?} (y {}..{}) escapes its bubble run {}..{}",
                column.text,
                column.y_top,
                column.y_top + height,
                y_start,
                y_start as f32 + run_len as f32
            );
        }
    }

    #[test]
    fn take_prefix_fitting_prefers_a_break_opportunity() {
        // A column boundary inside a word would be a mid-word split: back off to the space.
        assert_eq!(take_prefix_fitting("hello world foo", 8), "hello ");
        assert_eq!(take_prefix_fitting("hello world foo", 5), "hello");
        // CJK has a break opportunity at every character, so it fills the column exactly.
        assert_eq!(take_prefix_fitting("そしてこの任", 3), "そして");
        // One unbreakable run: hard-split rather than return an empty column (which would spin the
        // caller's own column loop).
        assert_eq!(take_prefix_fitting("abcdefghij", 4), "abcd");
        // Whole string fits, and the degenerate limits.
        assert_eq!(take_prefix_fitting("abc", 9), "abc");
        assert_eq!(take_prefix_fitting("abc", 0), "");
    }

    #[test]
    fn dilate_mask_grows_the_mask_by_the_requested_radius_only() {
        let (w, h) = (9u32, 9u32);
        let mut mask = vec![false; (w * h) as usize];
        mask[(4 * w + 4) as usize] = true;

        let grown = dilate_mask(&mask, w, h, 2);
        assert_eq!(grown.iter().filter(|&&m| m).count(), 25);
        assert!(grown[(4 * w + 4) as usize]);
        assert!(grown[(6 * w + 4) as usize]);
        assert!(grown[(2 * w) as usize + 4]);
        assert!(!grown[(7 * w + 4) as usize]);
        assert!(!grown[(4 * w) as usize]);

        // radius 0 is an identity, and a length-mismatched mask is left alone rather than read
        // out of bounds.
        assert_eq!(dilate_mask(&mask, w, h, 0), mask);
        assert_eq!(dilate_mask(&mask[..5], w, h, 2), mask[..5]);
    }
}
