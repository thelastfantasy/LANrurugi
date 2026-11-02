//! Offline evaluation (`specs/016-tankoubon-grouping-harness` US5, contracts/eval-cli.md): replay
//! the production grouping over the user's labelled archives under current and candidate
//! settings, and report pair precision and recall per stage. Read-only for library data and
//! labels; `Live` review may add review-cache entries, nothing else.

use std::collections::{HashMap, HashSet};

use lanrurugi_storage::grouping_labels::{GroupingLabelsRepository, PairLabelKind};
use lanrurugi_storage::redis::RedisDbs;
use serde::Serialize;

use super::review::{self, MemberFacts};
use super::{
    group_evidence, group_with, review_hints, tag_set, title_tokens_for, GroupingSettings, Scorer,
};

/// How the review stage is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalReview {
    /// Local stage only.
    Off,
    /// Apply cached reviews; uncached groups count as unreviewed.
    Cached,
    /// May call the provider for uncached groups (and caches the results).
    Live,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StageMetrics {
    /// Labelled pairs the stage put in one group.
    pub proposed_labelled: usize,
    /// Of those, labelled positive.
    pub true_positive: usize,
    /// Labelled positive pairs overall.
    pub positives: usize,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VariantReport {
    pub name: String,
    pub settings: GroupingSettings,
    pub groups: usize,
    pub local: StageMetrics,
    pub pipeline: Option<StageMetrics>,
    /// Groups reviewed by the provider during this run (each review is one or two calls).
    pub live_reviews: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvaluationReport {
    pub positive_labels: usize,
    pub negative_labels: usize,
    /// Labelled archives left out because they no longer exist or have no cached title vector.
    pub skipped_archives: usize,
    pub variants: Vec<VariantReport>,
}

/// Pair precision/recall of `groups` against `labels` (`(a, b)` sorted → positive?).
pub fn pair_metrics(
    groups: &[Vec<String>],
    labels: &HashMap<(String, String), bool>,
) -> StageMetrics {
    let mut together: HashSet<(String, String)> = HashSet::new();
    for group in groups {
        for (i, a) in group.iter().enumerate() {
            for b in &group[i + 1..] {
                together.insert(sorted(a, b));
            }
        }
    }
    let positives = labels.values().filter(|&&p| p).count();
    let mut m = StageMetrics {
        positives,
        ..Default::default()
    };
    for (pair, &positive) in labels {
        if together.contains(pair) {
            m.proposed_labelled += 1;
            if positive {
                m.true_positive += 1;
            }
        }
    }
    m.precision =
        (m.proposed_labelled > 0).then(|| m.true_positive as f64 / m.proposed_labelled as f64);
    m.recall = (positives > 0).then(|| m.true_positive as f64 / positives as f64);
    m
}

fn sorted(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Runs the evaluation. Returns `Ok(None)` when there are no labels to measure against.
pub async fn evaluate(
    redis: &RedisDbs,
    candidate: Option<GroupingSettings>,
    review_mode: EvalReview,
) -> Result<Option<EvaluationReport>, String> {
    let labels = GroupingLabelsRepository::new(redis.config.clone())
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    if labels.is_empty() {
        return Ok(None);
    }
    let label_map: HashMap<(String, String), bool> = labels
        .iter()
        .map(|l| (sorted(&l.a, &l.b), l.label.label == PairLabelKind::Pos))
        .collect();
    let mut ids: Vec<String> = labels
        .iter()
        .flat_map(|l| [l.a.clone(), l.b.clone()])
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    ids.sort();

    let archives = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    let vectors_repo =
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone());
    let mut titles: HashMap<String, String> = HashMap::new();
    let mut tags: HashMap<String, String> = HashMap::new();
    let mut vectors: HashMap<String, Vec<f32>> = HashMap::new();
    let mut skipped = 0;
    for id in &ids {
        let archive = archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
            .ok()
            .flatten();
        let vector = vectors_repo.get_vector(id).await.ok().flatten();
        match (archive, vector) {
            (Some(a), Some((_, v))) => {
                titles.insert(id.clone(), a.title);
                tags.insert(id.clone(), a.tags);
                vectors.insert(id.clone(), v);
            }
            _ => skipped += 1,
        }
    }
    let usable: Vec<String> = ids
        .into_iter()
        .filter(|id| vectors.contains_key(id))
        .collect();

    let mut variants = vec![("current".to_string(), GroupingSettings::default())];
    if let Some(c) = candidate {
        variants.push(("candidate".to_string(), c));
    }

    let mut reports = Vec::new();
    for (name, settings) in variants {
        let vector_refs: HashMap<&str, &[f32]> = vectors
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        let tag_refs: HashMap<&str, HashSet<&str>> =
            tags.iter().map(|(k, t)| (k.as_str(), tag_set(t))).collect();
        let token_refs: HashMap<&str, HashSet<&str>> = titles
            .iter()
            .map(|(k, t)| (k.as_str(), title_tokens_for(t, &settings)))
            .collect();
        let scorer = Scorer::new(&vector_refs, &tag_refs, &token_refs, &settings);
        let groups = group_with(&usable, &scorer);
        let with_hints: Vec<(Vec<String>, review::ReviewHints)> = groups
            .iter()
            .map(|g| {
                let evidence = group_evidence(g, &scorer);
                (g.clone(), review_hints(g, &evidence, &scorer))
            })
            .collect();
        let local = pair_metrics(&groups, &label_map);

        let mut live_reviews = 0;
        let pipeline = if review_mode == EvalReview::Off {
            None
        } else {
            let mut reviewed_groups = Vec::new();
            for (group, hints) in &with_hints {
                let facts: Vec<MemberFacts> = group
                    .iter()
                    .map(|id| MemberFacts {
                        id: id.clone(),
                        title: titles.get(id).cloned().unwrap_or_default(),
                        tags: review::split_tags(tags.get(id).map(String::as_str).unwrap_or("")),
                    })
                    .collect();
                let sid = lanrurugi_storage::ignored_group_suggestions::suggestion_id(group, None);
                let hash = review::content_hash(&facts, None);
                let was_cached = review::cached(&redis.config, &sid, &hash).await.is_some();
                let result = review::review_group(
                    &redis.config,
                    &sid,
                    &facts,
                    hints,
                    None,
                    &settings,
                    review_mode == EvalReview::Live,
                )
                .await;
                if !was_cached
                    && matches!(
                        result.status,
                        review::ReviewStatus::Reviewed | review::ReviewStatus::Repaired
                    )
                {
                    live_reviews += 1;
                }
                let kept = result.kept(group);
                if kept.len() >= 2 {
                    reviewed_groups.push(kept);
                }
            }
            Some(pair_metrics(&reviewed_groups, &label_map))
        };

        reports.push(VariantReport {
            name,
            settings,
            groups: groups.len(),
            local,
            pipeline,
            live_reviews,
        });
    }

    Ok(Some(EvaluationReport {
        positive_labels: label_map.values().filter(|&&p| p).count(),
        negative_labels: label_map.values().filter(|&&p| !p).count(),
        skipped_archives: skipped,
        variants: reports,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(v: &[(&str, &str, bool)]) -> HashMap<(String, String), bool> {
        v.iter().map(|(a, b, p)| (sorted(a, b), *p)).collect()
    }

    fn group(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn precision_and_recall_count_labelled_pairs_only() {
        let l = labels(&[("a", "b", true), ("a", "c", false), ("d", "e", true)]);
        // Groups a-b-c (one right pair, one wrong pair, b-c unlabelled); d-e missed.
        let m = pair_metrics(&[group(&["a", "b", "c"])], &l);
        assert_eq!(m.proposed_labelled, 2);
        assert_eq!(m.true_positive, 1);
        assert_eq!(m.positives, 2);
        assert_eq!(m.precision, Some(0.5));
        assert_eq!(m.recall, Some(0.5));
    }

    #[test]
    fn no_proposals_means_no_precision_rather_than_a_perfect_score() {
        let l = labels(&[("a", "b", true)]);
        let m = pair_metrics(&[], &l);
        assert_eq!(m.precision, None);
        assert_eq!(m.recall, Some(0.0));
    }
}
