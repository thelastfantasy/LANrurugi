//! OpenAI-compatible Chat Completions adapter (research.md §5/§15).
//!
//! Covers OpenAI itself, any OpenAI-compatible provider, and Ollama — the latter via its own
//! OpenAI-compatible `/v1` endpoint, configured as an instance of this adapter rather than a third
//! code path.
//!
//! No explicit cache marker exists in this API: OpenAI-family prompt caching is automatic and
//! exact-prefix-matched, so the only thing that makes it work is
//! [`TranslationRequest::render_user_message`]'s stable-content-first ordering.

use std::time::Instant;

use serde::Deserialize;

use crate::adapter::{
    error_for_status, parse_block_map, translation_json_schema, TranslationAdapter,
    TranslationError, TranslationRequest, TranslationResponse, TRANSLATION_SCHEMA_NAME,
};

/// Default cap on generated tokens. A batch of manga text blocks is short; this only guards a
/// pathological non-terminating response.
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct OpenAiCompatAdapter {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    provider_id: String,
}

impl std::fmt::Debug for OpenAiCompatAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // See `AnthropicAdapter`'s equivalent impl — never derive `Debug` on a credential holder.
        f.debug_struct("OpenAiCompatAdapter")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("provider_id", &self.provider_id)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl OpenAiCompatAdapter {
    /// `api_key` is `None` for a local Ollama instance, which requires no credential.
    pub fn new(
        client: reqwest::Client,
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            provider_id: "openai-compatible".to_string(),
        }
    }

    /// Distinguishes providers in the cache key so two backends never share cached output
    /// (FR-016).
    pub fn with_provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = provider_id.into();
        self
    }

    /// Builds the request body.
    ///
    /// `response_format: json_schema` is the strongest constraint of the three adapters — it pins
    /// the field structure, not just JSON validity. The field name is `response_format` because
    /// this adapter targets the **Chat Completions** endpoint; the newer Responses API moved the
    /// same concept to `text.format` and the two are not interchangeable.
    ///
    /// A server that doesn't understand `json_schema` (an older OpenAI-compatible gateway, some
    /// Ollama builds) typically ignores the unknown field, which is why
    /// [`crate::adapter::parse_block_map`] still accepts an unconstrained reply.
    fn request_body(&self, request: &TranslationRequest) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            // Deterministic output: the same page should not retranslate differently on a cache
            // miss after eviction.
            "temperature": 0.0,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": TRANSLATION_SCHEMA_NAME,
                    "strict": true,
                    "schema": translation_json_schema(),
                },
            },
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
    /// OpenAI reports cache hits nested under `prompt_tokens_details`.
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

impl TranslationAdapter for OpenAiCompatAdapter {
    fn provider_id(&self) -> &str {
        &self.provider_id
    }

    async fn translate(
        &self,
        request: &TranslationRequest,
    ) -> Result<TranslationResponse, TranslationError> {
        let started = Instant::now();

        let body = self.request_body(request);

        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);

        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let response = req
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

        let usage = parsed.usage.as_ref();
        Ok(TranslationResponse {
            blocks: parse_block_map(content)?,
            provider_latency_ms: started.elapsed().as_millis() as u64,
            model: Some(self.model.clone()),
            prompt_tokens: usage.and_then(|u| u.prompt_tokens),
            cached_prompt_tokens: usage
                .and_then(|u| u.prompt_tokens_details.as_ref())
                .and_then(|d| d.cached_tokens),
            // OpenAI-family APIs don't distinguish a cache *write* from an ordinary miss the way
            // Anthropic does — nothing to report here.
            cache_creation_tokens: None,
            completion_tokens: usage.and_then(|u| u.completion_tokens),
            total_tokens: usage.and_then(|u| u.total_tokens),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        let adapter = OpenAiCompatAdapter::new(
            reqwest::Client::new(),
            "https://api.example.com/v1/",
            "gpt-x",
            Some("secret".into()),
        );
        assert_eq!(adapter.base_url, "https://api.example.com/v1");
    }

    #[test]
    fn ollama_is_configured_as_this_adapter_without_a_key() {
        let adapter = OpenAiCompatAdapter::new(
            reqwest::Client::new(),
            "http://127.0.0.1:11434/v1",
            "qwen2.5",
            None,
        )
        .with_provider_id("ollama");
        assert_eq!(adapter.provider_id(), "ollama");
        assert!(adapter.api_key.is_none());
    }

    #[test]
    fn usage_parses_openai_cached_token_shape() {
        let parsed: ChatResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"{}"}}],
                "usage":{"total_tokens":120,"prompt_tokens_details":{"cached_tokens":64}}}"#,
        )
        .unwrap();
        let usage = parsed.usage.unwrap();
        assert_eq!(usage.total_tokens, Some(120));
        assert_eq!(usage.prompt_tokens_details.unwrap().cached_tokens, Some(64));
    }

    #[test]
    fn the_request_constrains_the_response_with_a_json_schema() {
        use crate::adapter::{BlockId, TranslationBlock};

        let adapter = OpenAiCompatAdapter::new(
            reqwest::Client::new(),
            "https://api.example.com/v1",
            "gpt-x",
            Some("secret".into()),
        );
        let body = adapter.request_body(&TranslationRequest::new(
            vec![TranslationBlock {
                block_id: BlockId::from("p1b0"),
                source_text: "こんにちは".into(),
            }],
            "en",
        ));

        // Chat Completions spells this `response_format`; the Responses API's `text.format` is a
        // different endpoint's shape and must not appear here.
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert!(body.get("text").is_none());
        let schema = &body["response_format"]["json_schema"];
        assert_eq!(schema["strict"], true);
        assert_eq!(
            schema["schema"]["properties"]["translations"]["type"],
            "array"
        );
    }

    #[test]
    fn response_without_usage_still_parses() {
        let parsed: ChatResponse =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"{}"}}]}"#).unwrap();
        assert!(parsed.usage.is_none());
    }
}
