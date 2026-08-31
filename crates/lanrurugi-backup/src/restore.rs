//! Restore-from-backup logic (User Story 5): re-attaches a `BackupDocument`'s metadata to
//! matching archive IDs on a fresh instance pointed at the same archive files (quickstart.md §5).
//! Archives themselves are not created by restore — they're expected to already exist (via the
//! normal ingestion pipeline, User Story 2) since a backup carries no file-path/content
//! information, only metadata keyed by archive ID.

use lanrurugi_core::entities::{Category, Grouping, Stamp};
use lanrurugi_core::ids::{ArchiveId, CategoryId, StampId, TankId};
use lanrurugi_storage::bookmarks::BookmarksRepository;
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, RepositoryError, StampRepository,
};

use crate::build::BackupDocument;

fn map_bookmarks_error(e: lanrurugi_storage::bookmarks::BookmarksError) -> RepositoryError {
    match e {
        lanrurugi_storage::bookmarks::BookmarksError::Redis(e) => RepositoryError::Redis(e),
        lanrurugi_storage::bookmarks::BookmarksError::Pool(e) => RepositoryError::Pool(e),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RestoreSummary {
    pub archives_updated: usize,
    pub archives_skipped_missing: usize,
    pub categories_restored: usize,
    pub tankoubons_restored: usize,
    pub stamps_restored: usize,
    pub bookmarks_restored: usize,
}

pub async fn restore(
    doc: &BackupDocument,
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    stamps: &StampRepository,
    bookmarks: &BookmarksRepository,
) -> Result<RestoreSummary, RepositoryError> {
    let mut summary = RestoreSummary::default();

    for backup_archive in &doc.archives {
        let Some(mut archive) = archives
            .get(&ArchiveId(backup_archive.arcid.clone()))
            .await?
        else {
            // Per Backup/Export document semantics: restore only re-attaches metadata to
            // archives that already exist on this instance (matched by ID); it never fabricates
            // an Archive record for content that isn't actually present on disk.
            summary.archives_skipped_missing += 1;
            continue;
        };
        archive.title = backup_archive.title.clone();
        archive.tags = backup_archive.tags.clone();
        archive.summary = backup_archive.summary.clone().unwrap_or_default();
        archive.thumbhash = backup_archive.thumbhash.clone();
        archives.save(&archive).await?;
        summary.archives_updated += 1;
    }

    for backup_category in &doc.categories {
        categories
            .save(&Category {
                catid: CategoryId(backup_category.catid.clone()),
                name: backup_category.name.clone(),
                search: backup_category.search.clone(),
                archives: backup_category
                    .archives
                    .iter()
                    .cloned()
                    .map(ArchiveId)
                    .collect(),
                pinned: false,
                visible_to_guest: false,
            })
            .await?;
        summary.categories_restored += 1;
    }

    for backup_tank in &doc.tankoubons {
        groupings
            .save(&Grouping {
                tankid: TankId(backup_tank.tankid.clone()),
                name: backup_tank.name.clone(),
                summary: backup_tank.summary.clone(),
                tags: backup_tank.tags.clone(),
                progress: 0,
                archives: backup_tank
                    .archives
                    .iter()
                    .cloned()
                    .map(ArchiveId)
                    .collect(),
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Default::default(),
                created_at: None,
                updated_at: None,
            })
            .await?;
        summary.tankoubons_restored += 1;
    }

    for backup_stamp in &doc.stamps {
        // Stamps are restored verbatim (not via `StampRepository::create`, which mints a fresh
        // key) — the backup's own `stamp_id` is authoritative so the archive's `stamp_ids` list
        // (already restored above via the archive's own tags/etc — no, stamp linkage lives on the
        // Stamp/Archive records independently) stays consistent with what's referenced elsewhere.
        stamps
            .restore_raw(&Stamp {
                stamp_id: StampId(backup_stamp.stamp_id.clone()),
                content: backup_stamp.content.clone(),
                position: backup_stamp.position.clone(),
                archive_id: ArchiveId(backup_stamp.archive_id.clone()),
                icon: backup_stamp.icon.clone(),
                rect: backup_stamp.rect.clone(),
            })
            .await?;
        summary.stamps_restored += 1;
    }

    for backup_bookmark in &doc.bookmarks {
        // `add` itself is idempotent (an existing bookmark's `bookmarked_at` is simply
        // overwritten with the backup's own value) — no existence check needed first, same
        // "restore re-applies verbatim" posture as stamps above. Unlike archives, a bookmark
        // referencing an archive id absent from this instance is *not* separately counted/skipped
        // — `BookmarksRepository` has no archive-existence dependency of its own (it's a flat
        // `archive_id:page` hash, not a foreign key), so restoring it is harmless even if that
        // archive never gets re-ingested; it simply never surfaces anywhere until it does.
        bookmarks
            .add(
                &backup_bookmark.archive_id,
                backup_bookmark.page,
                backup_bookmark.bookmarked_at,
            )
            .await
            .map_err(map_bookmarks_error)?;
        // Only written when the backup actually carries a name — `set_name(None)` would be a
        // harmless no-op either way (`BookmarksRepository::set_name`'s own docs), but skipping the
        // call entirely for an un-named backup entry avoids a wasted Redis round trip on the
        // overwhelmingly common case (backups exported before this feature existed, or a bookmark
        // that was simply never named).
        if backup_bookmark.name.is_some() {
            bookmarks
                .set_name(
                    &backup_bookmark.archive_id,
                    backup_bookmark.page,
                    backup_bookmark.name.as_deref(),
                )
                .await
                .map_err(map_bookmarks_error)?;
        }
        summary.bookmarks_restored += 1;
    }

    Ok(summary)
}

// Ensures the archive's own `stamps` field (the JSON list of stamp IDs it owns) still lists every
// restored stamp — `StampRepository::restore_raw` intentionally writes the stamp hash directly
// without touching this, so it's reconciled here as a separate, explicit pass.
pub async fn relink_stamp_ids(
    doc: &BackupDocument,
    archives: &ArchiveRepository,
) -> Result<(), RepositoryError> {
    use std::collections::HashMap;

    let mut by_archive: HashMap<ArchiveId, Vec<StampId>> = HashMap::new();
    for stamp in &doc.stamps {
        by_archive
            .entry(ArchiveId(stamp.archive_id.clone()))
            .or_default()
            .push(StampId(stamp.stamp_id.clone()));
    }

    for (archive_id, stamp_ids) in by_archive {
        if let Some(mut archive) = archives.get(&archive_id).await? {
            archive.stamp_ids = stamp_ids;
            archives.save(&archive).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{self, BackupArchive};
    use lanrurugi_core::entities::Archive;

    async fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = format!("{}/0", base.trim_end_matches('/'));
        lanrurugi_storage::test_support::test_pool_for_url(&url).await
    }

    #[tokio::test]
    async fn restore_reattaches_metadata_to_existing_archive_by_id_only() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());
        let bookmarks = BookmarksRepository::new(pool.clone());

        // Archive already exists on "this instance" (as if freshly re-scanned, no metadata yet).
        let id = ArchiveId("7".repeat(40));
        archives
            .save(&Archive {
                id: id.clone(),
                name: "n".into(),
                title: "n".into(),
                file: "/x.zip".into(),
                tags: String::new(),
                summary: String::new(),
                arcsize: 1,
                pagecount: 1,
                isnew: true,
                lastreadpage: 0,
                lastreadtime: 0,
                thumbhash: None,
                toc: vec![],
                stamp_ids: vec![],
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
            })
            .await
            .unwrap();

        let doc = build::BackupDocument {
            archives: vec![BackupArchive {
                arcid: id.to_string(),
                title: "Restored Title".into(),
                tags: "artist:restored".into(),
                summary: Some("restored summary".into()),
                thumbhash: Some("deadbeef".into()),
                filename: "n".into(),
            }],
            categories: vec![],
            tankoubons: vec![],
            stamps: vec![],
            bookmarks: vec![],
        };

        let summary = restore(
            &doc,
            &archives,
            &categories,
            &groupings,
            &stamps,
            &bookmarks,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 1);
        assert_eq!(summary.archives_skipped_missing, 0);

        let restored = archives.get(&id).await.unwrap().unwrap();
        assert_eq!(restored.title, "Restored Title");
        assert_eq!(restored.tags, "artist:restored");
        assert_eq!(restored.summary, "restored summary");
        assert_eq!(restored.thumbhash.as_deref(), Some("deadbeef"));

        archives.delete(&id).await.unwrap();
    }

    #[tokio::test]
    async fn restore_skips_archives_not_present_on_this_instance() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());
        let bookmarks = BookmarksRepository::new(pool.clone());

        let doc = build::BackupDocument {
            archives: vec![BackupArchive {
                arcid: "0".repeat(40),
                title: "Ghost".into(),
                tags: String::new(),
                summary: None,
                thumbhash: None,
                filename: "ghost".into(),
            }],
            categories: vec![],
            tankoubons: vec![],
            stamps: vec![],
            bookmarks: vec![],
        };

        let summary = restore(
            &doc,
            &archives,
            &categories,
            &groupings,
            &stamps,
            &bookmarks,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 0);
        assert_eq!(summary.archives_skipped_missing, 1);
    }
}
