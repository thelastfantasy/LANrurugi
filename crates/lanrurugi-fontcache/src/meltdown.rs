//! Meltdown re-classification for blocks that don't match a locked pattern (research.md §4).
//!
//! **The resolved review concern**: a meltdown result NEVER feeds `vote_pool`. It accumulates in a
//! separate `meltdown_tally`, and only a font recurring often enough *on its own* is considered for
//! promotion into the golden set. Merging the two pools (the original naive sketch) would let a
//! recurring "special scene" font gradually displace a legitimate low-frequency main font — the
//! exact failure mode this split exists to prevent.
//!
//! **Concurrency (constitution Principle III)**: like voting, this re-runs the full classifier, so
//! a batch goes through one batch-wide `parallel_map` dispatch, never a per-block `spawn_blocking`
//! loop.

use lanrurugi_core::concurrency::BlockingTaskError;

use crate::classify::{Classification, MIN_CLASSIFY_CONFIDENCE};
use crate::entities::{FontId, VolumeFontPattern, MAX_GOLDEN_SET, MELTDOWN_PROMOTION_THRESHOLD};
use crate::voting::VoteCandidate;

/// Re-classifies a batch of outlier blocks off the async reactor, in one rayon dispatch.
///
/// Same shape as [`crate::voting::classify_batch`] — the two stages differ in what they do with the
/// result, not in how they dispatch the work, so they share the batching helper rather than
/// hand-rolling two near-identical dispatches.
pub async fn reclassify_batch(
    candidates: Vec<VoteCandidate>,
) -> Result<Vec<Classification>, BlockingTaskError> {
    crate::voting::classify_batch(candidates).await
}

/// Records a meltdown re-classification result.
///
/// Returns the font to render this block with — the re-classified font itself, since the whole
/// point of meltdown is that this block genuinely isn't the volume's normal style (FR-009).
pub fn record_meltdown(
    pattern: &mut VolumeFontPattern,
    classification: &Classification,
) -> Option<FontId> {
    if classification.confidence < MIN_CLASSIFY_CONFIDENCE {
        return None;
    }

    *pattern
        .meltdown_tally
        .entry(classification.font.clone())
        .or_insert(0) += 1;

    Some(classification.font.clone())
}

/// Promotes any meltdown font that has recurred often enough into the golden set.
///
/// Deliberately additive: promotion never evicts an existing golden-set member. It only runs when
/// the golden set has room, so a recurring special-scene font can earn a slot without displacing an
/// established main font — the core of the resolved review concern.
///
/// Returns the fonts promoted by this call.
pub fn promote_recurring_meltdowns(pattern: &mut VolumeFontPattern) -> Vec<FontId> {
    if !pattern.is_locked || pattern.golden_set.len() >= MAX_GOLDEN_SET {
        return Vec::new();
    }

    let mut eligible: Vec<(FontId, u32)> = pattern
        .meltdown_tally
        .iter()
        .filter(|(font, &count)| {
            count >= MELTDOWN_PROMOTION_THRESHOLD && !pattern.golden_set.contains(font)
        })
        .map(|(f, &c)| (f.clone(), c))
        .collect();

    // Most-recurring first; font id breaks ties deterministically.
    eligible.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let room = MAX_GOLDEN_SET - pattern.golden_set.len();
    let promoted: Vec<FontId> = eligible.into_iter().take(room).map(|(f, _)| f).collect();

    for font in &promoted {
        pattern.golden_set.push(font.clone());
        // Clear the promoted font's tally: it is now a first-class golden-set member, and leaving
        // a stale count behind would let it be "re-promoted" if it were ever removed.
        pattern.meltdown_tally.remove(font);
    }

    promoted
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_ocr::entities::VolumeId;

    fn locked_pattern(fonts: &[&str]) -> VolumeFontPattern {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.golden_set = fonts.iter().map(|f| FontId::from(*f)).collect();
        p.is_locked = true;
        p.vote_pool.insert(FontId::from("dialogue-gothic"), 40);
        p
    }

    fn classification(font: &str) -> Classification {
        Classification {
            font: FontId::from(font),
            confidence: 0.9,
        }
    }

    #[test]
    fn meltdown_never_touches_the_vote_pool() {
        let mut p = locked_pattern(&["dialogue-gothic"]);
        let pool_before = p.vote_pool.clone();

        for _ in 0..50 {
            record_meltdown(&mut p, &classification("flashback-serif"));
        }

        assert_eq!(
            p.vote_pool, pool_before,
            "meltdown results must never feed the primary vote pool"
        );
        assert_eq!(p.meltdown_tally[&FontId::from("flashback-serif")], 50);
    }

    #[test]
    fn low_confidence_meltdowns_are_ignored() {
        let mut p = locked_pattern(&["dialogue-gothic"]);
        let weak = Classification {
            font: FontId::from("flashback-serif"),
            confidence: 0.1,
        };
        assert!(record_meltdown(&mut p, &weak).is_none());
        assert!(p.meltdown_tally.is_empty());
    }

    #[test]
    fn an_outlier_renders_in_its_own_reclassified_font() {
        let mut p = locked_pattern(&["dialogue-gothic"]);
        let font = record_meltdown(&mut p, &classification("flashback-serif"));
        assert_eq!(font, Some(FontId::from("flashback-serif")));
    }

    #[test]
    fn a_rare_outlier_is_never_promoted() {
        let mut p = locked_pattern(&["dialogue-gothic"]);
        for _ in 0..(MELTDOWN_PROMOTION_THRESHOLD - 1) {
            record_meltdown(&mut p, &classification("flashback-serif"));
        }
        assert!(promote_recurring_meltdowns(&mut p).is_empty());
        assert_eq!(p.golden_set, vec![FontId::from("dialogue-gothic")]);
    }

    #[test]
    fn a_persistently_recurring_outlier_earns_a_slot() {
        let mut p = locked_pattern(&["dialogue-gothic"]);
        for _ in 0..MELTDOWN_PROMOTION_THRESHOLD {
            record_meltdown(&mut p, &classification("flashback-serif"));
        }

        let promoted = promote_recurring_meltdowns(&mut p);

        assert_eq!(promoted, vec![FontId::from("flashback-serif")]);
        assert!(p.golden_set.contains(&FontId::from("flashback-serif")));
        assert!(
            p.golden_set.contains(&FontId::from("dialogue-gothic")),
            "promotion must never evict an established font"
        );
        assert!(!p
            .meltdown_tally
            .contains_key(&FontId::from("flashback-serif")));
    }

    #[test]
    fn promotion_stops_when_the_golden_set_is_full() {
        let mut p = locked_pattern(&["a", "b", "c"]);
        for _ in 0..(MELTDOWN_PROMOTION_THRESHOLD * 3) {
            record_meltdown(&mut p, &classification("flashback-serif"));
        }
        assert!(promote_recurring_meltdowns(&mut p).is_empty());
        assert_eq!(p.golden_set.len(), MAX_GOLDEN_SET);
    }
}
