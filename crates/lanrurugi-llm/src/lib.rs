//! Generic LLM chat client — currently DeepSeek only, designed for easy addition of
//! other providers (Qwen, OpenAI, etc.) through the [`ChatBackend`] trait.
//!
//! Callers get an API key via [`resolve_api_key`] and call [`chat`] / [`json_chat`].
//! Failures (missing key, network, bad response) return `None` so every caller gets
//! graceful fallback for free.

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
/// Returns `None` on any failure.
pub async fn chat(
    redis_config: &Pool,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Option<String> {
    let key = resolve_api_key(redis_config).await?;
    let body = json!({
        "model": "deepseek-chat",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
    });

    let client = reqwest::Client::new();
    let resp = match client
        .post("https://api.deepseek.com/chat/completions")
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "LLM API request failed");
            return None;
        }
    };
    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "LLM API response body read failed");
            return None;
        }
    };
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, body = %text, "LLM API response not valid JSON");
            return None;
        }
    };
    parsed["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
}

/// Calls DeepSeek chat API and deserialises the response as JSON.
/// The system prompt should instruct the model to output valid JSON matching `T`.
pub async fn json_chat<T: serde::de::DeserializeOwned>(
    redis_config: &Pool,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
) -> Option<T> {
    let text = chat(redis_config, system, user, temperature, max_tokens).await?;
    match serde_json::from_str::<T>(&text) {
        Ok(v) => Some(v),
        Err(_) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.as_object().and_then(|o| o.values().next()).cloned())
            .and_then(|arr| serde_json::from_value::<T>(arr).ok()),
    }
}
