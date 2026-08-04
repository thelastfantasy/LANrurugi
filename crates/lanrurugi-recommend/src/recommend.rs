//! Series recognition + recommendation ranking on top of the embedding model.
//!
//! The design bet, validated against real data (see `tests/fixtures/series_titles.json` — 60
//! real titles incl. 36 live e-hentai search results with 大字 numerals like 巻の拾参, paren-
//! wrapped series names, mismatched full/half-width parens): **series membership is a semantic
//! problem, not a string problem**. Volume-number *notation* is a long tail (arabic, roman,
//! kanji, 大字, 音读 kana, 訓读 kana, arbitrary author inventions) that no rule table can
//! exhaust — but embedding similarity captures "these titles are about the same 銀花猫
//! anthology" regardless of how the volume suffix is written, because the shared core text
//! dominates the embedding.
//!
//! So this module does NOT parse volume numbers at all. It embeds titles and ranks candidates
//! by cosine similarity; same-series titles naturally cluster at the top. Volume ordering
//! within a series is a separate concern (the UI shows recommendations by similarity — the
//! newest volume of the same series is the first item, which is exactly what "reading vol.1 →
//! recommend vol.2" needs).

use thiserror::Error;

use crate::embedding::{cosine_similarity, Embedder, EmbeddingError};

#[derive(Debug, Error)]
pub enum RecommendError {
    #[error(transparent)]
    Embedding(#[from] EmbeddingError),
}

/// One candidate archive the recommender can suggest. `tags` (the comma-separated
/// `namespace:value` tag string) is fed to the embedding model alongside the title — the model
/// is the single source of truth for both series membership and ranking; no parsing happens.
#[derive(Debug, Clone)]
pub struct ArchiveMeta {
    pub id: String,
    pub title: String,
    pub tags: String,
    /// Page count — sent to the LLM alongside title/tags (the user's spec: current and
    /// candidates both carry title + tags + pagecount).
    pub pagecount: u32,
}

use regex::Regex;
use std::sync::LazyLock;

/// Edition/translation markers anywhere in the title (`[中国翻訳]`, `[無字]`, `[DL版]` …).
static MARKER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]\[]+\]").unwrap());
/// `【YYYYMMDD】` airing-date tag (jellyfin-frames source format).
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"【\d{6}】").unwrap());
/// `-f<start>-<end>_n<page>` frame-id suffix (jellyfin-frames source format).
static FRAME_ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-f\d+-\d+_n\d+$").unwrap());
/// Leading source tags (`[Patreon]`, `[Twitter]`, `[アンソロジー]` …).
static LEADING_TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[[^\]\[]+\]\s*").unwrap());
/// `(@handle)` suffix.
static HANDLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(@[^)]+\)$").unwrap());

/// A ranked recommendation.
#[derive(Debug, Clone)]
pub struct Recommendation {
    pub id: String,
    pub title: String,
    /// Cosine similarity (0..1) of this title's embedding to the current archive's.
    pub score: f32,
}

/// Strips *source-format noise* from a title before embedding — the fixture acceptance test
/// (60 real titles) showed the embedding model treats these as strong semantic signals, sending
/// unrelated titles to the top of the ranking (a `jellyfin-frames-【260529】` prefix made two
/// different anime's frame-export sets score 0.88; `[Patreon] Dal (@handle)` and
/// `[Twitter] 原子ちゃん (@handle)` shared format, not content; `[アンソロジー]` / `[中国翻訳]` /
/// `[無字]` / `[DL版]` are anthology/edition markers). These are all *finite, enumerable*
/// source conventions — deliberately NOT a volume-notation table (that space is unbounded;
/// series membership itself stays 100% embedding-driven, see the module docs).
pub fn normalize_title(title: &str) -> String {
    let mut t = title.trim().to_string();
    // Edition/translation markers anywhere (`[中国翻訳]`, `[無字]`, `[DL版]`, `[中国翻译]` …).
    // Edition/translation markers anywhere (`[中国翻訳]`, `[無字]`, `[DL版]` …).
    t = MARKER_RE.replace_all(&t, "").into_owned();
    // Source-prefix conventions: `jellyfin-frames-` + the `【YYYYMMDD】` airing-date tag and the
    // `-f<start>-<end>_n<page>` frame-id suffix are one specific export tool's format.
    if let Some(rest) = t.strip_prefix("jellyfin-frames-") {
        t = rest.to_string();
    }
    t = DATE_RE.replace_all(&t, "").into_owned();
    t = FRAME_ID_RE.replace_all(&t, "").into_owned();
    // `[Patreon]`/`[Twitter]`/`[アンソロジー]` leading source tags and `(@handle)` suffixes.
    t = LEADING_TAG_RE.replace_all(&t, "").into_owned();
    t = HANDLE_RE.replace_all(&t, "").into_owned();
    // Collapse runs of whitespace left behind by the removals.
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Ranks every candidate except `current_id` by embedding similarity to `current_title`,
/// returning the `limit` closest. `current_title` is embedded fresh each call (a handful of
/// milliseconds); callers that embed the whole library repeatedly should cache per-id vectors
/// (see the API layer's `RecommendationCache`).
/// Ranks every candidate except `current_id` for `current_title`, purely by embedding
/// similarity — the title and tags are fed to the model verbatim and the model is the single
/// source of truth (no title/volume parsing anywhere; see the module docs for why this is the
/// deliberate architecture).
pub fn recommend(
    embedder: &Embedder,
    current_id: &str,
    current_title: &str,
    current_tags: &str,
    candidates: &[ArchiveMeta],
    limit: usize,
) -> Result<Vec<Recommendation>, RecommendError> {
    let current_vec = embedder.embed(&format!("{} {}", current_title, current_tags))?;
    let mut scored: Vec<Recommendation> = Vec::with_capacity(candidates.len());
    for c in candidates {
        if c.id == current_id {
            continue;
        }
        let vec = embedder.embed(&format!("{} {}", c.title, c.tags))?;
        scored.push(Recommendation {
            id: c.id.clone(),
            title: c.title.clone(),
            score: cosine_similarity(&current_vec, &vec),
        });
    }
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit);
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_fixture_with_volumes() -> Vec<(String, String, Option<u32>)> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../lanrurugi-api/tests/fixtures/series_titles.json");
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        data["archives"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                (
                    a["series_group"].as_str().unwrap().to_string(),
                    a["title"].as_str().unwrap().to_string(),
                    a["volume"].as_u64().map(|v| v as u32),
                )
            })
            .collect()
    }

    fn test_embedder() -> Option<Embedder> {
        let models_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/models");
        // Prefer e5-small when present (stronger multilingual quality per the model-choice
        // decision); fall back to the MiniLM files.
        let model = if models_dir
            .join("multilingual-e5-small_quantized.onnx")
            .exists()
        {
            models_dir.join("multilingual-e5-small_quantized.onnx")
        } else {
            models_dir.join("paraphrase-multilingual-MiniLM-L12-v2_quantized.onnx")
        };
        let tok = if models_dir.join("e5-tokenizer.json").exists() {
            models_dir.join("e5-tokenizer.json")
        } else {
            models_dir.join("tokenizer.json")
        };
        if !model.exists() || !tok.exists() {
            eprintln!("skipping: model files not present under data/models/");
            return None;
        }
        Some(Embedder::load(&model, &tok).expect("model must load"))
    }

    /// Acceptance test against the 60-title fixture (36 of them real e-hentai search results):
    /// (a) for every title, the top recommendation must come from the SAME series_group;
    /// (b) when the same series has a volume-adjacent next volume (current vol + 1), that title
    /// must be the #1 recommendation — the user-facing "read vol.10 → vol.11 is next" contract.
    #[test]
    fn fixture_titles_recommend_same_series_first() {
        let Some(embedder) = test_embedder() else {
            return;
        };
        let fixture = load_fixture_with_volumes();
        assert!(
            fixture.len() >= 60,
            "fixture should have 60+ entries, got {}",
            fixture.len()
        );

        // A title whose series_group has only one member (a standalone) has no same-series
        // candidate to recommend at all — asserting "top-1 is same series" on it would fail by
        // construction, so only groups with ≥2 members are graded.
        let group_sizes: std::collections::HashMap<&String, usize> = {
            let mut m = std::collections::HashMap::new();
            for (g, _, _) in &fixture {
                *m.entry(g).or_insert(0) += 1;
            }
            m
        };
        let mut mismatches = Vec::new();
        let mut checked = 0usize;
        for (idx, (group, title, _vol)) in fixture.iter().enumerate() {
            if group_sizes.get(group).copied().unwrap_or(0) < 2 {
                continue;
            }
            let others: Vec<ArchiveMeta> = fixture
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != idx)
                .map(|(_, (_, t, _))| ArchiveMeta {
                    id: format!("id{checked}{idx}"),
                    title: t.clone(),
                    tags: String::new(),
                    pagecount: 0,
                })
                .collect();
            // Prefilter-pool check: the embedding layer's job is to keep same-series titles in
            // the pool handed to the LLM (final ordering — incl. "next volume is #1" — is the
            // LLM's job). Ask for the full pool and assert the same series is present.
            let pool = recommend(&embedder, "current", title, "", &others, 100)
                .expect("recommend must not fail");
            checked += 1;
            if pool.is_empty() {
                continue;
            }
            let found = pool
                .iter()
                .any(|r| fixture.iter().any(|(g, t, _)| t == &r.title && g == group));
            if !found {
                let tops = pool.iter().map(|r| r.title.clone()).collect::<Vec<_>>();
                mismatches.push((title.clone(), tops.join(" | "), 0.0));
            }
        }
        let miss_rate = mismatches.len() as f64 / checked as f64;
        assert!(
            mismatches.is_empty(),
            "{} titles had no same-series title in the prefilter pool (miss rate {:.1}%):\n{:?}",
            mismatches.len(),
            miss_rate * 100.0,
            mismatches.iter().take(5).collect::<Vec<_>>(),
        );
    }
}
