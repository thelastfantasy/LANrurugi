//! Operator-triggered clean-up over every archive's stored tags. Deliberately not a startup
//! migration: it is idempotent and cheap enough to re-run at will (the settings page's button),
//! which matters because the mess it fixes — duplicate tags — can be reintroduced at any time by
//! another import or restore.

use std::collections::HashSet;

use lanrurugi_core::tags::collapse_duplicate_date_added;

use crate::repository::{ArchiveRepository, RepositoryError};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TidyTagsReport {
    /// Archives read.
    pub scanned: usize,
    /// Archives whose tags actually changed and were written back.
    pub repaired: usize,
}

/// Collapses each archive's tag list to one entry per tag: duplicate `date_added:` values keep the
/// newest (the one this instance's own scan wrote), anything else appearing more than once — case
/// insensitively, the way the search index keys tags — keeps its first occurrence. Separators and
/// surrounding blanks are normalised to a plain comma too. Archives that do not change are not
/// written back, so running this on an already-clean library always reports `repaired: 0`.
///
/// None of these rewrites can move a tag between search-index buckets: `indexer::folded_tag_key`
/// already trims and lower-cases each tag, so `Artist:X` and `artist:X` share one key and dropping
/// either leaves the other's membership in place — the index needs no maintenance here.
pub async fn tidy_tags(archives: &ArchiveRepository) -> Result<TidyTagsReport, RepositoryError> {
    let mut report = TidyTagsReport::default();
    for mut archive in archives.list_all().await? {
        report.scanned += 1;
        let tidied = tidy_tag_string(&archive.tags);
        if tidied != archive.tags {
            archive.tags = tidied;
            archives.save(&archive).await?;
            report.repaired += 1;
        }
    }
    Ok(report)
}

/// The pure tag-list rewrite behind [`tidy_tags`], exposed for tests.
pub fn tidy_tag_string(tags: &str) -> String {
    let collapsed = collapse_duplicate_date_added(tags);
    let mut seen = HashSet::new();
    let mut kept: Vec<&str> = Vec::new();
    for tag in collapsed.split(',') {
        let trimmed = tag.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_lowercase()) {
            kept.push(trimmed);
        }
    }
    kept.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{test_redis_dbs, RedisTestLock};
    use lanrurugi_core::entities::Archive;
    use lanrurugi_core::ids::ArchiveId;

    fn archive(id: &str, tags: &str) -> Archive {
        Archive {
            id: ArchiveId(id.to_string()),
            name: id.to_string(),
            title: id.to_string(),
            file: format!("/x/{id}.zip"),
            tags: tags.to_string(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 1,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
        }
    }

    #[test]
    fn keeps_the_newest_date_added_and_drops_repeats() {
        assert_eq!(
            tidy_tag_string("artist:a,date_added:1742054739,date_added:1793928516,female:b"),
            "artist:a,date_added:1793928516,female:b"
        );
        assert_eq!(
            tidy_tag_string("artist:a, artist:a,female:b,"),
            "artist:a,female:b"
        );
        // Case-insensitive, matching how tags are folded into the search index.
        assert_eq!(
            tidy_tag_string("Female:Big Breasts,female:big breasts,artist:a"),
            "Female:Big Breasts,artist:a"
        );
        // Empty segments from a legacy `", "` join disappear with the normalised separators.
        assert_eq!(tidy_tag_string("artist:a, ,female:b"), "artist:a,female:b");
    }

    #[test]
    fn leaves_a_clean_tag_list_byte_for_byte_alone() {
        for tags in [
            "",
            "artist:a,female:b",
            "date_added:1793928516",
            "artist:a,date_added:1793928516,female:b",
        ] {
            assert_eq!(tidy_tag_string(tags), tags);
        }
    }

    #[tokio::test]
    async fn rewrites_only_the_archives_that_change() {
        let Some(redis) = test_redis_dbs().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let lock = RedisTestLock::acquire(&redis.config, "tidy_tags_test").await;

        let archives = ArchiveRepository::new(redis.archive.clone());
        let dirty_id = "9".repeat(40);
        let clean_id = "8".repeat(40);
        archives
            .save(&archive(
                &dirty_id,
                "artist:a,date_added:1742054739,date_added:1793928516",
            ))
            .await
            .unwrap();
        archives
            .save(&archive(&clean_id, "artist:a,date_added:1793928516"))
            .await
            .unwrap();

        let report = tidy_tags(&archives).await.unwrap();
        assert!(report.scanned >= 2);
        assert!(report.repaired >= 1);

        let dirty = archives
            .get(&ArchiveId(dirty_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(dirty.tags, "artist:a,date_added:1793928516");
        let clean = archives
            .get(&ArchiveId(clean_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(clean.tags, "artist:a,date_added:1793928516");

        // Idempotent: a second run finds nothing left to do.
        let again = tidy_tags(&archives).await.unwrap();
        assert_eq!(again.repaired, 0);

        archives.delete(&ArchiveId(dirty_id)).await.unwrap();
        archives.delete(&ArchiveId(clean_id)).await.unwrap();
        lock.release().await;
    }
}
