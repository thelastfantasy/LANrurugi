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
                stamp_docs.push(to_backup_stamp(&stamp));
            }
        }
    }

    Ok(BackupDocument {
        archives: archive_list
            .iter()
            .map(|a| BackupArchive {
                arcid: a.id.clone(),
                title: a.title.clone(),
                tags: a.tags.clone(),
                summary: (!a.summary.is_empty()).then(|| a.summary.clone()),
                thumbhash: a.thumbhash.clone(),
                filename: a.name.clone(),
            })
            .collect(),
        categories: category_list.iter().map(to_backup_category).collect(),
        tankoubons: grouping_list.iter().map(to_backup_tankoubon).collect(),
        stamps: stamp_docs,
    })
}

fn to_backup_category(c: &Category) -> BackupCategory {
    BackupCategory {
        catid: c.catid.clone(),
        name: c.name.clone(),
        search: c.search.clone(),
        archives: c.archives.clone(),
    }
}

fn to_backup_tankoubon(g: &Grouping) -> BackupTankoubon {
    BackupTankoubon {
        tankid: g.tankid.clone(),
        name: g.name.clone(),
        summary: g.summary.clone(),
        tags: g.tags.clone(),
        archives: g.archives.clone(),
    }
}

fn to_backup_stamp(s: &Stamp) -> BackupStamp {
    BackupStamp {
        stamp_id: s.stamp_id.clone(),
        content: s.content.clone(),
        position: s.position.clone(),
        archive_id: s.archive_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::Archive;

    fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        deadpool_redis::Config::from_url(format!("{}/0", base.trim_end_matches('/')))
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()
    }

    #[tokio::test]
    async fn builds_a_consistent_snapshot_matching_legacy_shape() {
        let Some(pool) = test_pool() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamp_repo = StampRepository::new(pool.clone());

        let id = "9".repeat(40);
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
            })
            .await
            .unwrap();
        let stamp_id = stamp_repo
            .create(&id, 1, "hi", "1,2", 1_700_000_000_000)
            .await
            .unwrap();

        let doc = build(&archives, &categories, &groupings, &stamp_repo)
            .await
            .unwrap();
        let entry = doc.archives.iter().find(|a| a.arcid == id).unwrap();
        assert_eq!(entry.title, "My Title");
        assert_eq!(entry.tags, "artist:jane");
        assert_eq!(entry.thumbhash.as_deref(), Some("abc123"));
        assert!(doc.stamps.iter().any(|s| s.stamp_id == stamp_id));

        archives.delete(&id).await.unwrap();
        stamp_repo.delete(&stamp_id).await.unwrap();
    }
}
