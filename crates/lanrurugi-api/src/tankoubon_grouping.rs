//! `POST /api/tankoubons/ai-group-suggestions` — analyzes every archive not currently a member of
//! any Tankoubon and suggests groups of them that likely belong together as one, so the Categories
//! page's "AI grouping suggestions" modal can offer "these N archives look like the same series —
//! want to make them a Tankoubon?" without the user having to notice the pattern by hand first.
//!
//! Deliberately local-model-only, no LLM call — this is a title/tag *similarity* judgment (do
//! these two archives look like the same series?), the exact kind of semantic-clustering task the
//! embedding model already handles well (issue #70's `eval_fixture.rs` measured 97.9% top-1
//! same-series accuracy against a real-title fixture using title embeddings alone). An LLM call
//! per archive pair would also be O(n²) API calls for a step that doesn't need an LLM's actual
//! strengths (instruction-following, next-volume ordering) — this endpoint only asks "same series,
//! yes or no", not "what order do these go in" (that's `ai_rename_suggestions`' job, once a group
//! has already become a real Tankoubon). Nothing here requires `hasLlmKey`/`llm_api_key`; the only
//! precondition is the local embedding model being loaded (`state.recommender.ready()`).
//!
//! Similarity score is a 50/50 blend of two signals, not title embedding alone: two archives whose
//! *tags* overlap heavily but whose titles differ a lot (a single-story doujin re-released across
//! different anthology issues, say) are exactly the case pure title-embedding similarity misses —
//! and the reverse (near-identical titles, e.g. two same-named-but-different-artist "Vol. 1"s) is
//! exactly what tag overlap alone would over-merge. `artist:`/`circle:` tags specifically were
//! considered and rejected as a *sole* signal (not blended in as one more Jaccard-eligible
//! namespace, which is what actually happens here) — the same artist's magazine-serialized
//! chapters and their own standalone tankoubon release are routinely NOT the same series/grouping
//! despite sharing an artist tag, so weighting artist tags specially would misgroup that case
//! rather than avoid it; every OTHER tag namespace is treated uniformly in the Jaccard set (no
//! further per-namespace tuning).
//!
//! `category:` is the one namespace that IS special-cased, but as a hard pre-filter rather than a
//! Jaccard-eligible tag: two archives whose `category:` tags are both present and share no value
//! (`category:manga` vs. `category:cosplay`) can never be suggested as a group no matter how high
//! their blended score would otherwise be — see `category_conflict`'s own docs for why this has to
//! be a gate checked before scoring rather than just another tag folded into the overlap set (a
//! handful of other shared tags would otherwise "out-vote" a genuine content-type mismatch).

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Serialize;
use serde_json::json;

use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/tankoubons/ai-group-suggestions",
        post(ai_group_suggestions),
    )
}

/// Same threshold `recommend_precompute.rs`'s `precompute_one` uses to decide whether a newly-
/// embedded archive is similar enough to backfill into another archive's cached Top-N list — reused
/// here for consistency (both are "is this pair the same series?" judgments over the same title-
/// embedding cosine-similarity scale), not re-derived independently.
const SIMILARITY_THRESHOLD: f32 = 0.5;
/// Title-embedding-similarity weight in the blended score (tag-overlap gets the other half — see
/// this module's own docs for why neither signal is used alone).
const TITLE_WEIGHT: f32 = 0.5;
const TAG_WEIGHT: f32 = 0.5;
/// A suggested group of exactly one archive isn't a suggestion (nothing to group it *with*) —
/// matches `duplicates.rs`'s own `group.len() >= 2` filter on its connected components.
const MIN_GROUP_SIZE: usize = 2;

#[derive(Serialize)]
struct GroupSuggestion {
    archive_ids: Vec<String>,
}

/// Parses `namespace:value` tags into a plain set of the whole `"namespace:value"` strings (not
/// split further) — Jaccard similarity over this set treats `rating:5` and `rating:4` as
/// non-overlapping, which is correct (a group-suggestion signal shouldn't treat "differently
/// rated" as "same tag"), and naturally gives more overlap weight to archives sharing several tags
/// than ones sharing only one, without needing per-namespace weighting.
fn tag_set(tags: &str) -> HashSet<&str> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Every `category:`-namespaced value on an archive (legacy allows more than one — e.g. an
/// archive tagged both `category:manga` and `category:doujinshi` — so this is a set, not a single
/// value; see `category_conflict`'s own docs for how a multi-value set is compared).
fn category_values<'a>(tags: &HashSet<&'a str>) -> HashSet<&'a str> {
    tags.iter()
        .filter_map(|t| t.strip_prefix("category:"))
        .collect()
}

/// Hard exclusion: two archives that both carry `category:` tags but share none of them (e.g.
/// `category:manga` vs. `category:cosplay`) can never be suggested as the same group, regardless
/// of how similar their titles/other tags are — this is checked BEFORE the blended similarity
/// score, not folded into the Jaccard set as just one more tag to weigh, because burying a
/// content-type mismatch inside a large tag-overlap score lets enough *other* shared tags out-vote
/// it (an archive pair with 5 shared tags and 1 conflicting category would still score high on
/// Jaccard alone). An archive with NO category tag imposes no constraint either way (`is_empty()`
/// short-circuits to "no conflict") — this is a mismatch check, not a same-category requirement,
/// so untagged archives can still be grouped by title/other-tag similarity alone.
fn category_conflict(a: &HashSet<&str>, b: &HashSet<&str>) -> bool {
    let cats_a = category_values(a);
    let cats_b = category_values(b);
    if cats_a.is_empty() || cats_b.is_empty() {
        return false;
    }
    cats_a.is_disjoint(&cats_b)
}

/// Connected components over the "blended similarity ≥ threshold, AND no category conflict"
/// relation — same stack-based DFS shape as `duplicates.rs`'s `group_by_hamming_distance`, over a
/// different pairwise metric.
fn group_by_similarity(
    ids: &[String],
    vectors: &HashMap<&str, &[f32]>,
    tags: &HashMap<&str, HashSet<&str>>,
) -> Vec<Vec<String>> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut groups = Vec::new();

    for start_id in ids {
        let start_id = start_id.as_str();
        if visited.contains(start_id) {
            continue;
        }
        let mut stack = vec![start_id];
        let mut group = Vec::new();

        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            group.push(node.to_string());

            let Some(node_vec) = vectors.get(node) else {
                continue;
            };
            let empty_tags = HashSet::new();
            let node_tags = tags.get(node).unwrap_or(&empty_tags);

            for other_id in ids {
                let other_id = other_id.as_str();
                if visited.contains(other_id) || other_id == node {
                    continue;
                }
                let Some(other_vec) = vectors.get(other_id) else {
                    continue;
                };
                let other_tags = tags.get(other_id).unwrap_or(&empty_tags);
                if category_conflict(node_tags, other_tags) {
                    continue;
                }
                let title_score =
                    lanrurugi_recommend::embedding::cosine_similarity(node_vec, other_vec);
                let tag_score = jaccard(node_tags, other_tags);
                let blended = TITLE_WEIGHT * title_score + TAG_WEIGHT * tag_score;
                if blended >= SIMILARITY_THRESHOLD {
                    stack.push(other_id);
                }
            }
        }

        if group.len() >= MIN_GROUP_SIZE {
            group.sort();
            groups.push(group);
        }
    }

    groups
}

async fn ai_group_suggestions(State(state): State<AppState>) -> Response {
    if !state.recommender.ready() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({
                "error": "model_not_ready",
                "message": "The recommendation model is still downloading or loading — try again shortly.",
            })),
        )
            .into_response();
    }

    // "Not currently a member of any Tankoubon" is exactly `LRR_TANKGROUPED` membership (despite
    // the legacy-inherited name — see that key's own doc comment in lanrurugi-search::keys) minus
    // the Tankoubons' own aggregate entries (also members of this set, identifiable by their
    // `TANK_` id prefix — real archive ids are 40 lowercase-hex chars and never start with that).
    let mut search_conn = match state.redis.search.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };
    let ungrouped_ids: Vec<String> = match search_conn
        .smembers::<_, Vec<String>>(lanrurugi_search::keys::TANKGROUPED_KEY)
        .await
    {
        Ok(ids) => ids
            .into_iter()
            .filter(|id| !id.starts_with("TANK_"))
            .collect(),
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };

    if ungrouped_ids.len() < MIN_GROUP_SIZE {
        return axum::Json(json!({ "suggestions": Vec::<GroupSuggestion>::new() })).into_response();
    }

    // Title + tags for every ungrouped archive — tags come straight from the archive record
    // (cheap, no separate cache); title *vectors* prefer the persisted recommend-cache (populated
    // by `recommend_precompute.rs` at catalogue/title-change time) and fall back to embedding
    // on-the-spot for any archive that cache doesn't have yet (a fresh install before the initial
    // backfill finishes, or an archive whose title changed since — same graceful-degradation shape
    // `recommend.rs`'s own request path uses).
    let mut tags_by_id: HashMap<String, String> = HashMap::new();
    let mut vectors_by_id: HashMap<String, Vec<f32>> = HashMap::new();
    let mut missing_titles: Vec<(String, String)> = Vec::new();

    for id in &ungrouped_ids {
        let Ok(Some(archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
        else {
            continue;
        };
        tags_by_id.insert(id.clone(), archive.tags.clone());
        match state.recommend_cache.get_vector(id).await {
            Ok(Some((cached_title, vector))) if cached_title == archive.title => {
                vectors_by_id.insert(id.clone(), vector);
            }
            _ => missing_titles.push((id.clone(), archive.title)),
        }
    }

    if !missing_titles.is_empty() {
        let Some(embedder) = state.recommender.embedder() else {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({
                    "error": "model_not_ready",
                    "message": "The recommendation model is still downloading or loading — try again shortly.",
                })),
            )
                .into_response();
        };
        let embedder_for_blocking = embedder.clone();
        let computed: Vec<(String, String, Vec<f32>)> =
            match tokio::task::spawn_blocking(move || {
                missing_titles
                    .into_iter()
                    .filter_map(|(id, title)| {
                        let normalized = lanrurugi_recommend::recommend::normalize_title(&title);
                        embedder_for_blocking
                            .embed(&normalized)
                            .ok()
                            .map(|v| (id, title, v))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    return error(
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        "ai_group_suggestions",
                        e.to_string(),
                    )
                }
            };
        for (id, title, vector) in computed {
            // Best-effort cache the freshly-computed vector for next time — same reasoning as
            // `recommend.rs`'s own miss-path fire-and-forget `precompute_one`, but done inline
            // here (already off the async reactor via spawn_blocking above, and this handler
            // doesn't have a hot per-request latency budget to protect the way the live
            // recommendations endpoint does).
            let _ = state.recommend_cache.put_vector(&id, &title, &vector).await;
            vectors_by_id.insert(id, vector);
        }
    }

    // The borrowed `&str`/`&[f32]` views `group_by_similarity` wants are built INSIDE the blocking
    // closure (not passed in from out here) — `spawn_blocking` requires a `'static` closure, and a
    // `HashMap<&str, _>` borrowing from `vectors_by_id`/`tags_by_id` can't satisfy that from the
    // outside; moving the owned maps in and re-deriving the borrowed views locally sidesteps that
    // instead of fighting the borrow checker with unnecessary cloning.
    let groups = tokio::task::spawn_blocking(move || {
        let vector_refs: HashMap<&str, &[f32]> = vectors_by_id
            .iter()
            .map(|(id, v)| (id.as_str(), v.as_slice()))
            .collect();
        let tag_refs: HashMap<&str, HashSet<&str>> = tags_by_id
            .iter()
            .map(|(id, t)| (id.as_str(), tag_set(t)))
            .collect();
        group_by_similarity(&ungrouped_ids, &vector_refs, &tag_refs)
    })
    .await;
    let groups = match groups {
        Ok(g) => g,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "ai_group_suggestions",
                e.to_string(),
            )
        }
    };

    let suggestions: Vec<GroupSuggestion> = groups
        .into_iter()
        .map(|archive_ids| GroupSuggestion { archive_ids })
        .collect();
    axum::Json(json!({ "suggestions": suggestions })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaccard_of_disjoint_sets_is_zero() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let b: HashSet<&str> = ["category:cosplay", "artist:y"].into_iter().collect();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_of_identical_sets_is_one() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        assert_eq!(jaccard(&a, &a.clone()), 1.0);
    }

    #[test]
    fn jaccard_of_both_empty_is_zero_not_nan() {
        let a: HashSet<&str> = HashSet::new();
        let b: HashSet<&str> = HashSet::new();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_partial_overlap() {
        let a: HashSet<&str> = ["x", "y", "z"].into_iter().collect();
        let b: HashSet<&str> = ["y", "z", "w"].into_iter().collect();
        // intersection {y,z} = 2, union {x,y,z,w} = 4
        assert_eq!(jaccard(&a, &b), 0.5);
    }

    #[test]
    fn category_conflict_detects_disjoint_categories() {
        let a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let b: HashSet<&str> = ["category:cosplay", "artist:x"].into_iter().collect();
        assert!(
            category_conflict(&a, &b),
            "manga vs cosplay must conflict even with a shared artist tag"
        );
    }

    #[test]
    fn category_conflict_false_when_categories_overlap() {
        let a: HashSet<&str> = ["category:manga", "category:doujinshi"]
            .into_iter()
            .collect();
        let b: HashSet<&str> = ["category:doujinshi"].into_iter().collect();
        assert!(
            !category_conflict(&a, &b),
            "sharing at least one category value is not a conflict"
        );
    }

    #[test]
    fn category_conflict_false_when_either_side_has_no_category() {
        let with_category: HashSet<&str> = ["category:manga"].into_iter().collect();
        let without_category: HashSet<&str> = ["artist:x"].into_iter().collect();
        assert!(
            !category_conflict(&with_category, &without_category),
            "an untagged-category archive imposes no constraint"
        );
        assert!(!category_conflict(&without_category, &with_category));
    }

    #[test]
    fn group_by_similarity_only_keeps_groups_of_two_or_more() {
        // Three archives: a-b similar (both title and tags identical → blended score 1.0), c
        // isolated (zero similarity to either).
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![1.0, 0.0];
        let vc: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        vectors.insert("c", &vc);
        let tags_a: HashSet<&str> = ["category:manga"].into_iter().collect();
        let tags_b: HashSet<&str> = ["category:manga"].into_iter().collect();
        let tags_c: HashSet<&str> = HashSet::new();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("a", tags_a);
        tags.insert("b", tags_b);
        tags.insert("c", tags_c);

        let groups = group_by_similarity(&ids, &vectors, &tags);
        assert_eq!(
            groups.len(),
            1,
            "only the a-b pair forms a group, got {groups:?}"
        );
        assert_eq!(groups[0], vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn group_by_similarity_returns_empty_when_nothing_meets_threshold() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![0.0, 1.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags: HashMap<&str, HashSet<&str>> = HashMap::new();

        let groups = group_by_similarity(&ids, &vectors, &tags);
        assert!(groups.is_empty());
    }

    #[test]
    fn group_by_similarity_never_groups_across_a_category_conflict() {
        // a and b have IDENTICAL titles and tags except for category — would blend to a score of
        // 1.0 (well above threshold) if category weren't checked first.
        let ids = vec!["a".to_string(), "b".to_string()];
        let va: Vec<f32> = vec![1.0, 0.0];
        let vb: Vec<f32> = vec![1.0, 0.0];
        let mut vectors: HashMap<&str, &[f32]> = HashMap::new();
        vectors.insert("a", &va);
        vectors.insert("b", &vb);
        let tags_a: HashSet<&str> = ["category:manga", "artist:x"].into_iter().collect();
        let tags_b: HashSet<&str> = ["category:cosplay", "artist:x"].into_iter().collect();
        let mut tags: HashMap<&str, HashSet<&str>> = HashMap::new();
        tags.insert("a", tags_a);
        tags.insert("b", tags_b);

        let groups = group_by_similarity(&ids, &vectors, &tags);
        assert!(
            groups.is_empty(),
            "a category conflict must block grouping even at otherwise-perfect similarity, got {groups:?}"
        );
    }
}
