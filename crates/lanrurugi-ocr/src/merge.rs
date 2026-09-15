//! Folds raw per-line detection boxes into paragraph-level regions (research.md §3).
//!
//! Translation quality depends on the backend receiving a coherent block rather than fragments,
//! and font matching needs a stable per-block unit to classify — so two line boxes merge when
//! they either overlap meaningfully (IoU), sit close enough together to be the same speech
//! bubble, or share one real bubble mask (see [`merge_lines`]'s own doc comment for why bubble
//! membership is a *third*, independent merge reason rather than a second pass over this one's
//! own output).

use crate::bubble_segment::DetectedBubble;
use crate::entities::{BoundingBox, DetectedTextRegion, PageNumber};
use lanrurugi_core::ids::ArchiveId;

/// One raw detection before merging: a box plus the text recognized inside it.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedLine {
    pub bounding_box: BoundingBox,
    pub text: String,
}

impl DetectedLine {
    pub fn new(bounding_box: BoundingBox, text: impl Into<String>) -> Self {
        Self {
            bounding_box,
            text: text.into(),
        }
    }
}

/// Thresholds governing when two line boxes belong to the same region.
#[derive(Debug, Clone, Copy)]
pub struct MergeConfig {
    /// Any overlap at least this large merges regardless of distance.
    pub iou_threshold: f32,
    /// Max horizontal gap, as a multiple of the smaller box's width, to still merge.
    pub max_x_gap_ratio: f32,
    /// Max vertical gap, as a multiple of the smaller box's height, to still merge.
    pub max_y_gap_ratio: f32,
}

/// How large a group's own union bbox is allowed to grow, in either dimension, relative to the
/// largest single member line currently in that group, before a further merge into it is
/// rejected outright. Distance/bubble-membership checks alone only ever compare one *pair* of
/// current groups at a time — nothing stops a chain of individually-plausible pairwise merges
/// (A-B close, B-C close) from producing a group whose overall span is wildly disproportionate to
/// any real line it actually contains, since transitive grouping (see [`merge_lines`]'s own doc
/// comment) never re-validates the group as a whole. A real reported bug (2026-09-08): a single
/// ~75px-tall "タ" SFX glyph got transitively chained (via at least one intermediate stray/
/// spurious detection) into a group whose final bbox was 231px tall — nearly 3.1x that glyph's own
/// height — engulfing an unrelated background character's entire head. `4.0` is deliberately
/// generous (a real multi-line paragraph can legitimately span several times one line's own
/// height) — this guards against runaway chains, not against ordinary multi-line text.
const MAX_GROUP_SPAN_RATIO: f32 = 4.0;

impl Default for MergeConfig {
    fn default() -> Self {
        // Gaps are expressed relative to box size rather than in absolute pixels so the same
        // thresholds hold across page resolutions — a scan at 2x the DPI has 2x the line spacing.
        Self {
            iou_threshold: 0.1,
            max_x_gap_ratio: 0.6,
            max_y_gap_ratio: 0.8,
        }
    }
}

/// Fraction of a region's own area that must fall inside one bubble's mask for that region to
/// count as belonging to it. Deliberately well under 1.0 — a region's rectangular bounding box
/// routinely pokes slightly outside a bubble's real (non-rectangular, often round-cornered or
/// tailed) mask along its edges, so requiring full containment would miss real members; `0.5`
/// still rejects a region that only clips a bubble's corner incidentally.
const BUBBLE_MEMBERSHIP_THRESHOLD: f32 = 0.5;

/// A box's own aspect-ratio bucket — a cheap proxy for tategaki (vertical column) vs. horizontal
/// lettering, since detection never labels orientation directly. Anything not clearly lopsided
/// either way is `Ambiguous` (a single roughly-square glyph, an SFX blob, a short two-character
/// horizontal run) — these never block a merge on orientation grounds, only a confident
/// Vertical/Horizontal mismatch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AspectOrientation {
    Vertical,
    Horizontal,
    Ambiguous,
}

/// Ratio a box's longer side must exceed the shorter one by before its orientation is trusted —
/// see `AspectOrientation`'s own doc comment for why anything less lopsided stays `Ambiguous`.
const ORIENTATION_CONFIDENCE_RATIO: f32 = 1.5;

fn aspect_orientation(b: &BoundingBox) -> AspectOrientation {
    let (w, h) = (b.w as f32, b.h as f32);
    if h >= w * ORIENTATION_CONFIDENCE_RATIO {
        AspectOrientation::Vertical
    } else if w >= h * ORIENTATION_CONFIDENCE_RATIO {
        AspectOrientation::Horizontal
    } else {
        AspectOrientation::Ambiguous
    }
}

/// A group's orientation is a majority vote over its *individual members'* own
/// [`aspect_orientation`] — deliberately not the aspect ratio of the group's union bbox. Several
/// narrow vertical columns spread across one wide bubble (the exact shape `merge_lines`'s own
/// bubble-membership merge reason exists to bridge) produce a union box that reads as wide/
/// horizontal purely from being laid out side by side, even though every member column is itself
/// clearly vertical — a real regression this majority-vote design fixes (2026-09-15): the union-
/// bbox version of this check rejected a 4th column from joining 3 already-merged ones the moment
/// their combined width crossed the confidence ratio, misreading "many vertical things placed
/// horizontally" as "one horizontal thing". Voting over members side-steps that entirely, the same
/// way a real reference implementation (`manga-image-translator`'s own per-textline `direction`
/// plus a page-wide `Counter` majority vote for its own orientation decisions) resolves single-
/// glyph ambiguity by looking at more than one box rather than inventing a smarter single-box
/// heuristic — there isn't one; a lone character's own aspect ratio is inherently unreliable, and
/// the fix is always more evidence, not a better formula for less of it.
#[derive(Debug, Clone, Copy, Default)]
struct OrientationVotes {
    vertical: u32,
    horizontal: u32,
}

impl OrientationVotes {
    fn from_box(b: &BoundingBox) -> Self {
        match aspect_orientation(b) {
            AspectOrientation::Vertical => Self {
                vertical: 1,
                horizontal: 0,
            },
            AspectOrientation::Horizontal => Self {
                vertical: 0,
                horizontal: 1,
            },
            // An ambiguous single box casts no vote either way rather than a tie-breaking "half
            // vote" — a group of otherwise-unanimous members must not have its result diluted by
            // however many ambiguous glyphs (SFX blobs, single kanji) happen to be mixed in.
            AspectOrientation::Ambiguous => Self::default(),
        }
    }

    fn combine(self, other: Self) -> Self {
        Self {
            vertical: self.vertical + other.vertical,
            horizontal: self.horizontal + other.horizontal,
        }
    }

    fn resolve(self) -> AspectOrientation {
        use std::cmp::Ordering;
        match self.vertical.cmp(&self.horizontal) {
            Ordering::Greater => AspectOrientation::Vertical,
            Ordering::Less => AspectOrientation::Horizontal,
            // No votes at all, or a tie, are the same outcome here: neither side has a majority,
            // so the group carries no reliable orientation signal of its own.
            Ordering::Equal => AspectOrientation::Ambiguous,
        }
    }
}

/// Whether two orientations actively disagree — the only case any merge reason below refuses to
/// bridge, however strong that reason's own evidence (distance, bubble membership) otherwise is.
/// `Ambiguous` never conflicts with anything: a group with no reliable majority orientation of its
/// own yet (a single, roughly-square glyph; a tie) cannot veto a merge on grounds it doesn't
/// actually have evidence for.
fn orientation_conflicts(a: OrientationVotes, b: OrientationVotes) -> bool {
    matches!(
        (a.resolve(), b.resolve()),
        (AspectOrientation::Vertical, AspectOrientation::Horizontal)
            | (AspectOrientation::Horizontal, AspectOrientation::Vertical)
    )
}

/// What fraction of `bbox`'s own area is covered by `bubble`'s real (mask) shape, not just its
/// bounding box — a bubble's mask is `true` at page pixel `(x, y)` when
/// `bubble.mask[(y - bubble.bbox.y) * bubble.bbox.w + (x - bubble.bbox.x)]`, so this only samples
/// the intersection rectangle to stay cheap even on a page with many small text regions.
fn region_area_fraction_inside(bbox: &BoundingBox, bubble: &DetectedBubble) -> f32 {
    let ix0 = bbox.x.max(bubble.bbox.x);
    let iy0 = bbox.y.max(bubble.bbox.y);
    let ix1 = bbox.right().min(bubble.bbox.right());
    let iy1 = bbox.bottom().min(bubble.bbox.bottom());
    if ix1 <= ix0 || iy1 <= iy0 {
        return 0.0;
    }

    let mut inside = 0u64;
    for y in iy0..iy1 {
        let mask_row = (y - bubble.bbox.y) * bubble.bbox.w;
        for x in ix0..ix1 {
            let mask_idx = (mask_row + (x - bubble.bbox.x)) as usize;
            if bubble.mask.get(mask_idx).copied().unwrap_or(false) {
                inside += 1;
            }
        }
    }

    inside as f32 / bbox.area() as f32
}

/// The first bubble (if any) that owns enough of `bbox`'s own area to count as its home balloon —
/// see [`BUBBLE_MEMBERSHIP_THRESHOLD`]'s own doc comment for why "enough" is well under 1.0.
fn bubble_membership(bbox: &BoundingBox, bubbles: &[DetectedBubble]) -> Option<usize> {
    bubbles
        .iter()
        .position(|bubble| region_area_fraction_inside(bbox, bubble) >= BUBBLE_MEMBERSHIP_THRESHOLD)
}

/// Whether two *current group* bounding boxes should be merged into one, for a reason unrelated
/// to bubble membership (geometric proximity or direct overlap). `a`/`b` are each side's current
/// group union bbox (for distance/overlap); `a_max_dim`/`b_max_dim` are each side's current
/// largest-single-member `(w, h)` (see [`merge_lines`]'s own `group_max_dim`) — the distance-gap
/// tolerance below scales against *this*, not the union bbox's own size, for the same reason a
/// real reference implementation (`manga-image-translator`'s own `quadrilateral_can_merge_region`)
/// scales its distance tolerance against `char_size` (the smaller of the two boxes' own estimated
/// font size) rather than the merged region's own extent: a group that has already absorbed
/// several members must not thereby become *more* permissive about how far away the next member
/// can sit — the amount of real, physical whitespace manga lettering can legitimately have between
/// two lines of the same block doesn't grow just because a third line joined first. `a_votes`/
/// `b_votes` are each side's current [`OrientationVotes`] tally — see that type's own doc comment
/// for why a majority vote over members, not the union bbox's own aspect ratio, is what the
/// orientation guard checks.
fn should_merge_by_geometry(
    a: &BoundingBox,
    b: &BoundingBox,
    a_max_dim: (u32, u32),
    b_max_dim: (u32, u32),
    a_votes: OrientationVotes,
    b_votes: OrientationVotes,
    cfg: &MergeConfig,
) -> bool {
    if orientation_conflicts(a_votes, b_votes) {
        return false;
    }
    if a.iou(b) >= cfg.iou_threshold {
        return true;
    }

    let (dx, dy) = a.gap(b);
    // Scaled against each side's own largest *single member*, not the union bbox — see this
    // function's own doc comment for why. The same real 2026-09-15 bug this function's other two
    // guards address had a second, independent contributing cause here: with the union-bbox
    // version of this scale, a group that had already grown to 388x280 tolerated a gap of
    // `388 * 0.6 = 233px` to its next candidate, when the real single lines it started from were
    // never more than ~140px wide — the group's own growth was silently relaxing the very
    // tolerance meant to bound how far two genuinely-related lines can sit apart.
    let (aw, ah) = a_max_dim;
    let (bw, bh) = b_max_dim;
    let min_w = aw.min(bw) as f32;
    let min_h = ah.min(bh) as f32;

    // Both axes must be within tolerance: two bubbles side by side on the same line are close
    // vertically but far horizontally, and must not merge.
    (dx as f32) <= min_w * cfg.max_x_gap_ratio && (dy as f32) <= min_h * cfg.max_y_gap_ratio
}

/// Orders items into natural reading order by their bounding box. Manga lettering is
/// predominantly vertical (right-to-left columns), but ordering here uses the simpler,
/// orientation-agnostic rule that works for both: top to bottom, then right to left for ties,
/// which matches tategaki column order and degrades sensibly for horizontal text.
fn reading_order<T>(items: &mut [T], bbox_of: impl Fn(&T) -> BoundingBox) {
    items.sort_by(|a, b| {
        let ab = bbox_of(a);
        let bb = bbox_of(b);
        // Same visual row (boxes overlap vertically) → rightmost first.
        let same_row = ab.y < bb.bottom() && bb.y < ab.bottom();
        if same_row {
            bb.x.cmp(&ab.x)
        } else {
            ab.y.cmp(&bb.y)
        }
    });
}

/// Groups `lines` into paragraph-level regions — a single merge pass considering every reason two
/// lines belong together at once: geometric proximity/overlap ([`should_merge_by_geometry`]), or
/// sharing one real detected speech-bubble mask (research.md's translation-quality note on this
/// codebase's real issue #108: manga lettering is routinely split into several visually-separated
/// text columns *within one speech bubble* purely for layout balance — distance alone cannot
/// bridge the resulting gaps without also risking false merges between genuinely separate nearby
/// bubbles, since both cases look the same from line spacing alone; a real bubble mask resolves
/// that ambiguity distance can't).
///
/// This used to be two separate functions run as two passes — `merge_lines` (geometry only, line
/// granularity) followed by a `merge_regions_by_bubble` pass over its *output* (bubble membership
/// only, region granularity) — because bubble segmentation itself used to run after this function,
/// too late to fold into one pass. `translate_page` (`lanrurugi-api::translation_pipeline`) now
/// runs bubble segmentation up front and threads the result into detection before this function
/// ever runs, which is what makes a single unified pass possible at all: every reason to merge two
/// lines is now known before grouping starts, so there is no reason left to keep them as two
/// separate, sequential merge decisions that can each only see part of the picture.
///
/// That separation caused a real bug (2026-09-15): a horizontal stamp/caption ("不採用", printed
/// sideways in the middle of a letter) sat close enough to a vertical speech-bubble line that
/// *both* ended up claimed by the same bubble mask (the mask's own edge imprecision, not a real
/// containment) — `merge_lines`'s own geometry pass correctly kept them apart, but the *second*,
/// bubble-only pass had no orientation check of its own and merged them anyway, since it never
/// re-ran the geometry side's reasoning.
///
/// Merging into one pass fixed *that* incident, but an orientation guard shared across both merge
/// reasons turned out to be the wrong fix in general: a second real incident, the same day, found
/// a genuine three-column bubble ("はじまりは"/"俺が就職活動に"/"勤しんでいた頃…") silently losing
/// its third column because that column's own aspect ratio disagreed with the other two's —
/// correct bubble membership, vetoed by a guess about shape. Per a project-wide rule ("一个气泡中
/// 的文字必须是连在一起的才行" — every region inside one real bubble must render as a single
/// coherent whole, never a partial translation missing some of that bubble's own text), a bubble
/// mask is a *measurement* of which real speech bubble a line's pixels actually sit inside, while
/// orientation is only ever a *guess* about a raw line's own shape — a guess must never veto a
/// measurement. [`should_merge_by_geometry`]'s own orientation guard still applies to the
/// proximity-only merge reason (nothing there provides comparable certainty), but bubble
/// membership merges unconditionally once confirmed. The remaining defense against the *original*
/// incident's failure mode (two genuinely unrelated pieces of lettering wrongly claimed as the
/// same bubble by a mask-edge imprecision) is `bubble_membership`'s own matching precision, not an
/// orientation second-guess bolted onto every consumer of its result.
///
/// Uses transitive grouping (union-find style): A merges with B and B with C puts all three in one
/// region even when A and C alone wouldn't qualify — a column of text should not fragment just
/// because its first and last lines are far apart. Each candidate merge is still re-checked
/// against the *group's own current span* (see [`MAX_GROUP_SPAN_RATIO`]), not just the pairwise
/// merge test, so a chain of individually-plausible merges can't silently balloon into a region
/// far larger than any of its real member lines.
///
/// `bubbles` is `None` (or empty) when bubble segmentation itself failed or wasn't available for
/// this page — callers degrade to the geometry-only result rather than treating that as an error
/// (same fallback discipline `translation_pipeline::composite_and_cache`'s own bubble-segmentation
/// call already established for page erasure).
pub fn merge_lines(
    archive_id: &ArchiveId,
    page_number: PageNumber,
    is_cover: bool,
    lines: Vec<DetectedLine>,
    cfg: &MergeConfig,
    bubbles: Option<&[DetectedBubble]>,
) -> Vec<DetectedTextRegion> {
    if lines.is_empty() {
        return Vec::new();
    }

    let bubbles = bubbles.unwrap_or(&[]);
    let membership: Vec<Option<usize>> = lines
        .iter()
        .map(|l| bubble_membership(&l.bounding_box, bubbles))
        .collect();

    // `group[i]` is the index of the group line `i` currently belongs to.
    let mut group: Vec<usize> = (0..lines.len()).collect();
    // Per group root: the group's own current union bbox, the largest single member's own (w, h)
    // seen in that group so far, and the group's running [`OrientationVotes`] tally — all three
    // updated as merges commit, and consulted (via each side's *current* root) before every
    // further merge. Span is checked against `MAX_GROUP_SPAN_RATIO` (see that constant's own doc
    // comment); orientation votes are what `should_merge_by_geometry`/`orientation_conflicts`
    // check instead of the union bbox's own aspect ratio (see [`OrientationVotes`]'s own doc
    // comment for why).
    let mut group_bbox: Vec<BoundingBox> = lines.iter().map(|l| l.bounding_box).collect();
    let mut group_max_dim: Vec<(u32, u32)> = lines
        .iter()
        .map(|l| (l.bounding_box.w, l.bounding_box.h))
        .collect();
    let mut group_votes: Vec<OrientationVotes> = lines
        .iter()
        .map(|l| OrientationVotes::from_box(&l.bounding_box))
        .collect();

    fn find(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]]; // path compression
            i = group[i];
        }
        i
    }

    for i in 0..lines.len() {
        for j in (i + 1)..lines.len() {
            let (ri, rj) = (find(&mut group, i), find(&mut group, j));
            if ri == rj {
                continue;
            }

            // Two independent reasons to merge: current-group geometry (guarded by the
            // orientation check below — two things that merely sit near each other are only
            // trusted to be the same block when they don't actively disagree on direction), or the
            // two raw lines `i`/`j` sharing one real bubble mask, which is *not* orientation-
            // guarded — see this arm's own comment for why. Bubble membership is intentionally
            // tested on the raw lines, not the groups' own bboxes — a group's union box can
            // already span outside every bubble once it has absorbed a line or two, but each
            // individual line's own membership never changes.
            let by_geometry = should_merge_by_geometry(
                &group_bbox[ri],
                &group_bbox[rj],
                group_max_dim[ri],
                group_max_dim[rj],
                group_votes[ri],
                group_votes[rj],
                cfg,
            );
            // A real reported bug (2026-09-15): the same page-9 bubble whose three columns read
            // "はじまりは"/"俺が就職活動に"/"勤しんでいた頃…" got split into two separate regions
            // because the third column's own aspect ratio disagreed with the other two's — orientation
            // is a *guess* about a raw line's own shape, while `bubble_membership` (the bubble
            // segmentation model's own independently-detected mask) is a *measurement* of which
            // real speech bubble a line's pixels actually sit inside. A guess must never veto a
            // measurement: once two lines are confirmed to share one real bubble, they belong to
            // the same block by definition — a project-wide rule, not just a bug in this one case
            // ("一个气泡中的文字必须是连在一起的才行" — every region inside a bubble must render as
            // one coherent whole, never a partial translation with some of the bubble's own text
            // silently left untranslated because a heuristic happened to disagree with reality).
            let by_bubble = membership[i].is_some() && membership[i] == membership[j];
            if !by_geometry && !by_bubble {
                continue;
            }

            let merged_bbox = group_bbox[ri].union_with(&group_bbox[rj]);
            let (aw, ah) = group_max_dim[ri];
            let (bw, bh) = group_max_dim[rj];

            // MAX_GROUP_SPAN_RATIO only guards the *geometry* merge reason — see that constant's
            // own doc comment for the runaway-chain failure mode it exists to prevent. A bubble-
            // membership merge needs no such guard: the bubble's own mask is already a real,
            // independently-detected size bound (this function only ever merges lines the mask
            // itself claims), so there's nothing left for a span ratio to protect against. A real
            // regression (2026-09-15) applying this guard to the bubble reason too: four equal-
            // width vertical columns spread across one wide bubble (exactly the shape the bubble
            // merge reason exists to bridge) each have their own narrow (w, h) — the first two
            // columns merging already produces a union almost as wide as two columns placed side
            // by side, which blew past 4x either column's own width and rejected the merge outright,
            // even though every column undisputedly belonged to the same real bubble.
            if by_geometry {
                // Checked against the *smaller* side's own scale, not the larger — the failure
                // mode this guards against is a small group (potentially a single tiny glyph)
                // being inflated by absorbing something disproportionately larger; comparing
                // against the larger side's own dimensions would let that larger box "self-
                // justify" swallowing the small one just by already being big itself, defeating
                // the whole point.
                let (min_w, min_h) = (aw.min(bw), ah.min(bh));
                let span_ok = (merged_bbox.w as f32) <= (min_w as f32) * MAX_GROUP_SPAN_RATIO
                    && (merged_bbox.h as f32) <= (min_h as f32) * MAX_GROUP_SPAN_RATIO;
                if !span_ok {
                    continue;
                }
            }

            group[rj] = ri;
            group_bbox[ri] = merged_bbox;
            group_max_dim[ri] = (aw.max(bw), ah.max(bh));
            group_votes[ri] = group_votes[ri].combine(group_votes[rj]);
        }
    }

    // Collect each group's member lines, preserving first-appearance order of the groups
    // themselves so output ordering is deterministic.
    let mut group_order: Vec<usize> = Vec::new();
    let mut buckets: Vec<(usize, Vec<DetectedLine>)> = Vec::new();

    for (i, line) in lines.into_iter().enumerate() {
        let root = find(&mut group, i);
        match group_order.iter().position(|&g| g == root) {
            Some(pos) => buckets[pos].1.push(line),
            None => {
                group_order.push(root);
                buckets.push((root, vec![line]));
            }
        }
    }

    buckets
        .into_iter()
        .filter_map(|(_, mut members)| {
            reading_order(&mut members, |m| m.bounding_box);

            let bounding_box = members
                .iter()
                .map(|m| m.bounding_box)
                .reduce(|acc, b| acc.union_with(&b))?;

            // Japanese text is not space-delimited, so joining columns with a space would insert
            // separators that were never in the original; lines are concatenated directly.
            let source_text: String = members
                .iter()
                .map(|m| m.text.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("");

            if source_text.is_empty() {
                return None;
            }

            Some(DetectedTextRegion::new(
                archive_id.clone(),
                page_number,
                bounding_box,
                source_text,
                is_cover,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x: u32, y: u32, w: u32, h: u32, text: &str) -> DetectedLine {
        DetectedLine::new(BoundingBox::new(x, y, w, h), text)
    }

    fn archive() -> ArchiveId {
        ArchiveId::from("test-archive")
    }

    fn merge(
        lines: Vec<DetectedLine>,
        bubbles: Option<&[DetectedBubble]>,
    ) -> Vec<DetectedTextRegion> {
        merge_lines(
            &archive(),
            PageNumber(1),
            false,
            lines,
            &MergeConfig::default(),
            bubbles,
        )
    }

    /// A fully-solid rectangular bubble mask — the "same as the bounding box" case, good enough
    /// for tests that only care about which lines get grouped, not mask-edge precision.
    fn solid_bubble(x: u32, y: u32, w: u32, h: u32) -> DetectedBubble {
        DetectedBubble {
            bbox: BoundingBox::new(x, y, w, h),
            confidence: 0.9,
            mask: vec![true; (w * h) as usize],
        }
    }

    #[test]
    fn empty_input_yields_no_regions() {
        assert!(merge(Vec::new(), None).is_empty());
    }

    #[test]
    fn nearby_lines_merge_into_one_region() {
        // Two stacked lines 4px apart, each 20px tall — well within the vertical tolerance.
        let lines = vec![line(10, 10, 50, 20, "ああ"), line(10, 34, 50, 20, "いい")];
        let out = merge(lines, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_text, "ああいい");
    }

    #[test]
    fn a_horizontal_stamp_near_a_vertical_line_does_not_merge() {
        // Real bug (2026-09-15): a horizontal caption/stamp ("不採用", printed sideways) sitting
        // close to a vertical speech-bubble column got merged purely on geometric proximity,
        // scrambling the concatenated text into something neither OCR nor translation could make
        // sense of. A wide, short box (horizontal) and a narrow, tall one (vertical column),
        // close enough that geometry alone would merge them, must stay separate once orientation
        // disagrees.
        let lines = vec![
            line(10, 10, 60, 15, "不採用"),  // w/h = 4.0 — confidently horizontal
            line(15, 28, 15, 60, "何かに…"), // w/h = 0.25 — confidently vertical
        ];
        assert_eq!(merge(lines, None).len(), 2);
    }

    #[test]
    fn lines_confirmed_sharing_one_real_bubble_merge_even_on_orientation_conflict() {
        // Reversed from an earlier version of this test (2026-09-15): a project-wide rule —
        // "一个气泡中的文字必须是连在一起的才行" — every region inside one real bubble must render
        // as a single coherent whole, never a partial translation silently missing some of that
        // bubble's own text because a heuristic disagreed with reality. Orientation is a *guess*
        // about a raw line's own shape; bubble membership (the bubble segmentation model's own
        // independently-detected mask) is a *measurement* of which real speech bubble a line's
        // pixels actually sit inside. A guess must never veto a measurement — once two lines are
        // confirmed to share one real bubble, they belong together by definition, however their
        // individual aspect ratios disagree. (A real 2026-09-15 incident: a genuine three-column
        // bubble — "はじまりは"/"俺が就職活動に"/"勤しんでいた頃…" — silently lost its third column
        // to exactly this now-removed guard, rendering as a truncated, partially-untranslated
        // bubble on a real page.) Precision against the *opposite* failure — two genuinely
        // unrelated pieces of lettering wrongly claimed as the same bubble by a mask-edge
        // imprecision — belongs in `bubble_membership`'s own matching precision, not in a
        // second-guess bolted onto every consumer of its result.
        let lines = vec![
            line(10, 10, 60, 15, "不採用"),
            line(200, 10, 15, 60, "何かに…"),
        ];
        let bubble = solid_bubble(0, 0, 300, 100);
        assert_eq!(merge(lines, Some(&[bubble])).len(), 1);
    }

    #[test]
    fn ambiguous_aspect_boxes_still_merge_normally() {
        // Roughly-square boxes (a single glyph, an SFX blob) carry no reliable orientation signal
        // and must not be blocked from merging just because they're not lopsided either way —
        // only a confident Vertical/Horizontal mismatch blocks a merge.
        let lines = vec![line(10, 10, 20, 20, "あ"), line(10, 34, 20, 20, "い")];
        let out = merge(lines, None);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn distant_lines_stay_separate() {
        let lines = vec![line(10, 10, 50, 20, "ああ"), line(700, 900, 50, 20, "いい")];
        assert_eq!(merge(lines, None).len(), 2);
    }

    #[test]
    fn merging_is_transitive_across_a_chain() {
        // A-B and B-C are each close enough; A-C alone are not.
        let lines = vec![
            line(10, 10, 40, 20, "い"),
            line(10, 34, 40, 20, "ろ"),
            line(10, 58, 40, 20, "は"),
        ];
        let out = merge(lines, None);
        assert_eq!(out.len(), 1, "chained lines must form a single region");
        assert_eq!(out[0].source_text, "いろは");
        assert_eq!(out[0].bounding_box, BoundingBox::new(10, 10, 40, 68));
    }

    #[test]
    fn a_chain_of_individually_plausible_merges_is_rejected_once_the_group_span_blows_up() {
        // Reproduces the real reported bug (2026-09-08): a small isolated SFX glyph ("タ", here
        // "A") transitively chained through an intermediate stray/spurious detection ("B") into a
        // group whose final span (here simulated with "C" standing in for unrelated background
        // content the chain reached) would have engulfed unrelated page content. A-B and B-C are
        // each pairwise close enough to merge on their own — but A is only 30px tall, and the
        // resulting A+B+C span would be a 9.07x ratio, well past the 4.0x default tolerance.
        let lines = vec![
            line(10, 10, 40, 30, "タ"), // A: the real glyph, 30px tall.
            line(10, 46, 40, 30, "ー"), // B: a plausible second glyph, still small — merges with A.
            line(10, 82, 40, 200, "?"), // C: something much taller — merging this into A+B would
                                        // blow the group span to 272px, 9.07x A's own 30px height.
        ];
        let out = merge(lines, None);
        // A and B still merge (their combined span, 76px, is within 4x A's own 30px height) —
        // C is rejected from joining that group because doing so would blow the span too far.
        assert_eq!(
            out.len(),
            2,
            "the disproportionate box must be rejected into its own region, not swallow the others"
        );
        let ab = out
            .iter()
            .find(|r| r.source_text == "ター")
            .expect("A+B merged");
        assert_eq!(ab.bounding_box, BoundingBox::new(10, 10, 40, 66));
    }

    #[test]
    fn vertical_columns_read_right_to_left() {
        // Two side-by-side columns on the same visual row: rightmost is read first (tategaki).
        let lines = vec![line(10, 10, 20, 60, "left"), line(40, 10, 20, 60, "right")];
        let out = merge(lines, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_text, "rightleft");
    }

    #[test]
    fn blank_regions_are_dropped() {
        let lines = vec![line(10, 10, 50, 20, "   ")];
        assert!(
            merge(lines, None).is_empty(),
            "whitespace-only region must not be emitted"
        );
    }

    #[test]
    fn cover_flag_is_propagated() {
        let lines = vec![line(10, 10, 50, 20, "タイトル")];
        let out = merge_lines(
            &archive(),
            PageNumber(1),
            true,
            lines,
            &MergeConfig::default(),
            None,
        );
        assert!(out[0].is_cover, "cover flag must reach the region record");
    }

    #[test]
    fn no_bubbles_leaves_lines_merged_by_geometry_only() {
        let lines = vec![line(10, 10, 20, 100, "A"), line(200, 10, 20, 100, "B")];
        assert_eq!(merge(lines, None).len(), 2);
    }

    #[test]
    fn lines_sharing_one_bubble_merge_even_when_far_apart() {
        // Four columns spread across one wide panel-style bubble — same shape as the real
        // 2026-09-14 bug report (four tategaki columns inside one box, too far apart on the x-axis
        // for geometry alone to bridge).
        let lines = vec![
            line(400, 20, 30, 200, "column4"),
            line(300, 20, 30, 200, "column3"),
            line(200, 20, 30, 200, "column2"),
            line(100, 20, 30, 200, "column1"),
        ];
        let bubble = solid_bubble(80, 0, 400, 240);
        let out = merge(lines, Some(&[bubble]));
        assert_eq!(out.len(), 1);
        // Reading order is right-to-left for a shared row, so column4 (rightmost) reads first.
        assert_eq!(out[0].source_text, "column4column3column2column1");
    }

    #[test]
    fn lines_in_different_bubbles_stay_separate() {
        let lines = vec![line(10, 10, 20, 50, "A"), line(200, 10, 20, 50, "B")];
        let bubbles = vec![solid_bubble(0, 0, 60, 80), solid_bubble(180, 0, 60, 80)];
        assert_eq!(merge(lines, Some(&bubbles)).len(), 2);
    }

    #[test]
    fn a_line_outside_every_bubble_is_left_as_its_own_singleton() {
        // SFX lettering or a narration box outside any speech bubble — must survive unmerged, not
        // vanish or get folded into an unrelated bubble.
        let lines = vec![line(10, 10, 20, 50, "A"), line(500, 500, 20, 50, "SFX")];
        let bubble = solid_bubble(0, 0, 60, 80);
        let out = merge(lines, Some(&[bubble]));
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|r| r.source_text == "SFX"));
    }

    #[test]
    fn a_line_only_clipping_a_bubbles_corner_does_not_count_as_a_member() {
        // Line mostly outside the bubble (only overlaps its corner) — below the 0.5 containment
        // threshold, so it must not merge with a line that's genuinely inside via the bubble
        // reason (they're also too far apart for the geometry reason to bridge).
        let lines = vec![
            line(0, 0, 100, 100, "inside"),
            line(300, 300, 100, 100, "corner"),
        ];
        let bubble = solid_bubble(0, 0, 350, 350);
        assert_eq!(merge(lines, Some(&[bubble])).len(), 2);
    }
}
