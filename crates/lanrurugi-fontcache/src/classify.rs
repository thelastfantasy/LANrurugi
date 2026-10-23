//! Font classification: the expensive full classifier, and the cheap per-block feature router.
//!
//! Two deliberately different costs (research.md §4):
//! - [`classify_full`] is the heavy pass, run during voting and meltdown only.
//! - [`cheap_features`] extracts the handful of signals the locked fast path routes on (word count,
//!   aspect ratio, punctuation), so a locked volume never pays full classification per block.

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::entities::FontId;

/// Cheap, geometry/text-derived signals used to route a block among an already-locked golden set.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CheapFeatures {
    /// Region width / height. Sound-effect lettering tends to be far wider or taller relative to
    /// its text length than dialogue in a bubble.
    pub aspect_ratio: f32,
    /// Characters in the recognized text.
    pub char_count: usize,
    /// Whether the text carries sentence punctuation — dialogue does, sound effects rarely do.
    pub has_punctuation: bool,
    /// Mean glyph area: region area divided by character count.
    pub area_per_char: f32,
}

/// Japanese and Latin sentence punctuation that signals dialogue rather than a sound effect.
const PUNCTUATION: [char; 10] = ['。', '、', '！', '？', '…', '.', ',', '!', '?', '「'];

/// Extracts the cheap routing signals from a block's geometry and text.
pub fn cheap_features(width: u32, height: u32, text: &str) -> CheapFeatures {
    let char_count = text.chars().count();
    let area = (width as f32) * (height as f32);

    CheapFeatures {
        aspect_ratio: if height == 0 {
            0.0
        } else {
            width as f32 / height as f32
        },
        char_count,
        has_punctuation: text.chars().any(|c| PUNCTUATION.contains(&c)),
        area_per_char: if char_count == 0 {
            0.0
        } else {
            area / char_count as f32
        },
    }
}

/// The full classifier's verdict for one block.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub font: FontId,
    /// 0.0–1.0. Low-confidence classifications are not admitted to the vote pool.
    pub confidence: f32,
}

/// Below this the classification is too weak to vote with.
pub const MIN_CLASSIFY_CONFIDENCE: f32 = 0.35;

/// Style buckets this phase distinguishes. A real font-recognition model would return a specific
/// typeface; this phase classifies into lettering *styles* the renderer has a matching web font
/// for, which is what FR-008's "small set of matched fonts" actually needs.
pub const STYLE_DIALOGUE: &str = "dialogue-gothic";
pub const STYLE_EMPHASIS: &str = "dialogue-bold";
pub const STYLE_SFX: &str = "sfx-brush";
pub const STYLE_NARRATION: &str = "narration-mincho";

/// The full (expensive) classifier for one block.
///
/// Runs during voting and meltdown only — never on the locked fast path. Callers MUST bridge this
/// through `spawn_blocking` (see [`crate::voting`]/[`crate::meltdown`]), per constitution
/// Principle III.
///
/// This phase's implementation derives the style from the block's own rendered characteristics
/// (ink weight, glyph area, punctuation) rather than a trained typeface classifier: there is no
/// permissively-licensed manga font-recognition model to depend on, and FR-008 only requires
/// consistent routing among a small style set, not identification of a specific typeface.
pub fn classify_full(crop: &RgbImage, text: &str, is_bold: Option<bool>) -> Classification {
    let (w, h) = crop.dimensions();
    let f = cheap_features(w, h, text);

    // Sound effects: large glyphs, few characters, no sentence punctuation.
    let looks_like_sfx = !f.has_punctuation && f.char_count <= 6 && f.area_per_char > 4_000.0;
    // Narration boxes: punctuated, and squat/wide rather than bubble-shaped.
    let looks_like_narration = f.has_punctuation && f.aspect_ratio > 2.5;

    let (font, confidence) = if looks_like_sfx {
        (FontId::from(STYLE_SFX), 0.75)
    } else if looks_like_narration {
        (FontId::from(STYLE_NARRATION), 0.6)
    } else if is_bold == Some(true) {
        (FontId::from(STYLE_EMPHASIS), 0.65)
    } else if f.char_count == 0 {
        // Nothing to judge — deliberately below the vote threshold.
        (FontId::from(STYLE_DIALOGUE), 0.0)
    } else {
        (FontId::from(STYLE_DIALOGUE), 0.8)
    };

    Classification { font, confidence }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb as ImageRgb;

    fn crop(w: u32, h: u32) -> RgbImage {
        RgbImage::from_pixel(w, h, ImageRgb([255, 255, 255]))
    }

    #[test]
    fn punctuation_is_detected_in_japanese_text() {
        assert!(cheap_features(100, 50, "こんにちは。").has_punctuation);
        assert!(!cheap_features(100, 50, "ドドド").has_punctuation);
    }

    #[test]
    fn aspect_ratio_handles_zero_height() {
        assert_eq!(cheap_features(100, 0, "x").aspect_ratio, 0.0);
    }

    #[test]
    fn large_unpunctuated_text_classifies_as_sfx() {
        // 3 chars across a large area, no punctuation.
        let c = classify_full(&crop(300, 200), "ドドド", None);
        assert_eq!(c.font.as_str(), STYLE_SFX);
        assert!(c.confidence >= MIN_CLASSIFY_CONFIDENCE);
    }

    #[test]
    fn ordinary_dialogue_classifies_as_dialogue() {
        let c = classify_full(&crop(120, 90), "こんにちは、元気ですか。", None);
        assert_eq!(c.font.as_str(), STYLE_DIALOGUE);
    }

    #[test]
    fn wide_punctuated_block_classifies_as_narration() {
        let c = classify_full(&crop(400, 60), "その日、彼は町を出た。", None);
        assert_eq!(c.font.as_str(), STYLE_NARRATION);
    }

    #[test]
    fn bold_dialogue_classifies_as_emphasis() {
        let c = classify_full(&crop(120, 90), "やめろ！", Some(true));
        assert_eq!(c.font.as_str(), STYLE_EMPHASIS);
    }

    #[test]
    fn empty_text_yields_unusable_confidence() {
        let c = classify_full(&crop(100, 100), "", None);
        assert!(
            c.confidence < MIN_CLASSIFY_CONFIDENCE,
            "an empty block must not be admitted to the vote pool"
        );
    }
}
