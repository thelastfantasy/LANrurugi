//! Semantic review of a detected group, plus the read-only per-page comparison the UI shows.
//!
//! The reviewer only ever sees text and numbers (titles, tags, pages, sizes, sharpness) — the local
//! perceptual hash already established that the covers look alike, and images would cost far more
//! tokens without answering the question a hash cannot: are these the same work, and which copy is
//! worth keeping. Any invalid or unavailable answer falls back to a local rule that never guesses
//! beyond page completeness.

use std::collections::HashMap;

use axum::http::StatusCode;
use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{ArchiveMeta, GroupRecord, ReviewVerdict};
use crate::AppState;

pub const REVIEW_KEY: &str = "LANRURUGI_DUP_REVIEW";
const PROMPT_VERSION: u32 = 1;
const MAX_MEMBERS: usize = 8;
const MODEL: &str = "deepseek-v4-pro";

pub async fn load_all(state: &AppState) -> HashMap<String, ReviewVerdict> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return HashMap::new();
    };
    let raw: HashMap<String, String> = conn.hgetall(REVIEW_KEY).await.unwrap_or_default();
    raw.into_iter()
        .filter_map(|(key, value)| serde_json::from_str(&value).ok().map(|v| (key, v)))
        .collect()
}

/// Reviews one group. Never fails: no provider, an invalid answer, or a model error all produce a
/// `fallback: true` verdict from the local rule.
pub async fn ensure(state: &AppState, group: &GroupRecord, metas: &[ArchiveMeta]) -> ReviewVerdict {
    let mut cache = load_all(state).await;
    ensure_with(state, group, metas, &mut cache).await
}

/// Same as [`ensure`], reusing a review map the caller already loaded: a scan reviews every group
/// and one `HGETALL` per group made the pass quadratic in the number of groups.
pub async fn ensure_with(
    state: &AppState,
    group: &GroupRecord,
    metas: &[ArchiveMeta],
    cache: &mut HashMap<String, ReviewVerdict>,
) -> ReviewVerdict {
    let key = super::group_key(&group.members);
    let members: Vec<&ArchiveMeta> = group
        .members
        .iter()
        .filter_map(|id| metas.iter().find(|m| &m.id == id))
        .take(MAX_MEMBERS)
        .collect();
    if members.len() < 2 {
        return local_verdict(&members, "members_changed");
    }

    // The content key carries the prompt version, the model and a member signature, so upgrading
    // the prompt or the model invalidates old verdicts even though the group key is unchanged.
    let cache_key = review_cache_key(&key, &members);
    let has_provider = lanrurugi_llm::resolve_api_key(&state.redis.config)
        .await
        .is_some();
    // A fallback verdict is a local guess, not a model answer: show it while there is no provider,
    // but never let it shadow a real review once one is configured.
    let usable = |cached: &ReviewVerdict| !(cached.fallback && has_provider);

    if let Some(cached) = cache.get(&cache_key) {
        if usable(cached) {
            return cached.clone();
        }
    }
    // Verdicts written before the content key existed live under the bare group key.
    if let Some(cached) = cache.get(&key).cloned() {
        if usable(&cached) {
            cache.insert(cache_key.clone(), cached.clone());
            return cached;
        }
    }

    let verdict = if has_provider {
        ask_model(state, group, &members).await
    } else {
        local_verdict(&members, "not_configured")
    };

    cache.insert(cache_key.clone(), verdict.clone());
    // Also under the group key: the read endpoints load by group key, and a local fallback still
    // has to be visible in the UI before a model is configured.
    cache.insert(key.clone(), verdict.clone());
    if let Ok(mut conn) = state.redis.config.get().await {
        let value = serde_json::to_string(&verdict).unwrap_or_default();
        let _: Result<(), _> = conn.hset(REVIEW_KEY, &cache_key, &value).await;
        let _: Result<(), _> = conn.hset(REVIEW_KEY, &key, &value).await;
    }
    verdict
}

/// A group where one member's patch was built from another: the patched copy already holds what
/// made the other worth keeping, so no model is asked. Stored under the group key only — the
/// content-keyed cache stays for when the patch is removed.
pub async fn store_patched_verdict(
    state: &AppState,
    group: &GroupRecord,
    cache: &mut HashMap<String, ReviewVerdict>,
) -> ReviewVerdict {
    let key = super::group_key(&group.members);
    let verdict = patched_verdict(group);
    cache.insert(key.clone(), verdict.clone());
    if let Ok(mut conn) = state.redis.config.get().await {
        let value = serde_json::to_string(&verdict).unwrap_or_default();
        let _: Result<(), _> = conn.hset(REVIEW_KEY, &key, &value).await;
    }
    verdict
}

fn patched_verdict(group: &GroupRecord) -> ReviewVerdict {
    let sources: Vec<&String> = group.patch_links.iter().map(|l| &l.source).collect();
    let (discard, keep): (Vec<String>, Vec<String>) = group
        .members
        .iter()
        .cloned()
        .partition(|id| sources.contains(&id));
    ReviewVerdict {
        verdict: "patched".to_string(),
        keep,
        discard,
        // The UI words this from `patch_links`; the reason stays empty rather than English text.
        reason: String::new(),
        confidence: 1.0,
        model: String::new(),
        prompt_version: PROMPT_VERSION,
        fallback: false,
        created_at: super::now_secs(),
        notes: HashMap::new(),
    }
}

/// The one model call, returning a local fallback when the answer never arrives or fails validation.
async fn ask_model(
    state: &AppState,
    group: &GroupRecord,
    members: &[&ArchiveMeta],
) -> ReviewVerdict {
    let user = build_input(members, group);
    let spec = lanrurugi_llm::CallSpec::new(lanrurugi_llm::FeatureId::DUPLICATE_REVIEW);
    let answer = crate::repair::json_chat_checked::<LlmAnswer>(
        &state.redis.config,
        &spec,
        &crate::llm_prompts::duplicate_review_system(),
        &user.to_string(),
        0.0,
        700,
        |a| validate_answer(a, members.len()),
    )
    .await;

    let Ok(checked) = answer else {
        return local_verdict(members, "model_error");
    };
    verdict_from_answer(checked.value, members)
}

fn build_input(members: &[&ArchiveMeta], group: &GroupRecord) -> serde_json::Value {
    json!({
        "members": members.iter().enumerate().map(|(index, m)| json!({
            "index": index,
            "title": m.title,
            "artist": m.artists().join(", "),
            "circle": m.circles().join(", "),
            "language": m.language(),
            "pages": m.pages,
            "size": m.size,
            "avg_sharpness": serde_json::Value::Null,
        })).collect::<Vec<_>>(),
        "pairs": group.pairs.iter().filter_map(|p| {
            let a = members.iter().position(|m| m.id == p.a)?;
            let b = members.iter().position(|m| m.id == p.b)?;
            Some(json!({"a": a, "b": b, "similarity": p.similarity}))
        }).collect::<Vec<_>>(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmMember {
    index: usize,
    keep: bool,
    #[serde(default)]
    why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmAnswer {
    verdict: String,
    members: Vec<LlmMember>,
    reason: String,
    confidence: f64,
}

fn validate_answer(answer: &LlmAnswer, count: usize) -> Result<(), String> {
    if !matches!(
        answer.verdict.as_str(),
        "same_work" | "different_versions" | "adjacent_volumes" | "unrelated"
    ) {
        return Err("unknown verdict".into());
    }
    if answer.members.len() != count {
        return Err("member count mismatch".into());
    }
    let mut seen = vec![false; count];
    for member in &answer.members {
        if member.index >= count || seen[member.index] {
            return Err("member index out of range or repeated".into());
        }
        seen[member.index] = true;
        if member.why.trim().is_empty() || member.why.chars().count() > 200 {
            return Err("why is empty or too long".into());
        }
    }
    if answer.reason.trim().is_empty() || answer.reason.chars().count() > 300 {
        return Err("reason is empty or too long".into());
    }
    if !(0.0..=1.0).contains(&answer.confidence) {
        return Err("confidence out of range".into());
    }
    Ok(())
}

fn verdict_from_answer(answer: LlmAnswer, members: &[&ArchiveMeta]) -> ReviewVerdict {
    let mut keep = Vec::new();
    let mut discard = Vec::new();
    let mut notes = HashMap::new();
    let mut ordered = answer.members.clone();
    ordered.sort_by_key(|m| m.index);
    for member in ordered {
        if member.index >= members.len() {
            continue;
        }
        let id = members[member.index].id.clone();
        let why = member.why.trim();
        if !why.is_empty() {
            notes.insert(id.clone(), why.chars().take(120).collect());
        }
        if member.keep {
            keep.push(id);
        } else {
            discard.push(id);
        }
    }
    ReviewVerdict {
        verdict: answer.verdict,
        keep,
        discard,
        reason: answer.reason.chars().take(300).collect(),
        confidence: answer.confidence,
        model: MODEL.to_string(),
        prompt_version: PROMPT_VERSION,
        fallback: false,
        created_at: super::now_secs(),
        notes,
    }
}

/// Local rule: keep the copies with the most pages (within two pages of each other); everything
/// else is a lossy copy. Never claims more than that.
fn local_verdict(members: &[&ArchiveMeta], why: &str) -> ReviewVerdict {
    let best = members.iter().map(|m| m.pages).max().unwrap_or(0);
    let mut keep = Vec::new();
    let mut discard = Vec::new();
    for member in members {
        if member.pages + 2 >= best {
            keep.push(member.id.clone());
        } else {
            discard.push(member.id.clone());
        }
    }
    ReviewVerdict {
        verdict: "same_work".to_string(),
        keep,
        discard,
        reason: format!("local check ({}): kept the most complete copies", why),
        confidence: 0.4,
        model: String::new(),
        prompt_version: PROMPT_VERSION,
        fallback: true,
        created_at: super::now_secs(),
        notes: HashMap::new(),
    }
}

fn review_cache_key(key: &str, members: &[&ArchiveMeta]) -> String {
    let signature: String = members
        .iter()
        .map(|m| format!("{}:{}:{}", m.id, m.pages, m.size))
        .collect::<Vec<_>>()
        .join("|");
    let mut hasher = blake3::Hasher::new();
    hasher.update(key.as_bytes());
    hasher.update(b"|");
    hasher.update(crate::llm_prompts::DUPLICATE_REVIEW_PROMPT_VERSION.as_bytes());
    hasher.update(b"|");
    hasher.update(MODEL.as_bytes());
    hasher.update(b"|");
    hasher.update(signature.as_bytes());
    format!("v1:{}", hasher.finalize().to_hex())
}

/// Read-only per-page comparison of two catalogued archives.
pub async fn compare(
    state: &AppState,
    a_id: &str,
    b_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let a = load_archive(state, a_id).await?;
    let b = load_archive(state, b_id).await?;
    let a_path = std::path::PathBuf::from(&a.file);
    let b_path = std::path::PathBuf::from(&b.file);
    let (a_title, b_title) = (a.title, b.title);
    let (a_size, b_size) = (a.arcsize, b.arcsize);
    let (a_owned, b_owned) = (a_id.to_string(), b_id.to_string());

    let result = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_imgcompare::compare_archives(&a_path, &b_path, a_owned, a_size, b_owned, b_size)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("comparison failed: {e}"),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("comparison failed: {e}"),
        )
    })?;

    Ok(json!({
        "a": {"id": a_id, "title": a_title, "size": a_size, "pages": result.a_total_pages},
        "b": {"id": b_id, "title": b_title, "size": b_size, "pages": result.b_total_pages},
        "aligned_pairs": result.aligned_pairs,
        "likely_different_language": result.likely_different_language,
        "recommendation": result.recommendation,
        "samples": result.samples,
        "a_unmatched_pages": result.a_unmatched_pages,
        "b_unmatched_pages": result.b_unmatched_pages,
        "a_entries": result.a_entries,
        "b_entries": result.b_entries,
        "a_patch_pages": result.a_patch_pages,
        "b_patch_pages": result.b_patch_pages,
        "aligned": result.aligned,
    }))
}

pub(super) async fn load_archive(
    state: &AppState,
    id: &str,
) -> Result<lanrurugi_core::entities::Archive, (StatusCode, String)> {
    let archive = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(id.to_string()))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "archive no longer exists".to_string(),
            )
        })?;
    if archive.file.trim().is_empty() {
        return Err((
            StatusCode::CONFLICT,
            "archive is still being ingested".to_string(),
        ));
    }
    if !std::path::Path::new(&archive.file).is_file() {
        return Err((StatusCode::NOT_FOUND, "archive file is gone".to_string()));
    }
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, pages: u32) -> ArchiveMeta {
        ArchiveMeta {
            id: id.to_string(),
            title: "Work".to_string(),
            tags: vec![],
            pages,
            size: 1000,
        }
    }

    fn answer(indices: &[(usize, bool)]) -> LlmAnswer {
        LlmAnswer {
            verdict: "same_work".into(),
            members: indices
                .iter()
                .map(|(index, keep)| LlmMember {
                    index: *index,
                    keep: *keep,
                    why: "reason".into(),
                })
                .collect(),
            reason: "same work".into(),
            confidence: 0.8,
        }
    }

    #[test]
    fn validation_requires_every_index_exactly_once() {
        assert!(validate_answer(&answer(&[(0, true), (1, false)]), 2).is_ok());
        assert!(validate_answer(&answer(&[(0, true)]), 2).is_err());
        assert!(validate_answer(&answer(&[(0, true), (0, false)]), 2).is_err());
        assert!(validate_answer(&answer(&[(0, true), (2, false)]), 2).is_err());
    }

    #[test]
    fn invalid_verdict_or_confidence_is_rejected() {
        let mut a = answer(&[(0, true), (1, false)]);
        a.verdict = "duplicate".into();
        assert!(validate_answer(&a, 2).is_err());
        let mut a = answer(&[(0, true), (1, false)]);
        a.confidence = 1.5;
        assert!(validate_answer(&a, 2).is_err());
    }

    #[test]
    fn a_patched_copy_is_kept_and_its_source_discarded() {
        let group = GroupRecord {
            members: vec!["a".into(), "b".into(), "c".into()],
            patch_links: vec![super::super::model::PatchLink {
                target: "b".into(),
                source: "a".into(),
            }],
            ..Default::default()
        };
        let verdict = patched_verdict(&group);
        assert_eq!(verdict.keep, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(verdict.discard, vec!["a".to_string()]);
        assert!(!verdict.fallback);
    }

    #[test]
    fn fallback_keeps_the_most_complete_copies() {
        let full = meta("a", 214);
        let short = meta("b", 180);
        let verdict = local_verdict(&[&full, &short], "test");
        assert_eq!(verdict.keep, vec!["a".to_string()]);
        assert_eq!(verdict.discard, vec!["b".to_string()]);
        assert!(verdict.fallback);
    }

    #[test]
    fn review_cache_key_tracks_content_and_prompt() {
        let a = meta("a", 10);
        let k1 = review_cache_key("g", &[&a]);
        let k2 = review_cache_key("g", &[&a]);
        assert_eq!(k1, k2);
        let b = meta("a", 11);
        assert_ne!(k1, review_cache_key("g", &[&b]));
    }
}
