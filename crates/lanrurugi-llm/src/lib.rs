//! Generic LLM chat client — currently DeepSeek only, designed for easy addition of
//! other providers (Qwen, OpenAI, etc.).
//!
//! Every call takes a [`CallSpec`] (which feature is calling, its deadline, an optional
//! cancellation token) and runs under one policy (`retry`): a single deadline covering all
//! attempts, at most one retry for a retryable class, and a usage report folded into the ledger
//! (`usage`). Failures are an [`LlmError`], whose message is the human-readable text callers have
//! always shown and which converts losslessly into `String`.

pub mod error;
pub mod prefix;
pub mod retry;
pub mod usage;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

pub use error::{LlmError, LlmErrorClass};
pub use prefix::{Conversation, PrefixBreak};
pub use retry::{CallSpec, DEFAULT_DEADLINE};
pub use usage::{FeatureId, TokenUsage};

/// The one model name every `lanrurugi-llm` call site uses. `-pro`'s reasoning could exhaust the
/// whole `max_tokens` budget on `reasoning_content` and never emit `content` — switched to
/// `-flash` for all callers 2026-08-25 per explicit user direction.
pub const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";

/// Steers `reasoning_effort` down without disabling it — a soft mitigation for the truncation
/// issue above; a truncated stream's retry runs with thinking disabled as the real fallback.
fn thinking_low_effort() -> serde_json::Value {
    json!({ "type": "enabled", "reasoning_effort": "low" })
}

/// Resolves the LLM API key: Redis `LRR_CONFIG.llm_api_key` first, env var
/// `DEEPSEEK_API_KEY` fallback (e.g. pre-injected via compose).
pub async fn resolve_api_key(redis_config: &Pool) -> Option<String> {
    // An unreachable Redis still falls back to the env var rather than reading as "no key".
    let from_config = match redis_config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(lanrurugi_storage::keys::CONFIG_KEY, "llm_api_key")
            .await
            .ok()
            .flatten()
            .filter(|s| !s.trim().is_empty()),
        Err(_) => None,
    };
    from_config.or_else(|| {
        std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    })
}

/// How long a successful DeepSeek balance check is trusted before re-checking. Keeps the
/// pre-flight from doubling every LLM request's latency on chat-heavy flows (e.g. the plugin
/// wizard's multi-round agentic loop).
const BALANCE_CACHE_TTL: Duration = Duration::from_secs(60);

/// Last successful `{key} -> checked_at` pair. Only successes are cached; an account that just
/// ran out of balance must be re-checked on the very next call.
static BALANCE_CACHE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

fn deepseek_base_url() -> String {
    std::env::var("LANRURUGI_DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string())
}

/// The provider's own error message from a non-2xx body, or the raw body.
fn provider_message(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| text.to_string())
}

/// Checks the DeepSeek account's own `/user/balance` endpoint before an LLM call. For the
/// official DeepSeek API this is the authoritative "is the key both valid and funded" signal;
/// custom/mock base URLs that do not implement the endpoint are allowed to fall through (404 is
/// treated as "balance checking not supported", not as a hard failure).
async fn check_balance(key: &str) -> Result<(), LlmError> {
    let base_url = deepseek_base_url();
    let client = lanrurugi_core::http_client::default_client();
    let resp = client
        .get(format!("{base_url}/user/balance"))
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
        .map_err(LlmError::balance_request_failed)?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // A non-official/custom endpoint may simply not expose `/user/balance`.
        if status.as_u16() == 404 {
            return Ok(());
        }
        return Err(LlmError::from_balance_status(
            status.as_u16(),
            &provider_message(&text),
        ));
    }

    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if let Some(available) = body["is_available"].as_bool() {
        if !available {
            return Err(LlmError::balance_unavailable());
        }
    }
    Ok(())
}

/// Single availability gate used by every LLM call: verifies that a key is configured and, for
/// the official DeepSeek API, that the account currently has enough balance to make requests.
/// Success is cached briefly for the same key to avoid a `/user/balance` round-trip on every
/// model call.
pub async fn ensure_available(redis_config: &Pool) -> Result<(), LlmError> {
    let key = resolve_api_key(redis_config)
        .await
        .ok_or_else(LlmError::not_configured)?;

    {
        let cache = BALANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_key, checked_at)) = cache.as_ref() {
            if cached_key == &key && checked_at.elapsed() < BALANCE_CACHE_TTL {
                return Ok(());
            }
        }
    }

    check_balance(&key).await?;

    let mut cache = BALANCE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *cache = Some((key, Instant::now()));
    Ok(())
}

/// What a streaming call reports while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEvent<'a> {
    /// A chunk of the final answer's content.
    ContentDelta(&'a str),
    /// The round is being retried; anything streamed so far belongs to an abandoned attempt.
    /// Sent before any of the retry's output (FR-031).
    Retrying,
}

/// `LlmClient` is the single trait through which every LANrurugi feature talks to the LLM. Each
/// method verifies key/balance, then makes the call under the spec's deadline/retry/cancel policy.
#[allow(async_fn_in_trait)]
pub trait LlmClient: Send + Sync {
    async fn ensure_available(&self) -> Result<(), LlmError>;
    async fn chat(
        &self,
        spec: &CallSpec,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Result<String, LlmError>;
    async fn json_chat<T: serde::de::DeserializeOwned>(
        &self,
        spec: &CallSpec,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Result<T, LlmError>;
    async fn tool_chat(
        &self,
        spec: &CallSpec,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        force_json_content: bool,
    ) -> Result<ToolChatResponse, LlmError>;
    #[allow(clippy::too_many_arguments)]
    async fn tool_chat_streaming<F>(
        &self,
        spec: &CallSpec,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        force_json_content: bool,
        on_event: F,
    ) -> Result<ToolChatResponse, LlmError>
    where
        F: FnMut(StreamEvent<'_>) + Send;
}

/// The canonical backend is the Redis config pool itself; every `AppState.redis.config` Pool can
/// therefore be used directly as an `LlmClient`.
impl LlmClient for Pool {
    async fn ensure_available(&self) -> Result<(), LlmError> {
        ensure_available(self).await
    }

    async fn chat(
        &self,
        spec: &CallSpec,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        chat(self, spec, system, user, temperature, max_tokens).await
    }

    async fn json_chat<T: serde::de::DeserializeOwned>(
        &self,
        spec: &CallSpec,
        system: &str,
        user: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Result<T, LlmError> {
        json_chat(self, spec, system, user, temperature, max_tokens).await
    }

    async fn tool_chat(
        &self,
        spec: &CallSpec,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        force_json_content: bool,
    ) -> Result<ToolChatResponse, LlmError> {
        tool_chat(
            self,
            spec,
            messages,
            tools,
            temperature,
            max_tokens,
            force_json_content,
        )
        .await
    }

    async fn tool_chat_streaming<F>(
        &self,
        spec: &CallSpec,
        messages: &[Message],
        tools: &[Tool],
        temperature: f32,
        max_tokens: u32,
        force_json_content: bool,
        on_event: F,
    ) -> Result<ToolChatResponse, LlmError>
    where
        F: FnMut(StreamEvent<'_>) + Send,
    {
        tool_chat_streaming(
            self,
            spec,
            &StreamingRequest {
                messages,
                tools,
                temperature,
                max_tokens,
                force_json_content,
            },
            on_event,
        )
        .await
    }
}

/// Sends a chat-completions request and returns the response, or the class-bearing error for a
/// failed request or non-2xx status. Shared by every non-streaming call.
async fn send_chat_completion(
    key: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, LlmError> {
    let resp = lanrurugi_core::http_client::default_client()
        .post(format!("{}/chat/completions", deepseek_base_url()))
        .header("Authorization", format!("Bearer {key}"))
        .json(body)
        .send()
        .await
        .map_err(|e| {
            warn!(error = %e, stream, "LLM API request failed");
            LlmError::request_failed(e)
        })?;

    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = error::parse_retry_after(
        resp.headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
    );
    let text = resp.text().await.unwrap_or_default();
    warn!(status = %status, body = %text, stream, "LLM API returned error");
    Err(LlmError::from_status(
        status.as_u16(),
        &provider_message(&text),
        retry_after,
    ))
}

/// One non-streaming attempt: availability gate, request, parsed JSON body.
async fn post_chat_completion(
    redis_config: &Pool,
    body: &serde_json::Value,
) -> Result<serde_json::Value, LlmError> {
    ensure_available(redis_config).await?;
    let key = resolve_api_key(redis_config)
        .await
        .ok_or_else(LlmError::not_configured)?;
    let resp = send_chat_completion(&key, body, false).await?;
    let text = resp.text().await.map_err(|e| {
        warn!(error = %e, "LLM API response body read failed");
        LlmError::body_read_failed(e)
    })?;
    serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
        warn!(error = %e, body = %text, "LLM API response not valid JSON");
        LlmError::not_json(e)
    })
}

/// Runs a non-streaming request under the spec, turning each parsed body into `T` via `parse`.
async fn run_parsed<T>(
    redis_config: &Pool,
    spec: &CallSpec,
    body: serde_json::Value,
    parse: impl Fn(&serde_json::Value) -> Result<T, LlmError>,
) -> Result<T, LlmError> {
    let parse = &parse;
    retry::execute(redis_config, spec, |previous| {
        // A reply whose reasoning used the whole budget is retried once with thinking off, as the
        // streaming path does; the first attempt's body is untouched.
        let mut body = body.clone();
        if previous.as_ref().is_some_and(LlmError::is_truncation) {
            body["thinking"] = json!({ "type": "disabled" });
        }
        async move {
            match post_chat_completion(redis_config, &body).await {
                Ok(parsed) => (
                    parse(&parsed),
                    TokenUsage::from_usage_block(&parsed["usage"]),
                ),
                Err(e) => (Err(e), TokenUsage::default()),
            }
        }
    })
    .await
}

/// An empty answer cut off at the token limit: reasoning spent the whole `max_tokens` budget.
/// Small-budget JSON calls hit this, so it is a retryable truncation rather than bad output.
fn reasoning_exhausted(parsed: &serde_json::Value) -> bool {
    let choice = &parsed["choices"][0];
    choice["finish_reason"] == "length"
        && choice["message"]["content"]
            .as_str()
            .is_none_or(|c| c.trim().is_empty())
}

fn first_choice_content(parsed: &serde_json::Value) -> Result<String, LlmError> {
    if reasoning_exhausted(parsed) {
        warn!("LLM reply truncated with empty content (reasoning used the budget); retrying with thinking off");
        return Err(LlmError::truncated());
    }
    parsed["choices"][0]["message"]["content"]
        .as_str()
        .map(strip_markdown_fence)
        .ok_or_else(|| {
            warn!(body = %parsed, "LLM API response missing choices[0].message.content");
            LlmError::missing_content()
        })
}

fn chat_body(system: &str, user: &str, temperature: f32, max_tokens: u32) -> serde_json::Value {
    json!({
        "model": DEEPSEEK_MODEL,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "response_format": { "type": "json_object" },
    })
}

/// Calls the DeepSeek chat API and returns the raw text content of the first choice.
pub async fn chat(
    redis_config: &Pool,
    spec: &CallSpec,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String, LlmError> {
    let body = chat_body(system, user, temperature, max_tokens);
    run_parsed(redis_config, spec, body, first_choice_content).await
}

/// Calls DeepSeek chat API and deserialises the response as JSON.
/// The system prompt should instruct the model to output valid JSON matching `T`.
pub async fn json_chat<T: serde::de::DeserializeOwned>(
    redis_config: &Pool,
    spec: &CallSpec,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<T, LlmError> {
    let body = chat_body(system, user, temperature, max_tokens);
    run_parsed(redis_config, spec, body, |parsed| {
        let text = first_choice_content(parsed)?;
        serde_json::from_str::<T>(&text).map_err(|_| LlmError::unparseable(&text))
    })
    .await
}

/// Strips a markdown code fence wrapping the given text, if present, and trims the result —
/// shared cleanup for any raw model output that might come back fence-wrapped (`chat`'s JSON-mode
/// output, `tool_chat`'s final code/content output alike). Handles an arbitrary language tag right
/// after the opening ` ``` ` (` ```json `, ` ```ts `, ` ```typescript `, or none at all) by
/// discarding everything up to the first newline, rather than only recognizing a fixed list of
/// tags — a real, observed bug: the original fixed-tag version left a stray `ts\n` at the very
/// start of the output for a ` ```ts ` fence, since neither the `"```json"` nor the bare
/// `"```"` prefix match consumed the trailing `ts` language tag.
fn strip_markdown_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return text.to_string();
    };
    let Some(without_fence) = after_open.strip_suffix("```") else {
        return text.to_string();
    };
    // Discard everything before the first newline (the language tag, if any) — a tag is always
    // on the same line as the opening fence, never containing a newline itself.
    let body = match without_fence.split_once('\n') {
        Some((_tag, rest)) => rest,
        None => without_fence,
    };
    body.trim().to_string()
}

/// A single message in a [`tool_chat`] conversation — mirrors the OpenAI/DeepSeek wire shape
/// exactly (`role`, optional `content`, optional `tool_calls` on an assistant message requesting
/// tool execution, optional `tool_call_id` on a `role: "tool"` reply feeding a tool's result
/// back). `content: None` is only valid for an assistant message that carries `tool_calls`
/// instead (DeepSeek's own tool-calling contract, `specs/006-ai-plugin-wizard/research.md` §1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// A `role: "tool"` reply feeding a previously-requested tool call's result back to the
    /// model, per DeepSeek's own conversation-continuation contract — `tool_call_id` must match
    /// the `id` on the `ToolCall` this is answering.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    /// A plain assistant text message with no tool calls — for re-appending the model's own
    /// final `Content` answer back into the conversation history (e.g. before asking it to
    /// reformat that same answer in a follow-up turn), distinct from `assistant_tool_calls`.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Re-appends the model's own `tool_calls` request as an assistant message in the growing
    /// history — required by DeepSeek's conversation-continuation contract before any
    /// `tool_result` message answering one of these calls is valid.
    pub fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    }
}

/// One tool the model chose to invoke, as returned in an assistant message's `tool_calls` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// A JSON-encoded string (not a nested object) per the OpenAI/DeepSeek wire format — the
    /// caller deserializes this into whatever shape that specific tool's arguments take.
    pub arguments: String,
}

/// A tool declaration offered to the model in a [`tool_chat`] request — `parameters` is a JSON
/// Schema object describing the function's arguments.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl Tool {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: ToolFunction {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// [`tool_chat`]'s result — either the model's final answer (no further tool calls requested), or
/// one or more tools it wants executed before it can continue (FR-010's "system executes — AI
/// judges — proposes the next request" loop, `specs/006-ai-plugin-wizard/spec.md`).
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChatResponse {
    Content(String),
    ToolCalls(Vec<ToolCall>),
}

/// Calls the DeepSeek chat API with a full multi-turn `messages` history and an optional set of
/// tools the model may invoke, returning either its final content or the tool calls it wants
/// executed next. Model is [`DEEPSEEK_MODEL`] — see that constant's own docs.
///
/// `force_json_content` requests `response_format: json_object` for this call — DeepSeek's own
/// API documents `json_object` mode and `tools` as mutually exclusive on the same request, so this
/// MUST only ever be `true` on a call passing an empty `tools` slice (a round the caller already
/// knows won't request further tool use, typically a loop's final "give me your structured answer
/// now" turn) — never on a round where the model might still legitimately want to call a tool
/// instead of answering.
pub async fn tool_chat(
    redis_config: &Pool,
    spec: &CallSpec,
    messages: &[Message],
    tools: &[Tool],
    temperature: f32,
    max_tokens: u32,
    force_json_content: bool,
) -> Result<ToolChatResponse, LlmError> {
    check_conversation(redis_config, spec, messages, tools).await;
    let mut body = json!({
        "model": DEEPSEEK_MODEL,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "thinking": thinking_low_effort(),
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    } else if force_json_content {
        body["response_format"] = json!({ "type": "json_object" });
    }
    run_parsed(redis_config, spec, body, parse_tool_chat_response).await
}

/// Checks a conversation round extends the previous one (`prefix`), once per logical call — a
/// retry resends the same request and is not a new round. A break is logged and counted, never
/// refused: the round is still correct, only more expensive.
async fn check_conversation(
    redis_config: &Pool,
    spec: &CallSpec,
    messages: &[Message],
    tools: &[Tool],
) {
    let Some(conversation) = &spec.conversation else {
        return;
    };
    let messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or_default())
        .collect();
    if let Some(found) = conversation.advance(&json!(tools), &messages) {
        warn!(
            feature = spec.feature.as_str(),
            reason = found.as_str(),
            "conversation round does not extend the previous request; the prompt cache misses from here"
        );
        usage::record_prefix_break(redis_config, spec.feature).await;
    }
}

/// The actual message-loop decision logic `tool_chat` runs against a parsed API response body —
/// pulled out as its own pure function so it's unit-testable against a hand-built
/// `serde_json::Value` without needing a real (or mocked-transport) HTTP round-trip.
fn parse_tool_chat_response(parsed: &serde_json::Value) -> Result<ToolChatResponse, LlmError> {
    let message = &parsed["choices"][0]["message"];
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        if !tool_calls.is_empty() {
            let calls: Vec<ToolCall> = serde_json::from_value(message["tool_calls"].clone())
                .map_err(|e| {
                    warn!(error = %e, body = %parsed, "LLM API tool_calls not in expected shape");
                    LlmError::bad_tool_calls(e)
                })?;
            return Ok(ToolChatResponse::ToolCalls(calls));
        }
    }

    if reasoning_exhausted(parsed) {
        warn!("LLM tool reply truncated with empty content (reasoning used the budget); retrying with thinking off");
        return Err(LlmError::truncated());
    }
    let content = message["content"].as_str().ok_or_else(|| {
        warn!(body = %parsed, "LLM API response missing both tool_calls and content");
        LlmError::neither_tool_calls_nor_content()
    })?;

    Ok(ToolChatResponse::Content(strip_markdown_fence(content)))
}

/// A streaming request's shape — the same meaning and constraints as [`tool_chat`]'s parameters.
pub struct StreamingRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [Tool],
    pub temperature: f32,
    pub max_tokens: u32,
    pub force_json_content: bool,
}

/// Streaming twin of [`tool_chat`]: sets `"stream": true` and reports every content delta as it
/// arrives over DeepSeek's SSE response, rather than waiting for the full body. Added because a
/// real final-answer generation was observed taking 80+ seconds with zero visibility.
///
/// Tool-call deltas are accumulated internally and only surfaced once complete, as
/// `ToolChatResponse::ToolCalls` — a partial `arguments` string isn't meaningful to show. A
/// truncated stream (reasoning exhausted the budget, or no `finish_reason` ever arrived) is
/// retried once with thinking disabled; any retry is announced with [`StreamEvent::Retrying`]
/// before its output.
pub async fn tool_chat_streaming(
    redis_config: &Pool,
    spec: &CallSpec,
    req: &StreamingRequest<'_>,
    on_event: impl FnMut(StreamEvent<'_>) + Send,
) -> Result<ToolChatResponse, LlmError> {
    // Shared by reference into each attempt's future (an `FnMut` closure cannot lend its own
    // captured `&mut` to the future it returns).
    check_conversation(redis_config, spec, req.messages, req.tools).await;
    let on_event = Mutex::new(on_event);
    let on_event = &on_event;
    retry::execute(redis_config, spec, |previous| async move {
        let thinking_enabled = match &previous {
            Some(err) => {
                (on_event.lock().unwrap_or_else(|e| e.into_inner()))(StreamEvent::Retrying);
                if err.is_truncation() {
                    warn!("retrying tool_chat_streaming with thinking disabled after a truncated/incomplete response");
                }
                !err.is_truncation()
            }
            None => true,
        };
        tool_chat_streaming_once(redis_config, req, thinking_enabled, |text| {
            (on_event.lock().unwrap_or_else(|e| e.into_inner()))(StreamEvent::ContentDelta(text))
        })
        .await
    })
    .await
}

async fn tool_chat_streaming_once(
    redis_config: &Pool,
    req: &StreamingRequest<'_>,
    thinking_enabled: bool,
    mut on_content_delta: impl FnMut(&str),
) -> (Result<ToolChatResponse, LlmError>, TokenUsage) {
    use futures_util::StreamExt;

    if let Err(e) = ensure_available(redis_config).await {
        return (Err(e), TokenUsage::default());
    }
    let Some(key) = resolve_api_key(redis_config).await else {
        return (Err(LlmError::not_configured()), TokenUsage::default());
    };

    let thinking = if thinking_enabled {
        thinking_low_effort()
    } else {
        json!({ "type": "disabled" })
    };
    let mut body = json!({
        "model": DEEPSEEK_MODEL,
        "messages": req.messages,
        "temperature": req.temperature,
        "max_tokens": req.max_tokens,
        "stream": true,
        "thinking": thinking,
    });
    if !req.tools.is_empty() {
        body["tools"] = json!(req.tools);
    } else if req.force_json_content {
        body["response_format"] = json!({ "type": "json_object" });
    }

    let resp = match send_chat_completion(&key, &body, true).await {
        Ok(resp) => resp,
        Err(e) => return (Err(e), TokenUsage::default()),
    };

    let mut usage = TokenUsage::default();
    let mut content = String::new();
    let mut tool_calls: Vec<StreamingToolCall> = Vec::new();
    let mut byte_stream = resp.bytes_stream();
    let mut line_buf = String::new();
    // Diagnostics only: some trace of what the raw SSE stream contained on failure, without
    // keeping the whole stream in memory for the success path.
    let mut raw_line_count: usize = 0;
    let mut json_parse_failures: usize = 0;
    let mut last_raw_lines: std::collections::VecDeque<String> =
        std::collections::VecDeque::with_capacity(6);
    let mut saw_reasoning_content = false;
    let mut last_finish_reason: Option<String> = None;

    while let Some(chunk) = byte_stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                warn!(error = %e, "LLM streaming API body read failed");
                return (Err(LlmError::stream_read_failed(e)), usage);
            }
        };
        line_buf.push_str(&String::from_utf8_lossy(&chunk));

        // SSE frames are newline-delimited; a chunk boundary can split one mid-line, so only
        // consume complete lines and leave any trailing partial line buffered for the next chunk.
        while let Some(newline_at) = line_buf.find('\n') {
            let line = line_buf[..newline_at].trim_end_matches('\r').to_string();
            line_buf.drain(..=newline_at);

            let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            raw_line_count += 1;
            if last_raw_lines.len() >= 5 {
                last_raw_lines.pop_front();
            }
            last_raw_lines.push_back(data.to_string());
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                json_parse_failures += 1;
                continue;
            };
            if event["usage"].is_object() {
                usage = TokenUsage::from_usage_block(&event["usage"]);
            }
            let delta = &event["choices"][0]["delta"];
            if let Some(reason) = event["choices"][0]["finish_reason"].as_str() {
                last_finish_reason = Some(reason.to_string());
            }

            if delta["reasoning_content"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            {
                saw_reasoning_content = true;
            }

            if let Some(text) = delta["content"].as_str() {
                if !text.is_empty() {
                    on_content_delta(text);
                    content.push_str(text);
                }
            }

            if let Some(delta_calls) = delta["tool_calls"].as_array() {
                for delta_call in delta_calls {
                    let index = delta_call["index"].as_u64().unwrap_or(0) as usize;
                    if tool_calls.len() <= index {
                        tool_calls.resize(index + 1, StreamingToolCall::default());
                    }
                    let call = &mut tool_calls[index];
                    if let Some(id) = delta_call["id"].as_str() {
                        call.id.push_str(id);
                    }
                    if let Some(kind) = delta_call["type"].as_str() {
                        call.kind.push_str(kind);
                    }
                    if let Some(name) = delta_call["function"]["name"].as_str() {
                        call.name.push_str(name);
                    }
                    if let Some(args) = delta_call["function"]["arguments"].as_str() {
                        call.arguments.push_str(args);
                    }
                }
            }
        }
    }

    if !tool_calls.is_empty() {
        let calls: Vec<ToolCall> = tool_calls
            .into_iter()
            .map(|c| ToolCall {
                id: c.id,
                kind: if c.kind.is_empty() {
                    "function".to_string()
                } else {
                    c.kind
                },
                function: ToolCallFunction {
                    name: c.name,
                    arguments: c.arguments,
                },
            })
            .collect();
        return (Ok(ToolChatResponse::ToolCalls(calls)), usage);
    }

    if content.is_empty() {
        warn!(
            raw_line_count,
            json_parse_failures,
            saw_reasoning_content,
            last_finish_reason = ?last_finish_reason,
            last_raw_lines = ?last_raw_lines,
            "LLM streaming API produced neither content nor tool_calls",
        );
        if saw_reasoning_content && last_finish_reason.as_deref() == Some("length") {
            return (Err(LlmError::truncated()), usage);
        }
        return (Err(LlmError::neither_tool_calls_nor_content()), usage);
    }

    // Stream ended with no `finish_reason` ever seen — the connection was likely cut mid-response
    // rather than a normal completion, even though `content` may be non-empty.
    if last_finish_reason.is_none() {
        warn!(
            raw_line_count,
            json_parse_failures,
            content_len = content.len(),
            "LLM streaming API's connection ended before any finish_reason was ever observed \
             (likely a dropped connection mid-response, not a normal completion)",
        );
        return (Err(LlmError::truncated()), usage);
    }

    (
        Ok(ToolChatResponse::Content(strip_markdown_fence(&content))),
        usage,
    )
}

#[derive(Default, Clone)]
struct StreamingToolCall {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tool_chat_tests {
    use super::*;
    use serde_json::json;

    // T045 — unit coverage for `parse_tool_chat_response`'s message-loop plumbing: given a parsed
    // API response body (the same shape `post_chat_completion` returns), decide whether the model
    // produced final content or requested tool calls.

    #[test]
    fn plain_content_response_yields_content_variant() {
        let body = json!({
            "choices": [{
                "message": { "role": "assistant", "content": "export function pluginInfo() {}" }
            }]
        });
        let result = parse_tool_chat_response(&body).unwrap();
        assert_eq!(
            result,
            ToolChatResponse::Content("export function pluginInfo() {}".to_string())
        );
    }

    #[test]
    fn markdown_fenced_content_is_stripped() {
        let body = json!({
            "choices": [{
                "message": { "role": "assistant", "content": "```ts\nexport function pluginInfo() {}\n```" }
            }]
        });
        let result = parse_tool_chat_response(&body).unwrap();
        assert_eq!(
            result,
            ToolChatResponse::Content("export function pluginInfo() {}".to_string())
        );
    }

    #[test]
    fn tool_calls_response_yields_tool_calls_variant() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "fetch_page", "arguments": "{\"url\":\"https://example.com\"}" }
                    }]
                }
            }]
        });
        let result = parse_tool_chat_response(&body).unwrap();
        match result {
            ToolChatResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].function.name, "fetch_page");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }

    #[test]
    fn empty_tool_calls_array_falls_through_to_content() {
        let body = json!({
            "choices": [{
                "message": { "role": "assistant", "content": "final answer", "tool_calls": [] }
            }]
        });
        let result = parse_tool_chat_response(&body).unwrap();
        assert_eq!(
            result,
            ToolChatResponse::Content("final answer".to_string())
        );
    }

    #[test]
    fn missing_both_tool_calls_and_content_is_an_error() {
        let body = json!({
            "choices": [{ "message": { "role": "assistant" } }]
        });
        assert!(parse_tool_chat_response(&body).is_err());
    }

    #[test]
    fn malformed_tool_calls_shape_is_an_error() {
        let body = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{ "id": "call_1" }]
                }
            }]
        });
        assert!(parse_tool_chat_response(&body).is_err());
    }
}

#[cfg(test)]
mod strip_markdown_fence_tests {
    use super::*;

    // A real, observed bug (006-ai-plugin-wizard's own generation flow): a ` ```ts ` fence left a
    // stray "ts\n" at the start of the "stripped" output, since the original implementation only
    // special-cased "```json" and bare "```", not an arbitrary language tag.
    #[test]
    fn strips_an_arbitrary_language_tag() {
        assert_eq!(
            strip_markdown_fence("```ts\ninterface Foo {}\n```"),
            "interface Foo {}"
        );
        assert_eq!(
            strip_markdown_fence("```typescript\nconst x = 1;\n```"),
            "const x = 1;"
        );
    }

    #[test]
    fn strips_json_tag() {
        assert_eq!(
            strip_markdown_fence("```json\n{\"a\": 1}\n```"),
            "{\"a\": 1}"
        );
    }

    #[test]
    fn strips_bare_fence_with_no_language_tag() {
        assert_eq!(
            strip_markdown_fence("```\nplain content\n```"),
            "plain content"
        );
    }

    #[test]
    fn leaves_unfenced_text_untouched() {
        assert_eq!(strip_markdown_fence("no fence here"), "no fence here");
    }

    #[test]
    fn leaves_a_fence_missing_its_closing_backticks_untouched() {
        // The real bug this guards against (T-new, generate.rs's own `looks_like_plugin_code`
        // completeness check): a response truncated by `max_tokens` mid-file has an opening fence
        // but no closing one — must NOT be mistaken for a valid fence and mangled further; the
        // caller's own completeness check (balanced braces) is what catches this case, not this
        // function silently stripping a phantom fence.
        let truncated = "```ts\ninterface Foo {\n  bar: string;";
        assert_eq!(strip_markdown_fence(truncated), truncated);
    }
}
