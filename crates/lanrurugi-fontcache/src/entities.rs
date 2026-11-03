//! The Volume Font Pattern entity and its Redis schema (data-model.md).
//!
//! Three-stage lifecycle (research.md §4): **voting** accumulates a per-font tally from non-cover
//! pages; **locking** freezes a small golden set once there's enough evidence; **meltdown** handles
//! blocks that don't match the locked pattern — crucially into a *separate* tally that never feeds
//! the vote pool, so a recurring "special scene" font can't gradually displace a legitimate
//! low-frequency main font.

use std::collections::BTreeMap;

use lanrurugi_ocr::entities::VolumeId;
use serde::{Deserialize, Serialize};

/// How many classified non-cover blocks must accumulate before the pattern may lock. Below this
/// the sample is too small to trust — locking early on unrepresentative pages is the failure mode
/// FR-010's reset exists to rescue.
pub const MIN_VOTES_TO_LOCK: u32 = 24;

/// Upper bound on the golden set. Keeping it small is the point (FR-008: "a small, consistent
/// set"), while still allowing genuine intra-volume variety such as dialogue vs. sound effects.
pub const MAX_GOLDEN_SET: usize = 3;

/// A font must hold at least this share of the vote pool to enter the golden set, so a handful of
/// stray classifications don't earn a permanent slot.
pub const GOLDEN_SET_MIN_SHARE: f32 = 0.12;

/// How many times a meltdown-classified font must recur on its own before it's considered for
/// promotion into the golden set (research.md §4).
pub const MELTDOWN_PROMOTION_THRESHOLD: u32 = 12;

/// A font identifier as produced by the classifier and consumed by the compositor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FontId(pub String);

impl FontId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for FontId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for FontId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for FontId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The per-volume font pattern record.
///
/// `BTreeMap` rather than `HashMap` for both tallies so the serialized Redis value is
/// deterministic — an unordered map would produce a different JSON string for identical state,
/// defeating any downstream diffing or cache comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VolumeFontPattern {
    pub volume_id: String,
    pub is_locked: bool,
    /// Populated only from non-cover pages during voting; frozen once locked.
    pub vote_pool: BTreeMap<FontId, u32>,
    /// Selected from `vote_pool` at lock time.
    pub golden_set: Vec<FontId>,
    /// Separate from `vote_pool` by design — meltdown results never feed the primary pool.
    pub meltdown_tally: BTreeMap<FontId, u32>,
}

impl VolumeFontPattern {
    pub fn new(volume_id: &VolumeId) -> Self {
        Self {
            volume_id: volume_id.as_str().to_string(),
            ..Default::default()
        }
    }

    /// Total classified blocks in the vote pool.
    pub fn total_votes(&self) -> u32 {
        self.vote_pool.values().sum()
    }

    /// Whether there is now enough evidence to lock.
    pub fn is_ready_to_lock(&self) -> bool {
        !self.is_locked && self.total_votes() >= MIN_VOTES_TO_LOCK
    }

    /// Clears everything back to the unlocked state (FR-010).
    pub fn reset(&mut self) {
        self.is_locked = false;
        self.vote_pool.clear();
        self.golden_set.clear();
        self.meltdown_tally.clear();
    }
}

/// Redis key for a volume's font pattern. A namespace of this feature's own — nothing here touches
/// a Phase 1 archive/category/tankoubon key (constitution Principle I).
pub fn font_pattern_key(volume_id: &VolumeId) -> String {
    format!("LRR_TRANSLATION_FONTPATTERN_{}", volume_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pattern_is_unlocked_and_empty() {
        let p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        assert!(!p.is_locked);
        assert_eq!(p.total_votes(), 0);
        assert!(!p.is_ready_to_lock());
    }

    #[test]
    fn readiness_requires_the_minimum_vote_count() {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.vote_pool
            .insert(FontId::from("gothic"), MIN_VOTES_TO_LOCK - 1);
        assert!(!p.is_ready_to_lock());
        p.vote_pool
            .insert(FontId::from("gothic"), MIN_VOTES_TO_LOCK);
        assert!(p.is_ready_to_lock());
    }

    #[test]
    fn a_locked_pattern_is_never_ready_to_lock_again() {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.vote_pool.insert(FontId::from("gothic"), 100);
        p.is_locked = true;
        assert!(!p.is_ready_to_lock());
    }

    #[test]
    fn reset_clears_both_tallies_and_the_lock() {
        let mut p = VolumeFontPattern::new(&VolumeId::from("vol-1"));
        p.vote_pool.insert(FontId::from("gothic"), 50);
        p.meltdown_tally.insert(FontId::from("brush"), 9);
        p.golden_set.push(FontId::from("gothic"));
        p.is_locked = true;

        p.reset();

        assert!(!p.is_locked);
        assert!(p.vote_pool.is_empty());
        assert!(p.meltdown_tally.is_empty());
        assert!(p.golden_set.is_empty());
    }

    #[test]
    fn key_is_namespaced_away_from_phase_one_keys() {
        let key = font_pattern_key(&VolumeId::from("vol-1"));
        assert!(key.starts_with("LRR_TRANSLATION_"));
    }
}
