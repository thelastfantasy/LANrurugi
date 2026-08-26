//! Generic LLM chat client — currently DeepSeek only, designed for easy addition of
//! other providers (Qwen, OpenAI, etc.) through the [`ChatBackend`] trait.
//!
//! Callers get an API key via [`resolve_api_key`] and call [`chat`] / [`json_chat`] for a plain
//! single-turn system+user exchange, or [`tool_chat`] for a multi-turn, tool-calling-capable
//! conversation (`specs/006-ai-plugin-wizard`'s own agentic generation loop). Failures return
//! `Err` with a human-readable message suitable for surfacing to the frontend as a toast.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

/// The one model name every `lanrurugi-llm` call site uses. `-pro`'s reasoning could exhaust the
/// whole `max_tokens` budget on `reasoning_content` and never emit `content` — switched to
/// `-flash` for all callers 2026-08-25 per explicit user direction.
const DEEPSEEK_MODEL: &str = "deepseek-v4-flash";

/// Steers `reasoning_effort` down without disabling it — a soft mitigation for the truncation
/// issue above; `tool_chat_streaming`'s own retry-with-thinking-disabled is the real fallback.
fn thinking_low_effort() -> serde_json::Value {
    json!({ "type": "enabled", "reasoning_effort": "low" })
}

/// Resolves the LLM API key: Redis `LRR_CONFIG.llm_api_key` first, env var
/// `DEEPSEEK_API_KEY` fallback (e.g. pre-injected via compose).
pub async fn resolve_api_key(redis_config: &Pool) -> Option<String> {
    let mut conn = redis_config.get().await.ok()?;
    let from_config = conn
        .hget::<_, _, Option<String>>(lanrurugi_storage::keys::CONFIG_KEY, "llm_api_key")
        .await
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());
    from_config.or_else(|| {
        std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
    })
}

/// Posts a chat-completions request body to DeepSeek and returns the parsed JSON response, or a
/// human-readable error — the HTTP call/status-code-error-translation logic shared by [`chat`]
/// and [`tool_chat`], factored out per constitution's "near-identical logic... factored into a
/// shared helper" rule rather than duplicated between them.
async fn post_chat_completion(
    redis_config: &Pool,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let key = resolve_api_key(redis_config)
        .await
        .ok_or_else(|| "DeepSeek API key not configured".to_string())?;

    // Overridable for Playwright E2E (T047/T048, `plan.md`'s Testing note — no live external LLM
    // dependency in CI) to point at a local mock responder instead of the real DeepSeek API;
    // unset in every real deployment, where it's always the real endpoint.
    let base_url = std::env::var("LANRURUGI_DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/chat/completions"))
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            warn!(error = %e, "LLM API request failed");
            format!("AI API 请求失败: {e}")
        })?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| {
        warn!(error = %e, "LLM API response body read failed");
        format!("AI API 响应读取失败: {e}")
    })?;

    // Non-2xx: parse the error body from DeepSeek if possible
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| text.clone());
        warn!(status = %status, body = %text, "LLM API returned error");
        let detail = match status.as_u16() {
            401 => format!("LLM: API Key 无效 (401) — {msg}"),
            402 => format!("LLM: 账户余额不足 (402) — {msg}"),
            429 => format!("LLM: 请求频率超限 (429) — {msg}"),
            500 | 503 => format!("LLM: 服务暂时不可用 ({}) — {msg}", status.as_u16()),
            _ => format!("LLM: API 错误 ({}) — {msg}", status.as_u16()),
        };
        return Err(detail);
    }

    serde_json::from_str::<serde_json::Value>(&text).map_err(|e| {
        warn!(error = %e, body = %text, "LLM API response not valid JSON");
        format!("AI 返回了非 JSON 数据: {e}")
    })
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

/// Calls the DeepSeek chat API and returns the raw text content of the first choice.
pub async fn chat(
    redis_config: &Pool,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String, String> {
    let body = json!({
        "model": DEEPSEEK_MODEL,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "response_format": { "type": "json_object" },
    });

    let parsed = post_chat_completion(redis_config, body).await?;

    let content = parsed["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            warn!(body = %parsed, "LLM API response missing choices[0].message.content");
            "AI 返回格式异常，缺少 content 字段".to_string()
        })?;

    Ok(strip_markdown_fence(content))
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
/// instead of answering. Most callers (e.g. `generate.rs`'s plugin-code loop, whose final answer is
/// a raw `.ts` file, not JSON) pass `false` throughout and rely on prompt wording alone, same as
/// before this parameter existed; `analyze_login.rs`'s loop is the first caller that actually wants
/// the hard guarantee, since its final answer must be a real parseable JSON array.
pub async fn tool_chat(
    redis_config: &Pool,
    messages: &[Message],
    tools: &[Tool],
    temperature: f32,
    max_tokens: u32,
    force_json_content: bool,
) -> Result<ToolChatResponse, String> {
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

    let parsed = post_chat_completion(redis_config, body).await?;
    parse_tool_chat_response(&parsed)
}

/// The actual message-loop decision logic `tool_chat` runs against a parsed API response body —
/// pulled out as its own pure function so it's unit-testable against a hand-built
/// `serde_json::Value` without needing a real (or mocked-transport) HTTP round-trip.
fn parse_tool_chat_response(parsed: &serde_json::Value) -> Result<ToolChatResponse, String> {
    let message = &parsed["choices"][0]["message"];
    if let Some(tool_calls) = message["tool_calls"].as_array() {
        if !tool_calls.is_empty() {
            let calls: Vec<ToolCall> = serde_json::from_value(message["tool_calls"].clone())
                .map_err(|e| {
                    warn!(error = %e, body = %parsed, "LLM API tool_calls not in expected shape");
                    format!("AI 返回的 tool_calls 格式异常: {e}")
                })?;
            return Ok(ToolChatResponse::ToolCalls(calls));
        }
    }

    let content = message["content"].as_str().ok_or_else(|| {
        warn!(body = %parsed, "LLM API response missing both tool_calls and content");
        "AI 返回格式异常，既无 tool_calls 也无 content 字段".to_string()
    })?;

    Ok(ToolChatResponse::Content(strip_markdown_fence(content)))
}

/// Streaming twin of [`tool_chat`] — same request shape (`messages`/`tools`/`force_json_content`
/// carry the identical meaning and constraints), but sets `"stream": true` and reports every
/// content delta to `on_content_delta` as it arrives over DeepSeek's own SSE response, rather than
/// waiting for the full response body. Added specifically because a real final-answer generation
/// (code + explanation, `generate.rs`) was observed taking 80+ seconds with zero visibility into
/// whether anything was actually happening — streaming the same call turns that dead time into a
/// live, incrementally-growing answer instead.
///
/// Tool-call deltas (the model requesting `fetch_page`) are accumulated internally and only
/// surfaced once complete, as the returned `ToolChatResponse::ToolCalls` — `on_content_delta` is
/// never called for a round that turns out to be a tool call, since a partial, not-yet-complete
/// `arguments` JSON string isn't meaningful to show incrementally. Which shape a given round will
/// take isn't known until the stream starts arriving, so this same function handles both regardless
/// of a caller's actual intent for that round — it does not need calling differently in the tool
/// call loop vs. the final answer.
/// Bundles [`tool_chat_streaming`]'s request-shaping parameters so the retry call site can pass
/// the same request twice without an 8-argument function.
struct StreamingRequest<'a> {
    messages: &'a [Message],
    tools: &'a [Tool],
    temperature: f32,
    max_tokens: u32,
    force_json_content: bool,
}

pub async fn tool_chat_streaming(
    redis_config: &Pool,
    messages: &[Message],
    tools: &[Tool],
    temperature: f32,
    max_tokens: u32,
    force_json_content: bool,
    mut on_content_delta: impl FnMut(&str),
) -> Result<ToolChatResponse, String> {
    let req = StreamingRequest {
        messages,
        tools,
        temperature,
        max_tokens,
        force_json_content,
    };
    // Retry once with thinking disabled on either: reasoning consuming the whole token budget
    // (never emits `content`), or the connection dropping mid-stream with no `finish_reason` ever
    // observed (indistinguishable from a clean finish otherwise).
    match tool_chat_streaming_once(redis_config, &req, &mut on_content_delta, true).await {
        Err(StreamingFailure::RetryableTruncation) => {
            warn!("retrying tool_chat_streaming with thinking disabled after a truncated/incomplete response");
            tool_chat_streaming_once(redis_config, &req, &mut on_content_delta, false)
                .await
                .map_err(|e| e.into_message())
        }
        Err(e) => Err(e.into_message()),
        Ok(r) => Ok(r),
    }
}

enum StreamingFailure {
    /// Reasoning exhausted the token budget, or the stream ended with no `finish_reason` seen —
    /// see `tool_chat_streaming`'s own docs.
    RetryableTruncation,
    Other(String),
}

impl StreamingFailure {
    fn into_message(self) -> String {
        match self {
            StreamingFailure::RetryableTruncation => {
                "AI 返回的内容不完整（连接中断或响应被截断），已重试仍未成功，请再次尝试"
                    .to_string()
            }
            StreamingFailure::Other(msg) => msg,
        }
    }
}

async fn tool_chat_streaming_once(
    redis_config: &Pool,
    req: &StreamingRequest<'_>,
    on_content_delta: &mut impl FnMut(&str),
    thinking_enabled: bool,
) -> Result<ToolChatResponse, StreamingFailure> {
    use futures_util::StreamExt;

    let key = resolve_api_key(redis_config)
        .await
        .ok_or_else(|| StreamingFailure::Other("DeepSeek API key not configured".to_string()))?;
    let base_url = std::env::var("LANRURUGI_DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/chat/completions"))
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            warn!(error = %e, "LLM streaming API request failed");
            StreamingFailure::Other(format!("AI API 请求失败: {e}"))
        })?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| text.clone());
        warn!(status = %status, body = %text, "LLM streaming API returned error");
        let detail = match status.as_u16() {
            401 => format!("LLM: API Key 无效 (401) — {msg}"),
            402 => format!("LLM: 账户余额不足 (402) — {msg}"),
            429 => format!("LLM: 请求频率超限 (429) — {msg}"),
            500 | 503 => format!("LLM: 服务暂时不可用 ({}) — {msg}", status.as_u16()),
            _ => format!("LLM: API 错误 ({}) — {msg}", status.as_u16()),
        };
        return Err(StreamingFailure::Other(detail));
    }

    let mut content = String::new();
    let mut tool_calls: Vec<StreamingToolCall> = Vec::new();
    let mut byte_stream = resp.bytes_stream();
    let mut line_buf = String::new();
    // Diagnostics only, not used for anything functional — a real, previously-unanswerable report
    // ("流式的话...你能正确获取吗？" followed shortly by an actual empty-response failure with zero
    // way to tell whether DeepSeek genuinely sent nothing or this parser silently dropped
    // something, 2026-08-25) needs *some* trace of what the raw SSE stream actually contained on
    // failure, without keeping the entire (potentially large) stream in memory for the success
    // path, which never needs it.
    let mut raw_line_count: usize = 0;
    let mut json_parse_failures: usize = 0;
    let mut last_raw_lines: std::collections::VecDeque<String> =
        std::collections::VecDeque::with_capacity(6);
    let mut saw_reasoning_content = false;
    let mut last_finish_reason: Option<String> = None;

    while let Some(chunk) = byte_stream.next().await {
        let chunk = chunk.map_err(|e| {
            warn!(error = %e, "LLM streaming API body read failed");
            StreamingFailure::Other(format!("AI API 流式响应读取失败: {e}"))
        })?;
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
        return Ok(ToolChatResponse::ToolCalls(calls));
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
            return Err(StreamingFailure::RetryableTruncation);
        }
        return Err(StreamingFailure::Other(
            "AI 返回格式异常，既无 tool_calls 也无 content 字段".to_string(),
        ));
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
        return Err(StreamingFailure::RetryableTruncation);
    }

    Ok(ToolChatResponse::Content(strip_markdown_fence(&content)))
}

#[derive(Default, Clone)]
struct StreamingToolCall {
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

/// Calls DeepSeek chat API and deserialises the response as JSON.
/// The system prompt should instruct the model to output valid JSON matching `T`.
pub async fn json_chat<T: serde::de::DeserializeOwned>(
    redis_config: &Pool,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<T, String> {
    let text = chat(redis_config, system, user, temperature, max_tokens).await?;
    serde_json::from_str::<T>(&text).map_err(|_| format!("AI 返回了无法解析的格式: {text}"))
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
