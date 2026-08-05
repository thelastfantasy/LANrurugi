//! LLM-based reranking of the embedding-prefiltered recommendation candidates.
//!
//! Two-tier architecture: the local ONNX embedding model pre-filters the library down to
//! a ~100-candidate shortlist, then the DeepSeek LLM reranks them with an explicit
//! "next volume first" instruction — the one thing embedding models cannot do.
//!
//! The generic LLM call infrastructure lives in `crate::llm`; this module only provides
//! the recommendation-specific prompts and result parsing.

use serde::{Deserialize, Serialize};
use tracing::warn;

use lanrurugi_llm;

use crate::AppState;
use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};

/// How many candidates the embedding pre-filter hands to the LLM.
pub const PREFILTER_COUNT: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
struct LlmPick {
    id: String,
    #[serde(default)]
    title: String,
}

/// Reranks `candidates` via DeepSeek's chat API with an explicit "next volume first"
/// instruction. Returns `None` on any failure — the caller falls back to the embedding order.
pub async fn llm_rerank(
    state: &AppState,
    current_title: &str,
    current_tags: &str,
    current_pagecount: u32,
    candidates: &[ArchiveMeta],
    limit: usize,
) -> Option<Vec<Recommendation>> {
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

    let picks: Vec<LlmPick> =
        lanrurugi_llm::json_chat(&state.redis.config, system, &user, 0.2, 2000).await?;
    if picks.is_empty() {
        warn!("LLM rerank returned no picks — falling back to embedding order");
        return None;
    }

    let by_id: std::collections::HashMap<&str, &ArchiveMeta> =
        candidates.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut ranked: Vec<Recommendation> = Vec::with_capacity(picks.len());
    for pick in picks {
        if let Some(meta) = by_id.get(pick.id.as_str()) {
            ranked.push(Recommendation {
                id: meta.id.clone(),
                title: meta.title.clone(),
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
