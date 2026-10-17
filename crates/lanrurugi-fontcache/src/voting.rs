//! Voting stage: accumulating the vote pool and locking the golden set (FR-008, research.md §4).
//!
//! Cover pages are excluded from the sample entirely — covers use stylised title/logo lettering
//! that is unrepresentative of body dialogue, so voting on them would bias the whole volume. The
//! `is_cover` flag comes from Phase 1's own page metadata, never inferred from OCR output.
//!
//! **Concurrency (constitution Principle III)**: the full classifier is heavy CPU work, so a batch
//! of blocks is classified through a single batch-wide `parallel_map` dispatch — never a loop
//! issuing one `spawn_blocking` per block.

use image::RgbImage;
use lanrurugi_core::concurrency::{parallel_map, BlockingTaskError};

use crate::classify::{classify_full, Classification, MIN_CLASSIFY_CONFIDENCE};
use crate::entities::{FontId, VolumeFontPattern, GOLDEN_SET_MIN_SHARE, MAX_GOLDEN_SET};

/// One block offered to the voting stage.
pub struct VoteCandidate {
    pub crop: RgbImage,
    pub text: String,
    pub is_bold: Option<bool>,
    /// From Phase 1's page metadata. Cover blocks are dropped before classification even runs.
    pub is_cover: bool,
}

/// Classifies a batch of blocks off the async reactor, in one rayon dispatch.
pub async fn classify_batch(
    candidates: Vec<VoteCandidate>,
) -> Result<Vec<Classification>, BlockingTaskError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    parallel_map(candidates, |c| classify_full(&c.crop, &c.text, c.is_bold)).await
}

/// Adds classified blocks to `pattern`'s vote pool, then locks if there's now enough evidence.
///
/// Returns whether the pattern locked as a result of this call.
pub fn accumulate_votes(
    pattern: &mut VolumeFontPattern,
    classifications: &[(Classification, bool)],
) -> bool {
    if pattern.is_locked {
        return false; // The pool is frozen once locked.
    }

    for (classification, is_cover) in classifications {
        // Two independent exclusions: cover pages (FR-008) and low-confidence guesses.
        if *is_cover || classification.confidence < MIN_CLASSIFY_CONFIDENCE {
            continue;
        }
        *pattern
            .vote_pool
            .entry(classification.font.clone())
            .or_insert(0) += 1;
    }

    if pattern.is_ready_to_lock() {
        lock_golden_set(pattern);
        return true;
    }
    false
}

/// Freezes the top fonts into the golden set.
///
/// Takes at most [`MAX_GOLDEN_SET`] fonts, each holding at least [`GOLDEN_SET_MIN_SHARE`] of the
/// pool — preserving legitimate intra-volume variety (dialogue vs. sound effects) without letting
/// a handful of stray classifications earn a permanent slot.
pub fn lock_golden_set(pattern: &mut VolumeFontPattern) {
    let total = pattern.total_votes();
    if total == 0 {
        return;
    }

    let mut ranked: Vec<(FontId, u32)> = pattern
        .vote_pool
        .iter()
        .map(|(f, &c)| (f.clone(), c))
        .collect();

    // Count descending, then font id ascending so ties are deterministic rather than
    // map-iteration-order dependent.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let golden: Vec<FontId> = ranked
        .into_iter()
        .filter(|(_, count)| (*count as f32 / total as f32) >= GOLDEN_SET_MIN_SHARE)
        .take(MAX_GOLDEN_SET)
        .map(|(font, _)| font)
        .collect();

    // Never lock an empty set: if the share filter excluded everything, keep the single most
    // common font rather than leaving the volume with no font to route to.
    pattern.golden_set = if golden.is_empty() {
        pattern
            .vote_pool
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(f, _)| vec![f.clone()])
            .unwrap_or_default()
    } else {
        golden
    };

    pattern.is_locked = !pattern.golden_set.is_empty();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{VolumeFontPattern, MIN_VOTES_TO_LOCK};
    use lanrurugi_ocr::entities::VolumeId;

    fn classification(font: &str, confidence: f32) -> Classification {
        Classification {
            font: FontId::from(font),
            confidence,
        }
    }

    fn pattern() -> VolumeFontPattern {
        VolumeFontPattern::new(&VolumeId::from("vol-1"))
    }

    #[test]
    fn cover_blocks_never_reach_the_vote_pool() {
        let mut p = pattern();
        let votes: Vec<_> = (0..MIN_VOTES_TO_LOCK)
            .map(|_| (classification("dialogue-gothic", 0.9), true))
            .collect();

        accumulate_votes(&mut p, &votes);

        assert_eq!(p.total_votes(), 0, "cover pages must be excluded (FR-008)");
        assert!(!p.is_locked);
    }

    #[test]
    fn low_confidence_classifications_are_excluded() {
        let mut p = pattern();
        let votes = vec![(classification("dialogue-gothic", 0.1), false)];
        accumulate_votes(&mut p, &votes);
        assert_eq!(p.total_votes(), 0);
    }

    #[test]
    fn pattern_locks_once_enough_votes_accumulate() {
        let mut p = pattern();
        let votes: Vec<_> = (0..MIN_VOTES_TO_LOCK)
            .map(|_| (classification("dialogue-gothic", 0.9), false))
            .collect();

        let locked = accumulate_votes(&mut p, &votes);

        assert!(locked);
        assert!(p.is_locked);
        assert_eq!(p.golden_set, vec![FontId::from("dialogue-gothic")]);
    }

    #[test]
    fn golden_set_keeps_genuine_variety_but_caps_its_size() {
        let mut p = pattern();
        let mut votes = Vec::new();
        for _ in 0..20 {
            votes.push((classification("dialogue-gothic", 0.9), false));
        }
        for _ in 0..10 {
            votes.push((classification("sfx-brush", 0.9), false));
        }
        for _ in 0..8 {
            votes.push((classification("narration-mincho", 0.9), false));
        }
        for _ in 0..7 {
            votes.push((classification("dialogue-bold", 0.9), false));
        }

        accumulate_votes(&mut p, &votes);

        assert!(p.is_locked);
        assert!(p.golden_set.len() <= MAX_GOLDEN_SET);
        assert!(p.golden_set.contains(&FontId::from("dialogue-gothic")));
        assert!(
            p.golden_set.contains(&FontId::from("sfx-brush")),
            "a genuinely common second style must survive"
        );
    }

    #[test]
    fn rare_fonts_are_excluded_from_the_golden_set() {
        let mut p = pattern();
        let mut votes: Vec<_> = (0..40)
            .map(|_| (classification("dialogue-gothic", 0.9), false))
            .collect();
        votes.push((classification("sfx-brush", 0.9), false));

        accumulate_votes(&mut p, &votes);

        assert_eq!(
            p.golden_set,
            vec![FontId::from("dialogue-gothic")],
            "a single stray classification must not earn a golden-set slot"
        );
    }

    #[test]
    fn a_locked_pool_does_not_keep_accumulating() {
        let mut p = pattern();
        let votes: Vec<_> = (0..MIN_VOTES_TO_LOCK)
            .map(|_| (classification("dialogue-gothic", 0.9), false))
            .collect();
        accumulate_votes(&mut p, &votes);
        let frozen = p.total_votes();

        accumulate_votes(&mut p, &votes);

        assert_eq!(
            p.total_votes(),
            frozen,
            "vote pool must freeze at lock time"
        );
    }
}
