//! LLM-based reranking of the embedding-prefiltered recommendation candidates.
//!
//! Two-tier architecture (the user's constraint: don't dump the whole library at the API):
//! the local embedding model pre-filters every candidate down to a small shortlist (see
//! `RecommendService::recommendations` — top ~20), and this module sends ONLY that shortlist
//! (id + title, no tags) plus the current title/tags and an explicit instruction to DeepSeek's
//! chat API. The LLM is the part that understands "this is volume 10, the next volume 11 must
//! be #1" — something no embedding model can do (their similarity scores between same-series
//! volumes are indistinguishable noise).
//!
//! Failures degrade gracefully: any error (no API key, network, non-JSON reply) returns `None`
//! and the caller falls back to the embedding pre-filter's own ordering. The key is read from
//! `LRR_CONFIG`'s `llm_api_key` (Settings page) or the `DEEPSEEK_API_KEY` env var.

use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use crate::AppState;
use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};

/// How many candidates the embedding pre-filter hands to the LLM (the user's spec: filter a
/// few dozen to ~100, then send that pool + the instruction to the API). 100 titles ≈ a few
/// thousand tokens — comfortably within deepseek-chat's context, and large enough that the
/// true next-volume pick is essentially never filtered out.
pub const PREFILTER_COUNT: usize = 100;

/// Resolves the LLM API key: Settings-page field (`LRR_CONFIG.llm_api_key`) first, env var
/// fallback (deployment-injected secrets).
pub async fn llm_api_key(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
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

#[derive(Debug, Deserialize)]
struct LlmChoice {
    message: LlmMessage,
}

#[derive(Debug, Deserialize)]
struct LlmMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct LlmResponse {
    choices: Vec<LlmChoice>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LlmPick {
    id: String,
    #[serde(default)]
    title: String,
}

/// Reranks `candidates` via DeepSeek's chat API with an explicit "next volume first" instruction.
/// Returns `None` on any failure — the caller falls back to the embedding order.
pub async fn llm_rerank(
    state: &AppState,
    current_title: &str,
    current_tags: &str,
    current_pagecount: u32,
    candidates: &[ArchiveMeta],
    limit: usize,
) -> Option<Vec<Recommendation>> {
    let key = llm_api_key(state).await?;
    let candidate_list: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}. {}: {} | 标签: {} | 页数: {}页",
                i + 1,
                c.id,
                c.title,
                c.tags,
                c.pagecount
            )
        })
        .collect();
    let system =
        "你是一个漫画/同人志推荐引擎。你会收到当前漫画的标题和标签、以及候选漫画清单（id: 标题）。\
        请从候选中挑选并排序推荐。排序规则（按优先级）：\
        1) 同一系列的下一卷必须排第一（例如当前是第10卷，那么第11卷就是第一推荐，没有第二种可能）；\
        2) 同一系列的其它卷紧随其后，按卷号顺序；\
        3) 其余候选按与当前漫画的相关度降序。\
        必须选满要求的数量——即使后面部分候选与当前漫画关联度较低，也要补足数量，绝不能少于要求的数量。\
        只输出一个 JSON 数组，每项 {\"id\": \"...\", \"title\": \"...\"}，不要输出任何其它文字。";
    let user = format!(
        "当前漫画标题：{}\n当前漫画标签：{}\n当前漫画页数：{}页\n\n候选漫画：\n{}\n\n请推荐 {} 本。",
        current_title,
        current_tags,
        current_pagecount,
        candidate_list.join("\n"),
        limit
    );

    let body = json!({
        "model": "deepseek-chat",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "temperature": 0.2,
        "response_format": { "type": "json_object" },
        "max_tokens": 2000,
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
            warn!(error = %e, "LLM rerank request failed — falling back to embedding order");
            return None;
        }
    };
    let parsed: LlmResponse = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "LLM rerank response unparseable — falling back to embedding order");
            return None;
        }
    };
    let content = parsed.choices.first()?.message.content.clone();

    // The API's json_object mode wraps the array in an object — accept either shape.
    let picks: Vec<LlmPick> = match serde_json::from_str::<Vec<LlmPick>>(&content) {
        Ok(arr) => arr,
        Err(_) => serde_json::from_str::<serde_json::Value>(&content)
            .ok()
            .and_then(|v| v.as_object().and_then(|o| o.values().next()).cloned())
            .and_then(|arr| serde_json::from_value::<Vec<LlmPick>>(arr).ok())
            .unwrap_or_default(),
    };
    if picks.is_empty() {
        warn!("LLM rerank returned no picks — falling back to embedding order");
        return None;
    }

    // Map picks back to the candidate metadata (the LLM's id must match a candidate's).
    let by_id: std::collections::HashMap<&str, &ArchiveMeta> =
        candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut ranked: Vec<Recommendation> = Vec::with_capacity(picks.len());
    for pick in picks {
        if let Some(meta) = by_id.get(pick.id.as_str()) {
            ranked.push(Recommendation {
                id: meta.id.clone(),
                title: meta.title.clone(),
                // Score unknown from the LLM — order only; report it decreasing from 1.0 so the
                // frontend's display remains sensible.
                score: 1.0 - (ranked.len() as f32 * 0.05).min(0.95),
            });
        } else {
            warn!(id = %pick.id, "LLM returned a candidate id not in the prefiltered shortlist — skipped");
        }
    }
    if ranked.is_empty() {
        return None;
    }
    ranked.truncate(limit);
    // The LLM may return fewer than requested (or skip ids it hallucinated that aren't in the
    // shortlist) — top the list back up with the embedding pre-filter's own order so the
    // frontend always gets the full `limit`.
    if ranked.len() < limit {
        let picked: std::collections::HashSet<String> =
            ranked.iter().map(|r| r.id.clone()).collect();
        for c in candidates {
            if ranked.len() >= limit {
                break;
            }
            if !picked.contains(&c.id) {
                ranked.push(Recommendation {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    score: 1.0 - (ranked.len() as f32 * 0.05).min(0.95),
                });
            }
        }
    }
    Some(ranked)
}
