//! Anthropic Messages API adapter (research.md §5/§15).
//!
//! Differs from the OpenAI-compatible shape in several ways the constitution's Technology Stack
//! Constraints already fix: `system` is a top-level field rather than a message, content is a
//! block array, auth is `x-api-key` + `anthropic-version` rather than a bearer token, and
//! `max_tokens` is mandatory.
//!
//! **Caching is explicit here, unlike the other two adapters**: Anthropic only caches up to a
//! `cache_control` breakpoint the caller places. The breakpoint goes immediately after the stable
//! Terminology-Glossary-derived prefix, so that prefix is what gets reused across the batch
//! requests of a volume — placing it after the batch's own blocks would cache content that never
//! repeats.

use std::time::Instant;

use serde::Deserialize;

use crate::adapter::{
    error_for_status, parse_block_map, parse_block_value, translation_json_schema,
    TranslationAdapter, TranslationError, TranslationRequest, TranslationResponse,
    TRANSLATION_SCHEMA_NAME,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Anthropic declines to cache very short prefixes; below roughly this many characters the
/// breakpoint is not worth sending at all.
const MIN_CACHEABLE_PREFIX_CHARS: usize = 400;

pub struct AnthropicAdapter {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl std::fmt::Debug for AnthropicAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Hand-written rather than derived: a derive would print `api_key` verbatim into any log
        // line or panic message that formats this adapter (constitution Principle V).
        f.debug_struct("AnthropicAdapter")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicAdapter {
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key: api_key.into(),
        }
    }

    /// Splits the user message into a cacheable stable prefix and the dynamic batch tail.
    ///
    /// Returned as content blocks so the stable half can carry `cache_control` while the tail
    /// cannot — the whole reason this adapter doesn't just send one string.
    fn content_blocks(request: &TranslationRequest) -> Vec<serde_json::Value> {
        let prefix = request.context.render_prefix();
        let tail = request.render_blocks();

        let mut blocks = Vec::new();

        if prefix.chars().count() >= MIN_CACHEABLE_PREFIX_CHARS {
            blocks.push(serde_json::json!({
                "type": "text",
                "text": prefix,
                "cache_control": { "type": "ephemeral" },
            }));
        } else if !prefix.is_empty() {
            // Still send it, just not as a cache breakpoint.
            blocks.push(serde_json::json!({ "type": "text", "text": prefix }));
        }

        blocks.push(serde_json::json!({ "type": "text", "text": tail }));
        blocks
    }

    /// The single tool the model is forced to call, and its input schema.
    ///
    /// **Forced tool use rather than Anthropic's own `output_format` Structured Outputs**: the
    /// latter is a recent beta (`anthropic-beta: structured-outputs-2025-11-13`) with uncertain
    /// model coverage, while a tool's `input_schema` has been a long-supported way to get the same
    /// guarantee. The schema is the one every adapter shares, so a switch of provider never changes
    /// the shape callers parse.
    fn translation_tool() -> serde_json::Value {
        serde_json::json!({
            "name": TRANSLATION_SCHEMA_NAME,
            "description": "Return the translation of every requested block.",
            "input_schema": translation_json_schema(),
        })
    }

    fn request_body(&self, request: &TranslationRequest) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "temperature": 0.0,
            // Top-level `system`, not a message — the Messages API's own shape.
            "system": request.system_prompt(),
            "tools": [Self::translation_tool()],
            // `tool` (not `auto`) is what makes this a structured-output mechanism rather than an
            // option the model may decline.
            "tool_choice": { "type": "tool", "name": TRANSLATION_SCHEMA_NAME },
            "messages": [{
                "role": "user",
                "content": Self::content_blocks(request),
            }],
        })
    }

    /// Extracts the translations from a response's content blocks.
    ///
    /// The forced tool call means the answer normally arrives as a `tool_use` block's already-
    /// decoded `input`, with no string to re-parse. Plain `text` blocks are still handled as a
    /// fallback, so a model or gateway that answers in prose anyway is not a hard failure.
    fn blocks_from_content(
        content: &[ContentBlock],
    ) -> Result<Vec<crate::adapter::TranslatedBlock>, TranslationError> {
        if let Some(input) = content
            .iter()
            .find(|b| b.block_type.as_deref() == Some("tool_use"))
            .and_then(|b| b.input.as_ref())
        {
            return parse_block_value(input);
        }

        let text = content
            .iter()
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("");

        if text.is_empty() {
            return Err(TranslationError::MalformedResponse(
                "response contained neither a tool call nor text content".into(),
            ));
        }

        parse_block_map(&text)
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    #[serde(rename = "type")]
    block_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// A `tool_use` block's arguments — already-decoded JSON matching the tool's `input_schema`,
    /// which is where the translation comes back from now that the call is forced.
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    /// Tokens this request itself newly wrote into Anthropic's prompt cache — a distinct billing
    /// line from a plain cache miss (writing costs more than an ordinary fresh token).
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

impl TranslationAdapter for AnthropicAdapter {
    fn provider_id(&self) -> &str {
        "anthropic"
    }

    async fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslationError> {
        let started = Instant::now();

        let body = self.request_body(request);

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| TranslationError::Unreachable(e.to_string()))?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response.text().await.unwrap_or_default();
            return Err(error_for_status(status, &body));
        }

        let parsed: MessagesResponse = response
            .json()
            .await
            .map_err(|e| TranslationError::MalformedResponse(e.to_string()))?;

        let blocks = Self::blocks_from_content(&parsed.content)?;

        let usage = parsed.usage.as_ref();
        let total_tokens = usage.and_then(|u| match (u.input_tokens, u.output_tokens) {
            (Some(i), Some(o)) => Some(i + o),
            (Some(i), None) => Some(i),
            (None, Some(o)) => Some(o),
            _ => None,
        });

        Ok(TranslationResponse {
            blocks,
            provider_latency_ms: started.elapsed().as_millis() as u64,
            model: Some(self.model.clone()),
            prompt_tokens: usage.and_then(|u| u.input_tokens),
            cached_prompt_tokens: usage.and_then(|u| u.cache_read_input_tokens),
            cache_creation_tokens: usage.and_then(|u| u.cache_creation_input_tokens),
            completion_tokens: usage.and_then(|u| u.output_tokens),
            total_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{BlockId, GlossaryMatchSource, TranslationBlock, TranslationContext};

    fn request_with_prefix(prefix_entries: usize) -> TranslationRequest {
        let ctx = TranslationContext {
            glossary_matches: (0..prefix_entries)
                .map(|i| {
                    (
                        format!("名前{i}"),
                        vec![GlossaryMatchSource {
                            translation: format!("Name{i}"),
                            archive_id: "archive-a".to_string(),
                            chapter_name: None,
                        }],
                    )
                })
                .collect(),
            ..Default::default()
        };
        TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
                alternate_source_text: None,
            }],
            "en",
        )
        .with_context(ctx)
    }

    #[test]
    fn a_substantial_stable_prefix_gets_a_cache_breakpoint() {
        let blocks = AnthropicAdapter::content_blocks(&request_with_prefix(40));
        assert_eq!(blocks.len(), 2, "prefix and tail are separate blocks");
        assert!(
            blocks[0].get("cache_control").is_some(),
            "the stable prefix must carry the breakpoint"
        );
        assert!(
            blocks[1].get("cache_control").is_none(),
            "the dynamic batch tail must never be a cache breakpoint"
        );
    }

    #[test]
    fn a_tiny_prefix_is_sent_without_a_breakpoint() {
        let blocks = AnthropicAdapter::content_blocks(&request_with_prefix(1));
        assert!(blocks.iter().all(|b| b.get("cache_control").is_none()));
    }

    #[test]
    fn a_request_with_no_context_sends_only_the_batch() {
        let req = TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
                alternate_source_text: None,
            }],
            "en",
        );
        let blocks = AnthropicAdapter::content_blocks(&req);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0]["text"].as_str().unwrap().contains("こんにちは"));
    }

    #[test]
    fn the_request_forces_a_call_to_the_translation_tool() {
        let adapter = AnthropicAdapter::new(
            reqwest::Client::new(),
            "https://api.anthropic.com",
            "claude-sonnet-4-5",
            "sk-test",
        );
        let body = adapter.request_body(&request_with_prefix(1));

        // Deliberately NOT the `output_format` beta — see `translation_tool`'s own comment.
        assert!(body.get("output_format").is_none());
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], body["tools"][0]["name"]);
        assert_eq!(
            body["tools"][0]["input_schema"]["properties"]["translations"]["type"],
            "array"
        );
    }

    #[test]
    fn a_forced_tool_call_is_read_from_its_input_not_from_text() {
        let parsed: MessagesResponse = serde_json::from_str(
            r#"{"content":[{"type":"tool_use","id":"tu_1","name":"manga_translation",
                            "input":{"translations":[
                                {"block_id":"p1b0","translated_text":"Hello"}]}}]}"#,
        )
        .unwrap();
        let blocks = AnthropicAdapter::blocks_from_content(&parsed.content).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].translated_text, "Hello");
    }

    #[test]
    fn a_text_only_reply_still_parses_as_a_fallback() {
        let parsed: MessagesResponse =
            serde_json::from_str(r#"{"content":[{"type":"text","text":"{\"p1b0\":\"Hello\"}"}]}"#)
                .unwrap();
        let blocks = AnthropicAdapter::blocks_from_content(&parsed.content).unwrap();
        assert_eq!(blocks[0].translated_text, "Hello");
    }

    #[test]
    fn an_empty_reply_is_a_failure() {
        let parsed: MessagesResponse = serde_json::from_str(r#"{"content":[]}"#).unwrap();
        assert!(matches!(
            AnthropicAdapter::blocks_from_content(&parsed.content),
            Err(TranslationError::MalformedResponse(_))
        ));
    }

    #[test]
    fn usage_sums_input_and_output_tokens() {
        let parsed: MessagesResponse = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"{}"}],
                "usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":80}}"#,
        )
        .unwrap();
        let usage = parsed.usage.unwrap();
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(
            usage.input_tokens.unwrap() + usage.output_tokens.unwrap(),
            140
        );
    }
}
