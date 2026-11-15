//! LLM-based reranking of the embedding-prefiltered recommendation candidates.
//!
//! Two-tier architecture: the local ONNX embedding model pre-filters the library down to a
//! ~150-candidate shortlist, reading progress filters/reorders it, then the DeepSeek LLM reranks
//! the remainder with an explicit "next volume first" instruction — the one thing embedding
//! models cannot do.
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

use lanrurugi_recommend::reading::{progress_digest, RankedCandidate, ReadBucket, ReadingState};
use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};
use lanrurugi_recommend::title::{split_title, TitleParts};

use crate::AppState;

/// How many candidates the embedding pre-filter keeps per archive (`recommend_precompute`'s
/// cached Top-N length floor). Deliberately wider than [`MAX_PROMPT_CANDIDATES`]: the cache is
/// where Tankoubon sibling-exclusion and finished-work filtering headroom has to exist, since
/// both happen after the Top-N list is built. 150 is the spec's 1.5× floor (FR-010).
pub const PREFILTER_COUNT: usize = 150;

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
    bucket: ReadBucket,
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

/// Reranks `ordered` via DeepSeek's chat API with an explicit "next volume first" instruction.
/// `ordered` must already be reading-aware-ranked (finished works removed) by
/// [`lanrurugi_recommend::reading::rank`]. Returns `None` on any failure — the caller falls back to
/// that same reading-aware order, never to the raw embedding order.
///
/// The answer is cached (`lanrurugi_storage::keys::recommend_rerank_key`) because this call sits
/// on the request path of `GET /api/reader/recommendations/{id}`: it was observed taking 6–64 s,
/// and the panel is re-opened (or prefetched) far more often than the library changes. The cache
/// entry records the *candidate set* it was produced from, so a library edit — including the one
/// case the whole rerank exists for, a new volume of the same series appearing — invalidates it
/// rather than serving a stale ordering. The deadline bounds the pathological case instead of the
/// seam's three-minute default; timing out is just another fallback to the embedding order.
pub async fn llm_rerank(
    state: &AppState,
    archive_id: &str,
    anchor: &ArchiveMeta,
    anchor_state: ReadingState,
    ordered: &[RankedCandidate],
    limit: usize,
) -> Option<Vec<Recommendation>> {
    let current = split_title(&anchor.title);
    let current_display = if current.content.is_empty() {
        anchor.title.as_str()
    } else {
        &current.content
    };
    let anchor_bucket = anchor_state.bucket();

    // `ordered` is already reading-aware-ordered (finished works removed) by `reading::rank`,
    // so the first entry of each work is its highest-scoring edition — exactly the one worth
    // keeping — and the prompt cut below is a plain top-N by that order.
    let shortlist = dedupe_by_work(ordered);
    let shortlist = &shortlist[..shortlist.len().min(MAX_PROMPT_CANDIDATES)];

    let by_id: std::collections::HashMap<&str, &ShortlistEntry> =
        shortlist.iter().map(|e| (e.meta.id.as_str(), e)).collect();
    let candidate_fingerprint = candidate_fingerprint(shortlist.iter().map(|e| e.meta.id.as_str()));
    let digest = progress_digest(
        anchor_bucket,
        &shortlist
            .iter()
            .map(|e| (e.meta.id.as_str(), e.bucket))
            .collect::<Vec<_>>(),
    );

    if let Some(picks) =
        cached_picks(state, archive_id, limit, &candidate_fingerprint, &digest).await
    {
        let ranked = rank_picks(&picks, &by_id, shortlist, limit);
        if !ranked.is_empty() {
            return Some(ranked);
        }
        // Every remembered pick has since been deleted: fall through and ask again.
    }

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
                "{}. {}: {} | 标签: {} | 页数: {}页 | 阅读进度: {}{}",
                i + 1,
                entry.meta.id,
                entry.display_title(),
                entry.meta.tags,
                entry.meta.pagecount,
                entry.bucket.prompt_label(),
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
        "当前漫画标题：{}\n{}当前漫画标签：{}\n当前漫画页数：{}页\n当前阅读进度：{}\n\n候选漫画：\n{}\n\n请推荐 {} 本。",
        current_display,
        current_release,
        anchor.tags,
        anchor.pagecount,
        anchor_bucket.prompt_label(),
        candidate_list.join("\n"),
        limit
    );

    let known: std::collections::HashSet<&str> =
        shortlist.iter().map(|e| e.meta.id.as_str()).collect();
    let spec = lanrurugi_llm::CallSpec::new(lanrurugi_llm::FeatureId::RECOMMEND_RERANK)
        .with_deadline(RERANK_DEADLINE);
    let picks: Vec<LlmPick> = crate::repair::json_chat_checked(
        &state.redis.config,
        &spec,
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

    let ranked = rank_picks(
        &picks.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
        &by_id,
        shortlist,
        limit,
    );
    if ranked.is_empty() {
        return None;
    }
    remember_picks(
        state,
        archive_id,
        limit,
        &candidate_fingerprint,
        &digest,
        &ranked.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
    )
    .await;
    Some(ranked)
}

/// Ceiling for one rerank. The observed distribution is 6–64 s; 30 s keeps the answers that actually
/// arrive while cutting the tail that made a panel open cost a minute. Timing out falls back to the
/// embedding order, which is the same degradation as any other model failure.
const RERANK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// A stable digest of the exact candidate set an answer was produced from (order included — the
/// prompt numbers the lines, so order is part of the question). Changes whenever the shortlist
/// does, which is precisely when a remembered ordering stops being trustworthy.
fn candidate_fingerprint<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    let mut hasher = blake3::Hasher::new();
    for id in ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// Turns an ordered id list into [`Recommendation`]s, keeping only ids that are still in the
/// shortlist and de-duplicating. Shared by the fresh-answer and cache-hit paths so both produce
/// exactly the same ranking and scores.
fn rank_picks(
    ids: &[String],
    by_id: &std::collections::HashMap<&str, &ShortlistEntry>,
    shortlist: &[ShortlistEntry<'_>],
    limit: usize,
) -> Vec<Recommendation> {
    let mut ranked: Vec<Recommendation> = Vec::with_capacity(ids.len());
    let mut picked = std::collections::HashSet::new();
    for id in ids {
        if !picked.insert(id.as_str()) {
            continue;
        }
        if let Some(entry) = by_id.get(id.as_str()) {
            ranked.push(Recommendation {
                id: entry.meta.id.clone(),
                title: entry.meta.title.clone(),
                score: 1.0 - (ranked.len() as f32 * 0.05).min(0.95),
            });
        }
    }
    ranked.truncate(limit);
    if ranked.len() < limit {
        for entry in shortlist {
            if ranked.len() >= limit {
                break;
            }
            if !picked.contains(entry.meta.id.as_str()) {
                ranked.push(Recommendation {
                    id: entry.meta.id.clone(),
                    title: entry.meta.title.clone(),
                    score: 1.0 - (ranked.len() as f32 * 0.05).min(0.95),
                });
            }
        }
    }
    ranked
}

/// The remembered ordering for `(archive, limit)`, if it was produced from this exact candidate
/// set. Any Redis problem is a miss — a cache must never be able to fail the request.
async fn cached_picks(
    state: &AppState,
    archive_id: &str,
    limit: usize,
    candidate_fingerprint: &str,
    progress_digest: &str,
) -> Option<Vec<String>> {
    use deadpool_redis::redis::AsyncCommands;
    let mut conn = state.redis.config.get().await.ok()?;
    let key = lanrurugi_storage::keys::recommend_rerank_key(
        archive_id,
        limit,
        crate::llm_prompts::RECOMMEND_RERANK_PROMPT_VERSION,
        progress_digest,
    );
    let raw: String = conn.get(&key).await.ok().flatten()?;
    let cached: RerankCacheEntry = serde_json::from_str(&raw).ok()?;
    (cached.candidates == candidate_fingerprint && cached.progress_digest == progress_digest)
        .then_some(cached.picks)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RerankCacheEntry {
    /// The shortlist digest this ordering was produced from.
    candidates: String,
    /// The anchor/candidate reading-progress digest. Also part of the key; kept here so an entry
    /// written under an older key layout can never be misread after a manual rename.
    #[serde(default)]
    progress_digest: String,
    /// Ordered archive ids, exactly as the model returned them (then filtered).
    picks: Vec<String>,
}

async fn remember_picks(
    state: &AppState,
    archive_id: &str,
    limit: usize,
    candidate_fingerprint: &str,
    progress_digest: &str,
    picks: &[String],
) {
    use deadpool_redis::redis::AsyncCommands;
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let key = lanrurugi_storage::keys::recommend_rerank_key(
        archive_id,
        limit,
        crate::llm_prompts::RECOMMEND_RERANK_PROMPT_VERSION,
        progress_digest,
    );
    let entry = RerankCacheEntry {
        candidates: candidate_fingerprint.to_string(),
        progress_digest: progress_digest.to_string(),
        picks: picks.to_vec(),
    };
    let Ok(raw) = serde_json::to_string(&entry) else {
        return;
    };
    let _: Result<(), _> = conn
        .set_ex(
            key,
            raw,
            lanrurugi_storage::keys::RECOMMEND_RERANK_TTL_SECS as u64,
        )
        .await;
}

/// One entry per *work*: collapses `[中国翻訳]`/`[DL版]` re-releases of something already in the
/// list, keeping the first (highest-cosine) edition. Titles whose content is empty (nothing but
/// release metadata) fall back to the raw title as their dedupe key.
fn dedupe_by_work<'a>(candidates: &'a [RankedCandidate]) -> Vec<ShortlistEntry<'a>> {
    let mut seen = std::collections::HashSet::new();
    let mut shortlist = Vec::with_capacity(candidates.len());
    for c in candidates {
        let parts = split_title(&c.meta.title);
        let key = if parts.content.is_empty() {
            c.meta.title.trim().to_lowercase()
        } else {
            parts.content.to_lowercase()
        };
        if seen.insert(key) {
            shortlist.push(ShortlistEntry {
                meta: &c.meta,
                parts,
                bucket: c.state.bucket(),
            });
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

    fn cand(id: &str, title: &str) -> RankedCandidate {
        RankedCandidate {
            meta: meta(id, title),
            state: ReadingState::new(0, 0, 0),
            similarity: 0.5,
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
            cand("a", "性欲モンスター三姉妹"),
            cand("b", "性欲モンスター三姉妹 [中国翻訳]"),
            cand("c", "サキュバシスターズ ～聖職姉妹の背徳～"),
            cand("d", "サキュバシスターズ ～聖職姉妹の背徳～ [DL版]"),
            cand("e", "サキュバシスターズ ～別の副題～"),
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

    /// The cache stores ids only, so the ranking has to be reproducible from ids alone — otherwise
    /// a cache hit would reorder (or rescore) the same answer differently from the call that
    /// produced it.
    #[test]
    fn a_remembered_ordering_ranks_the_same_way_as_the_fresh_answer() {
        let candidates = vec![cand("a", "A巻"), cand("b", "B巻"), cand("c", "C巻")];
        let shortlist = dedupe_by_work(&candidates);
        let by_id: std::collections::HashMap<&str, &ShortlistEntry> =
            shortlist.iter().map(|e| (e.meta.id.as_str(), e)).collect();

        let ranked = rank_picks(&["c".to_string(), "a".to_string()], &by_id, &shortlist, 2);
        let ids: Vec<&str> = ranked.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "a"]);
        assert_eq!(ranked[0].score, 1.0);
        assert_eq!(ranked[1].score, 0.95);
    }

    /// A cached id that has since been deleted, or one repeated, must not shrink the answer below
    /// the requested limit — the shortlist's remaining order fills the gap, exactly as on the
    /// fresh-answer path.
    #[test]
    fn a_remembered_ordering_is_repaired_against_the_live_shortlist() {
        let candidates = vec![cand("a", "A巻"), cand("b", "B巻"), cand("c", "C巻")];
        let shortlist = dedupe_by_work(&candidates);
        let by_id: std::collections::HashMap<&str, &ShortlistEntry> =
            shortlist.iter().map(|e| (e.meta.id.as_str(), e)).collect();

        let ranked = rank_picks(
            &["b".to_string(), "gone".to_string(), "b".to_string()],
            &by_id,
            &shortlist,
            3,
        );
        let ids: Vec<&str> = ranked.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    /// The fingerprint is what makes a cached ordering trustworthy: it must change with the
    /// candidate set (including its order, which the prompt numbers) and stay stable otherwise.
    #[test]
    fn the_candidate_fingerprint_follows_the_shortlist() {
        let base = candidate_fingerprint(["a", "b", "c"].into_iter());
        assert_eq!(base, candidate_fingerprint(["a", "b", "c"].into_iter()));
        assert_ne!(base, candidate_fingerprint(["b", "a", "c"].into_iter()));
        assert_ne!(base, candidate_fingerprint(["a", "b"].into_iter()));
        assert_ne!(base, candidate_fingerprint(["a", "b", "d"].into_iter()));
    }
}
