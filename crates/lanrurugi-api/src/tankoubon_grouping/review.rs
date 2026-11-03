//! The grouping reviewer (`specs/016-tankoubon-grouping-harness` US2,
//! contracts/review-contract.md): the precision stage after local recall. It sees a compacted
//! input, can only reject members, is validated deterministically with one repair, and falls back
//! to the local result on any failure. Groups the local evidence settles are never sent.

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::ids::GroupSuggestionId;
use lanrurugi_llm::{CallSpec, FeatureId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{release_markers, GroupingSettings, NOISE_NAMESPACES};
use crate::llm_prompts::{grouping_review_system, GROUPING_REVIEW_PROMPT_VERSION};
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Reviewed,
    Repaired,
    Settled,
    FellBack,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub archive_id: String,
    pub keep: bool,
    pub reason: String,
}

/// One suggestion's review (data-model.md §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub status: ReviewStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verdicts: Vec<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The members actually sent; any other member is kept as proposed locally.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged: Vec<String>,
    pub prompt_version: String,
}

impl Review {
    fn simple(status: ReviewStatus, reason: Option<String>) -> Self {
        Self {
            status,
            verdicts: Vec::new(),
            reason,
            judged: Vec::new(),
            prompt_version: GROUPING_REVIEW_PROMPT_VERSION.to_string(),
        }
    }

    /// The members kept after this review (unjudged members always kept).
    pub fn kept(&self, members: &[String]) -> Vec<String> {
        let rejected: HashSet<&str> = self
            .verdicts
            .iter()
            .filter(|v| !v.keep)
            .map(|v| v.archive_id.as_str())
            .collect();
        members
            .iter()
            .filter(|m| !rejected.contains(m.as_str()))
            .cloned()
            .collect()
    }
}

/// What the reviewer needs to know about one member.
#[derive(Debug, Clone)]
pub struct MemberFacts {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
}

/// The per-suggestion data the suggestions call stores for the reviewer: each member's rarest
/// eligible tags and how strongly the local stage tied it to the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewHints {
    pub top_tags: HashMap<String, Vec<String>>,
    pub strength: HashMap<String, f32>,
}

fn creators(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = tags
        .iter()
        .filter_map(|t| super::creator_value(t))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

fn cosplayers(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| t.strip_prefix("cosplayer:"))
        .map(str::to_string)
        .collect()
}

/// FR-022: a group whose members all share a creator, or all share a cosplayer, is settled by
/// local evidence and never sent. Returns the shared value as the reason.
pub fn settled_reason(members: &[MemberFacts]) -> Option<String> {
    if members.len() < 2 {
        return None;
    }
    let common = |extract: fn(&[String]) -> Vec<String>| -> Option<String> {
        let mut sets = members
            .iter()
            .map(|m| extract(&m.tags).into_iter().collect::<HashSet<_>>());
        let first = sets.next()?;
        let shared = sets.fold(first, |acc, s| acc.intersection(&s).cloned().collect());
        let mut shared: Vec<String> = shared.into_iter().collect();
        shared.sort();
        shared.into_iter().next()
    };
    if let Some(c) = common(creators) {
        return Some(format!("creator:{c}"));
    }
    common(cosplayers).map(|c| format!("cosplayer:{c}"))
}

/// Whether a tag may be shown to the reviewer: never language/translation status or noise, and
/// creators travel separately.
pub fn eligible_review_tag(tag: &str) -> bool {
    let ns = tag.split(':').next().unwrap_or(tag);
    !NOISE_NAMESPACES.contains(&ns)
        && ns != "language"
        && !super::CREATOR_NAMESPACES.contains(&ns)
        && !tag.eq_ignore_ascii_case("other:translated")
}

/// Fallback when no stored hints exist: rank tags by how specific their namespace usually is.
fn tags_by_namespace_priority(tags: &[String], max: usize) -> Vec<String> {
    let rank = |t: &str| match t.split(':').next().unwrap_or(t) {
        "parody" | "series" => 0,
        "character" => 1,
        "cosplayer" => 2,
        "female" | "male" | "mixed" | "other" => 4,
        "category" => 5,
        _ => 3,
    };
    let mut eligible: Vec<&String> = tags.iter().filter(|t| eligible_review_tag(t)).collect();
    eligible.sort_by_key(|t| (rank(t), t.len()));
    eligible.into_iter().take(max).cloned().collect()
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The existing tankoubon an add-to-existing suggestion would extend.
#[derive(Debug, Clone)]
pub struct ExistingTankoubon {
    pub name: String,
    pub member_titles: Vec<String>,
}

/// The compacted reviewer input (FR-021) and the archive id behind each `m<i>`.
#[derive(Debug, Clone)]
pub struct CompactInput {
    pub payload: Value,
    pub ids: Vec<String>,
}

/// Builds the reviewer input: at most `review_max_members` members (the weakest-tied first when
/// capping), each as a cleaned, truncated title, ≤ 3 creators and ≤ `review_max_tags` tags.
pub fn compact_input(
    members: &[MemberFacts],
    hints: &ReviewHints,
    existing: Option<&ExistingTankoubon>,
    settings: &GroupingSettings,
) -> CompactInput {
    let mut judged: Vec<&MemberFacts> = members.iter().collect();
    if judged.len() > settings.review_max_members {
        judged.sort_by(|a, b| {
            let s = |m: &MemberFacts| hints.strength.get(&m.id).copied().unwrap_or(0.0);
            s(a).total_cmp(&s(b)).then(a.id.cmp(&b.id))
        });
        judged.truncate(settings.review_max_members);
        judged.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let members_json: Vec<Value> = judged
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let tags = match hints.top_tags.get(&m.id) {
                Some(top) => top
                    .iter()
                    .filter(|t| eligible_review_tag(t))
                    .take(settings.review_max_tags)
                    .cloned()
                    .collect(),
                None => tags_by_namespace_priority(&m.tags, settings.review_max_tags),
            };
            json!({
                "id": format!("m{}", i + 1),
                "title": truncate_chars(
                    &release_markers::clean_title(&m.title),
                    settings.review_title_chars,
                ),
                "creators": creators(&m.tags).into_iter().take(3).collect::<Vec<_>>(),
                "tags": tags,
            })
        })
        .collect();
    let mut payload = json!({ "members": members_json });
    if let Some(tank) = existing {
        payload["existing_tankoubon"] = json!({
            "name": truncate_chars(&tank.name, settings.review_title_chars),
            "member_titles": tank
                .member_titles
                .iter()
                .take(5)
                .map(|t| truncate_chars(&release_markers::clean_title(t), settings.review_title_chars))
                .collect::<Vec<_>>(),
        });
    }
    CompactInput {
        payload,
        ids: judged.iter().map(|m| m.id.clone()).collect(),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawVerdict {
    pub id: String,
    pub keep: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReviewerAnswer {
    #[serde(default)]
    pub verdicts: Vec<RawVerdict>,
}

/// FR-006: the verdict ids must be exactly `m1…mN`, each once. The error names the defect.
pub fn validate_verdicts(answer: &ReviewerAnswer, n: usize) -> Result<(), String> {
    let expected: HashSet<String> = (1..=n).map(|i| format!("m{i}")).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut problems = Vec::new();
    for v in &answer.verdicts {
        if !expected.contains(&v.id) {
            problems.push(format!("未知的 id {}", v.id));
        } else if !seen.insert(v.id.as_str()) {
            problems.push(format!("{} 出现了不止一次", v.id));
        }
    }
    let mut missing: Vec<&String> = expected
        .iter()
        .filter(|id| !seen.contains(id.as_str()))
        .collect();
    missing.sort();
    if !missing.is_empty() {
        problems.push(format!(
            "缺少 {}",
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("；"))
    }
}

/// A stable 64-bit FNV-1a, so cache keys survive a toolchain upgrade.
fn fnv64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hash of what the reviewer would see, so an edited title or tag misses the cache.
pub fn content_hash(members: &[MemberFacts], existing: Option<&ExistingTankoubon>) -> String {
    let mut sorted: Vec<&MemberFacts> = members.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut text = String::new();
    for m in sorted {
        let mut tags = m.tags.clone();
        tags.sort();
        text.push_str(&m.id);
        text.push('\u{1}');
        text.push_str(&m.title);
        text.push('\u{1}');
        text.push_str(&tags.join(","));
        text.push('\u{2}');
    }
    if let Some(t) = existing {
        text.push_str(&t.name);
    }
    format!("{:016x}", fnv64(text.bytes()))
}

/// A cached review, if one exists for these exact members, prompt and content.
pub async fn cached(pool: &Pool, id: &GroupSuggestionId, hash: &str) -> Option<Review> {
    let mut conn = pool.get().await.ok()?;
    let key =
        lanrurugi_storage::keys::grouping_review_key(id, GROUPING_REVIEW_PROMPT_VERSION, hash);
    let raw: Option<String> = conn.get(key).await.ok()?;
    raw.and_then(|r| serde_json::from_str(&r).ok())
}

async fn store(pool: &Pool, id: &GroupSuggestionId, hash: &str, review: &Review) {
    let key =
        lanrurugi_storage::keys::grouping_review_key(id, GROUPING_REVIEW_PROMPT_VERSION, hash);
    if let Ok(mut conn) = pool.get().await {
        let _: Result<(), _> = conn
            .set_ex(
                key,
                serde_json::to_string(review).unwrap_or_default(),
                lanrurugi_storage::keys::GROUPING_REVIEW_TTL_SECS as u64,
            )
            .await;
    }
}

/// Reviews one group: settled → cache → provider (with one repair) → fallback. Never fails.
/// `allow_provider` false stops before any provider call (the evaluation's cached mode).
pub async fn review_group(
    pool: &Pool,
    id: &GroupSuggestionId,
    members: &[MemberFacts],
    hints: &ReviewHints,
    existing: Option<&ExistingTankoubon>,
    settings: &GroupingSettings,
    allow_provider: bool,
) -> Review {
    let minimum = if existing.is_some() { 1 } else { 2 };
    if members.len() < minimum {
        return Review::simple(ReviewStatus::FellBack, Some("members_changed".into()));
    }
    if existing.is_none() {
        if let Some(reason) = settled_reason(members) {
            return Review::simple(ReviewStatus::Settled, Some(reason));
        }
    }
    let hash = content_hash(members, existing);
    if let Some(review) = cached(pool, id, &hash).await {
        return review;
    }
    if !allow_provider {
        return Review::simple(ReviewStatus::FellBack, Some("not_cached".into()));
    }
    if lanrurugi_llm::resolve_api_key(pool).await.is_none() {
        return Review::simple(ReviewStatus::NotConfigured, None);
    }

    let input = compact_input(members, hints, existing, settings);
    let n = input.ids.len();
    let user = input.payload.to_string();
    let system = grouping_review_system();
    let spec = CallSpec::new(FeatureId::TANKOUBON_GROUPING_REVIEW);
    let max_tokens = (60 * n + 100) as u32;
    let answer = match crate::repair::json_chat_checked::<ReviewerAnswer>(
        pool,
        &spec,
        &system,
        &user,
        0.0,
        max_tokens,
        |a| validate_verdicts(a, n),
    )
    .await
    {
        Ok(answer) => answer,
        Err(e) => {
            tracing::info!(
                class = e.class().as_str(),
                "grouping review fell back to the local result"
            );
            return Review::simple(ReviewStatus::FellBack, Some(e.class().as_str().to_string()));
        }
    };
    let repaired = answer.repaired;
    let answer = answer.value;
    let by_index: HashMap<String, &String> = input
        .ids
        .iter()
        .enumerate()
        .map(|(i, id)| (format!("m{}", i + 1), id))
        .collect();
    // Validation guarantees every id maps back; an archive outside the group cannot appear.
    let verdicts = answer
        .verdicts
        .iter()
        .filter_map(|v| {
            by_index.get(&v.id).map(|id| Verdict {
                archive_id: (*id).clone(),
                keep: v.keep,
                reason: v.reason.chars().take(200).collect(),
            })
        })
        .collect();
    let review = Review {
        status: if repaired {
            ReviewStatus::Repaired
        } else {
            ReviewStatus::Reviewed
        },
        verdicts,
        reason: None,
        judged: input.ids.clone(),
        prompt_version: GROUPING_REVIEW_PROMPT_VERSION.to_string(),
    };
    store(pool, id, &hash, &review).await;
    review
}

#[derive(Deserialize)]
pub(super) struct ReviewRequest {
    archive_ids: Vec<String>,
    #[serde(default)]
    existing_tankoubon_id: Option<String>,
}

/// Loads the current facts for `ids`, dropping archives that no longer exist.
pub(super) async fn member_facts(state: &AppState, ids: &[String]) -> Vec<MemberFacts> {
    let mut out = Vec::new();
    for id in ids {
        if let Ok(Some(a)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
        {
            out.push(MemberFacts {
                id: id.clone(),
                title: a.title,
                tags: split_tags(&a.tags),
            });
        }
    }
    out
}

pub(super) fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

async fn existing_tankoubon(state: &AppState, id: &str) -> Option<ExistingTankoubon> {
    let tank = state
        .repos
        .groupings
        .get(&lanrurugi_core::ids::TankId(id.to_string()))
        .await
        .ok()
        .flatten()?;
    let mut member_titles = Vec::new();
    for member in tank.archives.iter().take(5) {
        if let Ok(Some(a)) = state.repos.archives.get(member).await {
            member_titles.push(a.title);
        }
    }
    Some(ExistingTankoubon {
        name: tank.name,
        member_titles,
    })
}

/// `POST /tankoubons/ai-group-suggestions/review` — never an HTTP error for a review failure: the
/// card always renders, with the local result when the review could not be had.
pub(super) async fn review_suggestion(
    State(state): State<AppState>,
    Json(req): Json<ReviewRequest>,
) -> Response {
    if req.archive_ids.is_empty() {
        return crate::common::error(
            axum::http::StatusCode::BAD_REQUEST,
            "review_group_suggestion",
            "archive_ids must not be empty",
        );
    }
    let id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
        &req.archive_ids,
        req.existing_tankoubon_id.as_deref(),
    );
    let members = member_facts(&state, &req.archive_ids).await;
    let existing = match &req.existing_tankoubon_id {
        Some(tank_id) => existing_tankoubon(&state, tank_id).await,
        None => None,
    };
    let hints: ReviewHints = super::labels_repo(&state)
        .load_record::<super::SuggestionRecord>(&id)
        .await
        .ok()
        .flatten()
        .map(|r| r.hints)
        .unwrap_or_default();
    let hash = content_hash(&members, existing.as_ref());
    if let Some(review) = cached(&state.redis.config, &id, &hash).await {
        lanrurugi_llm::usage::record_local_cache_hit(
            &state.redis.config,
            FeatureId::TANKOUBON_GROUPING_REVIEW,
        )
        .await;
        return Json(json!({ "suggestion_id": id, "review": review })).into_response();
    }
    let review = review_group(
        &state.redis.config,
        &id,
        &members,
        &hints,
        existing.as_ref(),
        &GroupingSettings::default(),
        true,
    )
    .await;
    Json(json!({ "suggestion_id": id, "review": review })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, title: &str, tags: &[&str]) -> MemberFacts {
        MemberFacts {
            id: id.into(),
            title: title.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn a_shared_creator_or_cosplayer_settles_the_group() {
        let same = [
            member("a", "x", &["artist:one", "parody:p"]),
            member("b", "y", &["artist:one", "circle:c"]),
        ];
        assert_eq!(settled_reason(&same).as_deref(), Some("creator:one"));
        let coser = [
            member("a", "x", &["cosplayer:k", "artist:one"]),
            member("b", "y", &["cosplayer:k", "artist:two"]),
        ];
        assert_eq!(settled_reason(&coser).as_deref(), Some("cosplayer:k"));
        let open = [
            member("a", "x", &["artist:one"]),
            member("b", "y", &["parody:p"]),
        ];
        assert_eq!(settled_reason(&open), None);
    }

    #[test]
    fn compact_input_sends_only_cleaned_titles_creators_and_few_rare_tags() {
        let members = [
            member(
                "a",
                "[Creator One] Story A [中国翻訳] [DL版]",
                &[
                    "artist:creator one",
                    "language:chinese",
                    "language:translated",
                    "parody:p",
                    "date_added:1",
                    "female:x",
                ],
            ),
            member(
                "b",
                "[Creator Two] Story B [DL版]",
                &["artist:creator two", "language:chinese"],
            ),
        ];
        let input = compact_input(
            &members,
            &ReviewHints::default(),
            None,
            &GroupingSettings::default(),
        );
        let text = input.payload.to_string();
        assert!(!text.contains("language:"), "{text}");
        assert!(!text.contains("date_added"), "{text}");
        assert!(!text.contains("中国翻訳"), "{text}");
        assert!(
            !text.contains("\"a\""),
            "archive ids must not be sent: {text}"
        );
        assert_eq!(input.payload["members"][0]["id"], "m1");
        assert_eq!(input.payload["members"][0]["creators"][0], "creator one");
        assert_eq!(input.ids, ["a", "b"]);
    }

    #[test]
    fn a_large_group_sends_only_its_weakest_members() {
        let members: Vec<MemberFacts> = (0..15)
            .map(|i| member(&format!("id{i:02}"), "t", &["parody:p"]))
            .collect();
        let mut hints = ReviewHints::default();
        for (i, m) in members.iter().enumerate() {
            hints.strength.insert(m.id.clone(), i as f32);
        }
        let input = compact_input(&members, &hints, None, &GroupingSettings::default());
        assert_eq!(input.ids.len(), 12);
        // The three strongest (id12–id14) are kept locally, not sent.
        assert!(!input.ids.contains(&"id14".to_string()));
        assert!(input.ids.contains(&"id00".to_string()));
    }

    #[test]
    fn verdicts_must_name_exactly_the_members() {
        let ok = ReviewerAnswer {
            verdicts: vec![
                RawVerdict {
                    id: "m1".into(),
                    keep: true,
                    reason: String::new(),
                },
                RawVerdict {
                    id: "m2".into(),
                    keep: false,
                    reason: String::new(),
                },
            ],
        };
        assert!(validate_verdicts(&ok, 2).is_ok());
        let bad = ReviewerAnswer {
            verdicts: vec![
                RawVerdict {
                    id: "m1".into(),
                    keep: true,
                    reason: String::new(),
                },
                RawVerdict {
                    id: "m1".into(),
                    keep: true,
                    reason: String::new(),
                },
                RawVerdict {
                    id: "m9".into(),
                    keep: true,
                    reason: String::new(),
                },
            ],
        };
        let defect = validate_verdicts(&bad, 2).unwrap_err();
        assert!(
            defect.contains("m9")
                && defect.contains("m1 出现了不止一次")
                && defect.contains("缺少 m2"),
            "{defect}"
        );
    }

    #[test]
    fn kept_drops_only_rejected_members() {
        let review = Review {
            status: ReviewStatus::Reviewed,
            verdicts: vec![Verdict {
                archive_id: "b".into(),
                keep: false,
                reason: "x".into(),
            }],
            reason: None,
            judged: vec!["a".into(), "b".into()],
            prompt_version: "v1".into(),
        };
        let members: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(review.kept(&members), ["a", "c"]);
    }

    #[test]
    fn content_hash_changes_with_titles_and_tags() {
        let a = [member("a", "t1", &["x:1"]), member("b", "t2", &["y:1"])];
        let b = [
            member("a", "t1 edited", &["x:1"]),
            member("b", "t2", &["y:1"]),
        ];
        assert_ne!(content_hash(&a, None), content_hash(&b, None));
        let reordered = [member("b", "t2", &["y:1"]), member("a", "t1", &["x:1"])];
        assert_eq!(content_hash(&a, None), content_hash(&reordered, None));
    }
}
