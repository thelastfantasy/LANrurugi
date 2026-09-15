//! DeepSeek adapter (research.md §15).
//!
//! Wire-format-wise this is OpenAI Chat Completions shaped, but it is a distinct adapter because
//! its **caching behaviour differs in ways a maintainer reading the Anthropic adapter's
//! "needs explicit breakpoints" framing would get wrong**:
//!
//! - **Zero opt-in.** No `cache_control` field, no request parameter — context caching is automatic
//!   and disk-backed. There is deliberately nothing to set here; the only thing that earns a cache
//!   hit is the stable-prefix-first ordering every adapter already shares.
//! - **A much larger hit discount** than the other two providers, which makes keeping the
//!   Terminology-Glossary prefix stable disproportionately valuable on this backend.
//! - **Cache units are created opportunistically** (end of user input, end of model output,
//!   detected common prefixes, fixed token-interval cut points), so DeepSeek may cache more than
//!   Anthropic's explicit-breakpoint-only model without any extra work here.
//!
//! Cache hits are reported as `prompt_cache_hit_tokens`, a DeepSeek-specific field name — another
//! reason this isn't simply a configuration of the OpenAI-compatible adapter.

use std::time::Instant;

use serde::Deserialize;

use crate::adapter::{
    error_for_status, parse_block_map, TranslationAdapter, TranslationError, TranslationRequest,
    TranslationResponse,
};

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Matches the model `lanrurugi-llm` already standardised on for this project's other LLM call
/// sites. The older `deepseek-chat`/`deepseek-reasoner` names are discontinued and deliberately
/// not used.
pub const DEFAULT_MODEL: &str = "deepseek-v4-flash";

pub struct DeepSeekAdapter {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl std::fmt::Debug for DeepSeekAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // See `AnthropicAdapter`'s equivalent impl — never derive `Debug` on a credential holder.
        f.debug_struct("DeepSeekAdapter")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl DeepSeekAdapter {
    pub fn new(client: reqwest::Client, api_key: impl Into<String>) -> Self {
        Self {
            client,
            base_url: std::env::var("LANRURUGI_DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            model: DEFAULT_MODEL.to_string(),
            api_key: api_key.into(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Builds the request body.
    ///
    /// Note the absence of any cache-control field: caching here is automatic. The stable prefix
    /// ordering inside `render_user_message` is the entire mechanism.
    ///
    /// `response_format: json_object` is DeepSeek's only structured-output mode — it guarantees
    /// syntactically valid JSON but **not** a schema, unlike the OpenAI-compatible adapter's
    /// `json_schema`. That is why `parse_block_map` still accepts the older flat-map shape.
    ///
    /// `thinking: disabled` — DeepSeek's reasoning models default to thinking mode *on*, and
    /// reasoning tokens count against the same `max_tokens` budget as the answer itself. Confirmed
    /// live: real requests against this page-translation workload (short, mechanical — no multi-step
    /// reasoning needed) repeatedly spent the entire 4096-token budget on `reasoning_content` and
    /// returned empty `content`. `temperature`/`top_p` are documented as having no effect on
    /// thinking mode, so only this field actually addresses it.
    fn request_body(&self, request: &TranslationRequest) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "temperature": 0.0,
            "response_format": { "type": "json_object" },
            "thinking": { "type": "disabled" },
            "messages": [
                { "role": "system", "content": request.system_prompt() },
                { "role": "user", "content": request.render_user_message() },
            ],
        })
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
    /// DeepSeek's own cache-hit field — named differently from OpenAI's nested
    /// `prompt_tokens_details.cached_tokens`. Sums with `prompt_cache_miss_tokens` below to
    /// `prompt_tokens`.
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)] // Not separately surfaced — `prompt_tokens - prompt_cache_hit_tokens`
    // already gives the same figure, and `TranslationResponse` has no field of its own for it.
    prompt_cache_miss_tokens: Option<u64>,
}

impl TranslationAdapter for DeepSeekAdapter {
    fn provider_id(&self) -> &str {
        "deepseek"
    }

    async fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslationError> {
        let started = Instant::now();

        let body = self.request_body(request);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| TranslationError::Unreachable(e.to_string()))?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let body = response.text().await.unwrap_or_default();
            return Err(error_for_status(status, &body));
        }

        let parsed: ChatResponse = response
            .json()
            .await
            .map_err(|e| TranslationError::MalformedResponse(e.to_string()))?;

        let content = parsed
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .ok_or_else(|| {
                TranslationError::MalformedResponse("response contained no message content".into())
            })?;

        // `thinking: disabled` in the request body (see `request_body`) should make this
        // unreachable in normal operation — confirmed live: before that field was added, real
        // requests against this exact workload repeatedly spent the entire 4096-token budget on
        // `reasoning_content` and returned empty `content` with `finish_reason: "stop"`. Kept as a
        // safety net with its own log line so a regression here is distinguishable from "content
        // had text but it wasn't valid JSON".
        if content.trim().is_empty() {
            tracing::warn!(
                total_tokens = ?parsed.usage.as_ref().and_then(|u| u.total_tokens),
                "deepseek returned an empty message content despite thinking being disabled"
            );
        }

        let usage = parsed.usage.as_ref();
        Ok(TranslationResponse {
            blocks: parse_block_map(content)?,
            provider_latency_ms: started.elapsed().as_millis() as u64,
            model: Some(self.model.clone()),
            prompt_tokens: usage.and_then(|u| u.prompt_tokens),
            cached_prompt_tokens: usage.and_then(|u| u.prompt_cache_hit_tokens),
            // DeepSeek has no separate "cache write" charge (research.md's own docs on this
            // adapter: caching is automatic/opportunistic, not an explicit opt-in the caller pays
            // extra for) — unlike Anthropic, a cache miss here is priced the same as it would be
            // with no caching at all, so there's nothing to report in this field.
            cache_creation_tokens: None,
            completion_tokens: usage.and_then(|u| u.completion_tokens),
            total_tokens: usage.and_then(|u| u.total_tokens),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{
        BlockId, GlossaryMatchSource, TranslationBlock, TranslationContext, TranslationRequest,
    };

    #[test]
    fn deepseek_specific_cache_field_is_read() {
        let parsed: ChatResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"{}"}}],
                "usage":{"total_tokens":200,"prompt_cache_hit_tokens":128}}"#,
        )
        .unwrap();
        assert_eq!(
            parsed.usage.unwrap().prompt_cache_hit_tokens,
            Some(128),
            "DeepSeek reports cache hits under its own field name"
        );
    }

    #[test]
    fn request_body_carries_no_cache_control_field() {
        // Regression guard for the documented distinction: adding a breakpoint here would be
        // meaningless at best, and is not what makes DeepSeek cache.
        let req = TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
            }],
            "en",
        )
        .with_context(TranslationContext {
            glossary_matches: vec![(
                "さゆき".into(),
                vec![GlossaryMatchSource {
                    translation: "Sayuki".to_string(),
                    archive_id: "archive-a".to_string(),
                    chapter_name: None,
                }],
            )],
            ..Default::default()
        });

        let msg = req.render_user_message();
        assert!(!msg.contains("cache_control"));
        assert!(
            msg.find("Sayuki").unwrap() < msg.find("こんにちは").unwrap(),
            "stable-prefix ordering is the only caching mechanism on this backend"
        );
    }

    #[test]
    fn the_request_asks_for_a_json_object_response() {
        let adapter = DeepSeekAdapter::new(reqwest::Client::new(), "sk-test");
        let body = adapter.request_body(&TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
            }],
            "en",
        ));

        // DeepSeek has no `json_schema` mode; `json_object` is the strongest guarantee available.
        assert_eq!(body["response_format"]["type"], "json_object");
        assert!(body.get("cache_control").is_none());
        // ...and that mode rejects the request outright unless the prompt says "json".
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.to_lowercase().contains("json"));
    }

    #[test]
    fn default_model_is_the_current_non_discontinued_name() {
        assert_eq!(DEFAULT_MODEL, "deepseek-v4-flash");
    }
}
