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
}
