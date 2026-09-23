//! Orchestrates a page's translation: persisted-region reuse → context assembly → batched LLM call
//! → glossary capture → persistence (T028, research.md §14/§15/§16).
//!
//! Deliberately owns no HTTP concerns and no image work — the API layer supplies already-detected
//! regions and renders the result, so this module stays testable without a running server, a model
//! file, or a provider.

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::entities::{DetectedTextRegion, PageNumber};

use crate::adapter::{
    BlockId, TermKind, TranslationAdapter, TranslationBlock, TranslationError, TranslationRequest,
};
use crate::context_assembly::{assemble, capture_terms, TranslatedNeighbor};
use crate::glossary::TerminologyGlossary;

/// One page's regions awaiting translation.
pub struct PageWork {
    pub archive_id: ArchiveId,
    pub page_number: PageNumber,
    pub regions: Vec<DetectedTextRegion>,
    /// The `toc` chapter name active at this page, if the archive has a table of contents and this
    /// page falls within one of its entries (issue #105) — threaded through to
    /// [`crate::glossary::TerminologyGlossary::capture`] so a term captured here is scoped to the
    /// (archive, chapter) it actually came from, not folded into an unrelated same-named term from
    /// elsewhere in the volume. `None` for an archive with no `toc` at all, or a page before its
    /// first `toc` entry — the caller (which has the real `Archive.toc` on hand) resolves this,
    /// this crate has no storage dependency to look it up itself.
    pub chapter_name: Option<String>,
}

/// The provider/token/latency facts of one real LLM call (issue #100) — every field from
/// [`TranslationResponse`] except `blocks` (already consumed into [`BatchOutcome::pages`] by the
/// time this is built, so keeping it here too would just be a second copy of the same data).
pub struct UsageInfo {
    pub provider_id: String,
    pub model: Option<String>,
    pub provider_latency_ms: u64,
    pub prompt_tokens: Option<u64>,
    pub cached_prompt_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// What a batch translation produced.
pub struct BatchOutcome {
    /// Regions with `translated_text` filled in where translation succeeded.
    pub pages: Vec<PageWork>,
    /// Terms newly captured into the glossary (FR-007a) — the caller persists the glossary.
    pub captured_terms: Vec<String>,
    /// Tokens consumed, for budget accounting. `None` when the provider didn't report usage.
    pub total_tokens: Option<u64>,
    /// The full usage/model breakdown from the provider's own response (issue #100) — `None` when
    /// no provider call was made at all (everything was already translated, see
    /// [`translate_batch`]'s own early-return).
    pub usage: Option<UsageInfo>,
}

/// Assigns each still-untranslated region a `block_id`, keyed by page and index.
fn collect_blocks(pages: &[PageWork]) -> Vec<(usize, usize, TranslationBlock)> {
    let mut blocks = Vec::new();
    for (page_index, page) in pages.iter().enumerate() {
        for (region_index, region) in page.regions.iter().enumerate() {
            // Already-translated regions are skipped rather than re-sent: the persisted record is
            // authoritative (research.md §16), so re-translating would re-bill for work already
            // paid for.
            if region.translated_text.is_some() {
                continue;
            }
            if region.source_text.trim().is_empty() {
                continue;
            }
            blocks.push((
                page_index,
                region_index,
                TranslationBlock {
                    block_id: BlockId::for_region(page.page_number.get(), region_index),
                    source_text: region.source_text.clone(),
                    alternate_source_text: region.alternate_source_text.clone(),
                },
            ));
        }
    }
    blocks
}

/// Resolves which OCR candidate the provider actually translated.
///
/// The provider reports its choice through `TranslatedBlock::selected_source_text`; actual
/// candidate text is accepted verbatim (trimmed), and the bare labels `A` / `B` are accepted as a
/// convenience for models that echo only the option letter. An omitted/empty choice defaults to
/// the alternate candidate, because the alternate only exists when the original reading was
/// ambiguous or detector-fragmented; a malformed non-empty answer still falls back to A.
fn selected_source_text(block: &TranslationBlock, selected: Option<&str>) -> String {
    let Some(alternate) = &block.alternate_source_text else {
        return block.source_text.clone();
    };

    let selected = selected.map(str::trim).filter(|value| !value.is_empty());
    match selected {
        Some(value) if is_option_label(value, "a") => block.source_text.clone(),
        Some(value) if is_option_label(value, "b") => alternate.clone(),
        Some(value) if value == block.source_text.trim() => block.source_text.clone(),
        Some(value) if value == alternate.trim() => alternate.clone(),
        Some(value)
            if block.source_text.contains(value) || value.contains(block.source_text.as_str()) =>
        {
            block.source_text.clone()
        }
        Some(value) if alternate.contains(value) || value.contains(alternate.as_str()) => {
            alternate.clone()
        }
        // The provider was explicitly asked to report its choice. If it still omitted or left
        // the field empty, the alternate candidate exists precisely because the original reading
        // was ambiguous or detector-fragmented, so prefer the alternate rather than silently
        // persisting the least-plausible A reading. A malformed non-empty value from a provider
        // that *did* answer still falls through to A by the final arm below.
        Some(value) => {
            tracing::warn!(
                selected = %value,
                "provider returned an unrecognized selected_source_text; keeping candidate A"
            );
            block.source_text.clone()
        }
        None => {
            tracing::warn!(
                block_id = %block.block_id,
                "provider omitted selected_source_text; defaulting to alternate OCR candidate"
            );
            alternate.clone()
        }
    }
}

/// Whether `value` is the bare option label (or a lightly decorated form such as `B)` / `B:`)
/// for `expected` (`"a"` or `"b"`).
fn is_option_label(value: &str, expected: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    if lower == expected {
        return true;
    }
    let Some(rest) = lower.strip_prefix(expected) else {
        return false;
    };
    rest.starts_with([')', '.', ':', '：', '、', ' '])
}

/// Translates a batch of pages in one provider call.
///
/// The glossary is read (for context) and written (for newly-seen terms) here, but persisting it is
/// the caller's job so this stays free of storage dependencies.
pub async fn translate_batch<A: TranslationAdapter>(
    adapter: &A,
    glossary: &mut TerminologyGlossary,
    mut pages: Vec<PageWork>,
    target_language: &str,
) -> Result<BatchOutcome, TranslationError> {
    let blocks = collect_blocks(&pages);

    // Everything already translated: no provider call at all. This is the common case on a re-read
    // and is what keeps a cached page free.
    if blocks.is_empty() {
        return Ok(BatchOutcome {
            pages,
            captured_terms: Vec::new(),
            total_tokens: None,
            usage: None,
        });
    }

    // Tone reference comes from blocks on these pages already translated in an earlier batch
    // (FR-007e) — advisory only.
    let neighbors: Vec<TranslatedNeighbor> = pages
        .iter()
        .flat_map(|p| p.regions.iter())
        .filter_map(|r| {
            r.translated_text.as_ref().map(|t| TranslatedNeighbor {
                source_text: r.source_text.clone(),
                translated_text: t.clone(),
            })
        })
        .collect();

    let sources: Vec<String> = blocks
        .iter()
        .map(|(_, _, b)| b.source_text.clone())
        .collect();
    let context = assemble(glossary, &sources, &neighbors);

    let request = TranslationRequest::new(
        blocks.iter().map(|(_, _, b)| b.clone()).collect(),
        target_language,
    )
    .with_context(context);

    let response = adapter.translate(&request).await?;
    let usage = UsageInfo {
        provider_id: adapter.provider_id().to_string(),
        model: response.model.clone(),
        provider_latency_ms: response.provider_latency_ms,
        prompt_tokens: response.prompt_tokens,
        cached_prompt_tokens: response.cached_prompt_tokens,
        cache_creation_tokens: response.cache_creation_tokens,
        completion_tokens: response.completion_tokens,
        total_tokens: response.total_tokens,
    };

    // Map translations back onto their originating regions by `block_id`. Each applied translation
    // carries the (archive, chapter) it came from (issue #105) — a batch can span several pages,
    // and those pages aren't guaranteed to share an archive/chapter, so this can't be resolved once
    // for the whole batch the way it might look at first glance.
    let mut applied: Vec<(String, String, TermKind, ArchiveId, Option<String>)> = Vec::new();
    for translated in &response.blocks {
        let Some((page_index, region_index, block)) = blocks
            .iter()
            .find(|(_, _, b)| b.block_id == translated.block_id)
        else {
            // An id the request never contained — ignore rather than fail the batch.
            tracing::warn!(block_id = %translated.block_id, "provider returned an unknown block id");
            continue;
        };

        if translated.translated_text.trim().is_empty() {
            // Blank output is treated as "not translated", never rendered as empty text.
            continue;
        }

        let page = &pages[*page_index];
        let source_archive_id = page.archive_id.clone();
        let source_chapter_name = page.chapter_name.clone();
        let chosen_source = selected_source_text(block, translated.selected_source_text.as_deref());
        pages[*page_index].regions[*region_index].source_text = chosen_source.clone();
        pages[*page_index].regions[*region_index].translated_text =
            Some(translated.translated_text.clone());
        applied.push((
            chosen_source,
            translated.translated_text.clone(),
            translated.term_kind,
            source_archive_id,
            source_chapter_name,
        ));
    }

    // A provider dropping some block_ids from its response (rather than erroring outright) is a
    // real, silent failure mode this project hit in production (2026-09-14): a page with several
    // regions in the same speech bubble came back with only some of them translated and no error
    // anywhere — `response.blocks` simply didn't mention the rest (or echoed it back blank, the
    // `continue` above). Nothing else would ever notice, since a region that keeps
    // `translated_text: None` still renders fine (as the untouched original), so this is the only
    // place left that can see the gap. `applied.len()` (not `response.blocks.len()`) against
    // `blocks.len()` counts only *successfully* mapped blocks, so it also catches an unknown-id or
    // blank-text response that technically had the right count but didn't actually translate
    // everything.
    let requested = blocks.len();
    let translated = applied.len();
    if translated < requested {
        tracing::warn!(
            requested,
            translated,
            "provider translated fewer blocks than requested; some regions will render untranslated"
        );
    }

    // Grouped by (archive, chapter) rather than one `capture_terms` call per applied translation —
    // the common case (a batch entirely within one archive/chapter) still does exactly one call,
    // same as before this field existed.
    type CapturedGroup = (ArchiveId, Option<String>, Vec<(String, String, TermKind)>);
    let mut captured_terms = Vec::new();
    let mut groups: Vec<CapturedGroup> = Vec::new();
    for (source, translation, kind, archive_id, chapter_name) in applied {
        match groups
            .iter_mut()
            .find(|(a, c, _)| *a == archive_id && *c == chapter_name)
        {
            Some((_, _, items)) => items.push((source, translation, kind)),
            None => groups.push((archive_id, chapter_name, vec![(source, translation, kind)])),
        }
    }
    for (archive_id, chapter_name, items) in groups {
        captured_terms.extend(capture_terms(
            glossary,
            &items,
            archive_id.as_str(),
            chapter_name.as_deref(),
        ));
    }

    Ok(BatchOutcome {
        pages,
        captured_terms,
        total_tokens: response.total_tokens,
        usage: Some(usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{TranslatedBlock, TranslationResponse};
    use lanrurugi_ocr::entities::{BoundingBox, VolumeId};

    struct StubAdapter {
        reply: Vec<(String, String, TermKind)>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl StubAdapter {
        fn new(reply: &[(&str, &str)]) -> Self {
            Self::with_term_kinds(
                &reply
                    .iter()
                    .map(|(a, b)| (*a, *b, TermKind::None))
                    .collect::<Vec<_>>(),
            )
        }
        /// Like [`Self::new`], but lets a test control each reply's `term_kind` — needed for
        /// exercising glossary capture (FR-007a), which only fires for a non-`TermKind::None`
        /// block.
        fn with_term_kinds(reply: &[(&str, &str, TermKind)]) -> Self {
            Self {
                reply: reply
                    .iter()
                    .map(|(a, b, kind)| (a.to_string(), b.to_string(), *kind))
                    .collect(),
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl TranslationAdapter for StubAdapter {
        fn provider_id(&self) -> &str {
            "stub"
        }
        async fn translate(
            &self,
            _request: &TranslationRequest,
        ) -> Result<TranslationResponse, TranslationError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(TranslationResponse {
                blocks: self
                    .reply
                    .iter()
                    .map(|(id, text, kind)| TranslatedBlock {
                        block_id: BlockId::from(id.as_str()),
                        translated_text: text.clone(),
                        selected_source_text: None,
                        term_kind: *kind,
                    })
                    .collect(),
                provider_latency_ms: 1,
                model: Some("stub-model".to_string()),
                prompt_tokens: Some(30),
                cached_prompt_tokens: None,
                cache_creation_tokens: None,
                completion_tokens: Some(12),
                total_tokens: Some(42),
            })
        }
    }

    fn page(page_number: u32, texts: &[&str]) -> PageWork {
        PageWork {
            archive_id: ArchiveId::from("a"),
            page_number: PageNumber(page_number),
            regions: texts
                .iter()
                .map(|t| {
                    DetectedTextRegion::new(
                        ArchiveId::from("a"),
                        PageNumber(page_number),
                        BoundingBox::new(0, 0, 10, 10),
                        (*t).to_string(),
                        false,
                    )
                })
                .collect(),
            chapter_name: None,
        }
    }

    fn glossary() -> TerminologyGlossary {
        TerminologyGlossary::new(&VolumeId::from("vol-1"))
    }

    #[tokio::test]
    async fn translations_map_back_to_their_regions() {
        let adapter = StubAdapter::new(&[("p1b0", "Hello"), ("p1b1", "Goodbye")]);
        let mut g = glossary();

        let out = translate_batch(
            &adapter,
            &mut g,
            vec![page(1, &["こんにちは", "さようなら"])],
            "en",
        )
        .await
        .unwrap();

        assert_eq!(
            out.pages[0].regions[0].translated_text.as_deref(),
            Some("Hello")
        );
        assert_eq!(
            out.pages[0].regions[1].translated_text.as_deref(),
            Some("Goodbye")
        );
    }

    #[tokio::test]
    async fn already_translated_regions_are_never_re_sent() {
        // research.md §16: the persisted record is authoritative, so a re-read must cost nothing.
        let adapter = StubAdapter::new(&[]);
        let mut g = glossary();

        let mut p = page(1, &["こんにちは"]);
        p.regions[0].translated_text = Some("Hello".into());

        let out = translate_batch(&adapter, &mut g, vec![p], "en")
            .await
            .unwrap();

        assert_eq!(
            adapter.call_count(),
            0,
            "no provider call for a fully-translated page"
        );
        assert_eq!(
            out.pages[0].regions[0].translated_text.as_deref(),
            Some("Hello")
        );
    }

    #[tokio::test]
    async fn a_batch_spans_multiple_pages_in_one_call() {
        let adapter = StubAdapter::new(&[("p1b0", "One"), ("p2b0", "Two")]);
        let mut g = glossary();

        let out = translate_batch(
            &adapter,
            &mut g,
            vec![page(1, &["いち"]), page(2, &["に"])],
            "en",
        )
        .await
        .unwrap();

        assert_eq!(
            adapter.call_count(),
            1,
            "a batch is one request, not one per page"
        );
        assert_eq!(
            out.pages[0].regions[0].translated_text.as_deref(),
            Some("One")
        );
        assert_eq!(
            out.pages[1].regions[0].translated_text.as_deref(),
            Some("Two")
        );
    }

    #[tokio::test]
    async fn short_terms_are_captured_into_the_glossary() {
        let adapter = StubAdapter::with_term_kinds(&[("p1b0", "Sayuki", TermKind::PersonName)]);
        let mut g = glossary();

        let out = translate_batch(&adapter, &mut g, vec![page(1, &["さゆき"])], "en")
            .await
            .unwrap();

        assert_eq!(out.captured_terms, vec!["さゆき".to_string()]);
        assert_eq!(
            g.entries.get("さゆき").map(|v| v[0].translation.as_str()),
            Some("Sayuki")
        );
    }

    #[tokio::test]
    async fn blank_output_is_not_treated_as_a_translation() {
        let adapter = StubAdapter::new(&[("p1b0", "   ")]);
        let mut g = glossary();

        let out = translate_batch(&adapter, &mut g, vec![page(1, &["こんにちは"])], "en")
            .await
            .unwrap();

        assert_eq!(
            out.pages[0].regions[0].translated_text, None,
            "blank output must not render as an empty translation"
        );
    }

    #[tokio::test]
    async fn an_unknown_block_id_does_not_fail_the_batch() {
        let adapter = StubAdapter::new(&[("p9b9", "Stray"), ("p1b0", "Hello")]);
        let mut g = glossary();

        let out = translate_batch(&adapter, &mut g, vec![page(1, &["こんにちは"])], "en")
            .await
            .unwrap();

        assert_eq!(
            out.pages[0].regions[0].translated_text.as_deref(),
            Some("Hello")
        );
    }

    #[tokio::test]
    async fn empty_source_text_is_skipped() {
        let adapter = StubAdapter::new(&[]);
        let mut g = glossary();

        translate_batch(&adapter, &mut g, vec![page(1, &["   "])], "en")
            .await
            .unwrap();

        assert_eq!(adapter.call_count(), 0);
    }
    fn candidate_pair() -> TranslationBlock {
        TranslationBlock {
            block_id: BlockId::from("p1b0"),
            source_text: "ーッ娘スー母猫".into(),
            alternate_source_text: Some("スーツ母娘".into()),
        }
    }

    #[test]
    fn selected_source_text_accepts_actual_candidate_b_text() {
        assert_eq!(
            selected_source_text(&candidate_pair(), Some("スーツ母娘")),
            "スーツ母娘"
        );
    }

    #[test]
    fn selected_source_text_accepts_a_bare_b_label() {
        assert_eq!(
            selected_source_text(&candidate_pair(), Some(" B ")),
            "スーツ母娘"
        );
    }

    #[test]
    fn selected_source_text_defaults_to_a_when_unrecognized() {
        assert_eq!(
            selected_source_text(&candidate_pair(), Some("garbage")),
            "ーッ娘スー母猫"
        );
    }

    #[test]
    fn a_single_candidate_block_is_never_switched() {
        let block = TranslationBlock {
            block_id: BlockId::from("p1b0"),
            source_text: "こんにちは".into(),
            alternate_source_text: None,
        };
        assert_eq!(selected_source_text(&block, Some("anything")), "こんにちは");
    }
}
