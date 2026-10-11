//! Backup-JSON builder (User Story 5), shape verified against
//! `~/LANraragi/lib/LANraragi/Model/Backup.pm::build_backup_JSON` and
//! `tools/openapi.yaml`'s `BackupArchiveMetadataJson`/`BackupCategoryMetadataJson`/
//! `TankoubonBackupJson` schemas.
//!
//! **Consistency (FR-010)**: this module takes one `list_all()` snapshot per entity type. A
//! archive/category/tankoubon created or edited *during* the build may or may not be included
//! (ordinary snapshot-read semantics), but nothing half-written can appear — each entity is read
//! whole (a single Redis `HGETALL`/`ZRANGE`) or not at all, never partially, since the repository
//! layer never exposes a document mid-write.

use lanrurugi_core::entities::{Category, Grouping, Stamp};
use lanrurugi_storage::bookmarks::{Bookmark, BookmarksRepository};
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, RepositoryError, StampRepository,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupArchive {
    pub arcid: String,
    pub title: String,
    pub tags: String,
    pub summary: Option<String>,
    pub thumbhash: Option<String>,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupCategory {
    pub catid: String,
    pub name: String,
    pub search: Option<String>,
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupTankoubon {
    pub tankid: String,
    pub name: String,
    pub summary: String,
    pub tags: String,
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupStamp {
    pub stamp_id: String,
    pub content: String,
    pub position: String,
    pub archive_id: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub rect: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupBookmark {
    pub archive_id: String,
    pub page: u32,
    pub bookmarked_at: u64,
    /// Additive — `#[serde(default)]` so a backup exported before named bookmarks existed
    /// deserializes to `None` (no name) rather than a hard parse error, same posture `bookmarks`
    /// itself on `BackupDocument` already takes for a pre-bookmarks-feature backup entirely.
    #[serde(default)]
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// `specs/004-ocr-manga-translation` (FR-022, research.md §17) — Phase 2 translation entities.
//
// Included because they represent real, hard-to-reproduce user investment: a Terminology Glossary
// may hold dozens of hand-corrected character names, and translated regions represent LLM calls
// that were already paid for. Losing either on a restore is exactly the silent data loss
// constitution Principle I treats as a correctness bug.
//
// Deliberately NOT included: the rendered-page cache (re-derivable from the regions below by
// re-running compositing alone — research.md §16) and Usage Budget counters (a point-in-time
// consumption figure, not durable user-authored state — the same reasoning that keeps in-flight
// job status out of Phase 1's own backup).
//
// Every field is a plain `String`, not a domain newtype: these are external wire-format DTOs, which
// the constitution's newtype rule explicitly exempts.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupTerminologyGlossary {
    pub volume_id: String,
    /// source term → every (archive, chapter)-scoped translation candidate for it (issue #105) —
    /// mirrors `TerminologyGlossary::entries`'s own shape exactly, not re-flattened, since a
    /// backup/restore round trip must not silently lose the disambiguation a cross-archive/chapter
    /// name collision depends on.
    pub entries:
        std::collections::BTreeMap<String, Vec<lanrurugi_translate::glossary::GlossaryEntry>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupVolumeFontPattern {
    pub volume_id: String,
    pub is_locked: bool,
    pub vote_pool: std::collections::BTreeMap<String, u32>,
    pub golden_set: Vec<String>,
    pub meltdown_tally: std::collections::BTreeMap<String, u32>,
}

/// Translated regions for one (archive, page, language, provider) combination.
///
/// Stored under its original Redis key so a restore puts it back exactly where it was, without this
/// DTO needing to re-derive the key from its four components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupTextRegions {
    pub key: String,
    /// Serialized `DetectedTextRegion` records, kept opaque here so this crate doesn't need a
    /// dependency on `lanrurugi-ocr` purely to re-declare a shape it only passes through.
    pub regions: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackupDocument {
    pub archives: Vec<BackupArchive>,
    pub categories: Vec<BackupCategory>,
    pub tankoubons: Vec<BackupTankoubon>,
    pub stamps: Vec<BackupStamp>,
    /// Additive — no legacy equivalent (`bookmarks.rs`'s own module docs: this is a LANrurugi-only
    /// concept, legacy's "bookmark" was really just a category alias). `#[serde(default)]` so a
    /// pre-existing backup JSON exported before this field existed still deserializes fine (an
    /// old backup simply restores with zero bookmarks, not a hard parse error).
    #[serde(default)]
    pub bookmarks: Vec<BackupBookmark>,
    /// Phase 2 translation state (FR-022). `#[serde(default)]` for the same reason `bookmarks`
    /// carries it: a backup exported before this feature existed must still restore cleanly, with
    /// simply no translation data, rather than failing to parse.
    #[serde(default)]
    pub terminology_glossaries: Vec<BackupTerminologyGlossary>,
    #[serde(default)]
    pub volume_font_patterns: Vec<BackupVolumeFontPattern>,
    #[serde(default)]
    pub text_regions: Vec<BackupTextRegions>,
}

pub async fn build(
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    stamps: &StampRepository,
    bookmarks: &BookmarksRepository,
) -> Result<BackupDocument, RepositoryError> {
    let archive_list = archives.list_all().await?;
    let category_list = categories.list_all().await?;
    let grouping_list = groupings.list_all().await?;
    let bookmark_list = bookmarks.list_all().await.map_err(|e| match e {
        lanrurugi_storage::bookmarks::BookmarksError::Redis(e) => RepositoryError::Redis(e),
        lanrurugi_storage::bookmarks::BookmarksError::Pool(e) => RepositoryError::Pool(e),
    })?;

    let mut stamp_docs = Vec::new();
    for archive in &archive_list {
        for stamp_id in &archive.stamp_ids {
            if let Some(stamp) = stamps.get(stamp_id).await? {
                stamp_docs.push(to_backup_stamp(stamp));
            }
        }
    }

    Ok(BackupDocument {
        archives: archive_list
            .into_iter()
            .map(|a| BackupArchive {
                arcid: a.id.into_string(),
                title: a.title,
                tags: a.tags,
                summary: (!a.summary.is_empty()).then_some(a.summary),
                thumbhash: a.thumbhash,
                filename: a.name,
            })
            .collect(),
        categories: category_list.into_iter().map(to_backup_category).collect(),
        tankoubons: grouping_list.into_iter().map(to_backup_tankoubon).collect(),
        stamps: stamp_docs,
        bookmarks: bookmark_list.into_iter().map(to_backup_bookmark).collect(),
        // Populated separately by `collect_translation_state` — see `build`'s own doc comment.
        terminology_glossaries: Vec::new(),
        volume_font_patterns: Vec::new(),
        text_regions: Vec::new(),
    })
}

/// Adds Phase 2 translation state to an already-built document (FR-022, research.md §17).
///
/// A separate function rather than extra parameters on [`build`] so that this feature's repositories
/// aren't a hard requirement of taking a backup — an instance that has never enabled translation
/// simply produces empty vectors here, and `build`'s existing callers keep working unchanged.
///
/// Failures are logged and skipped rather than aborting: a malformed translation record must not
/// cost the user the rest of an otherwise-good library backup.
pub async fn collect_translation_state(
    doc: &mut BackupDocument,
    glossaries: &lanrurugi_translate::glossary::GlossaryRepository,
    font_patterns: &lanrurugi_fontcache::FontPatternRepository,
    regions: &lanrurugi_translate::regions::RegionRepository,
) {
    match glossaries.list_all().await {
        Ok(list) => {
            doc.terminology_glossaries = list
                .into_iter()
                .map(|g| BackupTerminologyGlossary {
                    volume_id: g.volume_id,
                    entries: g.entries,
                })
                .collect()
        }
        Err(e) => tracing::warn!(error = %e, "skipping terminology glossaries in backup"),
    }

    match font_patterns.list_all().await {
        Ok(list) => {
            doc.volume_font_patterns = list
                .into_iter()
                .map(|p| BackupVolumeFontPattern {
                    volume_id: p.volume_id,
                    is_locked: p.is_locked,
                    vote_pool: p
                        .vote_pool
                        .into_iter()
                        .map(|(font, count)| (font.0, count))
                        .collect(),
                    golden_set: p.golden_set.into_iter().map(|f| f.0).collect(),
                    meltdown_tally: p
                        .meltdown_tally
                        .into_iter()
                        .map(|(font, count)| (font.0, count))
                        .collect(),
                })
                .collect()
        }
        Err(e) => tracing::warn!(error = %e, "skipping volume font patterns in backup"),
    }

    match regions.list_all_translated().await {
        Ok(list) => {
            doc.text_regions = list
                .into_iter()
                .filter_map(|(key, regions)| {
                    serde_json::to_value(regions)
                        .ok()
                        .map(|regions| BackupTextRegions { key, regions })
                })
                .collect()
        }
        Err(e) => tracing::warn!(error = %e, "skipping translated text regions in backup"),
    }
}

pub(crate) fn to_backup_bookmark(b: Bookmark) -> BackupBookmark {
    BackupBookmark {
        archive_id: b.archive_id,
        page: b.page,
        bookmarked_at: b.bookmarked_at,
        name: b.name,
    }
}

pub(crate) fn to_backup_category(c: Category) -> BackupCategory {
    BackupCategory {
        catid: c.catid.into_string(),
        name: c.name,
        search: c.search,
        archives: c.archives.into_iter().map(|a| a.into_string()).collect(),
    }
}

pub(crate) fn to_backup_tankoubon(g: Grouping) -> BackupTankoubon {
    BackupTankoubon {
        tankid: g.tankid.into_string(),
        name: g.name,
        summary: g.summary,
        tags: g.tags,
        archives: g.archives.into_iter().map(|a| a.into_string()).collect(),
    }
}

pub(crate) fn to_backup_stamp(s: Stamp) -> BackupStamp {
    BackupStamp {
        stamp_id: s.stamp_id.into_string(),
        content: s.content,
        position: s.position,
        archive_id: s.archive_id.into_string(),
        icon: s.icon,
        rect: s.rect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::Archive;

    async fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = format!("{}/0", base.trim_end_matches('/'));
        lanrurugi_storage::test_support::test_pool_for_url(&url).await
    }

    #[tokio::test]
    async fn builds_a_consistent_snapshot_matching_legacy_shape() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamp_repo = StampRepository::new(pool.clone());
        let bookmarks = BookmarksRepository::new(pool.clone());

        let id = lanrurugi_core::ids::ArchiveId("9".repeat(40));
        archives
            .save(&Archive {
                id: id.clone(),
                name: "n".into(),
                title: "My Title".into(),
                file: "/x.zip".into(),
                tags: "artist:jane".into(),
                summary: "sum".into(),
                arcsize: 1,
                pagecount: 5,
                isnew: false,
                lastreadpage: 0,
                lastreadtime: 0,
                thumbhash: Some("abc123".into()),
                toc: vec![],
                stamp_ids: vec![],
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
            })
            .await
            .unwrap();
        let stamp_id = stamp_repo
            .create(&id, 1, "hi", "1,2", "", "", 1_700_000_000_000)
            .await
            .unwrap();
        bookmarks.add(id.as_str(), 3, 1_700_000_001).await.unwrap();

        let doc = build(&archives, &categories, &groupings, &stamp_repo, &bookmarks)
            .await
            .unwrap();
        let entry = doc
            .archives
            .iter()
            .find(|a| a.arcid == id.as_str())
            .unwrap();
        assert_eq!(entry.title, "My Title");
        assert_eq!(entry.tags, "artist:jane");
        assert_eq!(entry.thumbhash.as_deref(), Some("abc123"));
        assert!(doc.stamps.iter().any(|s| s.stamp_id == stamp_id.as_str()));
        assert!(doc
            .bookmarks
            .iter()
            .any(|b| b.archive_id == id.as_str() && b.page == 3));

        archives.delete(&id).await.unwrap();
        stamp_repo.delete(&stamp_id).await.unwrap();
        bookmarks
            .remove(id.as_str(), 3, 1_700_000_002)
            .await
            .unwrap();
    }
}
