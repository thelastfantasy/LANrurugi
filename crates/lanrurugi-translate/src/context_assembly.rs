//! Assembles a translation request's `context` (FR-007b/c/e, research.md §14).
//!
//! Three independently-optional sources, always in this order:
//! 1. **Exact-substring glossary hits** — deterministic, free, no LLM judgment (FR-007b).
//! 2. **The volume's other known names, names only** — lets the backend recognise a nickname or
//!    initialism as an already-known character (FR-007c). A shape-based rule can't do this: nickname
//!    formation is a cultural convention, and matching initialisms by shape would misfire on a real
//!    unrelated acronym.
//! 3. **The page's already-translated blocks** — advisory tone/style reference only (FR-007e).
//!    LANrurugi performs no tone classification of its own and stores no per-character tone.
//!
//! Assembly happens once, here, before any adapter sees the request — the same `context` also feeds
//! the locally-hosted-backend path via the `text-regions` endpoint, so that path gets the same
//! consistency benefit despite never routing its call through this server.

use crate::adapter::{GlossaryMatchSource, TermKind, TranslationContext};
use crate::glossary::TerminologyGlossary;

/// Caps how many tone-reference pairs ride along. Bounded on purpose: this is the one part of the
/// context that grows with page density, and an unbounded tail would work against the cost-aware
/// budgeting this feature is built around.
pub const MAX_TONE_REFERENCE_BLOCKS: usize = 8;

/// Caps the known-name list. A long-running series' cast can grow large; the list is a hint, not a
/// lookup table, so truncating it costs a little recall on rare characters rather than correctness.
pub const MAX_KNOWN_NAMES: usize = 60;

/// An already-translated block on the current page, offered as tone reference.
#[derive(Debug, Clone, PartialEq)]
pub struct TranslatedNeighbor {
    pub source_text: String,
    pub translated_text: String,
}

/// Builds the context for a batch.
///
/// `batch_sources` is every source string in the batch — exact-match lookup runs against all of
/// them so a term appearing in any block of the batch contributes its established translation.
pub fn assemble(
    glossary: &TerminologyGlossary,
    batch_sources: &[String],
    page_neighbors: &[TranslatedNeighbor],
) -> TranslationContext {
    // (1) Exact-substring hits, deduplicated across the batch's blocks. Every source candidate for
    // a term rides along (issue #105) — `TranslationContext::render_prefix` decides how much of
    // that to actually show the model (nothing extra when there's only one).
    let mut glossary_matches: Vec<(String, Vec<GlossaryMatchSource>)> = Vec::new();
    for source in batch_sources {
        for (term, entries) in glossary.exact_matches(source) {
            if !glossary_matches.iter().any(|(t, _)| t == &term) {
                let candidates = entries
                    .into_iter()
                    .map(|e| GlossaryMatchSource {
                        translation: e.translation,
                        archive_id: e.archive_id,
                        chapter_name: e.chapter_name,
                    })
                    .collect();
                glossary_matches.push((term, candidates));
            }
        }
    }

    // (2) Other known names — those already covered by an exact hit are redundant, since their
    // full translation is being sent anyway.
    let known_names: Vec<String> = glossary
        .known_names()
        .into_iter()
        .filter(|name| !glossary_matches.iter().any(|(t, _)| t == name))
        .take(MAX_KNOWN_NAMES)
        .collect();

    // (3) Tone reference from the same page, most recent first (nearest dialogue is the most
    // relevant), then truncated.
    let tone_reference: Vec<(String, String)> = page_neighbors
        .iter()
        .rev()
        .filter(|n| !n.source_text.trim().is_empty() && !n.translated_text.trim().is_empty())
        .take(MAX_TONE_REFERENCE_BLOCKS)
        .map(|n| (n.source_text.clone(), n.translated_text.clone()))
        .collect();

    TranslationContext {
        glossary_matches,
        known_names,
        tone_reference,
    }
}

/// Captures newly-translated terms back into the glossary (FR-007a).
///
/// Whether a block is a name/term candidate at all is the backend's own judgment (`TermKind`,
/// returned alongside the translation on the same call) — not inferred here from surface features
/// like string length or punctuation. That local heuristic was tried first and shipped a real
/// regression: it misclassified plain short phrases and interjections (e.g. "just now", a groan)
/// as names purely because they were short and unpunctuated, polluting the glossary and confusing
/// later requests that received them as "already-established names." Trusting the classification
/// the translation call itself already made is both more accurate and adds no extra request.
pub fn capture_terms(
    glossary: &mut TerminologyGlossary,
    translated: &[(String, String, TermKind)],
    archive_id: &str,
    chapter_name: Option<&str>,
) -> Vec<String> {
    let mut captured = Vec::new();

    for (source, translation, kind) in translated {
        if *kind == TermKind::None {
            continue;
        }
        let source = source.trim();
        let translation = translation.trim();
        if glossary.capture(source, translation, archive_id, chapter_name) {
            captured.push(source.to_string());
        }
    }
    captured
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_ocr::entities::VolumeId;

    const ARC: &str = "archive-a";

    fn glossary_with(entries: &[(&str, &str)]) -> TerminologyGlossary {
        let mut g = TerminologyGlossary::new(&VolumeId::from("vol-1"));
        for (term, translation) in entries {
            g.capture(term, translation, ARC, None);
        }
        g
    }

    #[test]
    fn exact_hits_and_remaining_names_do_not_overlap() {
        let g = glossary_with(&[("さゆき", "Sayuki"), ("たけし", "Takeshi")]);
        let ctx = assemble(&g, &["さゆき、おはよう".to_string()], &[]);

        assert_eq!(ctx.glossary_matches.len(), 1);
        assert_eq!(ctx.glossary_matches[0].1[0].translation, "Sayuki");
        assert_eq!(
            ctx.known_names,
            vec!["たけし".to_string()],
            "a term already sent with its translation is redundant in the name list"
        );
    }

    #[test]
    fn a_term_with_multiple_sources_surfaces_every_candidate() {
        let mut g = TerminologyGlossary::new(&VolumeId::from("vol-1"));
        g.capture("太郎", "Taro", "archive-a", None);
        g.capture("太郎", "Jiro", "archive-b", None);

        let ctx = assemble(&g, &["太郎です".to_string()], &[]);

        assert_eq!(ctx.glossary_matches.len(), 1);
        assert_eq!(
            ctx.glossary_matches[0].1.len(),
            2,
            "both candidates must ride along"
        );
        let prefix = ctx.render_prefix();
        assert!(
            prefix.contains("archive-a"),
            "must name the disambiguating source"
        );
        assert!(prefix.contains("archive-b"));
    }

    #[test]
    fn a_term_with_one_source_renders_the_plain_unambiguous_line() {
        let g = glossary_with(&[("さゆき", "Sayuki")]);
        let ctx = assemble(&g, &["さゆき、おはよう".to_string()], &[]);
        let prefix = ctx.render_prefix();
        assert!(
            prefix.contains("- さゆき => Sayuki"),
            "an unambiguous term must render as the plain line, no extra source noise: {prefix}"
        );
    }

    #[test]
    fn a_name_absent_from_the_batch_still_rides_along_for_variant_recognition() {
        // FR-007c: さっちゃん is a nickname of さゆき with no substring relationship, so only the
        // name list can help the backend connect them.
        let g = glossary_with(&[("さゆき", "Sayuki")]);
        let ctx = assemble(&g, &["さっちゃん、おはよう".to_string()], &[]);

        assert!(ctx.glossary_matches.is_empty());
        assert_eq!(ctx.known_names, vec!["さゆき".to_string()]);
    }

    #[test]
    fn tone_reference_is_bounded_and_most_recent_first() {
        let g = glossary_with(&[]);
        let neighbors: Vec<TranslatedNeighbor> = (0..20)
            .map(|i| TranslatedNeighbor {
                source_text: format!("原文{i}"),
                translated_text: format!("Line {i}"),
            })
            .collect();

        let ctx = assemble(&g, &["こんにちは".to_string()], &neighbors);

        assert_eq!(ctx.tone_reference.len(), MAX_TONE_REFERENCE_BLOCKS);
        assert_eq!(ctx.tone_reference[0].1, "Line 19");
    }

    #[test]
    fn empty_inputs_produce_an_empty_context() {
        let ctx = assemble(&glossary_with(&[]), &[], &[]);
        assert!(ctx.is_empty());
    }

    #[test]
    fn duplicate_hits_across_batch_blocks_are_deduplicated() {
        let g = glossary_with(&[("さゆき", "Sayuki")]);
        let ctx = assemble(
            &g,
            &["さゆきです".to_string(), "さゆきさん".to_string()],
            &[],
        );
        assert_eq!(ctx.glossary_matches.len(), 1);
    }

    #[test]
    fn a_block_the_backend_classified_as_a_person_name_is_captured() {
        let mut g = glossary_with(&[]);
        let captured = capture_terms(
            &mut g,
            &[("さゆき".into(), "Sayuki".into(), TermKind::PersonName)],
            ARC,
            None,
        );
        assert_eq!(captured, vec!["さゆき".to_string()]);
    }

    #[test]
    fn a_block_the_backend_classified_as_a_term_is_captured() {
        let mut g = glossary_with(&[]);
        let captured = capture_terms(
            &mut g,
            &[("学園".into(), "the Academy".into(), TermKind::Term)],
            ARC,
            None,
        );
        assert_eq!(captured, vec!["学園".to_string()]);
    }

    #[test]
    fn a_block_the_backend_classified_as_none_is_not_captured() {
        // The regression this replaces: a local length/punctuation heuristic used to capture
        // short, unpunctuated interjections and plain words like this one purely because they
        // were brief — polluting the glossary with non-names. Trusting the backend's own
        // classification (made on the same call, informed by real semantic understanding) instead
        // means a short phrase the backend correctly judges as ordinary dialogue is never captured
        // no matter how name-shaped it looks by surface features alone.
        let mut g = glossary_with(&[]);
        let captured = capture_terms(
            &mut g,
            &[("さっき".into(), "just now".into(), TermKind::None)],
            ARC,
            None,
        );
        assert!(
            captured.is_empty(),
            "a block classified as `none` must never become a glossary entry, regardless of length"
        );
        assert!(g.entries.is_empty());
    }

    #[test]
    fn full_sentences_classified_as_none_are_not_captured() {
        let mut g = glossary_with(&[]);
        let captured = capture_terms(
            &mut g,
            &[(
                "こんにちは、元気ですか。".into(),
                "Hello, how are you?".into(),
                TermKind::None,
            )],
            ARC,
            None,
        );
        assert!(captured.is_empty());
        assert!(g.entries.is_empty());
    }

    #[test]
    fn capturing_the_same_term_from_two_archives_keeps_both_translations() {
        let mut g = TerminologyGlossary::new(&VolumeId::from("vol-1"));
        capture_terms(
            &mut g,
            &[("太郎".into(), "Taro".into(), TermKind::PersonName)],
            "archive-a",
            None,
        );
        capture_terms(
            &mut g,
            &[("太郎".into(), "Jiro".into(), TermKind::PersonName)],
            "archive-b",
            None,
        );
        assert_eq!(g.entries["太郎"].len(), 2);
    }
}
