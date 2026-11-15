//! Reading-progress-aware ordering layered on top of embedding similarity.
//!
//! Embedding alone cannot tell "the next volume of the series I just finished" from "an unrelated
//! work that happens to score 0.88". This module derives a coarse reading bucket from the three
//! fields the archive already carries, filters out what a reader has finished, and adds a small
//! bounded boost. It is intentionally a pure function of its inputs so both the cache-hit and the
//! full-recompute paths can share exactly one ordering.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::recommend::{normalize_title, ArchiveMeta, Recommendation};

/// A volume counts as finished at 80% read: the last few pages are often credits or a preview of
/// the next volume, and a reader who stopped there is not coming back for more of this one.
pub const FINISHED_PROGRESS: f32 = 0.80;

pub const UNREAD_BOOST: f32 = 0.12;
pub const READING_BOOST: f32 = 0.06;
pub const UNKNOWN_BOOST: f32 = 0.03;
pub const SERIES_BOOST: f32 = 0.08;
/// State boost plus series boost never exceeds this, so a finished-adjacent work can still be
/// outranked by a clearly more similar one.
pub const MAX_BOOST: f32 = 0.15;

/// Same-series evidence only counts when the embeddings already agree this is a close work; below
/// this the shared `parody:` tag alone is too weak (many unrelated works share a parody tag).
pub const SERIES_MIN_SIMILARITY: f32 = 0.72;

/// How many items the all-finished fallback returns.
pub const FALLBACK_LIMIT: usize = 3;

/// Prefix length used for the "same normalized title" evidence. Long enough to keep a series name
/// intact, short enough that trailing edition notes cannot decide it.
const TITLE_PREFIX_CHARS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadingState {
    pub last_read_page: u32,
    pub page_count: u32,
    pub last_read_time: u64,
}

impl ReadingState {
    pub fn new(last_read_page: u32, page_count: u32, last_read_time: u64) -> Self {
        Self {
            last_read_page,
            page_count,
            last_read_time,
        }
    }

    /// Reading fraction in `0..=1`, or `None` when the page count is unknown.
    pub fn progress(self) -> Option<f32> {
        (self.page_count > 0).then(|| {
            (self.last_read_page.min(self.page_count) as f32 / self.page_count as f32)
                .clamp(0.0, 1.0)
        })
    }

    pub fn bucket(self) -> ReadBucket {
        if self.last_read_time == 0 && self.last_read_page == 0 {
            return ReadBucket::Unread;
        }
        match self.progress() {
            Some(p) if self.last_read_page >= self.page_count && p >= FINISHED_PROGRESS => {
                ReadBucket::Finished
            }
            Some(p) => ReadBucket::Reading(p),
            None => ReadBucket::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadBucket {
    Unread,
    Reading(f32),
    Unknown,
    Finished,
}

impl ReadBucket {
    pub fn boost(self) -> f32 {
        match self {
            ReadBucket::Unread => UNREAD_BOOST,
            ReadBucket::Reading(p) => READING_BOOST * (1.0 - p),
            ReadBucket::Unknown => UNKNOWN_BOOST,
            ReadBucket::Finished => 0.0,
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, ReadBucket::Finished)
    }

    /// Short stable code used inside the progress digest.
    pub fn code(self) -> String {
        match self {
            ReadBucket::Unread => "u".to_string(),
            ReadBucket::Reading(p) => format!("r{}", (p * 100.0).round() as u32),
            ReadBucket::Unknown => "?".to_string(),
            ReadBucket::Finished => "f".to_string(),
        }
    }

    /// Human-readable label for the LLM prompt (Chinese, like the rest of the prompt).
    pub fn prompt_label(self) -> String {
        match self {
            ReadBucket::Unread => "未读".to_string(),
            ReadBucket::Reading(p) => format!("在读 {}%", (p * 100.0).round() as u32),
            ReadBucket::Unknown => "进度未知".to_string(),
            ReadBucket::Finished => "已读完".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub meta: ArchiveMeta,
    pub state: ReadingState,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct RankOutcome {
    pub items: Vec<Recommendation>,
    /// Indices into the input `candidates`, best first — lets a caller re-order the full
    /// candidates (including tags/progress the response does not carry) for the LLM prompt.
    pub order: Vec<usize>,
    /// True only when every candidate was already finished and the similarity top-3 was used.
    /// Callers log it; it never changes the response shape.
    pub fallback_used: bool,
}

/// Orders `candidates` by `similarity + boost`, dropping finished works. Ties break on `id` so the
/// cache-hit and recompute paths cannot disagree. When every candidate is finished, returns the
/// top [`FALLBACK_LIMIT`] by similarity and sets [`RankOutcome::fallback_used`].
pub fn rank(anchor: &ArchiveMeta, candidates: &[RankedCandidate], limit: usize) -> RankOutcome {
    let mut scored: Vec<(f32, usize)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.state.bucket().is_finished())
        .map(|(index, c)| {
            let state_boost = c.state.bucket().boost();
            let series =
                if series_evidence(anchor, &c.meta) && c.similarity >= SERIES_MIN_SIMILARITY {
                    SERIES_BOOST
                } else {
                    0.0
                };
            let boost = (state_boost + series).min(MAX_BOOST);
            // Sort on the raw sum, not a clamped one: two candidates that both overflow 1.0 must
            // still be ordered by their similarity, not by their id (the tie-breaker). The clamp
            // below only shapes the number the client displays.
            (c.similarity + boost, index)
        })
        .collect();

    if scored.is_empty() {
        let mut by_similarity: Vec<usize> = (0..candidates.len()).collect();
        by_similarity.sort_by(|&a, &b| {
            candidates[b]
                .similarity
                .partial_cmp(&candidates[a].similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| candidates[a].meta.id.cmp(&candidates[b].meta.id))
        });
        let order: Vec<usize> = by_similarity
            .into_iter()
            .take(limit.min(FALLBACK_LIMIT))
            .collect();
        return RankOutcome {
            items: order
                .iter()
                .map(|&i| {
                    let c = &candidates[i];
                    Recommendation {
                        id: c.meta.id.clone(),
                        title: c.meta.title.clone(),
                        score: c.similarity.clamp(0.0, 1.0),
                    }
                })
                .collect(),
            order,
            fallback_used: true,
        };
    }

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| candidates[a.1].meta.id.cmp(&candidates[b.1].meta.id))
    });

    let chosen: Vec<(f32, usize)> = scored.into_iter().take(limit).collect();
    RankOutcome {
        items: chosen
            .iter()
            .map(|&(score, i)| {
                let c = &candidates[i];
                Recommendation {
                    id: c.meta.id.clone(),
                    title: c.meta.title.clone(),
                    score: score.clamp(0.0, 1.0),
                }
            })
            .collect(),
        order: chosen.iter().map(|&(_, i)| i).collect(),
        fallback_used: false,
    }
}

/// True when the two works share an `artist:`/`circle:`/`parody:` tag, or their normalized titles
/// start with the same [`TITLE_PREFIX_CHARS`] characters.
pub fn series_evidence(anchor: &ArchiveMeta, candidate: &ArchiveMeta) -> bool {
    shared_creator_tag(&anchor.tags, &candidate.tags)
        || shared_title_prefix(&anchor.title, &candidate.title)
}

fn shared_creator_tag(a: &str, b: &str) -> bool {
    let left = creator_tags(a);
    if left.is_empty() {
        return false;
    }
    creator_tags(b)
        .iter()
        .any(|tag| left.iter().any(|other| other == tag))
}

fn creator_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .filter_map(|tag| {
            let (namespace, value) = tag.split_once(':')?;
            let namespace = namespace.trim().to_ascii_lowercase();
            if !matches!(namespace.as_str(), "artist" | "circle" | "parody") {
                return None;
            }
            let value = value.trim().to_lowercase();
            (!value.is_empty()).then(|| format!("{namespace}:{value}"))
        })
        .collect()
}

fn shared_title_prefix(a: &str, b: &str) -> bool {
    let a = normalize_title(a).to_lowercase();
    let b = normalize_title(b).to_lowercase();
    let a: String = a.chars().take(TITLE_PREFIX_CHARS).collect();
    let b: String = b.chars().take(TITLE_PREFIX_CHARS).collect();
    // A 2-character title would otherwise match half the library; require some substance.
    a.chars().count() >= 6 && a == b
}

/// Digest of the anchor bucket plus the ordered candidate buckets — the rerank answer depends on
/// both, so a progress change must not reuse the previous ordering.
pub fn progress_digest(anchor: ReadBucket, ordered: &[(&str, ReadBucket)]) -> String {
    let mut hasher = DefaultHasher::new();
    anchor.code().hash(&mut hasher);
    for (id, bucket) in ordered {
        b'\n'.hash(&mut hasher);
        id.hash(&mut hasher);
        bucket.code().hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, title: &str, tags: &str) -> ArchiveMeta {
        ArchiveMeta {
            id: id.to_string(),
            title: title.to_string(),
            tags: tags.to_string(),
            pagecount: 40,
        }
    }

    fn cand(
        id: &str,
        title: &str,
        tags: &str,
        similarity: f32,
        state: ReadingState,
    ) -> RankedCandidate {
        RankedCandidate {
            meta: meta(id, title, tags),
            state,
            similarity,
        }
    }

    const FINISHED: ReadingState = ReadingState {
        last_read_page: 40,
        page_count: 40,
        last_read_time: 100,
    };
    const UNREAD: ReadingState = ReadingState {
        last_read_page: 0,
        page_count: 40,
        last_read_time: 0,
    };
    const READING_25: ReadingState = ReadingState {
        last_read_page: 10,
        page_count: 40,
        last_read_time: 100,
    };
    const UNKNOWN: ReadingState = ReadingState {
        last_read_page: 3,
        page_count: 0,
        last_read_time: 100,
    };

    #[test]
    fn buckets_cover_the_edges() {
        assert_eq!(UNREAD.bucket(), ReadBucket::Unread);
        assert_eq!(FINISHED.bucket(), ReadBucket::Finished);
        assert_eq!(UNKNOWN.bucket(), ReadBucket::Unknown);
        assert_eq!(READING_25.bucket(), ReadBucket::Reading(0.25));
        // Reaching the last page is required; stopping at 80% short of it is still "reading".
        assert_eq!(ReadingState::new(40, 40, 1).bucket(), ReadBucket::Finished);
        match ReadingState::new(32, 40, 1).bucket() {
            ReadBucket::Reading(p) => assert!((p - 0.8).abs() < 1e-5),
            other => panic!("expected Reading, got {other:?}"),
        }
        match ReadingState::new(31, 40, 1).bucket() {
            ReadBucket::Reading(p) => assert!((p - 0.775).abs() < 1e-5),
            other => panic!("expected Reading, got {other:?}"),
        }
    }

    #[test]
    fn state_boost_matches_the_spec() {
        assert!((ReadBucket::Unread.boost() - 0.12).abs() < 1e-6);
        assert!((ReadBucket::Unknown.boost() - 0.03).abs() < 1e-6);
        assert!((ReadBucket::Reading(0.5).boost() - 0.03).abs() < 1e-6);
        assert_eq!(ReadBucket::Finished.boost(), 0.0);
    }

    #[test]
    fn series_evidence_uses_creator_tags_and_title_prefix() {
        let anchor = meta("a", "[社团] 夏妻 第1巻", "artist:someone,parody:original");
        assert!(series_evidence(
            &anchor,
            &meta("b", "[社团] 夏妻 第2巻", "artist:someone")
        ));
        assert!(series_evidence(
            &anchor,
            &meta("c", "[社团] 夏妻 第3巻", "parody:original")
        ));
        assert!(!series_evidence(
            &anchor,
            &meta("d", "完全に別の作品", "artist:other")
        ));
        // Title prefixes match only past the substance floor.
        assert!(series_evidence(
            &meta(
                "e",
                "とても長い同名シリーズ作品のタイトルをここに書きます 上",
                ""
            ),
            &meta(
                "f",
                "とても長い同名シリーズ作品のタイトルをここに書きます 下",
                ""
            )
        ));
        assert!(!series_evidence(&meta("g", "短", ""), &meta("h", "短", "")));
    }

    #[test]
    fn finished_candidates_are_dropped_and_series_unread_wins() {
        let anchor = meta("anchor", "夏妻 第1巻", "artist:someone");
        let candidates = vec![
            cand("read", "夏妻 第1巻", "artist:someone", 0.99, FINISHED),
            cand("next", "夏妻 第2巻", "artist:someone", 0.96, UNREAD),
            cand("other", "別の作品", "artist:other", 0.88, UNREAD),
        ];
        let out = rank(&anchor, &candidates, 10);
        assert!(!out.fallback_used);
        assert_eq!(
            out.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["next", "other"]
        );
    }

    /// Two candidates can both exceed 1.0 after the boost (similarity ≥ 0.85 is common for
    /// same-series volumes). The sort must still follow similarity; clamping the sort key first
    /// would turn that into an id-alphabetical tie.
    #[test]
    fn a_boost_past_one_does_not_scramble_the_similarity_order() {
        let anchor = meta("anchor", "x", "");
        let candidates = vec![
            cand("a", "a", "", 0.97, UNREAD),
            cand("b", "b", "", 0.99, UNREAD),
            cand("c", "c", "", 0.95, READING_25),
        ];
        let out = rank(&anchor, &candidates, 10);
        assert_eq!(
            out.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a", "c"]
        );
        // The reported score is still a percentage, i.e. never above 1.0.
        assert!(out.items.iter().all(|r| r.score <= 1.0));
    }

    /// The cache-hit and the recompute paths hand `rank` the same candidates in different orders,
    /// so the result must be independent of the input order.
    #[test]
    fn the_order_does_not_depend_on_the_input_order() {
        let anchor = meta("anchor", "x", "");
        let forward = vec![
            cand("a", "a", "", 0.90, UNREAD),
            cand("b", "b", "", 0.90, READING_25),
            cand("c", "c", "", 0.70, UNKNOWN),
            cand("d", "d", "", 0.70, UNREAD),
        ];
        let mut backward = forward.clone();
        backward.reverse();
        let ids = |cs: &[RankedCandidate]| {
            rank(&anchor, cs, 10)
                .items
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&forward), ids(&backward));
    }

    #[test]
    fn ties_break_on_id() {
        let anchor = meta("anchor", "x", "");
        let candidates = vec![
            cand("b", "x2", "", 0.5, UNREAD),
            cand("a", "x3", "", 0.5, UNREAD),
        ];
        let out = rank(&anchor, &candidates, 10);
        assert_eq!(
            out.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn all_finished_falls_back_to_three_by_similarity() {
        let anchor = meta("anchor", "x", "");
        let candidates = vec![
            cand("a", "a", "", 0.9, FINISHED),
            cand("b", "b", "", 0.8, FINISHED),
            cand("c", "c", "", 0.7, FINISHED),
            cand("d", "d", "", 0.6, FINISHED),
        ];
        let out = rank(&anchor, &candidates, 10);
        assert!(out.fallback_used);
        assert_eq!(
            out.items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn digest_tracks_progress_and_order() {
        let a = progress_digest(
            ReadBucket::Unread,
            &[("x", ReadBucket::Unread), ("y", ReadBucket::Reading(0.4))],
        );
        let same = progress_digest(
            ReadBucket::Unread,
            &[("x", ReadBucket::Unread), ("y", ReadBucket::Reading(0.4))],
        );
        let progressed = progress_digest(
            ReadBucket::Unread,
            &[("x", ReadBucket::Unread), ("y", ReadBucket::Reading(0.5))],
        );
        let reordered = progress_digest(
            ReadBucket::Unread,
            &[("y", ReadBucket::Reading(0.4)), ("x", ReadBucket::Unread)],
        );
        assert_eq!(a, same);
        assert_ne!(a, progressed);
        assert_ne!(a, reordered);
        assert_eq!(a.len(), 16);
    }
}
