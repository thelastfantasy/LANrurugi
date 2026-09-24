//! The locked-volume fast path: routing a block among the golden set (research.md §4).
//!
//! Deliberately **two sequential checks, not one combined branch** — a lock-state check first, then
//! (only if locked) a cheap per-block feature classifier deciding "matches the established pattern"
//! vs. "outlier → meltdown". They are separate functions because they answer separate questions,
//! which is the clarity concern raised in the original design review.
//!
//! The fast path routes among *all* fonts in the golden set (typically 2–3), not just the single
//! most common one, so legitimate intra-volume variety (dialogue vs. sound effects) survives.

use crate::classify::{cheap_features, CheapFeatures};
use crate::entities::{FontId, VolumeFontPattern};

/// What the fast path decided for one block.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// The volume hasn't locked yet — this block belongs in the voting stage.
    NotLocked,
    /// Matched a golden-set font.
    Matched(FontId),
    /// A clear outlier from the established pattern — hand to meltdown re-classification.
    Outlier,
}

/// **Check 1**: is this volume's pattern locked at all?
///
/// Separated from [`route_block`] so the caller's control flow states the two questions
/// independently rather than folding them into one condition.
pub fn is_locked(pattern: &VolumeFontPattern) -> bool {
    pattern.is_locked && !pattern.golden_set.is_empty()
}

/// **Check 2**: route an already-locked volume's block among its golden set.
///
/// Returns [`RouteDecision::NotLocked`] if called on an unlocked pattern, so a caller that skips
/// check 1 still can't silently route against an empty golden set.
pub fn route_block(
    pattern: &VolumeFontPattern,
    width: u32,
    height: u32,
    text: &str,
) -> RouteDecision {
    if !is_locked(pattern) {
        return RouteDecision::NotLocked;
    }

    let features = cheap_features(width, height, text);

    // Score every golden-set font on the cheap features and take the best, provided it clears the
    // outlier floor. Scoring all of them (rather than defaulting to the most common) is what keeps
    // sound-effect lettering from being collapsed into the dialogue font.
    let best = pattern
        .golden_set
        .iter()
        .map(|font| (font, affinity(font, &features)))
        .max_by(|a, b| a.1.total_cmp(&b.1));

    match best {
        Some((font, score)) if score >= OUTLIER_FLOOR => RouteDecision::Matched(font.clone()),
        _ => RouteDecision::Outlier,
    }
}

/// Below this best-affinity score the block doesn't resemble anything in the golden set and is
/// treated as an outlier (FR-009) rather than forced into the volume's normal pattern.
pub const OUTLIER_FLOOR: f32 = 0.3;

/// How well a block's cheap features match a given style, in 0.0–1.0.
fn affinity(font: &FontId, f: &CheapFeatures) -> f32 {
    let big_glyphs = f.area_per_char > 4_000.0;
    let short = f.char_count <= 6;
    let wide = f.aspect_ratio > 2.5;

    match font.as_str() {
        crate::classify::STYLE_SFX => {
            // Sound effects: big, short, unpunctuated.
            score(&[(big_glyphs, 0.4), (short, 0.3), (!f.has_punctuation, 0.3)])
        }
        crate::classify::STYLE_NARRATION => {
            score(&[(f.has_punctuation, 0.4), (wide, 0.4), (!big_glyphs, 0.2)])
        }
        crate::classify::STYLE_EMPHASIS => {
            score(&[(!f.has_punctuation, 0.3), (short, 0.3), (!big_glyphs, 0.4)])
        }
        // Dialogue is the general case: punctuated, moderately sized, not extreme in shape.
        _ => score(&[(f.has_punctuation, 0.4), (!big_glyphs, 0.3), (!wide, 0.3)]),
    }
}

fn score(signals: &[(bool, f32)]) -> f32 {
    signals
        .iter()
        .filter(|(hit, _)| *hit)
        .map(|(_, weight)| weight)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_ocr::entities::VolumeId;

    fn locked_pattern(fonts: &[&str]) -> VolumeFontPattern {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.golden_set = fonts.iter().map(|f| FontId::from(*f)).collect();
        p.is_locked = true;
        p
    }

    #[test]
    fn unlocked_pattern_routes_to_voting() {
        let p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        assert!(!is_locked(&p));
        assert_eq!(
            route_block(&p, 100, 50, "こんにちは。"),
            RouteDecision::NotLocked
        );
    }

    #[test]
    fn a_locked_flag_with_an_empty_golden_set_is_not_locked() {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.is_locked = true;
        assert!(
            !is_locked(&p),
            "an empty golden set has nothing to route to"
        );
    }

    #[test]
    fn dialogue_routes_to_the_dialogue_font() {
        let p = locked_pattern(&["dialogue-gothic", "sfx-brush"]);
        assert_eq!(
            route_block(&p, 150, 100, "こんにちは、元気ですか。"),
            RouteDecision::Matched(FontId::from("dialogue-gothic"))
        );
    }

    #[test]
    fn sound_effects_route_to_the_sfx_font_not_the_most_common_one() {
        // The key property from research.md §4: the fast path routes among all golden-set fonts
        // rather than collapsing everything into the single most common font.
        let p = locked_pattern(&["dialogue-gothic", "sfx-brush"]);
        assert_eq!(
            route_block(&p, 400, 300, "ドドド"),
            RouteDecision::Matched(FontId::from("sfx-brush"))
        );
    }

    #[test]
    fn a_block_matching_nothing_is_an_outlier() {
        // Golden set holds only sfx; a punctuated, small-glyph dialogue block matches it poorly.
        let p = locked_pattern(&["sfx-brush"]);
        assert_eq!(
            route_block(&p, 120, 90, "こんにちは、元気ですか。"),
            RouteDecision::Outlier
        );
    }
}
