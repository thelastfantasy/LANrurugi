//! Fire-and-forget LLM backfill of a missing artist/coser tag at ingest time — hooked into
//! `plugins::run_enabled_metadata_plugins_on_archive`, run once per archive right after every
//! enabled metadata plugin has had its turn (same timing as the recommend-cache precompute right
//! above that call site).
//!
//! Motivation (issue #74 follow-up, confirmed live): `tankoubon_grouping.rs`'s AI grouping
//! suggestions endpoint gives a real scoring bonus to archives sharing a `cosplayer:` tag (a much
//! stronger same-series signal for cosplay works than a shared `artist:` tag is for manga — see
//! that module's own docs), but that bonus only fires when the tag actually exists. A real batch
//! of four same-coser archives, none tagged `cosplayer:` at all (only the coser's handle written
//! into the free-text title, e.g. `"<handle> - <subject>"`), scored consistently just under
//! `SIMILARITY_THRESHOLD` and never got suggested as a group — the signal a human recognizes
//! instantly from the title was simply never captured as structured data at all. Rather than keep
//! tuning downstream scoring weights against an incomplete signal, this fills the actual gap at
//! the source: if there's enough signal to know what *kind* of work this is but the corresponding
//! creator-identity tag is missing, ask the LLM to read it off the title (which is usually where a
//! human would have gotten it from too) and write it in as a real tag, the same way any other run
//! of `run_enabled_metadata_plugins_on_archive` would have.
//!
//! Two paths, deliberately kept separate (confirmed with the user) rather than always routing
//! through the LLM:
//!
//! 1. **Archive already has a `category:` tag** — classification is already known with certainty,
//!    so a plain Rust match decides the target namespace with no LLM call at all (cheaper, and
//!    doesn't burn a request on something already deterministic):
//!    - `category:cosplay` → `cosplayer:` (the coser's handle)
//!    - `category:manga` or `category:doujinshi` → `artist:` (individual) or `circle:` (group) —
//!      the LLM itself decides which of the two, since that distinction isn't always obvious from
//!      the title alone.
//!    - `category:anthology` or anything else — no applicable target, skipped.
//!
//! 2. **No `category:` tag at all** — the common case for a locally-uploaded archive with no
//!    metadata plugin run against it. Rather than skip outright or guess blindly, this checks
//!    whether the archive is a member of any LANrurugi `Category` (the separate saved-grouping
//!    entity, `categories.rs` / the Categories page — NOT the same thing as a `category:` tag) and,
//!    if so, passes that Category's own name to the LLM as an extra hint alongside the title,
//!    letting the LLM judge both the work's type AND the tag to write in one combined call — a
//!    Category named e.g. "cosplay" is a real (if informal, user-authored, not guaranteed
//!    consistent) signal worth handing to the LLM rather than pattern-matching in Rust, since the
//!    LLM can weigh it alongside the title itself instead of trusting it blindly. An archive in NO
//!    category at all (neither a `category:` tag nor a Category membership) has no signal to work
//!    from and is skipped without ever calling the LLM.
//!
//! Anthology (both via `category:anthology` and via the LLM's own path-2 judgment) is deliberately
//! NOT backfilled — an anthology is inherently a multi-creator collection; crediting it to one
//! inferred `artist:`/`circle:` would misrepresent it, unlike the other categories where a single
//! creator/coser credit is the norm.
//!
//! Requires an LLM key (`lanrurugi_llm::resolve_api_key`) — unlike `tankoubon_grouping.rs`'s own
//! pure-embedding similarity judgment, identifying a specific real name from free text (and, in
//! path 2, classifying the work at all) is a task the local embedding model has no ability to do.
//! No key configured is a silent skip, not a degraded fallback — same "optional enrichment, never
//! blocks ingestion" posture every other LLM-backed feature in this codebase already has
//! (`ai_rename_suggestions` etc.).
//!
//! Tag values are written verbatim in whatever language/script the title itself uses — no
//! translation/romanization step. A `cosplayer:xansoon` handle is already Latin-script in the
//! title it came from; a Japanese artist credit stays in Japanese. Consistency of the *tag value*
//! across an artist's own multiple archives depends on their titles already being consistent
//! (which is the same assumption every other tag on this archive already relies on) — this
//! backfill doesn't normalize or deduplicate variant spellings.

use serde::Deserialize;

use crate::AppState;

/// Trims delimiter noise (brackets, underscores, dashes, whitespace) the LLM sometimes copies
/// verbatim off a bracketed/underscore-padded title fragment (e.g. `"叉子宝宝_"` from a title
/// literally containing `"[叉子宝宝_  ]"`) — found live: two of five same-coser archives whose
/// titles all used the same handle got a tag differing only by a trailing `_`, which silently
/// defeated `tankoubon_grouping.rs`'s exact-match `cosplayer:` bonus and split one group into two.
/// Same delimiter set as `tankoubon_grouping::title_tokens`, applied to the edges only (not
/// splitting) since a handle can legitimately contain an internal space/hyphen.
fn clean_llm_name(name: &str) -> String {
    name.trim_matches([
        ' ', '\t', '_', '-', '－', '–', '—', ':', '：', '·', '•', '[', ']', '【', '】', '(', ')',
        '（', '）',
    ])
    .to_string()
}

/// Every `category:`-namespaced value on an archive, lowercased for matching (tag values in this
/// codebase are otherwise treated case-sensitively elsewhere, but `category:Cosplay` vs
/// `category:cosplay` typos are exactly the kind of thing worth tolerating here rather than
/// silently never backfilling because of a casing mismatch).
fn category_tag_values(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter_map(|t| t.strip_prefix("category:"))
        .map(|v| v.to_ascii_lowercase())
        .collect()
}

/// Which artist-identity tag namespace(s) apply, or `None` if backfill doesn't apply — no
/// `category:` tag matched a known type, or only `category:anthology` (deliberately excluded —
/// see this module's own top-level docs).
#[derive(Debug, PartialEq, Eq)]
enum BackfillTarget {
    /// Write a `cosplayer:` tag.
    Cosplayer,
    /// Write either `artist:` (individual) or `circle:` (group) — the LLM's own call, prompted to
    /// pick whichever fits.
    ArtistOrCircle,
}

/// Path 1 — the archive already has at least one `category:` tag, so classification is certain
/// and this is a plain deterministic match, no LLM involved.
fn backfill_target_from_category_tag(tags: &str) -> Option<BackfillTarget> {
    let categories = category_tag_values(tags);
    if categories.iter().any(|c| c == "cosplay") {
        return Some(BackfillTarget::Cosplayer);
    }
    if categories.iter().any(|c| c == "manga" || c == "doujinshi") {
        return Some(BackfillTarget::ArtistOrCircle);
    }
    // Anthology or any other/unrecognized category value.
    None
}

/// Whether `tags` already has a value in the namespace(s) `target` would write — if so, there's
/// nothing to backfill (an existing plugin/user-supplied credit is never overwritten).
fn already_has_target_tag(tags: &str, target: &BackfillTarget) -> bool {
    let values: Vec<&str> = tags.split(',').map(str::trim).collect();
    match target {
        BackfillTarget::Cosplayer => values.iter().any(|t| t.starts_with("cosplayer:")),
        BackfillTarget::ArtistOrCircle => values
            .iter()
            .any(|t| t.starts_with("artist:") || t.starts_with("circle:")),
    }
}

#[derive(Deserialize)]
struct CosplayerBackfillResponse {
    /// `None` when the LLM couldn't confidently identify a handle from the title — not every
    /// title actually contains one, and guessing would write a wrong tag that's worse than no
    /// tag at all.
    cosplayer: Option<String>,
}

#[derive(Deserialize)]
struct ArtistBackfillResponse {
    /// Exactly one of `artist`/`circle` set, matching whichever the LLM judged the credit to be
    /// (or both `None` if it couldn't identify one at all).
    artist: Option<String>,
    circle: Option<String>,
}

/// Path 2's response shape — the LLM judges classification AND the tag value in one call, since
/// there's no `category:` tag to already tell it what kind of work this is. `kind` mirrors
/// `category:`'s own real values so the caller can reuse the exact same namespace-per-kind
/// mapping path 1 uses, rather than duplicating that decision inside the prompt/response shape.
#[derive(Deserialize)]
struct ClassifyAndBackfillResponse {
    /// One of `"cosplay"`, `"manga"`, `"doujinshi"`, `"anthology"`, or `"unknown"` (the LLM's own
    /// judgment when the title + Category name hint aren't enough to tell) — `"anthology"`/
    /// `"unknown"` both result in nothing being written, same as path 1's own anthology handling.
    kind: String,
    cosplayer: Option<String>,
    artist: Option<String>,
    circle: Option<String>,
}

/// Runs the actual LLM call for path 1 (classification already known) and returns the tag(s) to
/// append (already `namespace:value` formatted), or an empty `Vec` if the LLM found nothing to
/// add. Never returns `Err` for "the LLM didn't find a name" — only for actual call failures
/// (network/key/parse), which the caller logs and otherwise ignores (this is best-effort
/// enrichment, not a required step).
async fn resolve_tags_for_known_kind(
    state: &AppState,
    title: &str,
    target: &BackfillTarget,
) -> Result<Vec<String>, String> {
    match target {
        BackfillTarget::Cosplayer => {
            let system = "你是一个从中文/日文/英文标题中识别コスプレイヤー（coser）网名的助手。\
                标题的常见格式是「<coser网名> - <拍摄主题>」，coser网名通常在标题最前面，用 - 或空格分隔。\
                如果标题里明显包含一个coser网名，原样输出（不翻译、不音译，保持原始大小写/文字）；\
                如果无法确信地识别出网名，输出 null。\n\n\
                只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
                ```typescript\n\
                interface Response { cosplayer: string | null }\n\
                ```";
            let user = format!("标题：{title}");
            let response = lanrurugi_llm::json_chat::<CosplayerBackfillResponse>(
                &state.redis.config,
                system,
                &user,
                0.2,
                200,
            )
            .await?;
            Ok(response
                .cosplayer
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
                .map(|s| format!("cosplayer:{s}"))
                .into_iter()
                .collect())
        }
        BackfillTarget::ArtistOrCircle => {
            let system = "你是一个从中文/日文/英文的漫画/同人志标题中识别作者或社团名的助手。\
                标题中可能包含个人作者名（通常用方括号标出，如「[作者名]」）或社团名（同人志社团），\
                请判断这是个人创作还是社团作品，只填其中一个字段。\
                原样输出识别到的名字（不翻译、不音译，保持原始文字）；\
                如果无法确信地识别出作者或社团名，两个字段都输出 null。\n\n\
                只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
                ```typescript\n\
                interface Response { artist: string | null; circle: string | null }\n\
                ```";
            let user = format!("标题：{title}");
            let response = lanrurugi_llm::json_chat::<ArtistBackfillResponse>(
                &state.redis.config,
                system,
                &user,
                0.2,
                200,
            )
            .await?;
            let mut tags = Vec::new();
            if let Some(artist) = response
                .artist
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
            {
                tags.push(format!("artist:{artist}"));
            }
            if let Some(circle) = response
                .circle
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
            {
                tags.push(format!("circle:{circle}"));
            }
            Ok(tags)
        }
    }
}

/// Path 2 — no `category:` tag at all. `category_name_hint` is the archive's LANrurugi Category
/// name if it's a member of exactly one static category worth mentioning (see caller for how
/// that's resolved) — passed to the LLM as a soft hint, not trusted blindly, since a Category is a
/// free-form user-authored label with no guaranteed relationship to actual content type.
async fn resolve_tags_by_classifying(
    state: &AppState,
    title: &str,
    category_name_hint: Option<&str>,
) -> Result<Vec<String>, String> {
    let system = "你是一个漫画/同人志/cosplay作品的分类与信息提取助手。\
        给定一个档案标题（可能还附带一个用户自定义的分类名称作为参考线索，该线索不一定准确），\
        请判断这个作品属于以下哪种类型：cosplay（角色扮演摄影）、doujinshi（同人志）、manga（漫画）、\
        anthology（多作者合集）、unknown（无法判断）。\n\
        判断类型后：\n\
        - 如果是 cosplay，尝试从标题中识别coser网名，填入 cosplayer 字段\n\
        - 如果是 doujinshi 或 manga，尝试识别个人作者名或社团名，只填 artist 或 circle 其中一个\n\
        - 如果是 anthology 或 unknown，或者无法确信地识别出对应名字，相应字段留 null\n\
        名字原样输出（不翻译、不音译，保持原始文字）。\n\n\
        只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
        ```typescript\n\
        interface Response {\n\
          kind: \"cosplay\" | \"doujinshi\" | \"manga\" | \"anthology\" | \"unknown\"\n\
          cosplayer: string | null\n\
          artist: string | null\n\
          circle: string | null\n\
        }\n\
        ```";
    let user = match category_name_hint {
        Some(name) => format!("标题：{title}\n用户自定义分类名称（参考线索，不一定准确）：{name}"),
        None => format!("标题：{title}"),
    };
    let response = lanrurugi_llm::json_chat::<ClassifyAndBackfillResponse>(
        &state.redis.config,
        system,
        &user,
        0.2,
        250,
    )
    .await?;

    let mut tags = Vec::new();
    match response.kind.as_str() {
        "cosplay" => {
            if let Some(cosplayer) = response
                .cosplayer
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
            {
                tags.push(format!("cosplayer:{cosplayer}"));
            }
        }
        "doujinshi" | "manga" => {
            if let Some(artist) = response
                .artist
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
            {
                tags.push(format!("artist:{artist}"));
            }
            if let Some(circle) = response
                .circle
                .map(|s| clean_llm_name(&s))
                .filter(|s| !s.is_empty())
            {
                tags.push(format!("circle:{circle}"));
            }
        }
        // "anthology" | "unknown" | anything else the LLM might emit — no tag to write.
        _ => {}
    }
    Ok(tags)
}

/// Resolves whether `archive_id` belongs to any LANrurugi `Category` (the saved-grouping entity —
/// NOT a `category:` tag) worth passing to the LLM as a hint, per this module's own path-2 docs.
/// Only static categories are checked (`archives` is meaningless for a dynamic/saved-search
/// category — see `Category::is_dynamic`'s own docs); the *first* matching category's name is
/// used if the archive happens to be in more than one, since this is a soft hint the LLM is free
/// to weigh or disregard, not a value correctness depends on picking "the right" one.
async fn find_category_name_hint(state: &AppState, archive_id: &str) -> Option<String> {
    let categories = state.repos.categories.list_all().await.ok()?;
    let target = lanrurugi_core::ids::ArchiveId(archive_id.to_string());
    categories
        .into_iter()
        .find(|c| !c.is_dynamic() && c.archives.contains(&target))
        .map(|c| c.name)
}

/// Entry point, called fire-and-forget from `plugins::run_enabled_metadata_plugins_on_archive`
/// (same spot/timing as the recommend-cache precompute). Silently does nothing when: no LLM key
/// is configured, the archive already has a tag in whichever namespace would be written, or the
/// LLM call itself fails — this is best-effort enrichment layered on top of ingestion, never a
/// blocking step.
pub async fn backfill_artist_tag(state: &AppState, archive_id: &str, title: &str, tags: &str) {
    if lanrurugi_llm::resolve_api_key(&state.redis.config)
        .await
        .is_none()
    {
        return;
    }

    let has_category_tag = !category_tag_values(tags).is_empty();
    let new_tags = if let Some(target) = backfill_target_from_category_tag(tags) {
        // Path 1: category: tag present and recognized.
        if already_has_target_tag(tags, &target) {
            return;
        }
        match resolve_tags_for_known_kind(state, title, &target).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(%archive_id, error = %e, "artist/cosplayer tag backfill LLM call failed");
                return;
            }
        }
    } else if has_category_tag {
        // Has category: tag(s), but none recognized (e.g. anthology, or something else
        // entirely) — no applicable target, and NOT a case for path 2's classification fallback
        // either (a category: tag already present, just not one this module handles, is a
        // different situation from having none at all).
        return;
    } else {
        // Path 2: no category: tag at all.
        if already_has_target_tag(tags, &BackfillTarget::Cosplayer)
            || already_has_target_tag(tags, &BackfillTarget::ArtistOrCircle)
        {
            // Already has SOME creator-identity tag despite no category: tag — trust it, don't
            // second-guess with an LLM classification call.
            return;
        }
        let category_hint = find_category_name_hint(state, archive_id).await;
        match resolve_tags_by_classifying(state, title, category_hint.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(%archive_id, error = %e, "artist/cosplayer classify-and-backfill LLM call failed");
                return;
            }
        }
    };

    if new_tags.is_empty() {
        return;
    }

    let Ok(Some(mut archive)) = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
        .await
    else {
        return;
    };
    // Re-check against the archive's current tags (not the `tags` snapshot passed in) — this call
    // runs fire-and-forget after the caller's own function returns, so another concurrent write
    // could have landed in between.
    let already_has_any = new_tags.iter().any(|new_tag| {
        let ns = new_tag.split(':').next().unwrap_or(new_tag);
        archive
            .tags
            .split(',')
            .map(str::trim)
            .any(|t| t.starts_with(&format!("{ns}:")))
    });
    if already_has_any {
        return;
    }
    let old_tags = archive.tags.clone();
    let mut seen: std::collections::HashSet<String> = old_tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let mut merged: Vec<String> = old_tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    for tag in new_tags {
        if seen.insert(tag.clone()) {
            merged.push(tag);
        }
    }
    archive.tags = merged.join(",");

    if let Err(e) = state.repos.archives.save(&archive).await {
        tracing::warn!(%archive_id, error = %e, "failed to save LLM-backfilled artist/cosplayer tag");
        return;
    }
    if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
        &state.redis.search,
        archive_id,
        &old_tags,
        &archive.tags,
    )
    .await
    {
        tracing::warn!(%archive_id, error = %e, "failed to update tag search index after artist/cosplayer backfill");
    }
}

/// Full-library sweep, run as a tracked background job (`JobRegistry`, visible on the Jobs page)
/// — the retroactive counterpart to `backfill_artist_tag`'s own per-archive ingest hook, for the
/// two cases that hook alone can never reach: an archive already in the library before this
/// feature shipped, and an archive ingested before the user had ever configured an LLM key.
/// Triggered from `settings::put_settings` when `llm_api_key` transitions from unset to set (see
/// that call site's own comment) — NOT run automatically at startup the way the recommend-cache
/// backfill is, since there's no equivalent "model finished loading" moment to hang it off; a
/// user might add the key days after their library was already fully ingested.
///
/// Sequential, not batched with a thread pool — unlike `recommend_precompute`'s embedding work
/// (CPU-bound, genuinely parallelizable across cores), each archive here is one network round
/// trip to the LLM API, and DeepSeek's own rate limits make a large concurrent fan-out
/// counterproductive rather than faster. Every archive is still visited (via
/// `backfill_artist_tag`'s own cheap early-return checks) even though only some will end up
/// making an LLM call at all — the per-archive function itself already skips anything with no
/// applicable category or an existing tag, so the real LLM-call count is far smaller than the
/// library size for most libraries.
pub async fn spawn_full_backfill_job(state: &AppState, reason: &str) -> String {
    let jobs = state.jobs.clone();
    let job_id = jobs.create("artist_backfill").await;
    let job_id_for_task = job_id.clone();
    let state = state.clone();
    let reason = reason.to_string();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        tracing::info!(reason, "artist_backfill: full library sweep starting");

        let archives = match state.repos.archives.list_all().await {
            Ok(a) => a,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };
        let total = archives.len();
        if total == 0 {
            jobs.finish(&job_id_for_task, serde_json::json!({ "archives": 0 }))
                .await;
            return;
        }

        for (i, archive) in archives.iter().enumerate() {
            backfill_artist_tag(&state, archive.id.as_str(), &archive.title, &archive.tags).await;
            jobs.set_progress(&job_id_for_task, (i + 1) as f32 / total as f32)
                .await;
        }

        jobs.finish(&job_id_for_task, serde_json::json!({ "archives": total }))
            .await;
    });

    job_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_llm_name_strips_trailing_underscore_from_bracketed_handle() {
        // Real bug: a title like "[叉子宝宝_  ] 2B ..." led the LLM to copy "叉子宝宝_" verbatim,
        // producing a tag that differed from this same coser's other archives (tagged plain
        // "叉子宝宝") only by a trailing underscore — defeating the exact-match cosplayer bonus.
        assert_eq!(clean_llm_name("叉子宝宝_"), "叉子宝宝");
        assert_eq!(clean_llm_name("  叉子宝宝_ "), "叉子宝宝");
    }

    #[test]
    fn clean_llm_name_strips_surrounding_brackets() {
        assert_eq!(clean_llm_name("[someartist]"), "someartist");
        assert_eq!(clean_llm_name("【someartist】"), "someartist");
    }

    #[test]
    fn clean_llm_name_leaves_internal_punctuation_alone() {
        assert_eq!(clean_llm_name("some-artist_name"), "some-artist_name");
    }

    #[test]
    fn backfill_target_from_category_tag_cosplay_wants_cosplayer() {
        assert_eq!(
            backfill_target_from_category_tag("category:cosplay,other:x"),
            Some(BackfillTarget::Cosplayer)
        );
    }

    #[test]
    fn backfill_target_from_category_tag_manga_wants_artist_or_circle() {
        assert_eq!(
            backfill_target_from_category_tag("category:manga"),
            Some(BackfillTarget::ArtistOrCircle)
        );
    }

    #[test]
    fn backfill_target_from_category_tag_doujinshi_wants_artist_or_circle() {
        assert_eq!(
            backfill_target_from_category_tag("category:doujinshi"),
            Some(BackfillTarget::ArtistOrCircle)
        );
    }

    #[test]
    fn backfill_target_from_category_tag_anthology_is_excluded() {
        assert_eq!(
            backfill_target_from_category_tag("category:anthology"),
            None
        );
    }

    #[test]
    fn backfill_target_from_category_tag_no_category_tag_is_none() {
        // The common locally-uploaded-archive case — no category: tag at all, not even an empty
        // one. Must not be confused with "has category:anthology" or any other applicable value.
        assert_eq!(
            backfill_target_from_category_tag("artist:someone,other:x"),
            None
        );
        assert_eq!(backfill_target_from_category_tag(""), None);
    }

    #[test]
    fn backfill_target_from_category_tag_is_case_insensitive() {
        assert_eq!(
            backfill_target_from_category_tag("category:Cosplay"),
            Some(BackfillTarget::Cosplayer)
        );
    }

    #[test]
    fn already_has_target_tag_detects_existing_cosplayer() {
        assert!(already_has_target_tag(
            "cosplayer:someone,category:cosplay",
            &BackfillTarget::Cosplayer
        ));
    }

    #[test]
    fn already_has_target_tag_false_when_missing() {
        assert!(!already_has_target_tag(
            "category:cosplay",
            &BackfillTarget::Cosplayer
        ));
    }

    #[test]
    fn already_has_target_tag_accepts_either_artist_or_circle() {
        assert!(already_has_target_tag(
            "artist:someone",
            &BackfillTarget::ArtistOrCircle
        ));
        assert!(already_has_target_tag(
            "circle:somegroup",
            &BackfillTarget::ArtistOrCircle
        ));
        assert!(!already_has_target_tag(
            "category:manga",
            &BackfillTarget::ArtistOrCircle
        ));
    }

    #[test]
    fn category_tag_values_parses_multiple() {
        let values = category_tag_values("category:manga,category:doujinshi,other:x");
        assert_eq!(values, vec!["manga", "doujinshi"]);
    }

    #[test]
    fn category_tag_values_empty_when_none_present() {
        assert!(category_tag_values("artist:x,female:y").is_empty());
    }
}
