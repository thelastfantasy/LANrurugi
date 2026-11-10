//! Data shapes for duplicate detection and review (see `specs/024-duplicate-review/data-model.md`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairEvidence {
    pub a: String,
    pub b: String,
    pub distance: u32,
    pub similarity: f64,
    pub title_similarity: f64,
    pub pages_ratio: f64,
    pub size_ratio: f64,
    pub shared_tags: Vec<String>,
    #[serde(default)]
    pub chained: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    pub algorithm: String,
    pub threshold: u32,
    pub built_at: i64,
    pub members: Vec<String>,
    pub pairs: Vec<PairEvidence>,
    #[serde(default)]
    pub chained: bool,
    #[serde(default)]
    pub middle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReviewVerdict {
    pub verdict: String,
    pub keep: Vec<String>,
    pub discard: Vec<String>,
    pub reason: String,
    pub confidence: f64,
    pub model: String,
    pub prompt_version: u32,
    #[serde(default)]
    pub fallback: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DecisionRecord {
    #[serde(default)]
    pub decisions: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub not_duplicate: Vec<[String; 2]>,
    #[serde(default)]
    pub updated_at: i64,
}

impl DecisionRecord {
    /// A member with no explicit decision is kept.
    pub fn state(&self, id: &str) -> &str {
        self.decisions.get(id).map(String::as_str).unwrap_or("keep")
    }

    pub fn rejects(&self, a: &str, b: &str) -> bool {
        self.not_duplicate
            .iter()
            .any(|[x, y]| (x == a && y == b) || (x == b && y == a))
    }
}

/// Compact metadata the detector and the reviewer both need.
#[derive(Debug, Clone)]
pub struct ArchiveMeta {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub pages: u32,
    pub size: u64,
}

impl ArchiveMeta {
    pub fn artists(&self) -> Vec<&str> {
        tag_values(&self.tags, "artist")
    }

    pub fn circles(&self) -> Vec<&str> {
        tag_values(&self.tags, "circle")
    }

    pub fn language(&self) -> String {
        tag_values(&self.tags, "language")
            .first()
            .map(|s| (*s).to_string())
            .unwrap_or_default()
    }

    /// Artist and circle as one set: a different circle with the same artist is still a different
    /// release, and vice versa.
    pub fn creators(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tag in &self.tags {
            let lower = tag.to_ascii_lowercase();
            if lower.starts_with("artist:") || lower.starts_with("circle:") {
                out.push(lower);
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

fn tag_values<'a>(tags: &'a [String], namespace: &str) -> Vec<&'a str> {
    let prefix = format!("{namespace}:");
    tags.iter()
        .filter_map(|t| t.strip_prefix(&prefix))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect()
}
