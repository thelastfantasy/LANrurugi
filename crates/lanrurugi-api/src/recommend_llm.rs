//! LLM-based reranking of the embedding-prefiltered recommendation candidates.
//!
//! Two-tier architecture: the local ONNX embedding model pre-filters the library down to
//! a ~100-candidate shortlist, then the DeepSeek LLM reranks them with an explicit
//! "next volume first" instruction — the one thing embedding models cannot do.
//!
//! Before the shortlist reaches the model it is collapsed by *work*: release/edition blocks
//! (`[中国翻訳]`, `【完全版】`, `[DL版]`) are stripped and the remaining title is used as the
//! dedupe key, so a translated or digital re-release of something already in the list cannot take
//! a second slot. Subtitles are part of that key on purpose — they are what distinguishes two
//! volumes of one series, and dropping them would merge genuinely different works.
//!
//! The generic LLM call infrastructure lives in `crate::llm`; this module only provides
//! the recommendation-specific prompts and result parsing.

use serde::{Deserialize, Serialize};
use tracing::warn;

use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};
use lanrurugi_recommend::title::{split_title, TitleParts};

use crate::AppState;

/// How many candidates the embedding pre-filter keeps per archive (`recommend_precompute`'s
/// cached Top-N length floor). Deliberately wider than [`MAX_PROMPT_CANDIDATES`]: the cache is
/// where Tankoubon sibling-exclusion headroom has to exist, since it happens before the prompt.
pub const PREFILTER_COUNT: usize = 100;

/// How many candidates actually reach the prompt. The list handed in is cosine-ordered, so this
/// is a plain "top N by similarity" cut — the same-series entry sits at ~0.97 against ~0.88 for
/// the best unrelated work, nowhere near this boundary. Kept separate from [`PREFILTER_COUNT`]
/// because every line carries the candidate's full tag string: 100 of them is ~80KB of prompt,
/// which costs latency and pushes the model toward malformed/truncated answers (which then fall
/// back to the embedding order — the exact shortlist quality being fixed here).
pub const MAX_PROMPT_CANDIDATES: usize = 40;

#[derive(Debug, Serialize, Deserialize)]
struct LlmPick {
    id: String,
    #[serde(default)]
    title: String,
}

/// One candidate as the model will see it: the archive itself, plus its title split into the work
/// (what the model compares against) and the release notes (kept, but only as an annotation).
struct ShortlistEntry<'a> {
    meta: &'a ArchiveMeta,
    parts: TitleParts,
}

impl ShortlistEntry<'_> {
    /// The title to show. Falls back to the raw one for a title that is *entirely* release
    /// metadata (nothing but `[DL版]`), which would otherwise display as an empty string.
    fn display_title(&self) -> &str {
        if self.parts.content.is_empty() {
            &self.meta.title
        } else {
            &self.parts.content
        }
    }
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
    let current = split_title(current_title);
    let current_display = if current.content.is_empty() {
        current_title
    } else {
        &current.content
    };

    // `candidates` is already cosine-ordered by both callers, so the first entry of each work is
    // its highest-scoring edition — exactly the one worth keeping — and the prompt cut below is a
    // plain top-N by similarity.
    let shortlist = dedupe_by_work(candidates);
    let shortlist = &shortlist[..shortlist.len().min(MAX_PROMPT_CANDIDATES)];

    let candidate_list: Vec<String> = shortlist
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let release = if entry.parts.release.is_empty() {
                String::new()
            } else {
                format!(" | 版本: {}", entry.parts.release.join(" / "))
            };
            format!(
                "{}. {}: {} | 标签: {} | 页数: {}页{}",
                i + 1,
                entry.meta.id,
                entry.display_title(),
                entry.meta.tags,
                entry.meta.pagecount,
                release
            )
        })
        .collect();
    let current_release = if current.release.is_empty() {
        String::new()
    } else {
        format!("当前漫画版本：{}\n", current.release.join(" / "))
    };
    let system = crate::llm_prompts::recommend_llm_system();
    let user = format!(
        "当前漫画标题：{}\n{}当前漫画标签：{}\n当前漫画页数：{}页\n\n候选漫画：\n{}\n\n请推荐 {} 本。",
        current_display,
        current_release,
        current_tags,
        current_pagecount,
        candidate_list.join("\n"),
        limit
    );

    let known: std::collections::HashSet<&str> =
        shortlist.iter().map(|e| e.meta.id.as_str()).collect();
    let picks: Vec<LlmPick> = crate::repair::json_chat_checked(
        &state.redis.config,
        &lanrurugi_llm::CallSpec::new(lanrurugi_llm::FeatureId::RECOMMEND_RERANK),
        &system,
        &user,
        0.2,
        2000,
        |picks: &Vec<LlmPick>| check_picks(picks, &known),
    )
    .await
    .map_err(|e| warn!(error = %e, "LLM rerank failed — falling back to embedding order"))
    .ok()?
    .value;

    let by_id: std::collections::HashMap<&str, &ShortlistEntry> =
        shortlist.iter().map(|e| (e.meta.id.as_str(), e)).collect();
    let mut ranked: Vec<Recommendation> = Vec::with_capacity(picks.len());
    let mut picked = std::collections::HashSet::new();
    for pick in picks {
        if !picked.insert(pick.id.clone()) {
            continue;
        }
        if let Some(entry) = by_id.get(pick.id.as_str()) {
            ranked.push(Recommendation {
                id: entry.meta.id.clone(),
                title: entry.meta.title.clone(),
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
        for entry in shortlist {
            if ranked.len() >= limit {
                break;
            }
            if !picked.contains(&entry.meta.id) {
                ranked.push(Recommendation {
                    id: entry.meta.id.clone(),
                    title: entry.meta.title.clone(),
                    score: 1.0 - (ranked.len() as f32 * 0.05).min(0.95),
                });
            }
        }
    }
    Some(ranked)
}

/// One entry per *work*: collapses `[中国翻訳]`/`[DL版]` re-releases of something already in the
/// list, keeping the first (highest-cosine) edition. Titles whose content is empty (nothing but
/// release metadata) fall back to the raw title as their dedupe key.
fn dedupe_by_work<'a>(candidates: &'a [ArchiveMeta]) -> Vec<ShortlistEntry<'a>> {
    let mut seen = std::collections::HashSet::new();
    let mut shortlist = Vec::with_capacity(candidates.len());
    for c in candidates {
        let parts = split_title(&c.title);
        let key = if parts.content.is_empty() {
            c.title.trim().to_lowercase()
        } else {
            parts.content.to_lowercase()
        };
        if seen.insert(key) {
            shortlist.push(ShortlistEntry { meta: c, parts });
        }
    }
    shortlist
}

/// The picks must be non-empty, name only shortlisted candidates, and name each at most once.
fn check_picks(picks: &[LlmPick], known: &std::collections::HashSet<&str>) -> Result<(), String> {
    if picks.is_empty() {
        return Err("没有返回任何推荐".to_string());
    }
    let unknown: Vec<&str> = picks
        .iter()
        .map(|p| p.id.as_str())
        .filter(|id| !known.contains(id))
        .take(5)
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "这些 id 不在候选清单里：{}；id 必须原样取自候选清单",
            unknown.join("、")
        ));
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(dup) = picks.iter().find(|p| !seen.insert(p.id.as_str())) {
        return Err(format!("{} 出现了不止一次", dup.id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(id: &str) -> LlmPick {
        LlmPick {
            id: id.into(),
            title: String::new(),
        }
    }

    fn meta(id: &str, title: &str) -> ArchiveMeta {
        ArchiveMeta {
            id: id.into(),
            title: title.into(),
            tags: String::new(),
            pagecount: 0,
        }
    }

    #[test]
    fn picks_must_be_known_unique_and_non_empty() {
        let known: std::collections::HashSet<&str> = ["a", "b"].into_iter().collect();
        assert!(check_picks(&[pick("a"), pick("b")], &known).is_ok());
        assert!(check_picks(&[], &known).is_err());
        assert!(check_picks(&[pick("a"), pick("zzz")], &known)
            .unwrap_err()
            .contains("zzz"));
        assert!(check_picks(&[pick("a"), pick("a")], &known).is_err());
    }

    /// The shortlist the model sees: one entry per *work*, subtitles kept, release notes moved to
    /// an annotation instead of being part of the comparison text.
    #[test]
    fn shortlist_collapses_editions_and_keeps_subtitles() {
        let candidates = vec![
            meta("a", "性欲モンスター三姉妹"),
            meta("b", "性欲モンスター三姉妹 [中国翻訳]"),
            meta("c", "サキュバシスターズ ～聖職姉妹の背徳～"),
            meta("d", "サキュバシスターズ ～聖職姉妹の背徳～ [DL版]"),
            meta("e", "サキュバシスターズ ～別の副題～"),
        ];
        let shortlist = dedupe_by_work(&candidates);
        let kept: Vec<&str> = shortlist.iter().map(|e| e.meta.id.as_str()).collect();
        // Same work in two editions collapses; two different subtitles do not.
        assert_eq!(kept, vec!["a", "c", "e"]);
        // The surviving entry is the highest-cosine edition, shown in its folded form (`～` folds
        // to `~`, same as it does for the embedding), with release notes kept as an annotation
        // rather than in the title the model compares.
        assert_eq!(
            shortlist[1].display_title(),
            "サキュバシスターズ ~聖職姉妹の背徳~"
        );
        assert!(shortlist[0].parts.release.is_empty());
        // The kept edition is the first one, so the one carrying `[DL版]` is the one that was
        // dropped — while a candidate that only exists in a translated edition keeps its own
        // release note as an annotation.
        assert!(shortlist[1].parts.release.is_empty());
        assert_eq!(
            dedupe_by_work(&candidates[1..2])[0].parts.release,
            vec!["中国翻訳".to_string()]
        );
    }
}
