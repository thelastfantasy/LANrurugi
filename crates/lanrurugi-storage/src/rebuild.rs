//! Rebuild-index core logic (User Story 6): recomputes every tracked archive's ID with the
//! size-aware algorithm and re-keys any that changed, per `data-model.md`'s "Rebuild/Reindex
//! operation".
//!
//! **Why re-keying alone isn't the whole fix**: legacy's false-merge defect (verified:
//! `~/LANraragi/lib/Shinobu.pm::update_filemap_entry`) means a historically-colliding pair has
//! exactly **one** tracked Redis record whose `file` field points at whichever of the two files
//! was scanned *last* — the other file is completely untracked, not merely misfiled. Re-keying the
//! one tracked record (this module) recovers that record's correct identity, but the orphaned
//! sibling file only gets (re)discovered by a fresh filesystem scan afterward — that's
//! `lanrurugi-scanner`'s job (it can't live here: `lanrurugi-storage` doesn't depend on the
//! scanner crate), orchestrated together by whichever caller drives the full rebuild
//! (`lanrurugi-server`'s `rebuild-index` CLI subcommand and `POST /database/rebuild-index`).

use std::path::Path;

use lanrurugi_core::concurrency::parallel_map;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_core::jobs::JobRegistry;

use crate::id::size_aware_id;
use crate::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, RepositoryError,
};

#[derive(Debug, Clone, Default)]
pub struct RekeySummary {
    pub unchanged: usize,
    pub rekeyed: Vec<(ArchiveId, ArchiveId)>,
    pub missing_file: usize,
    /// The archive was deleted (e.g. by a concurrent user action) between this pass listing it
    /// and attempting to rename it — not an error, just a record that's no longer there to fix.
    pub concurrently_deleted: usize,
}

/// Recomputes each tracked archive's ID from its current on-disk file and re-keys any that
/// differ, updating Category/Grouping references to the new ID in the same pass (FR-012, T075) —
/// `ArchiveRepository::rename_id` handles the archive hash's own key; this function additionally
/// walks every Category/Grouping and rewrites their archive-ID-list membership.
pub async fn rekey_all(
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    jobs: &JobRegistry,
    job_id: &str,
) -> Result<RekeySummary, RepositoryError> {
    let all = archives.list_all().await?;
    let total = all.len().max(1);
    let mut summary = RekeySummary::default();

    // FR-022: archive-identity hashing during reindex must scale with available CPU cores, not
    // be limited to single-threaded throughput — compute every archive's current on-disk ID in
    // parallel (rayon, off the async reactor) before doing any of the (comparatively cheap)
    // Redis rename/reference-update work below, rather than hashing one file at a time.
    let files: Vec<String> = all.iter().map(|archive| archive.file.clone()).collect();
    let new_ids: Vec<Option<String>> =
        parallel_map(files, |file| size_aware_id(Path::new(&file)).ok()).await?;

    for (i, (archive, new_id)) in all.iter().zip(new_ids).enumerate() {
        jobs.set_progress(job_id, (i + 1) as f32 / total as f32)
            .await;

        let Some(new_id) = new_id else {
            summary.missing_file += 1;
            continue;
        };
        let new_id = ArchiveId(new_id);
        if new_id == archive.id {
            summary.unchanged += 1;
            continue;
        }
        if archives.get(&new_id).await?.is_some() {
            // Another tracked archive already owns this exact content (a genuine, already-known
            // duplicate) — nothing to do for *this* record; leave it alone rather than clobber
            // the existing one.
            continue;
        }

        if let Err(e) = archives.rename_id(&archive.id, &new_id).await {
            if e.to_string().contains("no such key") {
                // The source was deleted out from under us since `list_all()` — the rename is
                // simply moot now, not a failure worth aborting the whole rebuild for.
                summary.concurrently_deleted += 1;
                continue;
            }
            return Err(e);
        }
        update_references(categories, groupings, &archive.id, &new_id).await?;
        summary.rekeyed.push((archive.id.clone(), new_id));
    }

    Ok(summary)
}

/// Issue #67: re-derives the `archive_id -> [category_id]`/`archive_id -> [tankoubon_id]` reverse
/// indexes (`keys::archive_categories_key`/`archive_tankoubons_key`) from every Category/
/// Grouping's own forward `archives` list. Needed because the reverse index didn't always exist: a
/// Category/Grouping saved before this feature shipped has never had its membership written into
/// the index at all, and nothing else in this codebase ever touches every Category/Grouping
/// unconditionally except a full rebuild — this is that rebuild's hook for it.
///
/// Deliberately calls each repository's `reindex_archive_membership` (an unconditional `SADD`, no
/// diffing) rather than just re-`save`-ing every entry unchanged: `save`'s own diff logic compares
/// the *new* archives list being written against `self.get()`'s *current* stored state — for a
/// category that's never been modified since before this index existed, those are identical (it's
/// the same stored data being "saved" right back), so every diff comes up empty and no `SADD` ever
/// happens. That's exactly the case this backfill exists to fix, so it can't route through `save`'s
/// diff at all. Safe to run repeatedly (each `SADD` on an already-indexed archive is a harmless
/// no-op) and cheap on an index that's already fully backfilled. Wired into
/// `POST /database/rebuild-index`/`lanrurugi rebuild-index`, run unconditionally alongside the
/// archive re-keying pass (not gated behind "did anything actually get rekeyed") so a fresh deploy
/// of this feature is fully backfilled on the very next rebuild regardless of whether any archive
/// ID happened to change.
pub async fn backfill_reverse_indexes(
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
) -> Result<(), RepositoryError> {
    for category in categories.list_all().await? {
        categories.reindex_archive_membership(&category).await?;
    }
    for grouping in groupings.list_all().await? {
        groupings.reindex_archive_membership(&grouping).await?;
    }
    Ok(())
}

async fn update_references(
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    old_id: &ArchiveId,
    new_id: &ArchiveId,
) -> Result<(), RepositoryError> {
    for mut category in categories.list_all().await? {
        if let Some(pos) = category.archives.iter().position(|id| id == old_id) {
            category.archives[pos] = new_id.clone();
            categories.save(&category).await?;
        }
    }
    for mut grouping in groupings.list_all().await? {
        if let Some(pos) = grouping.archives.iter().position(|id| id == old_id) {
            grouping.archives[pos] = new_id.clone();
            groupings.save(&grouping).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::{Archive, Category, Grouping};

    fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        deadpool_redis::Config::from_url(format!("{}/0", base.trim_end_matches('/')))
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()
    }

    #[tokio::test]
    async fn rekeys_a_legacy_id_and_updates_category_and_grouping_references() {
        let Some(pool) = test_pool() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.zip");
        std::fs::write(&path, b"some archive content for rebuild test").unwrap();

        // Simulate a legacy-migrated record: keyed by the OLD (legacy) id, not the size-aware one.
        let legacy_id = ArchiveId(crate::id::legacy_id(&path).unwrap());
        let new_id = ArchiveId(crate::id::size_aware_id(&path).unwrap());
        assert_ne!(
            legacy_id, new_id,
            "test fixture must actually change ID under rebuild"
        );

        archives
            .save(&Archive {
                id: legacy_id.clone(),
                name: "book".into(),
                title: "Book".into(),
                file: path.to_string_lossy().to_string(),
                tags: "artist:jane".into(),
                summary: String::new(),
                arcsize: 1,
                pagecount: 1,
                isnew: false,
                lastreadpage: 3,
                lastreadtime: 1000,
                thumbhash: None,
                toc: vec![],
                stamp_ids: vec![],
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
            })
            .await
            .unwrap();

        let catid = lanrurugi_core::ids::CategoryId("SET_1700000099".to_string());
        categories
            .save(&Category {
                catid: catid.clone(),
                name: "Cat".into(),
                search: None,
                archives: vec![legacy_id.clone()],
                pinned: false,
            })
            .await
            .unwrap();
        let tankid = lanrurugi_core::ids::TankId("TANK_1700000099".to_string());
        groupings
            .save(&Grouping {
                tankid: tankid.clone(),
                name: "Tank".into(),
                summary: String::new(),
                tags: String::new(),
                progress: 0,
                archives: vec![legacy_id.clone()],
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Default::default(),
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();

        let jobs = JobRegistry::new();
        let job_id = jobs.create("rebuild").await;
        let summary = rekey_all(&archives, &categories, &groupings, &jobs, &job_id)
            .await
            .unwrap();

        assert_eq!(summary.rekeyed, vec![(legacy_id.clone(), new_id.clone())]);
        assert!(archives.get(&legacy_id).await.unwrap().is_none());
        let rekeyed_archive = archives.get(&new_id).await.unwrap().unwrap();
        assert_eq!(rekeyed_archive.tags, "artist:jane");
        assert_eq!(rekeyed_archive.lastreadpage, 3);

        let updated_category = categories.get(&catid).await.unwrap().unwrap();
        assert_eq!(updated_category.archives, vec![new_id.clone()]);
        let updated_grouping = groupings.get(&tankid).await.unwrap().unwrap();
        assert_eq!(updated_grouping.archives, vec![new_id.clone()]);

        archives.delete(&new_id).await.unwrap();
        categories.delete(&catid).await.unwrap();
        groupings.delete(&tankid).await.unwrap();
    }

    #[tokio::test]
    async fn category_save_keeps_the_archive_reverse_index_in_sync() {
        let Some(pool) = test_pool() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let categories = CategoryRepository::new(pool.clone());
        let archive_a = ArchiveId("a".repeat(40));
        let archive_b = ArchiveId("b".repeat(40));

        let catid = lanrurugi_core::ids::CategoryId("SET_1700000199".to_string());
        categories
            .save(&Category {
                catid: catid.clone(),
                name: "Cat".into(),
                search: None,
                archives: vec![archive_a.clone()],
                pinned: false,
            })
            .await
            .unwrap();
        assert_eq!(
            categories.for_archive(&archive_a).await.unwrap().len(),
            1,
            "archive_a should be indexed after the first save"
        );
        assert_eq!(categories.for_archive(&archive_b).await.unwrap().len(), 0);

        // Swap membership from archive_a to archive_b — `save` must diff against the previous
        // state and update both archives' index entries, not just add the new one.
        categories
            .save(&Category {
                catid: catid.clone(),
                name: "Cat".into(),
                search: None,
                archives: vec![archive_b.clone()],
                pinned: false,
            })
            .await
            .unwrap();
        assert_eq!(
            categories.for_archive(&archive_a).await.unwrap().len(),
            0,
            "archive_a must be removed from the index once no longer a member"
        );
        assert_eq!(categories.for_archive(&archive_b).await.unwrap().len(), 1);

        categories.delete(&catid).await.unwrap();
        assert_eq!(
            categories.for_archive(&archive_b).await.unwrap().len(),
            0,
            "delete must clear the category out of its members' index entries"
        );
    }

    #[tokio::test]
    async fn backfill_reverse_indexes_recovers_membership_saved_before_the_index_existed() {
        let Some(pool) = test_pool() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let archive = ArchiveId("c".repeat(40));

        let catid = lanrurugi_core::ids::CategoryId("SET_1700000299".to_string());
        categories
            .save(&Category {
                catid: catid.clone(),
                name: "Cat".into(),
                search: None,
                archives: vec![archive.clone()],
                pinned: false,
            })
            .await
            .unwrap();
        let tankid = lanrurugi_core::ids::TankId("TANK_1700000299".to_string());
        groupings
            .save(&Grouping {
                tankid: tankid.clone(),
                name: "Tank".into(),
                summary: String::new(),
                tags: String::new(),
                progress: 0,
                archives: vec![archive.clone()],
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Default::default(),
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();

        // Simulate a pre-existing deploy: wipe just the reverse-index entries `save` above wrote,
        // as if this Category/Grouping had been created before the index existed at all.
        let mut conn = pool.get().await.unwrap();
        let _: () = deadpool_redis::redis::AsyncCommands::del(
            &mut conn,
            crate::keys::archive_categories_key(archive.as_str()),
        )
        .await
        .unwrap();
        let _: () = deadpool_redis::redis::AsyncCommands::del(
            &mut conn,
            crate::keys::archive_tankoubons_key(archive.as_str()),
        )
        .await
        .unwrap();
        assert_eq!(categories.for_archive(&archive).await.unwrap().len(), 0);
        assert_eq!(groupings.for_archive(&archive).await.unwrap().len(), 0);

        backfill_reverse_indexes(&categories, &groupings)
            .await
            .unwrap();

        assert_eq!(categories.for_archive(&archive).await.unwrap().len(), 1);
        assert_eq!(groupings.for_archive(&archive).await.unwrap().len(), 1);

        categories.delete(&catid).await.unwrap();
        groupings.delete(&tankid).await.unwrap();
    }
}
