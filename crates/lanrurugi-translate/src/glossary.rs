//! The per-volume Terminology Glossary (FR-007a–d, data-model.md, research.md §14).
//!
//! A name/term → chosen-translation map, scoped per volume (or per archive when ungrouped,
//! mirroring Volume Font Pattern's own scoping). Entries are captured automatically the first time
//! a term is translated — no confirmation gate, consistent with this feature's low-friction posture
//! — and a wrong entry is corrected after the fact rather than prevented up front.
//!
//! Deliberately **no bulk-clear operation**, unlike Volume Font Pattern's reset: a wrong entry is
//! independent of the others, so wiping the whole glossary would discard already-correct entries
//! for no benefit.

use std::collections::BTreeMap;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_ocr::entities::VolumeId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GlossaryError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, GlossaryError>;

/// One term's translation as captured from one specific source location — the (archive, chapter)
/// pair it was actually seen in (issue #105). A Tankoubon groups multiple otherwise-unrelated
/// archives under one `VolumeId`, and a single archive's own `toc` can itself span multiple
/// unrelated anthology chapters — so the *same* source string (a common name like "太郎") can
/// legitimately need *different* translations depending on which work it actually appeared in.
/// Scoping every entry to its origin, rather than one global translation per string, is what makes
/// that distinguishable instead of the second occurrence silently reusing the first's translation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub translation: String,
    pub archive_id: String,
    /// The `toc` chapter name active at the page this term was captured on, if the archive has a
    /// table of contents and the page falls within one of its entries. `None` for an archive with
    /// no `toc` at all, or a page before its first `toc` entry.
    #[serde(default)]
    pub chapter_name: Option<String>,
}

/// A volume's glossary.
///
/// `BTreeMap` keeps serialization deterministic — important because the glossary is rendered into
/// the *cacheable prefix* of every translation request: an unordered map would reshuffle that
/// prefix between requests and silently destroy every prompt-cache hit (research.md §15). Each
/// term maps to a `Vec` of [`GlossaryEntry`] (issue #105) rather than one translation, because the
/// same source string can legitimately need different translations depending on which archive/
/// chapter it came from (see that type's own docs) — the overwhelming common case is still exactly
/// one entry per term, so callers that don't care about disambiguation can just take the first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerminologyGlossary {
    pub volume_id: String,
    pub entries: BTreeMap<String, Vec<GlossaryEntry>>,
}

impl TerminologyGlossary {
    pub fn new(volume_id: &VolumeId) -> Self {
        Self {
            volume_id: volume_id.as_str().to_string(),
            entries: BTreeMap::new(),
        }
    }

    /// Entries whose source term appears verbatim in `source_text` — the deterministic, free,
    /// zero-LLM-judgment path (FR-007b). Every source (archive, chapter) this term was ever seen
    /// under is returned alongside its own translation — the caller decides how to present multiple
    /// candidates (issue #105: only when there's genuinely more than one does the prompt need to
    /// spell out the disambiguating source info at all).
    ///
    /// Longest terms first, so a full name is preferred over a shorter name nested inside it.
    pub fn exact_matches(&self, source_text: &str) -> Vec<(String, Vec<GlossaryEntry>)> {
        let mut matches: Vec<(String, Vec<GlossaryEntry>)> = self
            .entries
            .iter()
            .filter(|(term, _)| !term.is_empty() && source_text.contains(term.as_str()))
            .map(|(t, v)| (t.clone(), v.clone()))
            .collect();

        matches.sort_by(|a, b| {
            b.0.chars()
                .count()
                .cmp(&a.0.chars().count())
                .then_with(|| a.0.cmp(&b.0))
        });
        matches
    }

    /// Every known source term, names only.
    ///
    /// Sent as context so the backend can recognise a nickname or initialism referring to an
    /// already-known name (FR-007c) — the judgment a substring rule provably cannot make, and which
    /// would misfire on a genuine unrelated acronym if attempted by shape alone.
    pub fn known_names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Captures a first-seen term for one (archive, chapter) source (FR-007a, issue #105). Never
    /// overwrites an existing entry *for that same source* — an established translation is what
    /// consistency depends on, and a user correction (FR-007d) must not be silently reverted by a
    /// later automatic capture. A different source seeing the same term string for the first time
    /// gets its own independent entry rather than being folded into an unrelated one.
    ///
    /// Returns whether this call actually added an entry.
    pub fn capture(
        &mut self,
        source_term: &str,
        translation: &str,
        archive_id: &str,
        chapter_name: Option<&str>,
    ) -> bool {
        let term = source_term.trim();
        let translation = translation.trim();
        if term.is_empty() || translation.is_empty() {
            return false;
        }
        let existing = self.entries.entry(term.to_string()).or_default();
        if existing
            .iter()
            .any(|e| e.archive_id == archive_id && e.chapter_name.as_deref() == chapter_name)
        {
            return false;
        }
        existing.push(GlossaryEntry {
            translation: translation.to_string(),
            archive_id: archive_id.to_string(),
            chapter_name: chapter_name.map(str::to_string),
        });
        true
    }

    /// User edit of a single entry (FR-007d) — unlike [`Self::capture`], this does overwrite. Edits
    /// the entry for the given (archive, chapter) source if one already exists there, otherwise adds
    /// a new one for it — an edit is always scoped to one specific source, never a blanket rewrite
    /// of every source sharing this term string.
    pub fn set(
        &mut self,
        source_term: &str,
        translation: &str,
        archive_id: &str,
        chapter_name: Option<&str>,
    ) {
        let term = source_term.trim().to_string();
        let translation = translation.trim().to_string();
        let existing = self.entries.entry(term).or_default();
        match existing
            .iter_mut()
            .find(|e| e.archive_id == archive_id && e.chapter_name.as_deref() == chapter_name)
        {
            Some(entry) => entry.translation = translation,
            None => existing.push(GlossaryEntry {
                translation,
                archive_id: archive_id.to_string(),
                chapter_name: chapter_name.map(str::to_string),
            }),
        }
    }

    /// Removes every source's entry for `source_term` — a term-level removal, not scoped to one
    /// source, matching the existing single-argument removal endpoint's contract (no per-source
    /// removal UI exists yet).
    pub fn remove(&mut self, source_term: &str) -> bool {
        self.entries.remove(source_term.trim()).is_some()
    }
}

/// Redis key for a volume's glossary — this feature's own additive namespace.
pub fn glossary_key(volume_id: &VolumeId) -> String {
    format!("LRR_TRANSLATION_GLOSSARY_{}", volume_id.as_str())
}

#[derive(Clone)]
pub struct GlossaryRepository {
    pool: Pool,
}

impl GlossaryRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, volume_id: &VolumeId) -> Result<TerminologyGlossary> {
        let key = glossary_key(volume_id);
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(&key).await?;

        match raw {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| GlossaryError::Json(key.clone(), e))
            }
            None => Ok(TerminologyGlossary::new(volume_id)),
        }
    }

    pub async fn save(&self, glossary: &TerminologyGlossary) -> Result<()> {
        let key = glossary_key(&VolumeId::from(glossary.volume_id.clone()));
        let json =
            serde_json::to_string(glossary).map_err(|e| GlossaryError::Json(key.clone(), e))?;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(&key, json).await?;
        Ok(())
    }

    /// Every stored glossary — used by backup/export (FR-022).
    pub async fn list_all(&self) -> Result<Vec<TerminologyGlossary>> {
        let mut conn = self.pool.get().await?;
        let keys: Vec<String> = conn.keys("LRR_TRANSLATION_GLOSSARY_*").await?;

        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let raw: Option<String> = conn.get(&key).await?;
            if let Some(json) = raw {
                match serde_json::from_str(&json) {
                    Ok(g) => out.push(g),
                    Err(e) => {
                        tracing::warn!(key = %key, error = %e, "skipping malformed glossary")
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary() -> TerminologyGlossary {
        TerminologyGlossary::new(&VolumeId::from("vol-1"))
    }

    const ARC_A: &str = "archive-a";
    const ARC_B: &str = "archive-b";

    #[test]
    fn a_first_seen_term_is_captured_without_confirmation() {
        let mut g = glossary();
        assert!(g.capture("さゆき", "Sayuki", ARC_A, None));
        assert_eq!(
            g.entries.get("さゆき").map(|v| v[0].translation.as_str()),
            Some("Sayuki")
        );
    }

    #[test]
    fn capture_never_overwrites_an_established_translation_for_the_same_source() {
        let mut g = glossary();
        g.capture("さゆき", "Sayuki", ARC_A, None);
        assert!(!g.capture("さゆき", "Sayuki-chan", ARC_A, None));
        assert_eq!(g.entries["さゆき"][0].translation, "Sayuki");
    }

    #[test]
    fn capture_never_reverts_a_user_correction() {
        // FR-007d: an edit takes effect on the next request and must survive later auto-capture.
        let mut g = glossary();
        g.capture("さゆき", "Sayuke", ARC_A, None);
        g.set("さゆき", "Sayuki", ARC_A, None);
        g.capture("さゆき", "Sayuke", ARC_A, None);
        assert_eq!(g.entries["さゆき"][0].translation, "Sayuki");
    }

    #[test]
    fn empty_terms_are_not_captured() {
        let mut g = glossary();
        assert!(!g.capture("  ", "Something", ARC_A, None));
        assert!(!g.capture("さゆき", "   ", ARC_A, None));
        assert!(g.entries.is_empty());
    }

    #[test]
    fn exact_substring_matching_finds_terms_in_a_block() {
        let mut g = glossary();
        g.capture("さゆき", "Sayuki", ARC_A, None);
        g.capture("たけし", "Takeshi", ARC_A, None);

        let matches = g.exact_matches("さゆき、おはよう");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1[0].translation, "Sayuki");
    }

    #[test]
    fn longer_terms_are_matched_before_shorter_nested_ones() {
        let mut g = glossary();
        g.capture("山田", "Yamada", ARC_A, None);
        g.capture("山田太郎", "Taro Yamada", ARC_A, None);

        let matches = g.exact_matches("山田太郎です");
        assert_eq!(
            matches[0].0, "山田太郎",
            "the more specific term must take precedence"
        );
    }

    #[test]
    fn known_names_carry_no_translations() {
        // FR-007c sends names only — sending full pairs would grow the prefix unboundedly.
        let mut g = glossary();
        g.capture("さゆき", "Sayuki", ARC_A, None);
        assert_eq!(g.known_names(), vec!["さゆき".to_string()]);
    }

    #[test]
    fn a_single_entry_can_be_removed_without_touching_the_others() {
        let mut g = glossary();
        g.capture("さゆき", "Sayuki", ARC_A, None);
        g.capture("たけし", "Takeshi", ARC_A, None);

        assert!(g.remove("さゆき"));
        assert!(!g.remove("さゆき"));
        assert_eq!(g.entries.len(), 1, "other entries must survive (FR-007d)");
    }

    #[test]
    fn serialization_is_deterministic_for_prompt_cache_stability() {
        let mut a = glossary();
        let mut b = glossary();
        for term in ["ち", "あ", "た", "さ"] {
            a.capture(term, "x", ARC_A, None);
        }
        for term in ["さ", "た", "あ", "ち"] {
            b.capture(term, "x", ARC_A, None);
        }
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "insertion order must not change the rendered prefix"
        );
    }

    // --- issue #105: cross-archive/chapter disambiguation -------------------------------------

    #[test]
    fn the_same_term_from_a_different_archive_gets_its_own_independent_translation() {
        let mut g = glossary();
        assert!(g.capture("太郎", "Taro", ARC_A, None));
        assert!(
            g.capture("太郎", "Jiro", ARC_B, None),
            "a different archive seeing this term for the first time must get its own entry, \
             not be silently folded into archive A's"
        );
        assert_eq!(g.entries["太郎"].len(), 2);
        let a_entry = g.entries["太郎"]
            .iter()
            .find(|e| e.archive_id == ARC_A)
            .unwrap();
        let b_entry = g.entries["太郎"]
            .iter()
            .find(|e| e.archive_id == ARC_B)
            .unwrap();
        assert_eq!(a_entry.translation, "Taro");
        assert_eq!(b_entry.translation, "Jiro");
    }

    #[test]
    fn the_same_term_from_a_different_chapter_of_the_same_archive_also_gets_its_own_entry() {
        let mut g = glossary();
        g.capture("太郎", "Taro", ARC_A, Some("Chapter 1"));
        assert!(
            g.capture("太郎", "Jiro", ARC_A, Some("Chapter 2")),
            "a different chapter within the same anthology archive is a different source too"
        );
        assert_eq!(g.entries["太郎"].len(), 2);
    }

    #[test]
    fn exact_matches_returns_every_source_candidate_for_an_ambiguous_term() {
        let mut g = glossary();
        g.capture("太郎", "Taro", ARC_A, None);
        g.capture("太郎", "Jiro", ARC_B, None);

        let matches = g.exact_matches("太郎です");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].1.len(),
            2,
            "both source candidates must be surfaced"
        );
    }

    #[test]
    fn set_edits_only_the_entry_for_its_own_source() {
        let mut g = glossary();
        g.capture("太郎", "Taro", ARC_A, None);
        g.capture("太郎", "Jiro", ARC_B, None);

        g.set("太郎", "Taro-Corrected", ARC_A, None);

        let a_entry = g.entries["太郎"]
            .iter()
            .find(|e| e.archive_id == ARC_A)
            .unwrap();
        let b_entry = g.entries["太郎"]
            .iter()
            .find(|e| e.archive_id == ARC_B)
            .unwrap();
        assert_eq!(a_entry.translation, "Taro-Corrected");
        assert_eq!(
            b_entry.translation, "Jiro",
            "editing one source must not touch another's"
        );
    }
}
