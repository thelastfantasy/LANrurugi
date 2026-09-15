//! The normalized LLM provider adapter contract (`contracts/llm-provider-adapter.md`,
//! research.md §15).
//!
//! Callers depend only on these shapes, never on a provider-specific one, so failure handling
//! (FR-019/FR-020) needs no per-provider branching.
//!
//! **Batched, not per-block** (research.md §15, superseding the contract's original single-string
//! shape): one request carries the text blocks of a small fixed group of pages, each tagged with a
//! `block_id` so responses map back unambiguously. Batching amortises the fixed per-request cost
//! (HTTP round-trip plus the repeated glossary prefix) across several blocks, at the cost of
//! failure now being batch-granular — accepted because the batch size is deliberately small and
//! capped.
//!
//! **Content ordering is load-bearing**: every provider here caches by *exact prefix match*, so a
//! request is assembled stable-content-first (glossary), dynamic-content-last (this batch's
//! blocks). Putting the batch first would mean no two requests ever share a reusable prefix.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies one text block within a batch, so a response can be mapped back to the region it
/// came from. A newtype rather than a bare `String` because a block id, an archive id, and a
/// volume id are all string-shaped and would otherwise be interchangeable at a call site.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub String);

impl BlockId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The canonical id for a region: page number plus its index on that page.
    pub fn for_region(page_number: u32, region_index: usize) -> Self {
        Self(format!("p{page_number}b{region_index}"))
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for BlockId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for BlockId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// One text block awaiting translation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationBlock {
    pub block_id: BlockId,
    pub source_text: String,
}

/// What kind of glossary candidate a block's source text is, per the backend's own judgment
/// (FR-007a). Recognizing a proper noun is a semantic judgment — the backend is asked to make it
/// directly on the same call already producing the translation, rather than LANrurugi guessing
/// from surface features like string length or punctuation (a heuristic that, in practice,
/// misclassified plain words and interjections as names — see the regression this replaces).
///
/// A three-way enum with an explicit `None` variant, rather than `Option<TermKind>`, because a
/// strict-mode JSON schema (`translation_json_schema`) requires every property in `required` to
/// always be present with some value — there is no schema-level way to say "this field may be
/// absent," so the "not a term" case has to be a real value the model returns, not an omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TermKind {
    /// Ordinary dialogue/narration — the common case, most blocks are this.
    #[default]
    None,
    /// A character/person name — the case FR-007c's nickname/initialism recognition exists for.
    PersonName,
    /// A recurring non-person term (a place, an organization, an in-universe concept) worth
    /// keeping consistent, but not a name a nickname could refer to.
    Term,
}

/// One translated block, mapped back by `block_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslatedBlock {
    pub block_id: BlockId,
    pub translated_text: String,
    /// `TermKind::None` for ordinary dialogue/narration (the common case). Never inferred
    /// locally — see [`TermKind`]. Defaulted for the flat-map fallback shape (research.md §14),
    /// which predates this field and carries no classification at all.
    #[serde(default)]
    pub term_kind: TermKind,
}

/// One (archive, chapter) source's translation for a glossary term — only surfaced to the model at
/// all when a term has more than one of these (issue #105: an unambiguous term still renders as
/// the simple `term => translation` line `render_prefix` always has, so the common case never pays
/// for disambiguation it doesn't need).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlossaryMatchSource {
    pub translation: String,
    pub archive_id: String,
    #[serde(default)]
    pub chapter_name: Option<String>,
}

/// The stable, cacheable prefix of a request (research.md §14/§15).
///
/// Assembled once in [`crate::context_assembly`] before any adapter sees the request. Each part is
/// independently optional — the first block translated on a page has nothing for `tone_reference`
/// yet, and a volume with no glossary yet has nothing for the first two.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslationContext {
    /// Glossary entries whose source term is an exact substring of a block in this batch —
    /// deterministic reuse, no LLM judgment involved (FR-007b). Every source candidate for that
    /// term rides along (issue #105) — almost always exactly one, but a term seen under more than
    /// one (archive, chapter) source needs all of them present for the model to pick the right one
    /// for the batch's own current archive.
    pub glossary_matches: Vec<(String, Vec<GlossaryMatchSource>)>,
    /// The volume's other known glossary source names, names only — lets the backend recognise a
    /// nickname or initialism as an already-known character (FR-007c).
    pub known_names: Vec<String>,
    /// Already-translated blocks from the same page, as tone/style reference only (FR-007e).
    /// LANrurugi performs no tone classification of its own.
    pub tone_reference: Vec<(String, String)>,
}

impl TranslationContext {
    pub fn is_empty(&self) -> bool {
        self.glossary_matches.is_empty()
            && self.known_names.is_empty()
            && self.tone_reference.is_empty()
    }

    /// Renders the context as the stable prefix text sent ahead of the batch's own blocks.
    ///
    /// Ordering within the prefix is fixed and deterministic (glossary → names → tone reference)
    /// because prompt caching is exact-prefix-matched: reordering these between two requests would
    /// silently destroy the cache hit even with identical content.
    pub fn render_prefix(&self) -> String {
        let mut out = String::new();

        if !self.glossary_matches.is_empty() {
            out.push_str(
                "Established translations for terms appearing in this batch. Reuse them exactly:\n",
            );
            for (source, candidates) in &self.glossary_matches {
                match candidates.as_slice() {
                    // The common case: one source, no ambiguity — same plain line as before
                    // issue #105, so a volume with no cross-archive/chapter name collisions never
                    // pays any extra prompt cost for disambiguation it doesn't need.
                    [only] => out.push_str(&format!("- {source} => {}\n", only.translation)),
                    multiple => {
                        out.push_str(&format!(
                            "- {source} has been translated differently depending on which work \
                             it appeared in — pick the entry matching this batch's own source, or \
                             coin a new translation if none of these apply:\n"
                        ));
                        for candidate in multiple {
                            let origin = match &candidate.chapter_name {
                                Some(chapter) => format!("{} — {chapter}", candidate.archive_id),
                                None => candidate.archive_id.clone(),
                            };
                            out.push_str(&format!(
                                "  - [{origin}] {source} => {}\n",
                                candidate.translation
                            ));
                        }
                    }
                }
            }
            out.push('\n');
        }

        if !self.known_names.is_empty() {
            out.push_str(
                "The following is a plain list of names/terms already established elsewhere in \
                 this volume — reference data, not part of the text to translate. Only relevant \
                 if a block below refers to one of them by a nickname, abbreviation, or \
                 initialism; if so, reuse that established translation instead of coining a new \
                 one. Ignore this list entirely for any block that doesn't:\n",
            );
            out.push_str(&self.known_names.join(", "));
            out.push_str("\n\n");
        }

        if !self.tone_reference.is_empty() {
            out.push_str(
                "Other text already translated on this page, for tone and style reference only:\n",
            );
            for (source, translation) in &self.tone_reference {
                out.push_str(&format!("- {source} => {translation}\n"));
            }
            out.push('\n');
        }

        out
    }
}

/// A batched translation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationRequest {
    /// Stable prefix content — always serialized ahead of `blocks`.
    pub context: TranslationContext,
    /// This batch's own blocks — the dynamic tail.
    pub blocks: Vec<TranslationBlock>,
    pub target_language: String,
    pub source_language_hint: Option<String>,
}

impl TranslationRequest {
    pub fn new(blocks: Vec<TranslationBlock>, target_language: impl Into<String>) -> Self {
        Self {
            context: TranslationContext::default(),
            blocks,
            target_language: target_language.into(),
            source_language_hint: None,
        }
    }

    pub fn with_context(mut self, context: TranslationContext) -> Self {
        self.context = context;
        self
    }

    /// The instruction text sent as the system prompt. Constant across every request for a given
    /// target language, so it sits at the very front of the cacheable prefix. Wording lives in
    /// `crate::llm_prompts` (centralized-prompts convention, mirroring `lanrurugi-api`'s own
    /// `llm_prompts.rs`), not here.
    pub fn system_prompt(&self) -> String {
        crate::llm_prompts::translation_system(&self.target_language)
    }

    /// The user-message body: stable context prefix first, this batch's blocks last.
    pub fn render_user_message(&self) -> String {
        let mut out = self.context.render_prefix();
        out.push_str("Translate these blocks:\n");
        for block in &self.blocks {
            out.push_str(&format!("[{}] {}\n", block.block_id, block.source_text));
        }
        out
    }
}

/// A successful batched response.
///
/// The `*_tokens` fields (issue #100) are each independently `Option` since providers disagree on
/// what they report at all — DeepSeek's `prompt_cache_hit_tokens`/`prompt_cache_miss_tokens` sum
/// to `prompt_tokens`, Anthropic separates `cache_creation_input_tokens` (writing a new cache
/// entry) from `cache_read_input_tokens` (reading one), OpenAI-compatible APIs nest cache info
/// under `prompt_tokens_details.cached_tokens`. Every adapter normalizes its own shape into these
/// same field names rather than callers branching on provider — see each adapter's own `Usage`
/// struct for the raw shape being normalized from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationResponse {
    pub blocks: Vec<TranslatedBlock>,
    pub provider_latency_ms: u64,
    /// The concrete model that served this request (e.g. "deepseek-v4-flash") — needed to look up
    /// the right price-table entry, since a provider's per-token price varies by model.
    #[serde(default)]
    pub model: Option<String>,
    /// Total tokens read from the prompt (system + context + this batch's blocks), cache hits and
    /// misses combined. `None` only if the provider's response carried no usage block at all.
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    /// Of `prompt_tokens`, however many were served from the provider's own prompt cache (billed
    /// at a lower rate) rather than freshly processed.
    #[serde(default)]
    pub cached_prompt_tokens: Option<u64>,
    /// Of `prompt_tokens`, however many this request itself newly wrote into the provider's cache
    /// (Anthropic-specific — its own separate `cache_creation_input_tokens`, typically billed
    /// *higher* than a fresh miss since writing a cache entry costs more than reading one; `None`
    /// on providers that don't distinguish this from an ordinary cache miss).
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    /// Tokens the model generated in its reply.
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    /// `prompt_tokens + completion_tokens` as the provider itself reports it (kept alongside the
    /// split-out fields above rather than always recomputed, since a provider's own total is the
    /// authoritative billing figure even if this project's own sum of parts were ever to drift).
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

/// Normalized failure kinds (FR-019/FR-020). A raw provider error shape never reaches callers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslationError {
    #[error("translation backend is unreachable: {0}")]
    Unreachable(String),
    #[error("translation backend rejected the credentials")]
    AuthFailed,
    #[error("translation backend rate-limited the request")]
    RateLimited,
    #[error("translation backend returned a malformed or empty response: {0}")]
    MalformedResponse(String),
}

impl TranslationError {
    /// Stable machine-readable kind for the API layer and the frontend's per-page error state.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unreachable(_) => "unreachable",
            Self::AuthFailed => "auth_failed",
            Self::RateLimited => "rate_limited",
            Self::MalformedResponse(_) => "malformed_response",
        }
    }
}

/// Maps an HTTP status from any provider onto a normalized error.
pub fn error_for_status(status: u16, body: &str) -> TranslationError {
    match status {
        401 | 403 => TranslationError::AuthFailed,
        429 => TranslationError::RateLimited,
        // 5xx is the provider being unavailable rather than the request being wrong.
        500..=599 => TranslationError::Unreachable(format!("provider returned HTTP {status}")),
        _ => TranslationError::MalformedResponse(format!("HTTP {status}: {}", truncate(body, 200))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "..."
}

/// The name of the tool/schema every adapter uses for structured output.
pub const TRANSLATION_SCHEMA_NAME: &str = "manga_translation";

/// The JSON Schema every adapter constrains its response with — OpenAI's `json_schema` response
/// format and Anthropic's forced-tool `input_schema` are both fed this exact value.
///
/// An **array of `{block_id, translated_text}`** rather than the more obvious "object whose keys are
/// the block ids": strict `json_schema` mode has no way to describe arbitrary dynamic keys
/// (`additionalProperties` must be `false`), so a dynamic-key object cannot be expressed at all.
pub fn translation_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "translations": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "block_id": {
                            "type": "string",
                            "description": "The block id copied verbatim from the input.",
                        },
                        "translated_text": {
                            "type": "string",
                            "description": "The translation of that block.",
                        },
                        "term_kind": {
                            "type": "string",
                            "enum": ["person_name", "term", "none"],
                            "description": "Classify this block's *source* text: \"person_name\" \
                                if it is a character/person name, \"term\" if it is some other \
                                recurring named thing worth keeping consistent (a place, an \
                                organization, an in-universe concept — not a name a nickname \
                                could refer to), or \"none\" for ordinary dialogue/narration \
                                (the common case). A short phrase, interjection, or sound effect \
                                is \"none\", not a term, even if it's brief.",
                        },
                    },
                    // Strict schema mode requires every property listed above; `term_kind` still
                    // behaves as optional in practice via its own explicit "none" — see
                    // `TranslatedBlock::term_kind`'s `Option<TermKind>` mapping of that string.
                    "required": ["block_id", "translated_text", "term_kind"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["translations"],
        "additionalProperties": false,
    })
}

/// The structured shape [`translation_json_schema`] describes.
#[derive(Debug, Deserialize)]
struct StructuredTranslations {
    translations: Vec<TranslatedBlock>,
}

/// Parses a provider's JSON reply into normalized blocks.
///
/// Shared by every adapter. The schema-constrained shape (`{"translations": [{block_id,
/// translated_text}, ...]}`) is tried first; a flat `block_id -> translation` object is still
/// accepted as a fallback, because DeepSeek's `json_object` mode only guarantees *syntactically*
/// valid JSON — not this schema — and a model may still answer in the older shape. Tolerance for
/// prose or a ```json fence around the object is kept for the same reason.
pub fn parse_block_map(content: &str) -> Result<Vec<TranslatedBlock>, TranslationError> {
    let json = extract_json_object(content).ok_or_else(|| {
        TranslationError::MalformedResponse("response contained no JSON object".into())
    })?;

    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| TranslationError::MalformedResponse(e.to_string()))?;

    parse_block_value(&value)
}

/// Parses the already-decoded structured value — used directly by the Anthropic adapter, whose
/// forced tool call hands back a JSON value rather than a string to re-parse.
pub fn parse_block_value(
    value: &serde_json::Value,
) -> Result<Vec<TranslatedBlock>, TranslationError> {
    let blocks: Vec<TranslatedBlock> =
        match serde_json::from_value::<StructuredTranslations>(value.clone()) {
            Ok(structured) => structured
                .translations
                .into_iter()
                .filter(|b| !b.block_id.as_str().is_empty())
                .collect(),
            // Fallback: the flat `{"p1b0": "Hello"}` shape.
            Err(_) => value
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(id, v)| {
                            v.as_str().map(|text| TranslatedBlock {
                                block_id: BlockId::from(id.clone()),
                                translated_text: text.to_string(),
                                term_kind: TermKind::None,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };

    if blocks.is_empty() {
        // An empty translation is treated as a failure, not a success with blank text (spec.md
        // Edge Cases — a malformed/empty response must not render as successfully translated).
        return Err(TranslationError::MalformedResponse(
            "response contained no translated blocks".into(),
        ));
    }

    Ok(blocks)
}

/// Finds the outermost JSON object in a reply that may be wrapped in prose or a ```json fence.
fn extract_json_object(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (i, c) in content[start..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&content[start..start + i + c.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The contract every provider adapter implements.
#[allow(async_fn_in_trait)]
pub trait TranslationAdapter: Send + Sync {
    /// Provider identifier, used as part of the cache key (FR-016) so a backend change never
    /// reuses another backend's cached output.
    fn provider_id(&self) -> &str;

    /// Translates one batch.
    async fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslationError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_source(translation: &str) -> Vec<GlossaryMatchSource> {
        vec![GlossaryMatchSource {
            translation: translation.to_string(),
            archive_id: "archive-a".to_string(),
            chapter_name: None,
        }]
    }

    #[test]
    fn context_prefix_orders_glossary_before_names_before_tone() {
        let ctx = TranslationContext {
            glossary_matches: vec![("さゆき".into(), one_source("Sayuki"))],
            known_names: vec!["さゆき".into(), "たけし".into()],
            tone_reference: vec![("おはよう".into(), "Morning!".into())],
        };
        let prefix = ctx.render_prefix();

        let glossary_at = prefix.find("Sayuki").unwrap();
        let names_at = prefix.find("already established").unwrap();
        let tone_at = prefix.find("tone and style reference").unwrap();

        assert!(
            glossary_at < names_at && names_at < tone_at,
            "prefix ordering is load-bearing for exact-prefix prompt caching"
        );
    }

    #[test]
    fn empty_context_renders_nothing() {
        assert!(TranslationContext::default().render_prefix().is_empty());
    }

    #[test]
    fn user_message_puts_stable_context_before_dynamic_blocks() {
        let req = TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
            }],
            "en",
        )
        .with_context(TranslationContext {
            glossary_matches: vec![("さゆき".into(), one_source("Sayuki"))],
            ..Default::default()
        });

        let msg = req.render_user_message();
        assert!(
            msg.find("Sayuki").unwrap() < msg.find("こんにちは").unwrap(),
            "stable content must precede batch content so the prefix is cacheable"
        );
    }

    #[test]
    fn block_ids_are_unique_per_page_and_index() {
        assert_eq!(BlockId::for_region(3, 1).as_str(), "p3b1");
        assert_ne!(BlockId::for_region(3, 1), BlockId::for_region(1, 3));
    }

    #[test]
    fn the_schema_constrained_array_shape_is_parsed() {
        let blocks = parse_block_map(
            r#"{"translations":[{"block_id":"p1b0","translated_text":"Hello"},
                               {"block_id":"p1b1","translated_text":"Goodbye"}]}"#,
        )
        .unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_id, BlockId::from("p1b0"));
        assert_eq!(blocks[1].translated_text, "Goodbye");
    }

    #[test]
    fn a_structured_value_is_parsed_without_restringifying() {
        // Anthropic's forced tool call hands back a decoded value, not text.
        let value = serde_json::json!({
            "translations": [{"block_id": "p2b1", "translated_text": "こんにちは"}]
        });
        let blocks = parse_block_value(&value).unwrap();
        assert_eq!(blocks[0].translated_text, "こんにちは");
    }

    #[test]
    fn the_schema_describes_an_array_not_dynamic_keys() {
        // Strict `json_schema` mode can't express arbitrary keys, so the array shape is the whole
        // reason this schema exists — a regression here would silently make the constraint invalid.
        let schema = translation_json_schema();
        assert_eq!(schema["properties"]["translations"]["type"], "array");
        assert_eq!(schema["additionalProperties"], false);
        let item = &schema["properties"]["translations"]["items"];
        assert!(item["properties"]["block_id"].is_object());
        assert!(item["properties"]["translated_text"].is_object());
        assert!(item["properties"]["term_kind"].is_object());
        assert_eq!(
            item["required"],
            serde_json::json!(["block_id", "translated_text", "term_kind"])
        );
    }

    #[test]
    fn term_kind_round_trips_through_the_structured_schema() {
        let value = serde_json::json!({
            "translations": [
                {"block_id": "p1b0", "translated_text": "Sayuki", "term_kind": "person_name"},
                {"block_id": "p1b1", "translated_text": "the Academy", "term_kind": "term"},
                {"block_id": "p1b2", "translated_text": "Hello", "term_kind": "none"},
            ]
        });
        let blocks = parse_block_value(&value).unwrap();
        assert_eq!(blocks[0].term_kind, TermKind::PersonName);
        assert_eq!(blocks[1].term_kind, TermKind::Term);
        assert_eq!(blocks[2].term_kind, TermKind::None);
    }

    #[test]
    fn a_missing_term_kind_defaults_to_none_for_the_flat_map_fallback() {
        // The pre-existing flat `{"id": "text"}` shape (research.md §14) predates `term_kind` and
        // carries no classification at all — it must still parse, not fail closed.
        let blocks = parse_block_map(r#"{"p1b0":"Hello"}"#).unwrap();
        assert_eq!(blocks[0].term_kind, TermKind::None);
    }

    #[test]
    fn the_system_prompt_mentions_json_for_deepseeks_json_object_mode() {
        // DeepSeek rejects `response_format: json_object` outright if the prompt never says "json".
        let req = TranslationRequest::new(vec![], "en");
        assert!(req.system_prompt().to_lowercase().contains("json"));
    }

    #[test]
    fn plain_json_object_is_parsed() {
        let blocks = parse_block_map(r#"{"p1b0":"Hello","p1b1":"Goodbye"}"#).unwrap();
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn fenced_and_prose_wrapped_json_is_recovered() {
        let reply = "Sure!\n```json\n{\"p1b0\": \"Hello\"}\n```\nHope that helps.";
        let blocks = parse_block_map(reply).unwrap();
        assert_eq!(blocks[0].translated_text, "Hello");
    }

    #[test]
    fn braces_inside_strings_do_not_truncate_parsing() {
        let blocks = parse_block_map(r#"{"p1b0":"a } b","p1b1":"c"}"#).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].translated_text, "a } b");
    }

    #[test]
    fn empty_response_is_a_failure_not_a_blank_success() {
        assert!(matches!(
            parse_block_map("{}"),
            Err(TranslationError::MalformedResponse(_))
        ));
        assert!(matches!(
            parse_block_map("no json here"),
            Err(TranslationError::MalformedResponse(_))
        ));
    }

    #[test]
    fn http_statuses_map_to_normalized_kinds() {
        assert_eq!(error_for_status(401, "").kind(), "auth_failed");
        assert_eq!(error_for_status(429, "").kind(), "rate_limited");
        assert_eq!(error_for_status(503, "").kind(), "unreachable");
        assert_eq!(error_for_status(400, "bad").kind(), "malformed_response");
    }
}
