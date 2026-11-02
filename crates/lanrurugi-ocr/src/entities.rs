//! Core OCR domain types (`data-model.md`'s Detected Text Region).
//!
//! Per the constitution's newtype rule, every primary-key-shaped field here is a newtype rather
//! than a raw `String`/integer: `ArchiveId` is reused from `lanrurugi-core`, and [`PageNumber`] and
//! [`VolumeId`] are introduced here since Phase 1 has no equivalent of its own.

use lanrurugi_core::ids::ArchiveId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A page's 1-based index within an archive. A newtype rather than a bare `usize` because a page
/// number and a region index/count are both "small numbers" that would otherwise be freely
/// interchangeable at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageNumber(pub u32);

impl PageNumber {
    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PageNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for PageNumber {
    fn from(n: u32) -> Self {
        Self(n)
    }
}

/// The scope key for volume-level state (Volume Font Pattern, Terminology Glossary): a Phase 1
/// Grouping/Tankoubon id when the archive belongs to one, else the archive's own id
/// (data-model.md — "per-archive if ungrouped").
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VolumeId(pub String);

impl VolumeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The ungrouped case: a volume scope that is exactly one archive.
    pub fn from_archive(archive_id: &ArchiveId) -> Self {
        Self(archive_id.as_str().to_string())
    }
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for VolumeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for VolumeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// The reading direction OCR actually observed for a detected text block.
///
/// Persisted so the compositor can honour the original lettering direction instead of guessing
/// from the translated text / bounding-box aspect ratio — see `composite::is_vertical_bubble`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WritingDirection {
    Vertical,
    Horizontal,
}

/// An axis-aligned box in page pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl BoundingBox {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> u32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.h
    }

    pub fn area(&self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }

    /// Intersection-over-union against another box — the primary merge signal (research.md §3).
    pub fn iou(&self, other: &BoundingBox) -> f32 {
        let ix0 = self.x.max(other.x);
        let iy0 = self.y.max(other.y);
        let ix1 = self.right().min(other.right());
        let iy1 = self.bottom().min(other.bottom());

        if ix1 <= ix0 || iy1 <= iy0 {
            return 0.0;
        }

        let intersection = u64::from(ix1 - ix0) * u64::from(iy1 - iy0);
        let union = self.area() + other.area() - intersection;
        if union == 0 {
            0.0
        } else {
            intersection as f32 / union as f32
        }
    }

    /// Smallest box containing both — used when two line boxes merge into one region.
    pub fn union_with(&self, other: &BoundingBox) -> BoundingBox {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        BoundingBox {
            x,
            y,
            w: self.right().max(other.right()) - x,
            h: self.bottom().max(other.bottom()) - y,
        }
    }

    /// Gap between the two boxes' nearest edges, per axis, in pixels. `0` on an axis means they
    /// already overlap along it. Feeds the distance-threshold half of the merge rule.
    pub fn gap(&self, other: &BoundingBox) -> (u32, u32) {
        let dx = if self.right() < other.x {
            other.x - self.right()
        } else if other.right() < self.x {
            self.x - other.right()
        } else {
            0
        };
        let dy = if self.bottom() < other.y {
            other.y - self.bottom()
        } else if other.bottom() < self.y {
            self.y - other.bottom()
        } else {
            0
        };
        (dx, dy)
    }
}

/// An 8-bit RGB colour estimated from a region's pixels (FR-008a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// CSS `#rrggbb`, the form the frontend compositor consumes directly.
    pub fn to_css_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Perceived luminance (ITU-R BT.601), used to decide which of two clustered colours is the
    /// glyph and which is the bubble, and for the contrast check behind a low-confidence result.
    pub fn luminance(self) -> f32 {
        (0.299 * f32::from(self.r) + 0.587 * f32::from(self.g) + 0.114 * f32::from(self.b)) / 255.0
    }
}

/// A region's own coarse "text is definitely somewhere in here" prior, cropped to its
/// [`DetectedTextRegion::bounding_box`]'s own dimensions and bit-packed (one bit per pixel, row
/// major, most-significant-bit first within each byte) — the detection model's own raw per-pixel
/// probability output (`crate::detect::TextDetector::detect_batch_with_raw_mask`'s own doc
/// comment covers where this comes from and why it exists at all: a `local_background_stroke_mask`
/// coverage-sanity-check input, immune to that function's own specific local-window-contamination
/// failure mode since this comes from a completely different model).
///
/// Bit-packed rather than a plain `Vec<bool>` specifically because this field is persisted
/// verbatim into Redis on every `DetectedTextRegion` (that struct's own doc comment covers why) —
/// a raw `f32`-per-pixel probability map would be 32x this size for no benefit downstream
/// (`local_background_stroke_mask`'s own coverage check only ever needs a boolean "is this pixel
/// plausibly text," not the model's exact confidence value), and even an unpacked `Vec<bool>`
/// (one byte per pixel under `serde_json`) is a real, avoidable 8x size difference multiplied
/// across every translated region in a library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawTextMask {
    pub width: u32,
    pub height: u32,
    /// `((width * height) as usize).div_ceil(8)` bytes — see the type's own doc comment for the
    /// bit layout.
    bits: Vec<u8>,
}

impl RawTextMask {
    /// Packs a row-major `width * height`-long boolean mask (the detection model's own raw
    /// probability, already thresholded by the caller — see
    /// `crate::detect::TextDetector::detect_batch_with_raw_mask`'s own doc comment for the exact
    /// threshold used and why) into its bit-packed storage form.
    ///
    /// Panics if `values.len() != (width * height) as usize` — a real caller always has the exact
    /// pixel count for the dimensions it's packing, so a mismatch here is a programming error, not
    /// a runtime condition to recover from.
    pub fn pack(width: u32, height: u32, values: &[bool]) -> Self {
        assert_eq!(
            values.len(),
            (width * height) as usize,
            "RawTextMask::pack: value count must match width * height exactly"
        );
        let mut bits = vec![0u8; values.len().div_ceil(8)];
        for (i, &v) in values.iter().enumerate() {
            if v {
                bits[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        Self {
            width,
            height,
            bits,
        }
    }

    /// Whether pixel `(x, y)` (within this mask's own `width`/`height`, i.e. already in the
    /// region's own local coordinate frame) was flagged by the detection model.
    ///
    /// Panics on an out-of-bounds `(x, y)` — same contract as a direct `Vec` index, since every
    /// real caller already has `width`/`height` available to bounds-check against before calling.
    pub fn get(&self, x: u32, y: u32) -> bool {
        assert!(
            x < self.width && y < self.height,
            "RawTextMask::get: index out of bounds"
        );
        let i = (y * self.width + x) as usize;
        (self.bits[i / 8] & (1 << (7 - (i % 8)))) != 0
    }

    /// Unpacks back into a row-major `Vec<bool>`, for callers (like
    /// `lanrurugi_inpaint::stroke_mask`'s own coverage-check consumers) that want to work with the
    /// same plain boolean-mask shape every other mask in this codebase already uses, rather than
    /// bit-indexing directly.
    pub fn to_bool_vec(&self) -> Vec<bool> {
        (0..self.width * self.height)
            .map(|i| self.get(i % self.width, i / self.width))
            .collect()
    }
}

/// A merged, paragraph-level block of text on a page — the unit of translation and of font-style
/// matching (data-model.md).
///
/// Per research.md §16 this record, not the rendered image, is the authoritative persisted
/// translation result: it holds the text, position, and every resolved style attribute needed to
/// re-composite a page from scratch, so the rendered image stays a freely-evictable cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedTextRegion {
    pub archive_id: ArchiveId,
    pub page_number: PageNumber,
    pub bounding_box: BoundingBox,
    pub source_text: String,
    /// From Phase 1's own page metadata, never inferred from OCR output — excludes this region
    /// from Volume Font Pattern voting (FR-008).
    pub is_cover: bool,
    /// Each of the three style estimates is independently `None` when the heuristic had low
    /// confidence; one being absent must never suppress the others (FR-008a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg_color: Option<Rgb>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg_color: Option<Rgb>,
    /// `None` means "couldn't tell", deliberately distinct from `Some(false)` ("estimated not
    /// bold") — data-model.md.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_bold: Option<bool>,
    /// Resolved from the volume's golden set at translation time and persisted, so re-compositing
    /// never re-runs font classification (research.md §16).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Absent until this region has been translated for the requested (language, backend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_text: Option<String>,
    /// The detection model's own coarse text-probability prior for this region, cropped to
    /// `bounding_box`'s own dimensions — see [`RawTextMask`]'s own doc comment. `None` when
    /// detection was run through a code path that doesn't produce one (e.g. an older persisted
    /// region from before this field existed, or `crate::detect::TextDetector::detect_batch` —
    /// the box-only entry point — was used instead of `detect_batch_with_raw_mask`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text_mask: Option<RawTextMask>,
    /// A second OCR reading of the same crop, produced by rotating an ambiguous-axis crop 90°
    /// clockwise (and, for multi-box merged regions, by recognizing the merged union crop).
    /// Translation sends both candidates to the LLM, which picks the more plausible Japanese
    /// source and reports it back via `TranslatedBlock::selected_source_text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_source_text: Option<String>,
    /// Reading direction observed by the OCR orientation pass. `None` means the detector never
    /// produced a confident axis (vertical vs. horizontal stayed ambiguous), so compositing falls
    /// back to its legacy aspect-ratio + target-script heuristic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writing_direction: Option<WritingDirection>,
}

impl DetectedTextRegion {
    /// A freshly detected region, before translation or style resolution.
    pub fn new(
        archive_id: ArchiveId,
        page_number: PageNumber,
        bounding_box: BoundingBox,
        source_text: String,
        is_cover: bool,
    ) -> Self {
        Self {
            archive_id,
            page_number,
            bounding_box,
            source_text,
            is_cover,
            fg_color: None,
            bg_color: None,
            is_bold: None,
            font: None,
            translated_text: None,
            raw_text_mask: None,
            alternate_source_text: None,
            writing_direction: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(x: u32, y: u32, w: u32, h: u32) -> BoundingBox {
        BoundingBox::new(x, y, w, h)
    }

    #[test]
    fn old_persisted_json_without_new_ocr_fields_still_deserializes() {
        // Handoff warning: Redis already contains `DetectedTextRegion` JSON written before
        // `alternate_source_text` / `writing_direction` existed. Those fields must serde-default
        // to `None` or every old cached page becomes unreadable after deploy.
        let region = DetectedTextRegion::new(
            ArchiveId("a".repeat(40)),
            PageNumber(15),
            bb(99, 717, 93, 48),
            "娘士".into(),
            false,
        );
        let mut value = serde_json::to_value(&region).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("alternate_source_text");
        object.remove("writing_direction");
        let restored: DetectedTextRegion = serde_json::from_value(value).unwrap();
        assert_eq!(restored.source_text, "娘士");
        assert_eq!(restored.alternate_source_text, None);
        assert_eq!(restored.writing_direction, None);
    }

    #[test]
    fn raw_text_mask_round_trips_through_pack_and_unpack() {
        // Deliberately not byte-aligned (3x3=9 bits, spanning a byte boundary) to exercise the
        // bit-indexing math itself, not just a convenient whole-byte case.
        let values = vec![true, false, true, false, true, false, true, false, true];
        let mask = RawTextMask::pack(3, 3, &values);
        assert_eq!(mask.to_bool_vec(), values);
        assert!(mask.get(0, 0));
        assert!(!mask.get(1, 0));
        assert!(mask.get(2, 2));
    }

    #[test]
    fn raw_text_mask_all_false_round_trips() {
        let values = vec![false; 17]; // odd, non-byte-aligned length
        let mask = RawTextMask::pack(17, 1, &values);
        assert_eq!(mask.to_bool_vec(), values);
    }

    #[test]
    fn raw_text_mask_all_true_round_trips() {
        let values = vec![true; 16]; // exactly two bytes
        let mask = RawTextMask::pack(4, 4, &values);
        assert_eq!(mask.to_bool_vec(), values);
    }

    #[test]
    #[should_panic(expected = "value count must match")]
    fn raw_text_mask_pack_rejects_a_mismatched_length() {
        let _ = RawTextMask::pack(3, 3, &[true, false]);
    }

    #[test]
    fn iou_is_zero_for_disjoint_boxes() {
        assert_eq!(bb(0, 0, 10, 10).iou(&bb(100, 100, 10, 10)), 0.0);
    }

    #[test]
    fn iou_is_one_for_identical_boxes() {
        assert_eq!(bb(5, 5, 20, 20).iou(&bb(5, 5, 20, 20)), 1.0);
    }

    #[test]
    fn iou_is_partial_for_half_overlap() {
        // Two 10x10 boxes sharing exactly half their area: intersection 50, union 150.
        let iou = bb(0, 0, 10, 10).iou(&bb(5, 0, 10, 10));
        assert!((iou - (50.0 / 150.0)).abs() < 1e-6, "got {iou}");
    }

    #[test]
    fn gap_is_zero_on_an_overlapping_axis() {
        // Vertically stacked, horizontally aligned: no x gap, 5px y gap.
        assert_eq!(bb(0, 0, 10, 10).gap(&bb(0, 15, 10, 10)), (0, 5));
    }

    #[test]
    fn union_covers_both_boxes() {
        let u = bb(0, 0, 10, 10).union_with(&bb(20, 30, 10, 10));
        assert_eq!(u, bb(0, 0, 30, 40));
    }

    #[test]
    fn css_hex_is_zero_padded() {
        assert_eq!(Rgb::new(0, 8, 255).to_css_hex(), "#0008ff");
    }
}
