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
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, RepositoryError, StampRepository,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupArchive {
    pub arcid: String,
    pub title: String,
    pub tags: String,
    pub summary: Option<String>,
    pub thumbhash: Option<String>,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupCategory {
    pub catid: String,
    pub name: String,
    pub search: Option<String>,
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupTankoubon {
    pub tankid: String,
    pub name: String,
    pub summary: String,
    pub tags: String,
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupDocument {
    pub archives: Vec<BackupArchive>,
    pub categories: Vec<BackupCategory>,
    pub tankoubons: Vec<BackupTankoubon>,
    pub stamps: Vec<BackupStamp>,
}

pub async fn build(
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    stamps: &StampRepository,
) -> Result<BackupDocument, RepositoryError> {
    let archive_list = archives.list_all().await?;
    let category_list = categories.list_all().await?;
    let grouping_list = groupings.list_all().await?;

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
    })
}

fn to_backup_category(c: Category) -> BackupCategory {
    BackupCategory {
        catid: c.catid.into_string(),
        name: c.name,
        search: c.search,
        archives: c.archives.into_iter().map(|a| a.into_string()).collect(),
    }
}

fn to_backup_tankoubon(g: Grouping) -> BackupTankoubon {
    BackupTankoubon {
        tankid: g.tankid.into_string(),
        name: g.name,
        summary: g.summary,
        tags: g.tags,
        archives: g.archives.into_iter().map(|a| a.into_string()).collect(),
    }
}

fn to_backup_stamp(s: Stamp) -> BackupStamp {
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

        let doc = build(&archives, &categories, &groupings, &stamp_repo)
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

        archives.delete(&id).await.unwrap();
        stamp_repo.delete(&stamp_id).await.unwrap();
    }
}
