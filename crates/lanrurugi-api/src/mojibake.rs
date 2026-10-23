//! Retroactive repair of doubly-encoded ("mojibake") metadata — issue #115.
//!
//! The legacy-import path already repairs this on the way in, but that repair only landed
//! 2026-08-29 (`lanrurugi_backup::import_legacy::repair_mojibake`'s own docs describe the real
//! historical LANraragi data-quality issue it addresses). Records imported before it — a real
//! Tankoubon in the maintainer's own library among them — kept their mangled text with no way to
//! fix it short of re-running the whole import. This module is the missing retroactive half: the
//! same deliberately strict detector, run over records that are already stored.
//!
//! **Report first, write only on request.** [`run`] with `apply: false` returns every candidate
//! without touching anything; the endpoint exposes that as a dry run and the UI shows the
//! before/after pairs before asking. The detector's own accuracy requirement (a false-positive
//! "repair" corrupts correct text) is exactly why nothing is rewritten unasked.

use serde::Serialize;

use crate::AppState;

/// One repairable field. `before`/`after` travel with the finding so a caller can show precisely
/// what a repair would change — and so the applied run reports what it actually wrote.
#[derive(Debug, Clone, Serialize)]
pub struct RepairFinding {
    /// `archive` / `category` / `tankoubon` / `stamp`.
    pub kind: &'static str,
    pub id: String,
    pub field: &'static str,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Default, Serialize)]
pub struct RepairReport {
    /// Records examined (all four kinds together).
    pub scanned: usize,
    pub findings: Vec<RepairFinding>,
    /// Whether this run wrote the repairs or only reported them.
    pub applied: bool,
    pub repaired_fields: usize,
}

/// Rewrites `value` in place when it is double-encoded, recording the change.
fn repair_field(
    findings: &mut Vec<RepairFinding>,
    kind: &'static str,
    id: &str,
    field: &'static str,
    value: &mut String,
) {
    let Some(repaired) = lanrurugi_backup::import_legacy::repair_mojibake(value) else {
        return;
    };
    findings.push(RepairFinding {
        kind,
        id: id.to_string(),
        field,
        before: value.clone(),
        after: repaired.clone(),
    });
    *value = repaired;
}

/// Scans every metadata record for double-encoded text and, when `apply` is set, writes the
/// repairs back — keeping the search indexes in step with any title/tag change, the same way the
/// normal metadata-update paths do.
///
/// Tankoubon `updated_at` is deliberately left alone: a repair is not a content edit, and bumping
/// it would reshuffle every date-sorted library view for a fix the user did not make.
pub async fn run(state: &AppState, apply: bool) -> Result<RepairReport, String> {
    let mut report = RepairReport {
        applied: apply,
        ..Default::default()
    };

    let archives = state
        .repos
        .archives
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    let mut stamp_ids = Vec::new();
    for mut archive in archives {
        report.scanned += 1;
        stamp_ids.extend(archive.stamp_ids.iter().cloned());
        let id = archive.id.to_string();
        let old_title = archive.title.clone();
        let old_tags = archive.tags.clone();
        let mut touched = Vec::new();
        repair_field(&mut touched, "archive", &id, "name", &mut archive.name);
        repair_field(&mut touched, "archive", &id, "title", &mut archive.title);
        repair_field(
            &mut touched,
            "archive",
            &id,
            "summary",
            &mut archive.summary,
        );
        repair_field(&mut touched, "archive", &id, "tags", &mut archive.tags);
        if touched.is_empty() {
            continue;
        }
        if apply {
            state
                .repos
                .archives
                .save(&archive)
                .await
                .map_err(|e| e.to_string())?;
            if archive.title != old_title {
                if let Err(e) = lanrurugi_search::indexer::update_title_index(
                    &state.redis.search,
                    &state.equivalence,
                    &archive.id,
                    &old_title,
                    &archive.title,
                )
                .await
                {
                    tracing::warn!(id, error = %e, "mojibake repair: title index update failed");
                }
                // The recommender's embedding is derived from the title, so a repaired title needs
                // the same background recompute `update_archive_metadata` triggers.
                let state = state.clone();
                let precompute_id = id.clone();
                let title = archive.title.clone();
                tokio::spawn(async move {
                    crate::recommend_precompute::precompute_one(&state, &precompute_id, &title)
                        .await;
                });
            }
            if archive.tags != old_tags {
                if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
                    &state.redis.search,
                    &state.equivalence,
                    &archive.id,
                    &old_tags,
                    &archive.tags,
                )
                .await
                {
                    tracing::warn!(id, error = %e, "mojibake repair: tag index update failed");
                }
            }
        }
        report.repaired_fields += touched.len();
        report.findings.extend(touched);
    }

    let categories = state
        .repos
        .categories
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    for mut category in categories {
        report.scanned += 1;
        let id = category.catid.to_string();
        let mut touched = Vec::new();
        repair_field(&mut touched, "category", &id, "name", &mut category.name);
        if touched.is_empty() {
            continue;
        }
        if apply {
            state
                .repos
                .categories
                .save(&category)
                .await
                .map_err(|e| e.to_string())?;
        }
        report.repaired_fields += touched.len();
        report.findings.extend(touched);
    }

    let groupings = state
        .repos
        .groupings
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    for mut grouping in groupings {
        report.scanned += 1;
        let id = grouping.tankid.to_string();
        let old_name = grouping.name.clone();
        let old_tags = grouping.tags.clone();
        let mut touched = Vec::new();
        repair_field(&mut touched, "tankoubon", &id, "name", &mut grouping.name);
        repair_field(
            &mut touched,
            "tankoubon",
            &id,
            "summary",
            &mut grouping.summary,
        );
        repair_field(&mut touched, "tankoubon", &id, "tags", &mut grouping.tags);
        if touched.is_empty() {
            continue;
        }
        if apply {
            state
                .repos
                .groupings
                .save(&grouping)
                .await
                .map_err(|e| e.to_string())?;
            if grouping.name != old_name {
                if let Err(e) = lanrurugi_search::indexer::update_title_index(
                    &state.redis.search,
                    &state.equivalence,
                    grouping.tankid.as_str(),
                    &old_name,
                    &grouping.name,
                )
                .await
                {
                    tracing::warn!(id, error = %e, "mojibake repair: tank title index update failed");
                }
            }
            if grouping.tags != old_tags {
                if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
                    &state.redis.search,
                    &state.equivalence,
                    grouping.tankid.as_str(),
                    &old_tags,
                    &grouping.tags,
                )
                .await
                {
                    tracing::warn!(id, error = %e, "mojibake repair: tank tag index update failed");
                }
            }
        }
        report.repaired_fields += touched.len();
        report.findings.extend(touched);
    }

    // Stamps hang off their owning archive's own `stamps` list rather than having a listing of
    // their own; `stamp_ids` was collected while walking the archives above.
    for stamp_id in stamp_ids {
        let Ok(Some(mut stamp)) = state.repos.stamps.get(&stamp_id).await else {
            continue;
        };
        report.scanned += 1;
        let id = stamp.stamp_id.to_string();
        let mut touched = Vec::new();
        repair_field(&mut touched, "stamp", &id, "content", &mut stamp.content);
        if touched.is_empty() {
            continue;
        }
        if apply {
            state
                .repos
                .stamps
                .update(&stamp.stamp_id, Some(&stamp.content), None, None, None)
                .await
                .map_err(|e| e.to_string())?;
        }
        report.repaired_fields += touched.len();
        report.findings.extend(touched);
    }

    if apply {
        tracing::info!(
            scanned = report.scanned,
            repaired = report.repaired_fields,
            "repaired double-encoded metadata"
        );
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the corrupted form the way it really occurs: each UTF-8 byte of the original text
    /// reinterpreted as its own character (the same helper the legacy importer's own tests use).
    /// Synthetic text only — a real work's title must never be committed to source.
    fn mojibake_of(s: &str) -> String {
        s.as_bytes().iter().map(|&b| b as char).collect()
    }

    #[test]
    fn a_double_encoded_name_is_reported_with_both_forms() {
        let original = "修復テスト 標題";
        let mut findings = Vec::new();
        let mut name = mojibake_of(original);
        repair_field(&mut findings, "tankoubon", "TANK_1", "name", &mut name);

        assert_eq!(name, original);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "tankoubon");
        assert_eq!(findings[0].field, "name");
        assert_eq!(findings[0].before, mojibake_of(original));
        assert_eq!(findings[0].after, original);
    }

    #[test]
    fn healthy_text_is_left_completely_alone() {
        let mut findings = Vec::new();
        let mut title = "テスト 中文标题 Sample Title".to_string();
        repair_field(&mut findings, "archive", "abc", "title", &mut title);

        assert_eq!(title, "テスト 中文标题 Sample Title");
        assert!(findings.is_empty());
    }

    #[test]
    fn plain_ascii_is_not_reported_as_a_repair() {
        // Every step of the detector's own criteria would pass for pure ASCII if the final
        // non-ASCII check weren't there — a "repair" that changed nothing would still show up as a
        // destructive-looking finding in the UI.
        let mut findings = Vec::new();
        let mut tags = "artist:someone, parody:something".to_string();
        repair_field(&mut findings, "archive", "abc", "tags", &mut tags);

        assert!(findings.is_empty());
    }
}
