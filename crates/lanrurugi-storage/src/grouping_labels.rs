//! Labels and records for the tankoubon-grouping harness (`specs/016-tankoubon-grouping-harness`
//! data-model §5–§6). Labels are the user's own accept/reject decisions as archive pairs — the
//! evaluation's ground truth, so they never expire; the user can delete them all. Suggestion
//! records keep what was shown and why, for 90 days.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::ids::GroupSuggestionId;
use serde::{Deserialize, Serialize};

use crate::keys::{
    grouping_suggestion_key, GROUPING_DISAGREEMENTS_KEY, GROUPING_LABELS_KEY,
    GROUPING_SUGGESTION_TTL_SECS,
};

#[derive(Debug, thiserror::Error)]
pub enum GroupingLabelsError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, GroupingLabelsError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairLabelKind {
    Pos,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelSource {
    Created,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairLabel {
    pub label: PairLabelKind,
    pub source: LabelSource,
    pub suggestion_id: GroupSuggestionId,
    pub at: u64,
}

/// One labelled pair, ids sorted.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelledPair {
    pub a: String,
    pub b: String,
    pub label: PairLabel,
}

/// Counts for the label endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct LabelCounts {
    pub positive: usize,
    pub negative: usize,
    pub disagreements: usize,
}

/// The hash field for a pair: ids sorted so `(a, b)` and `(b, a)` are one label.
pub fn pair_field(a: &str, b: &str) -> String {
    if a <= b {
        format!("{a}|{b}")
    } else {
        format!("{b}|{a}")
    }
}

/// Every pair of `ids` with each id once; a pair of an id with itself is never produced.
pub fn pairs<'a>(ids: &'a [String]) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
    ids.iter().enumerate().flat_map(move |(i, a)| {
        ids[i + 1..]
            .iter()
            .filter(move |b| *b != a)
            .map(move |b| (a.as_str(), b.as_str()))
    })
}

pub struct GroupingLabelsRepository {
    pool: Pool,
}

impl GroupingLabelsRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Writes one label per pair; the latest decision about a pair wins.
    pub async fn record_pairs(
        &self,
        pairs: impl IntoIterator<Item = (String, String, PairLabel)>,
    ) -> Result<usize> {
        let mut pipe = deadpool_redis::redis::pipe();
        let mut n = 0;
        for (a, b, label) in pairs {
            if a == b {
                continue;
            }
            let value = serde_json::to_string(&label).unwrap_or_default();
            pipe.hset(GROUPING_LABELS_KEY, pair_field(&a, &b), value)
                .ignore();
            n += 1;
        }
        if n > 0 {
            let mut conn = self.pool.get().await?;
            pipe.query_async::<()>(&mut conn).await?;
        }
        Ok(n)
    }

    /// A member the reviewer rejected that the user kept.
    pub async fn record_disagreement(
        &self,
        suggestion_id: &GroupSuggestionId,
        archive_id: &str,
        at: u64,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let value = serde_json::json!({ "reviewer": "reject", "user": "keep", "at": at });
        let _: () = conn
            .hset(
                GROUPING_DISAGREEMENTS_KEY,
                format!("{suggestion_id}|{archive_id}"),
                value.to_string(),
            )
            .await?;
        Ok(())
    }

    pub async fn counts(&self) -> Result<LabelCounts> {
        let mut conn = self.pool.get().await?;
        let labels: HashMap<String, String> = conn.hgetall(GROUPING_LABELS_KEY).await?;
        let disagreements: usize = conn.hlen(GROUPING_DISAGREEMENTS_KEY).await?;
        let mut counts = LabelCounts {
            disagreements,
            ..Default::default()
        };
        for raw in labels.values() {
            match serde_json::from_str::<PairLabel>(raw).map(|l| l.label) {
                Ok(PairLabelKind::Pos) => counts.positive += 1,
                Ok(PairLabelKind::Neg) => counts.negative += 1,
                Err(_) => {}
            }
        }
        Ok(counts)
    }

    /// Every labelled pair, for the evaluation.
    pub async fn list_all(&self) -> Result<Vec<LabelledPair>> {
        let mut conn = self.pool.get().await?;
        let labels: HashMap<String, String> = conn.hgetall(GROUPING_LABELS_KEY).await?;
        let mut out: Vec<LabelledPair> = labels
            .into_iter()
            .filter_map(|(field, raw)| {
                let (a, b) = field.split_once('|')?;
                let label = serde_json::from_str(&raw).ok()?;
                Some(LabelledPair {
                    a: a.to_string(),
                    b: b.to_string(),
                    label,
                })
            })
            .collect();
        out.sort_by(|x, y| (&x.a, &x.b).cmp(&(&y.a, &y.b)));
        Ok(out)
    }

    /// Deletes every label and disagreement (FR-017).
    pub async fn delete_all(&self) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .del(&[GROUPING_LABELS_KEY, GROUPING_DISAGREEMENTS_KEY])
            .await?;
        Ok(())
    }

    /// Saves a suggestion record (any serializable shape) for 90 days.
    pub async fn save_record<T: Serialize>(
        &self,
        id: &GroupSuggestionId,
        record: &T,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let value = serde_json::to_string(record).unwrap_or_default();
        let _: () = conn
            .set_ex(
                grouping_suggestion_key(id),
                value,
                GROUPING_SUGGESTION_TTL_SECS as u64,
            )
            .await?;
        Ok(())
    }

    /// Saves many records in one round trip.
    pub async fn save_records<T: Serialize>(
        &self,
        records: &[(GroupSuggestionId, T)],
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut pipe = deadpool_redis::redis::pipe();
        for (id, record) in records {
            pipe.set_ex(
                grouping_suggestion_key(id),
                serde_json::to_string(record).unwrap_or_default(),
                GROUPING_SUGGESTION_TTL_SECS as u64,
            )
            .ignore();
        }
        let mut conn = self.pool.get().await?;
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    pub async fn load_record<T: for<'de> Deserialize<'de>>(
        &self,
        id: &GroupSuggestionId,
    ) -> Result<Option<T>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(grouping_suggestion_key(id)).await?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_fields_are_order_independent() {
        assert_eq!(pair_field("b", "a"), pair_field("a", "b"));
        assert_eq!(pair_field("a", "b"), "a|b");
    }

    #[test]
    fn pairs_cover_each_unordered_pair_once() {
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let got: Vec<(&str, &str)> = pairs(&ids).collect();
        assert_eq!(got, [("a", "b"), ("a", "c"), ("b", "c")]);
    }

    fn label(kind: PairLabelKind, source: LabelSource) -> PairLabel {
        PairLabel {
            label: kind,
            source,
            suggestion_id: GroupSuggestionId("s1".into()),
            at: 1,
        }
    }

    /// One test owns both global keys, so nothing else in this binary races it.
    #[tokio::test]
    async fn labels_round_trip_through_redis() {
        let Some(redis) = crate::test_support::test_redis_dbs().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = GroupingLabelsRepository::new(redis.config.clone());
        repo.delete_all().await.unwrap();

        let written = repo
            .record_pairs([
                (
                    "b".into(),
                    "a".into(),
                    label(PairLabelKind::Pos, LabelSource::Created),
                ),
                (
                    "a".into(),
                    "c".into(),
                    label(PairLabelKind::Neg, LabelSource::Created),
                ),
                (
                    "x".into(),
                    "x".into(),
                    label(PairLabelKind::Pos, LabelSource::Created),
                ),
            ])
            .await
            .unwrap();
        assert_eq!(written, 2, "a self-pair is not a label");
        // A later dismissal of the same pair replaces the earlier decision.
        repo.record_pairs([(
            "a".into(),
            "b".into(),
            label(PairLabelKind::Neg, LabelSource::Dismissed),
        )])
        .await
        .unwrap();
        repo.record_disagreement(&GroupSuggestionId("s1".into()), "c", 2)
            .await
            .unwrap();

        let all = repo.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].a.as_str(), all[0].b.as_str()), ("a", "b"));
        assert_eq!(all[0].label.source, LabelSource::Dismissed);
        assert_eq!(
            repo.counts().await.unwrap(),
            LabelCounts {
                positive: 0,
                negative: 2,
                disagreements: 1
            }
        );

        repo.delete_all().await.unwrap();
        assert_eq!(repo.counts().await.unwrap(), LabelCounts::default());
    }
}
