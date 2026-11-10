//! One-time repairs for Redis data written by an older version of this app. Each migration is
//! gated by a flag in the config DB, so it runs at most once per instance and costs nothing on
//! every later start.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::tags::collapse_duplicate_date_added;
use thiserror::Error;

use crate::repository::{ArchiveRepository, RepositoryError};

/// Set once [`dedupe_date_added_tags`] has completed.
pub const DATE_ADDED_DEDUP_FLAG_KEY: &str = "LANRURUGI_MIGRATION_DATE_ADDED_DEDUP";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DateAddedDedupReport {
    /// Archives read. Zero when [`Self::already_done`].
    pub scanned: usize,
    /// Archives whose tags actually carried more than one `date_added:` and were rewritten.
    pub repaired: usize,
    pub already_done: bool,
}

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

/// Collapses the duplicate `date_added:` tags a LANraragi import used to leave behind (both
/// libraries' tags were unioned), keeping the newer value — the one this instance's own scan
/// wrote. Archives are only written back when they actually change.
pub async fn dedupe_date_added_tags(
    archives: &ArchiveRepository,
    config: &Pool,
) -> Result<DateAddedDedupReport, MigrationError> {
    let mut conn = config.get().await?;
    let done: bool = conn.exists(DATE_ADDED_DEDUP_FLAG_KEY).await?;
    drop(conn);
    if done {
        return Ok(DateAddedDedupReport {
            already_done: true,
            ..Default::default()
        });
    }

    let mut report = DateAddedDedupReport::default();
    for mut archive in archives.list_all().await? {
        report.scanned += 1;
        let collapsed = collapse_duplicate_date_added(&archive.tags);
        if collapsed != archive.tags {
            archive.tags = collapsed;
            archives.save(&archive).await?;
            report.repaired += 1;
        }
    }

    let mut conn = config.get().await?;
    let _: () = conn.set(DATE_ADDED_DEDUP_FLAG_KEY, "1").await?;
    Ok(report)
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

    async fn clear_flag(config: &Pool) {
        let mut conn = config.get().await.unwrap();
        let _: i64 = deadpool_redis::redis::cmd("DEL")
            .arg(DATE_ADDED_DEDUP_FLAG_KEY)
            .query_async(&mut conn)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn collapses_duplicate_date_added_tags_then_marks_itself_done() {
        let Some(redis) = test_redis_dbs().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let lock = RedisTestLock::acquire(&redis.config, DATE_ADDED_DEDUP_FLAG_KEY).await;
        clear_flag(&redis.config).await;

        let archives = ArchiveRepository::new(redis.archive.clone());
        let id = "9".repeat(40);
        archives
            .save(&archive(
                &id,
                "artist:a,date_added:1742054739,date_added:1793928516",
            ))
            .await
            .unwrap();

        let report = dedupe_date_added_tags(&archives, &redis.config)
            .await
            .unwrap();
        assert!(!report.already_done);
        assert!(report.scanned >= 1);
        assert!(report.repaired >= 1);

        let saved = archives.get(&ArchiveId(id.clone())).await.unwrap().unwrap();
        assert_eq!(saved.tags, "artist:a,date_added:1793928516");

        // Second start: the flag short-circuits before any archive is read.
        let again = dedupe_date_added_tags(&archives, &redis.config)
            .await
            .unwrap();
        assert!(again.already_done);
        assert_eq!(again.scanned, 0);

        archives.delete(&ArchiveId(id)).await.unwrap();
        clear_flag(&redis.config).await;
        lock.release().await;
    }
}
