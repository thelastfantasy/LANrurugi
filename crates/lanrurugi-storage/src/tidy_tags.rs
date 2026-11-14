//! Operator-triggered clean-up over every archive's stored tags. Deliberately not a startup
//! migration: it is idempotent and cheap enough to re-run at will (the settings page's button),
//! which matters because the mess it fixes — duplicate tags — can be reintroduced at any time by
//! another import or restore.

use std::collections::HashSet;

use lanrurugi_core::tags::collapse_duplicate_date_added;

use crate::repository::{ArchiveRepository, RepositoryError};

/// One archive whose tags were rewritten, with both sides of the change. The search index keys
/// tags by their folded spelling, so dropping an empty-value tag is the one rewrite here that does
/// remove an index key — the caller has to apply [`Self::old_tags`]/[`Self::new_tags`] to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyTagsChange {
    pub id: String,
    pub old_tags: String,
    pub new_tags: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TidyTagsReport {
    /// Archives read.
    pub scanned: usize,
    /// Archives whose tags actually changed and were written back.
    pub repaired: usize,
    /// What each of those rewrites was, in id order.
    pub changes: Vec<TidyTagsChange>,
}

/// Collapses each archive's tag list to one entry per tag. Archives that do not change are not
/// written back, so running this on an already-clean library always reports `repaired: 0`.
///
/// The rules, all of them reachable only through the settings page's button:
/// - duplicate `date_added:` values keep the newest (the one this instance's own scan wrote);
/// - any other tag appearing twice keeps its first occurrence, compared case-insensitively the way
///   the search index keys tags;
/// - separators and surrounding blanks are normalised to a plain comma;
/// - a namespace part is lower-cased (`Artist:X` → `artist:X`), its value left untouched;
/// - a tag whose value is empty (`artist:`) is dropped.
///
/// Only the last rule removes an index key (`INDEX_artist:`); the others fold to the same key or
/// rewrite only the display side, which is why the report carries the before/after tags for the
/// caller to apply where it matters.
pub async fn tidy_tags(archives: &ArchiveRepository) -> Result<TidyTagsReport, RepositoryError> {
    let mut report = TidyTagsReport::default();
    for mut archive in archives.list_all().await? {
        report.scanned += 1;
        let tidied = tidy_tag_string(&archive.tags);
        if tidied != archive.tags {
            report.changes.push(TidyTagsChange {
                id: archive.id.to_string(),
                old_tags: archive.tags.clone(),
                new_tags: tidied.clone(),
            });
            archives.set_tags(&archive.id, &tidied).await?;
            archive.tags = tidied;
            report.repaired += 1;
        }
    }
    Ok(report)
}

/// The pure tag-list rewrite behind [`tidy_tags`], exposed for tests.
pub fn tidy_tag_string(tags: &str) -> String {
    let collapsed = collapse_duplicate_date_added(tags);
    let mut seen = HashSet::new();
    let mut kept: Vec<String> = Vec::new();
    for tag in collapsed.split(',') {
        let trimmed = tag.trim();
        if trimmed.is_empty() || has_empty_value(trimmed) {
            continue;
        }
        let normalised = normalise_namespace(trimmed);
        if seen.insert(normalised.to_lowercase()) {
            kept.push(normalised);
        }
    }
    kept.join(",")
}

/// `artist:` names a namespace but carries nothing — noise left by a botched import or edit.
fn has_empty_value(tag: &str) -> bool {
    tag.split_once(':')
        .is_some_and(|(_, value)| value.trim().is_empty())
}

/// Lower-cases the namespace part of a tag, matching how every other part of the app spells
/// namespaces (`artist:`, never `Artist:`). Tags without a colon are "other" tags and have no
/// namespace to normalise; values are never touched, so case-sensitive values stay intact.
fn normalise_namespace(tag: &str) -> String {
    match tag.split_once(':') {
        Some((namespace, value)) => format!("{}:{}", namespace.trim().to_lowercase(), value),
        None => tag.to_string(),
    }
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
            blake3: None,
        }
    }

    #[test]
    fn lowers_the_namespace_and_drops_empty_value_tags() {
        assert_eq!(
            tidy_tag_string("Artist:saki uara,SOURCE:https://e-hentai.org/g/1,a:,"),
            "artist:saki uara,source:https://e-hentai.org/g/1"
        );
        assert_eq!(tidy_tag_string("other,plain tag"), "other,plain tag");
        assert_eq!(tidy_tag_string("Female: ,male:x"), "male:x");
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
        // Case-insensitive, matching how tags are folded into the search index; the winner keeps
        // its value's spelling but not its namespace's.
        assert_eq!(
            tidy_tag_string("Female:Big Breasts,female:big breasts,artist:a"),
            "female:Big Breasts,artist:a"
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
