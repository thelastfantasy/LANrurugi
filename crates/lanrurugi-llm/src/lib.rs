//! Generic LLM chat client — currently DeepSeek only, designed for easy addition of
//! other providers (Qwen, OpenAI, etc.) through the [`ChatBackend`] trait.
//!
//! Callers get an API key via [`resolve_api_key`] and call [`chat`] / [`json_chat`].
//! Failures return `Err` with a human-readable message suitable for surfacing to the
//! frontend as a toast.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde_json::json;
use tracing::warn;

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

/// Calls the DeepSeek chat API and returns the raw text content of the first choice.
pub async fn chat(
    redis_config: &Pool,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Result<String, String> {
    let key = resolve_api_key(redis_config)
        .await
        .ok_or_else(|| "DeepSeek API key not configured".to_string())?;

    let body = json!({
        "model": "deepseek-chat",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "response_format": { "type": "json_object" },
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.deepseek.com/chat/completions")
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

    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        warn!(error = %e, body = %text, "LLM API response not valid JSON");
        format!("AI 返回了非 JSON 数据: {e}")
    })?;

    let mut content = parsed["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            warn!(body = %text, "LLM API response missing choices[0].message.content");
            "AI 返回格式异常，缺少 content 字段".to_string()
        })?;

    // Strip markdown code fences (```json / ```) if the model wrapped the output
    if let Some(inner) = content
        .strip_prefix("```json")
        .and_then(|s| s.strip_suffix("```"))
        .or_else(|| {
            content
                .strip_prefix("```")
                .and_then(|s| s.strip_suffix("```"))
        })
    {
        content = inner.trim().to_string();
    }

    Ok(content)
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
