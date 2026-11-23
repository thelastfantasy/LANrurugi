//! Cross-group merge of suggestions that the graph split up.
//!
//! The clustering requires *every* pair in a group to clear the threshold, so one weak edge breaks a
//! series into several suggestions — the live case that started this: one 20-volume series arrived as
//! 6 / 10 / 2, and nothing downstream put it back together (the per-group reviewer may only remove
//! members, and it never sees a second group).
//!
//! Local work does the scoping, the model only answers "which of these are the same series": a
//! creator-based candidate cluster keeps the prompt small (on a real 1420-suggestion library only
//! ~134 clusters have two or more suggestions at all), and every answer is re-validated against the
//! deterministic gates before anything is merged.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::{creator_value, title_creator, GroupSuggestion, GroupingSettings};
use crate::AppState;

/// Titles quoted per suggestion in the prompt. Two is what tells a series from an artist's separate
/// works without turning the prompt into a listing.
const TITLES_PER_SUGGESTION: usize = 2;
/// A cluster needs at least this many suggestions to be worth a question.
const MIN_CLUSTER_SUGGESTIONS: usize = 2;

/// One group of suggestions that share a creator and may be one series.
#[derive(Debug, Clone)]
struct Cluster {
    /// 1-based number in the prompt, and the only way the answer refers back to it.
    label: usize,
    /// The creator values the member suggestions have in common, for the prompt line.
    creator: String,
    /// Indices into the suggestion list.
    suggestions: Vec<usize>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct MergeAnswer {
    /// Groups of `s<n>` labels — one per suggestion. Strings rather than numbers because the prompt
    /// also numbers the clusters, and a model answering with cluster numbers would otherwise look
    /// like a perfectly valid answer about the wrong suggestions.
    #[serde(default)]
    merges: Vec<Vec<String>>,
}

/// Merges suggestions that the model says are one series, keeping everything the deterministic gates
/// reject untouched. Returns the same list when nothing is merged or the model is unavailable.
pub(super) async fn same_series(
    state: &AppState,
    suggestions: Vec<GroupSuggestion>,
    orphans: &[String],
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
    settings: &GroupingSettings,
) -> Vec<GroupSuggestion> {
    if !settings.merge_same_series || suggestions.len() < MIN_CLUSTER_SUGGESTIONS {
        return suggestions;
    }
    // A work the graph left alone (every pair of it was just under the threshold, or the clique
    // needed one pair it could not supply) never becomes a suggestion at all, so it could never be
    // merged either. It is offered here as a one-member candidate: adopted when the model says it
    // belongs to a series, dropped again when it does not (`finish`).
    let orphan_sets: HashSet<String> = orphans.iter().cloned().collect();
    let mut pool = suggestions;
    for id in orphans {
        pool.push(GroupSuggestion::orphan(id.clone()));
    }
    let clusters = candidate_clusters(&pool, titles, tags);
    if clusters.is_empty() {
        return finish(pool, &orphan_sets);
    }
    if lanrurugi_llm::resolve_api_key(&state.redis.config)
        .await
        .is_none()
    {
        return finish(pool, &orphan_sets);
    }

    // One question per series-sized block, never one per run: shown a single block the model answers
    // correctly, while every attempt to hand it several unrelated blocks at once had it either decline
    // the obvious series or over-merge an artist's separate works. Each block's answer is cached by
    // its own content, so an unchanged block is free on the next run and one bad answer costs only
    // that block.
    let mut merges: Vec<Vec<usize>> = Vec::new();
    let mut accepted_keys: HashSet<String> = HashSet::new();
    for block in blocks(&clusters, &pool, titles) {
        let (lines, by_label) = block_prompt_lines(&block, &pool, titles);
        if by_label.len() < MIN_CLUSTER_SUGGESTIONS {
            continue;
        }
        let answer = match cached_answer(&state.redis.config, &lines).await {
            Some(answer) => answer,
            None => match ask_model(&state.redis.config, &lines, by_label.len()).await {
                Some(answer) => {
                    store_answer(&state.redis.config, &lines, &answer).await;
                    answer
                }
                None => continue,
            },
        };
        let before = merges.len();
        let accepted: Vec<Vec<usize>> = answer
            .merges
            .iter()
            .filter_map(|group| map_merge(group, &by_label))
            .collect();
        for group in &accepted {
            // The model has just said these suggestions are one series, so the series key is now an
            // established fact for this run: later suggestions with the same key are folded in
            // without another question (`converge`).
            for &index in group {
                let key = dominant_series_key(&pool[index], titles);
                if !key.is_empty() {
                    accepted_keys.insert(key);
                }
            }
        }
        merges.extend(accepted);
        tracing::info!(
            block = %block.key,
            asked = by_label.len(),
            proposed = answer.merges.len(),
            accepted = merges.len() - before,
            accepted_groups = ?answer
                .merges
                .iter()
                .filter(|group| map_merge(group, &by_label).is_some())
                .collect::<Vec<_>>(),
            "grouping merge block judged"
        );
    }
    if merges.is_empty() {
        return finish(pool, &orphan_sets);
    }
    let merged = apply_merges(pool, &merges, titles, tags, settings);
    finish(converge(merged, &accepted_keys, titles), &orphan_sets)
}

/// One series-sized block: the suggestions of one cluster that share a title key.
struct Block {
    /// Suggestion indices, in the pool.
    suggestions: Vec<usize>,
    /// The title key the block's lines share, or the cluster's creator when no title key survived.
    key: String,
}

/// Folds the leftovers of an accepted series into one suggestion.
///
/// The per-block answer only covers the suggestions that block was shown. On a real library the same
/// series can still arrive in a second block (an adopted orphan pair, a group whose representative
/// titles differ slightly), and asking again about one or two lines is both slow and unreliable — the
/// model's earlier answer about that series key already settled the question, so this applies it.
fn converge(
    suggestions: Vec<GroupSuggestion>,
    accepted_keys: &HashSet<String>,
    titles: &HashMap<String, String>,
) -> Vec<GroupSuggestion> {
    if accepted_keys.is_empty() {
        return suggestions;
    }
    // Keys of every archive already inside an accepted series, taken from the titles themselves rather
    // than from a suggestion's "dominant" key: a two-member leftover can easily have a representative
    // title whose own key differs, and that is exactly the case this pass exists to catch.
    let mut target_by_key: HashMap<String, usize> = HashMap::new();
    for (index, suggestion) in suggestions.iter().enumerate() {
        for id in &suggestion.archive_ids {
            let Some(key) = titles.get(id).and_then(|t| series_key(t)) else {
                continue;
            };
            if accepted_keys.contains(&key) {
                target_by_key.entry(key).or_insert(index);
            }
        }
    }
    if target_by_key.is_empty() {
        return suggestions;
    }
    let mut absorbed: HashSet<usize> = HashSet::new();
    let mut additions: HashMap<usize, Vec<String>> = HashMap::new();
    for (index, suggestion) in suggestions.iter().enumerate() {
        let mut best: Option<(usize, usize)> = None;
        for id in &suggestion.archive_ids {
            let Some(key) = titles.get(id).and_then(|t| series_key(t)) else {
                continue;
            };
            let Some(&target) = target_by_key.get(&key) else {
                continue;
            };
            if target == index || absorbed.contains(&target) {
                continue;
            }
            // Prefer the target this suggestion shares the most archives' keys with.
            let score = best.map_or(1, |(_, n)| n + 1);
            best = Some((target, score));
        }
        let Some((target, _)) = best else {
            continue;
        };
        if absorbed.insert(index) {
            additions
                .entry(target)
                .or_default()
                .extend(suggestion.archive_ids.iter().cloned());
        }
    }
    if absorbed.is_empty() {
        return suggestions;
    }
    let mut out = Vec::with_capacity(suggestions.len());
    for (index, mut suggestion) in suggestions.into_iter().enumerate() {
        if absorbed.contains(&index) {
            continue;
        }
        if let Some(extra) = additions.remove(&index) {
            for id in extra {
                if !suggestion.archive_ids.contains(&id) {
                    suggestion.archive_ids.push(id);
                }
            }
            suggestion.archive_ids.sort();
            suggestion.suggestion_id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
                &suggestion.archive_ids,
                suggestion.existing_tankoubon_id.as_deref(),
            );
        }
        out.push(suggestion);
    }
    out
}

/// The blocks worth asking about: two or more suggestions, and at least one of them a real
/// (multi-member) suggestion — a pair of orphans alone is not a series anybody asked about.
fn blocks(
    clusters: &[Cluster],
    suggestions: &[GroupSuggestion],
    titles: &HashMap<String, String>,
) -> Vec<Block> {
    let mut out = Vec::new();
    for cluster in clusters {
        let mut buckets: Vec<Block> = Vec::new();
        for &index in &cluster.suggestions {
            let key = dominant_series_key(&suggestions[index], titles);
            match buckets.iter_mut().find(|b| b.key == key) {
                Some(block) => block.suggestions.push(index),
                None => buckets.push(Block {
                    suggestions: vec![index],
                    key: if key.is_empty() {
                        cluster.creator.clone()
                    } else {
                        key
                    },
                }),
            }
        }
        for block in buckets {
            if block.suggestions.len() < MIN_CLUSTER_SUGGESTIONS {
                continue;
            }
            let has_real = block
                .suggestions
                .iter()
                .any(|&i| suggestions[i].archive_ids.len() >= MIN_CLUSTER_SUGGESTIONS);
            if has_real {
                out.push(block);
            }
        }
    }
    out
}

/// The prompt body for one block, plus the label → (cluster, suggestion) mapping validation uses.
///
/// Labels are local to the block (`s1`..`sn`) because that is the only question asked: a global
/// numbering would have to be trusted across calls.
fn block_prompt_lines(
    block: &Block,
    suggestions: &[GroupSuggestion],
    titles: &HashMap<String, String>,
) -> (String, HashMap<String, (usize, usize)>) {
    let mut text = String::new();
    if !block.key.is_empty() {
        text.push_str(&format!("—— 标题近似：{}\n", block.key));
    }
    let mut by_label = HashMap::new();
    for (i, &suggestion) in block.suggestions.iter().enumerate() {
        let id = format!("s{}", i + 1);
        by_label.insert(id.clone(), (0usize, suggestion));
        let mut shown: Vec<String> = suggestions[suggestion]
            .archive_ids
            .iter()
            .filter_map(|id| titles.get(id))
            .map(|title| lanrurugi_scanner::release_markers::clean_title(title))
            .take(TITLES_PER_SUGGESTION)
            .collect();
        shown.retain(|title| !title.is_empty());
        text.push_str(&format!(
            "{id} {}本 {}\n",
            suggestions[suggestion].archive_ids.len(),
            shown
                .iter()
                .map(|title| format!("「{title}」"))
                .collect::<Vec<_>>()
                .join("")
        ));
    }
    (text, by_label)
}

/// The cached answer for exactly these lines, if one was stored. Keyed by content, so a rerun whose
/// library did not change re-asks nothing.
async fn cached_answer(pool: &deadpool_redis::Pool, lines: &str) -> Option<MergeAnswer> {
    let mut conn = pool.get().await.ok()?;
    let raw: Option<String> =
        deadpool_redis::redis::AsyncCommands::get(&mut conn, answer_key(lines))
            .await
            .ok()?;
    serde_json::from_str(&raw?).ok()
}

async fn store_answer(pool: &deadpool_redis::Pool, lines: &str, answer: &MergeAnswer) {
    let Ok(mut conn) = pool.get().await else {
        return;
    };
    let Ok(payload) = serde_json::to_string(answer) else {
        return;
    };
    // A month: long enough that an unchanged block is never re-asked in practice, short enough that
    // an abandoned library's cache does not live for ever.
    let _: Result<(), _> = deadpool_redis::redis::AsyncCommands::set_ex(
        &mut conn,
        answer_key(lines),
        payload,
        30 * 24 * 60 * 60,
    )
    .await;
}

fn answer_key(lines: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lines.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("LANRURUGI_GROUPING_MERGE_{hash:016x}")
}

/// Suggestions sharing a creator, as clusters of two or more, biggest first.
fn candidate_clusters(
    suggestions: &[GroupSuggestion],
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
) -> Vec<Cluster> {
    // Suggestion -> the creator values any of its members states (tags, or the title when the tags
    // carry none). `creator_value` sees an `artist:` tag the scorer derived from the title too, so a
    // bracket-only creator still counts.
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, suggestion) in suggestions.iter().enumerate() {
        for creator in creators_of(suggestion, titles, tags) {
            by_key
                .entry(format!("c:{creator}"))
                .or_default()
                .push(index);
        }
        // A suggestion whose members carry no creator tag at all (common: the name is only in the
        // title, or nowhere) can only be reached by its titles — the live case this fixes: 16 volumes
        // of one series in one group and the remaining 2 in another, the two connected by nothing but
        // their own titles.
        for key in series_keys(suggestion, titles) {
            by_key.entry(format!("t:{key}")).or_default().push(index);
        }
    }

    // Connected components over "shares a creator or a title key with", which is what catches an
    // `artist:` value and a `circle:` value naming the same person, and a group of volumes whose
    // creator is stated nowhere.
    let mut parent: Vec<usize> = (0..suggestions.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for members in by_key.values() {
        for pair in members.windows(2) {
            let (a, b) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..suggestions.len() {
        let root = find(&mut parent, index);
        components.entry(root).or_default().push(index);
    }
    let mut clusters: Vec<Cluster> = components
        .into_values()
        .filter(|members| members.len() >= MIN_CLUSTER_SUGGESTIONS)
        .map(|mut members| {
            members.sort_unstable();
            Cluster {
                label: 0,
                creator: shared_key(&members, &by_key),
                suggestions: members,
            }
        })
        .collect();
    clusters.sort_by_key(|a| std::cmp::Reverse(a.suggestions.len()));
    for (i, cluster) in clusters.iter_mut().enumerate() {
        cluster.label = i + 1;
    }
    clusters
}

/// The creator (or, failing that, the title key) a cluster's suggestions have in common, for the
/// prompt line.
fn shared_key(members: &[usize], by_key: &HashMap<String, Vec<usize>>) -> String {
    let mut best: Option<(usize, &String)> = None;
    for (key, owners) in by_key {
        let count = members.iter().filter(|m| owners.contains(m)).count();
        if count >= MIN_CLUSTER_SUGGESTIONS && best.is_none_or(|(b, _)| count > b) {
            best = Some((count, key));
        }
    }
    best.map(|(_, key)| key.trim_start_matches("c:").to_string())
        .unwrap_or_default()
}

/// The series keys of one suggestion's member titles: what a title says once release markers, volume
/// tokens and bare catalogue numbers are gone (`第18巻`, `VOL.02`, `俺の上京性生活21`). Used only to
/// connect suggestions into one candidate cluster — never to decide a merge.
fn series_keys(suggestion: &GroupSuggestion, titles: &HashMap<String, String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for id in &suggestion.archive_ids {
        if let Some(title) = titles.get(id) {
            if let Some(key) = series_key(title) {
                out.insert(key);
            }
        }
    }
    out
}

/// Collection words that sit *inside* the series name and would otherwise split one series' volumes
/// from its collected edition (`俺の上京性生活` vs `俺の上京性生活総集編`). Not release markers — the
/// scanner's own marker list is about a listing's boilerplate, not about a title's own wording.
const COLLECTION_WORDS: [&str; 6] = ["総集編", "総集篇", "合本版", "完全版", "単行本版", "電子版"];

fn series_key(title: &str) -> Option<String> {
    let mut cleaned = lanrurugi_scanner::release_markers::clean_title(title).to_lowercase();
    for word in COLLECTION_WORDS {
        cleaned = cleaned.replace(word, " ");
    }
    let mut words: Vec<String> = Vec::new();
    for word in cleaned.split([
        ' ', '\t', '_', '-', '－', '–', '—', ':', '：', '·', '•', '[', ']', '【', '】', '「', '」',
        '『', '』', '(', ')', '（', '）',
    ]) {
        let word = word.trim();
        if word.is_empty() {
            continue;
        }
        let volume = word.starts_with('第') && word.ends_with(['巻', '卷', '話', '话', '編', '编']);
        let digits = !word.is_empty() && word.chars().all(|c| c.is_ascii_digit());
        if volume || digits {
            continue;
        }
        // A volume number glued to the series name (`俺の上京性生活21`) is still a volume number.
        let trimmed: String = word
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim()
            .to_string();
        if trimmed.is_empty() {
            continue;
        }
        words.push(trimmed);
    }
    // At most the first two words: the creator (or series name) plus the series name. Volume numbers,
    // subtitles and collection markers come after them, so a full-title key would give every volume
    // of one series its own key — the live case that made four suggestions of one series form four
    // single-line "blocks" that were never asked about at all.
    let key = words.into_iter().take(2).collect::<Vec<_>>().join(" ");
    // Four characters: a series name can be that short in Japanese, and over-connecting two
    // candidates only gives the model one more pair to reject (it decides), while under-connecting
    // leaves the split this whole pass exists to fix.
    (key.chars().count() >= 4).then_some(key)
}

/// Every creator value of one suggestion's members.
fn creators_of(
    suggestion: &GroupSuggestion,
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for id in &suggestion.archive_ids {
        let tag_string = tags.get(id).map(String::as_str).unwrap_or("");
        for tag in tag_string.split(',') {
            if let Some(value) = creator_value(tag.trim()) {
                out.insert(value.to_lowercase());
            }
        }
        if let Some(title) = titles.get(id) {
            if let Some(value) = title_creator(title) {
                out.insert(value);
            }
        }
    }
    out
}

/// Drops the one-member candidates that no merge took: an orphan is only ever a member of a series
/// the model accepted, never a suggestion of its own.
fn finish(suggestions: Vec<GroupSuggestion>, orphans: &HashSet<String>) -> Vec<GroupSuggestion> {
    suggestions
        .into_iter()
        .filter(|s| !(s.archive_ids.len() == 1 && orphans.contains(&s.archive_ids[0])))
        .collect()
}

/// The title key most of a suggestion's members share — what the prompt groups its line by.
fn dominant_series_key(suggestion: &GroupSuggestion, titles: &HashMap<String, String>) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for id in &suggestion.archive_ids {
        if let Some(key) = titles.get(id).and_then(|title| series_key(title)) {
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(key, _)| key)
        .unwrap_or_default()
}

/// One model call. `None` on any failure: the run keeps the graph's own grouping for this batch.
async fn ask_model(pool: &deadpool_redis::Pool, lines: &str, count: usize) -> Option<MergeAnswer> {
    let user = format!(
        "同一作者往往有很多彼此无关的作品，只有确实属于同一系列或同一作品的建议才合并。\n\
         候选（C=候选簇，#=建议编号，括号内是该建议的代表标题）：\n{lines}\n\
         只输出 JSON：{{\"merges\":[[\"s1\",\"s4\"]]}}；每个数组是被判定为同一系列、应当合并的编号\
         （至少两个，必须用 s 开头的编号）；空行分隔的块之间不要合并；\
         没有可合并的返回 {{\"merges\":[]}}。"
    );
    let system = crate::llm_prompts::grouping_merge_system();
    let spec = lanrurugi_llm::CallSpec::new(lanrurugi_llm::FeatureId::TANKOUBON_GROUPING_REVIEW);
    let max_tokens = (24 * count + 100) as u32;
    match crate::repair::json_chat_checked::<MergeAnswer>(
        pool,
        &spec,
        &system,
        &user,
        0.0,
        max_tokens,
        |answer| validate_answer(answer, count),
    )
    .await
    {
        Ok(checked) => Some(checked.value),
        Err(e) => {
            tracing::info!(
                class = e.class().as_str(),
                "grouping merge fell back to the graph's own grouping"
            );
            None
        }
    }
}

/// Rejects an answer the repair path should retry: out-of-range or repeated labels, or a "merge" of
/// one.
fn validate_answer(answer: &MergeAnswer, count: usize) -> Result<(), String> {
    for group in &answer.merges {
        if group.len() < 2 {
            return Err("每个合并至少要有两个编号".to_string());
        }
        for label in group {
            let Some(n) = label
                .strip_prefix('s')
                .and_then(|n| n.parse::<usize>().ok())
            else {
                return Err(format!("编号 {label} 不是 s<数字> 的形式"));
            };
            if n < 1 || n > count {
                return Err(format!("编号 {label} 不在候选范围内"));
            }
        }
    }
    Ok(())
}

/// Turns one answer group into suggestion indices, dropping anything that cannot be a merge.
fn map_merge(group: &[String], by_label: &HashMap<String, (usize, usize)>) -> Option<Vec<usize>> {
    let mut seen = HashSet::new();
    let mut cluster = None;
    let mut out = Vec::new();
    for label in group {
        let (cluster_of, suggestion) = *by_label.get(label)?;
        if cluster.is_some_and(|c| c != cluster_of) {
            // One merge across two clusters would merge unrelated creators' works.
            return None;
        }
        cluster = Some(cluster_of);
        if seen.insert(suggestion) {
            out.push(suggestion);
        }
    }
    (out.len() >= 2).then_some(out)
}

/// Applies validated merges: members are unioned, evidence and hints combined, and every merge the
/// deterministic gates reject is dropped.
fn apply_merges(
    mut suggestions: Vec<GroupSuggestion>,
    merges: &[Vec<usize>],
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
    settings: &GroupingSettings,
) -> Vec<GroupSuggestion> {
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut merged: Vec<GroupSuggestion> = Vec::new();
    for group in merges {
        if group.len() < 2
            || group
                .iter()
                .any(|i| consumed.contains(i) || *i >= suggestions.len())
        {
            continue;
        }
        let mut archive_ids: Vec<String> = Vec::new();
        let mut existing: Option<String> = None;
        let mut evidence = std::collections::BTreeMap::new();
        let mut hints = super::review::ReviewHints::default();
        let mut conflict = false;
        let mut two_tanks = false;
        for &index in group {
            let suggestion = &suggestions[index];
            for id in &suggestion.archive_ids {
                if !archive_ids.contains(id) {
                    archive_ids.push(id.clone());
                }
            }
            match (&existing, &suggestion.existing_tankoubon_id) {
                (Some(a), Some(b)) if a != b => two_tanks = true,
                (None, Some(b)) => existing = Some(b.clone()),
                _ => {}
            }
            for pair in &suggestion.evidence {
                evidence
                    .entry((pair.a.clone(), pair.b.clone()))
                    .or_insert_with(|| pair.clone());
            }
            hints.top_tags.extend(suggestion.hints.top_tags.clone());
            hints.strength.extend(suggestion.hints.strength.clone());
        }
        if two_tanks {
            tracing::info!("grouping merge skipped: two different Tankoubons in one group");
            continue;
        }
        archive_ids.sort();
        let tag_sets = member_tag_sets(&archive_ids, titles, tags);
        let refs: Vec<HashSet<&str>> = tag_sets
            .iter()
            .map(|set| set.iter().map(String::as_str).collect())
            .collect();
        for i in 0..refs.len() {
            for j in i + 1..refs.len() {
                if settings.creator_gate
                    && !super::shares_cosplayer(&refs[i], &refs[j])
                    && super::creator_conflict(&refs[i], &refs[j])
                {
                    conflict = true;
                }
            }
        }
        if conflict {
            tracing::info!("grouping merge skipped: two unrelated creators in one group");
            continue;
        }
        let suggestion_id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
            &archive_ids,
            existing.as_deref(),
        );
        for &index in group {
            consumed.insert(index);
        }
        merged.push(GroupSuggestion {
            archive_ids,
            existing_tankoubon_id: existing,
            suggestion_id,
            evidence: evidence.into_values().collect(),
            review: None,
            hints,
        });
    }
    if merged.is_empty() {
        return suggestions;
    }
    let mut out: Vec<GroupSuggestion> = suggestions
        .drain(..)
        .enumerate()
        .filter(|(i, _)| !consumed.contains(i))
        .map(|(_, s)| s)
        .collect();
    out.extend(merged);
    out
}

/// Each member's scoring tags: the stored tags plus the creator its title states, which is what the
/// scorer itself sees (see `run.rs`'s derived-creator pass).
fn member_tag_sets(
    ids: &[String],
    titles: &HashMap<String, String>,
    tags: &HashMap<String, String>,
) -> Vec<HashSet<String>> {
    ids.iter()
        .map(|id| {
            let raw = tags.get(id).map(String::as_str).unwrap_or("");
            let mut set: HashSet<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            if !set.iter().any(|t| creator_value(t).is_some()) {
                if let Some(creator) = titles.get(id).and_then(|t| title_creator(t)) {
                    set.insert(format!("artist:{creator}"));
                }
            }
            set
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(ids: &[&str]) -> GroupSuggestion {
        GroupSuggestion::for_test(ids.iter().map(|i| (*i).to_string()).collect(), None)
    }

    fn maps(items: &[(&str, &str, &str)]) -> (HashMap<String, String>, HashMap<String, String>) {
        let mut titles = HashMap::new();
        let mut tags = HashMap::new();
        for (id, title, tag) in items {
            titles.insert((*id).to_string(), (*title).to_string());
            tags.insert((*id).to_string(), (*tag).to_string());
        }
        (titles, tags)
    }

    #[test]
    fn clusters_split_an_artist_with_unrelated_works_by_title_creator() {
        // No creator tags anywhere: the bracketed name in the title is the only signal.
        let (titles, tags) = maps(&[
            (
                "a",
                "[anko×ANTAI×haron] 凸感ロマンス 第01巻",
                "category:manga",
            ),
            (
                "b",
                "[anko×ANTAI×haron] 凸感ロマンス 第02巻",
                "category:manga",
            ),
            (
                "c",
                "[龍大和] 俺の指先で濡れる世界 第01巻",
                "category:manga",
            ),
            (
                "d",
                "[龍大和] 俺の指先で濡れる世界 第02巻",
                "category:manga",
            ),
        ]);
        let suggestions = vec![suggestion(&["a", "b"]), suggestion(&["c", "d"])];
        let clusters = candidate_clusters(&suggestions, &titles, &tags);
        assert!(
            clusters.is_empty(),
            "no shared creator, so nothing to merge"
        );

        let (titles, tags) = maps(&[
            ("a", "[同じ] シリーズ 第01巻", "category:manga"),
            ("b", "[同じ] シリーズ 第02巻", "category:manga"),
            ("c", "[同じ] シリーズ 第03巻", "category:manga"),
        ]);
        let suggestions = vec![suggestion(&["a", "b"]), suggestion(&["c"])];
        let clusters = candidate_clusters(&suggestions, &titles, &tags);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].suggestions, vec![0, 1]);
    }

    #[test]
    fn volumes_without_any_creator_tag_still_reach_one_cluster_by_title() {
        // The live case: 16 volumes landed in one group, the remaining 2 in another, and nothing but
        // the titles links them (no artist/circle tag on either side).
        let (titles, tags) = maps(&[
            ("a", "シリーズ名 第1巻", ""),
            ("b", "シリーズ名 第2巻", ""),
            ("c", "シリーズ名 第18巻", ""),
        ]);
        let suggestions = vec![suggestion(&["a", "b"]), suggestion(&["c"])];
        let clusters = candidate_clusters(&suggestions, &titles, &tags);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].suggestions, vec![0, 1]);
    }

    #[test]
    fn a_title_key_separates_unrelated_titles() {
        let (titles, tags) = maps(&[
            ("a", "[同じ] シリーズ 第1巻", "artist:same"),
            ("b", "[同じ] 別の話 第1巻", "artist:same"),
        ]);
        // Same artist, so they share a creator cluster — but the title keys must not be equal, or an
        // artist's separate works would look like one series before the model ever sees them.
        let clusters =
            candidate_clusters(&[suggestion(&["a"]), suggestion(&["b"])], &titles, &tags);
        assert_eq!(
            clusters.len(),
            1,
            "creator still connects them; the model decides"
        );
        assert_ne!(
            series_key("[同じ] シリーズ 第1巻"),
            series_key("[同じ] 別の話 第1巻")
        );
    }

    #[test]
    fn series_keys_of_one_series_match_across_its_volumes() {
        // The exact shapes that survived as separate 2-member pockets after a merge.
        for title in [
            "[白玉湯] 俺の上京性生活21「冬休みの思い出編」",
            "[白玉湯] 俺の上京性生活12「一周年祭」",
            "[白玉湯] 俺の上京性生活15「精液検査編」",
            "[白玉湯] 俺の上京性生活「精液検査編2」",
            "[白玉湯] 俺の上京性生活「マタニティ姉妹編」",
            "[白玉湯] 俺の上京性生活総集編【4-6】",
        ] {
            let key = series_key(title);
            assert_eq!(
                key.as_deref(),
                Some("白玉湯 俺の上京性生活"),
                "title {title:?} produced {key:?}"
            );
        }
    }

    #[test]
    fn an_orphan_reaches_a_cluster_through_its_title() {
        let (titles, tags) = maps(&[
            ("a", "シリーズ名 第1巻", ""),
            ("b", "シリーズ名 第2巻", ""),
            ("c", "シリーズ名 第16巻", ""),
        ]);
        let real = suggestion(&["a", "b"]);
        let orphan = GroupSuggestion::orphan("c".to_string());
        let clusters = candidate_clusters(&[real, orphan], &titles, &tags);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].suggestions, vec![0, 1]);
    }

    #[test]
    fn an_unadopted_orphan_never_becomes_a_suggestion_of_its_own() {
        let (titles, tags) = maps(&[
            ("a", "シリーズ名 第1巻", ""),
            ("b", "シリーズ名 第2巻", ""),
            ("c", "別の作品 第1巻", ""),
        ]);
        let real = suggestion(&["a", "b"]);
        let orphan = GroupSuggestion::orphan("c".to_string());
        let orphans: HashSet<String> = ["c".to_string()].into_iter().collect();
        let settings = GroupingSettings::default();
        let merged = apply_merges(vec![real, orphan], &[vec![0, 1]], &titles, &tags, &settings);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].archive_ids,
            vec!["a", "b", "c"],
            "adopted into the series"
        );
        let kept = finish(
            vec![
                suggestion(&["a", "b"]),
                GroupSuggestion::orphan("c".to_string()),
            ],
            &orphans,
        );
        assert_eq!(kept.len(), 1, "an orphan no merge took is dropped");
    }

    #[test]
    fn a_merge_across_two_clusters_is_dropped() {
        let mut by_label = HashMap::new();
        by_label.insert("s1".to_string(), (1, 0));
        by_label.insert("s2".to_string(), (2, 1));
        assert!(map_merge(&["s1".into(), "s2".into()], &by_label).is_none());
        assert_eq!(map_merge(&["s1".into()], &by_label), None);
        by_label.insert("s3".to_string(), (1, 2));
        assert_eq!(
            map_merge(&["s1".into(), "s3".into()], &by_label),
            Some(vec![0, 2])
        );
    }

    #[test]
    fn applying_a_merge_combines_members_evidence_and_hints() {
        let (titles, tags) = maps(&[
            ("a", "[同じ] シリーズ 第01巻", "artist:same"),
            ("b", "[同じ] シリーズ 第02巻", "artist:same"),
            ("c", "[同じ] シリーズ 第03巻", "artist:same"),
        ]);
        let suggestions = vec![suggestion(&["a", "b"]), suggestion(&["c"])];
        let settings = GroupingSettings::default();
        let out = apply_merges(suggestions, &[vec![0, 1]], &titles, &tags, &settings);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].archive_ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_merge_of_two_different_tankoubons_is_dropped() {
        let (titles, tags) = maps(&[
            ("a", "[同じ] 第1巻", "artist:same"),
            ("b", "[同じ] 第2巻", "artist:same"),
        ]);
        let mut first = suggestion(&["a"]);
        first.existing_tankoubon_id = Some("TANK_1".into());
        let mut second = suggestion(&["b"]);
        second.existing_tankoubon_id = Some("TANK_2".into());
        let out = apply_merges(
            vec![first, second],
            &[vec![0, 1]],
            &titles,
            &tags,
            &GroupingSettings::default(),
        );
        assert_eq!(
            out.len(),
            2,
            "two Tankoubons are never merged into one suggestion"
        );
    }

    #[test]
    fn a_merge_that_would_join_disjoint_creators_is_dropped() {
        let (titles, tags) = maps(&[
            ("a", "[甲] 作品A 第1巻", "artist:甲"),
            ("b", "[乙] 作品B 第1巻", "artist:乙"),
        ]);
        let out = apply_merges(
            vec![suggestion(&["a"]), suggestion(&["b"])],
            &[vec![0, 1]],
            &titles,
            &tags,
            &GroupingSettings::default(),
        );
        assert_eq!(out.len(), 2);
    }
}
