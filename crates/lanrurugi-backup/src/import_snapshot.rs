//! Redis persistence for the Time-Machine-style rollback snapshots [`crate::import_legacy::
//! import_from_legacy`] captures — one [`BackupDocument`] per LANraragi import, containing only
//! the records that import actually overwrote (see that function's own docs on why pre-write-only
//! is correct). Structural template: [`lanrurugi_storage::activity`] (same `thiserror` error enum
//! shape, same `Pool` field, same `LANRURUGI_TEST_REDIS_URL`-gated test convention, same
//! insertion-order `SORTED SET` index), minus the time-range querying that module needs and this
//! one doesn't — the only ordering this needs is "oldest first, for trimming" and "newest first,
//! for listing", both trivial `ZRANGE` directions over the same index.
//!
//! Retention is a fixed count (`MAX_SNAPSHOTS`), not a TTL — the whole point of this feature is a
//! deliberate, user-visible history the operator browses and picks from, not an auto-expiring
//! cache; a TTL would silently remove the one snapshot a user meant to come back to next week.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::build::BackupDocument;

#[derive(Debug, Error)]
pub enum ImportSnapshotStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, ImportSnapshotStorageError>;

/// Keep the 10 most recent snapshots — confirmed as the desired retention window (not a TTL, see
/// this module's own top-of-file docs) rather than something the caller configures.
const MAX_SNAPSHOTS: isize = 10;

const ORDER_KEY: &str = "LANRURUGI_IMPORT_SNAPSHOT_ORDER";
const IMPORT_COUNT_KEY: &str = "LANRURUGI_IMPORT_LEGACY_COUNT";

fn record_key(id: &str) -> String {
    format!("LANRURUGI_IMPORT_SNAPSHOT_{id}")
}

/// One stored rollback point, with just enough metadata to render a list row without ever
/// deserializing the (potentially large) `document` field — [`list_metadata`] never touches
/// `document` at all, only [`get`] does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportSnapshot {
    pub id: String,
    pub created_at: i64,
    pub archive_count: usize,
    pub category_count: usize,
    pub tankoubon_count: usize,
    pub stamp_count: usize,
    pub document: BackupDocument,
}

/// The same record minus `document` — what [`ImportSnapshotRepository::list_metadata`] returns,
/// since a list view never needs the full (potentially large) backup payload, only enough to
/// render a row and let the caller pick an id to [`ImportSnapshotRepository::get`] or
/// [`ImportSnapshotRepository::delete`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSnapshotMetadata {
    pub id: String,
    pub created_at: i64,
    pub archive_count: usize,
    pub category_count: usize,
    pub tankoubon_count: usize,
    pub stamp_count: usize,
}

impl From<&ImportSnapshot> for ImportSnapshotMetadata {
    fn from(s: &ImportSnapshot) -> Self {
        Self {
            id: s.id.clone(),
            created_at: s.created_at,
            archive_count: s.archive_count,
            category_count: s.category_count,
            tankoubon_count: s.tankoubon_count,
            stamp_count: s.stamp_count,
        }
    }
}

#[derive(Clone)]
pub struct ImportSnapshotRepository {
    pool: Pool,
}

impl ImportSnapshotRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Saves `document` as a new snapshot, then trims the index down to [`MAX_SNAPSHOTS`] —
    /// oldest-first, matching the "most recent N" retention this module's own docs describe. A
    /// caller with an empty `document` (an import that touched nothing — e.g. every record was
    /// skipped, or the whole document matched nothing) should not call this at all; this method
    /// itself does not special-case emptiness.
    pub async fn save(&self, now: i64, document: BackupDocument) -> Result<ImportSnapshot> {
        let snapshot = ImportSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now,
            archive_count: document.archives.len(),
            category_count: document.categories.len(),
            tankoubon_count: document.tankoubons.len(),
            stamp_count: document.stamps.len(),
            document,
        };

        let mut conn = self.pool.get().await?;
        let key = record_key(&snapshot.id);
        let raw = serde_json::to_string(&snapshot)
            .map_err(|e| ImportSnapshotStorageError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.zadd(ORDER_KEY, &snapshot.id, now).await?;

        self.trim(&mut conn).await?;

        Ok(snapshot)
    }

    /// Removes the oldest entries beyond [`MAX_SNAPSHOTS`] — called once per [`save`], since that
    /// is the only path that can push the count over the limit.
    async fn trim(&self, conn: &mut deadpool_redis::Connection) -> Result<()> {
        let count: isize = conn.zcard(ORDER_KEY).await?;
        let overflow = count - MAX_SNAPSHOTS;
        if overflow <= 0 {
            return Ok(());
        }
        let stale_ids: Vec<String> = conn.zrange(ORDER_KEY, 0, overflow - 1).await?;
        for id in stale_ids {
            let _: () = conn.del(record_key(&id)).await?;
            let _: () = conn.zrem(ORDER_KEY, &id).await?;
        }
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<ImportSnapshot>> {
        let mut conn = self.pool.get().await?;
        let key = record_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| ImportSnapshotStorageError::Json(key, e)),
        }
    }

    /// Newest first — the natural order for a "recent activity" list; a `ZREVRANGE` over the same
    /// index [`save`] maintains, not a second sort pass over already-fetched records.
    pub async fn list_metadata(&self) -> Result<Vec<ImportSnapshotMetadata>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(ORDER_KEY, 0, -1).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(snapshot) = self.get(&id).await? {
                out.push(ImportSnapshotMetadata::from(&snapshot));
            }
        }
        Ok(out)
    }

    /// Idempotent — deleting an already-absent id is a no-op, not an error (same convention
    /// `api_tokens.rs::delete` and `activity.rs::delete` already use).
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(record_key(id)).await?;
        let _: () = conn.zrem(ORDER_KEY, id).await?;
        Ok(())
    }

    /// Increments and returns the running count of LANraragi imports this instance has ever run
    /// (`queue_import_legacy` calls this once per completed import, regardless of `on_existing`
    /// mode or whether anything actually changed) — a plain Redis `INCR`, not tied to
    /// [`MAX_SNAPSHOTS`]'s 10-entry retention window at all, since the point of this counter is to
    /// tell the frontend "has this operator done this before" for a one-time-per-import warning
    /// ("you're about to import again — consider a full backup first"), which must stay accurate
    /// even after old snapshots have long since been trimmed away.
    pub async fn increment_import_count(&self) -> Result<i64> {
        let mut conn = self.pool.get().await?;
        Ok(conn.incr(IMPORT_COUNT_KEY, 1).await?)
    }

    pub async fn import_count(&self) -> Result<i64> {
        let mut conn = self.pool.get().await?;
        let count: Option<i64> = conn.get(IMPORT_COUNT_KEY).await?;
        Ok(count.unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::BackupArchive;

    async fn test_pool() -> Option<Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = format!("{}/0", base.trim_end_matches('/'));
        lanrurugi_storage::test_support::test_pool_for_url(&url).await
    }

    fn empty_document() -> BackupDocument {
        BackupDocument {
            archives: Vec::new(),
            categories: Vec::new(),
            tankoubons: Vec::new(),
            stamps: Vec::new(),
            bookmarks: Vec::new(),
        }
    }

    fn document_with_one_archive() -> BackupDocument {
        let mut doc = empty_document();
        doc.archives.push(BackupArchive {
            arcid: "a".repeat(40),
            title: "Original Title".to_string(),
            tags: "artist:original".to_string(),
            summary: None,
            thumbhash: None,
            filename: "original.zip".to_string(),
        });
        doc
    }

    #[tokio::test]
    async fn saves_gets_lists_and_deletes_a_snapshot() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ImportSnapshotRepository::new(pool);

        let saved = repo.save(1_000, document_with_one_archive()).await.unwrap();
        assert_eq!(saved.archive_count, 1);

        let fetched = repo.get(&saved.id).await.unwrap().unwrap();
        assert_eq!(fetched.document.archives[0].title, "Original Title");

        let list = repo.list_metadata().await.unwrap();
        assert!(list.iter().any(|m| m.id == saved.id));

        repo.delete(&saved.id).await.unwrap();
        assert_eq!(repo.get(&saved.id).await.unwrap(), None);

        // Idempotent.
        repo.delete(&saved.id).await.unwrap();
    }

    #[tokio::test]
    async fn list_metadata_is_newest_first() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ImportSnapshotRepository::new(pool);

        let first = repo.save(1_000, empty_document()).await.unwrap();
        let second = repo.save(2_000, empty_document()).await.unwrap();

        let list = repo.list_metadata().await.unwrap();
        let first_pos = list.iter().position(|m| m.id == first.id);
        let second_pos = list.iter().position(|m| m.id == second.id);
        assert!(
            second_pos < first_pos,
            "newest (second) must sort before oldest (first)"
        );

        repo.delete(&first.id).await.unwrap();
        repo.delete(&second.id).await.unwrap();
    }

    #[tokio::test]
    async fn saving_beyond_the_retention_limit_trims_the_oldest() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ImportSnapshotRepository::new(pool);

        let mut ids = Vec::new();
        for i in 0..(MAX_SNAPSHOTS + 3) {
            let saved = repo.save(1_000 + i as i64, empty_document()).await.unwrap();
            ids.push(saved.id);
        }

        let list = repo.list_metadata().await.unwrap();
        assert_eq!(list.len() as isize, MAX_SNAPSHOTS);

        // The three oldest must be gone; the rest must still be present.
        for stale_id in &ids[..3] {
            assert_eq!(
                repo.get(stale_id).await.unwrap(),
                None,
                "oldest snapshots must be trimmed"
            );
        }
        for kept_id in &ids[3..] {
            assert!(
                repo.get(kept_id).await.unwrap().is_some(),
                "recent snapshots must survive trimming"
            );
        }

        for id in ids {
            repo.delete(&id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn increment_import_count_increases_and_persists() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ImportSnapshotRepository::new(pool.clone());

        // A global counter (not scoped to this test), so only assert relative growth, not an
        // absolute starting value — other tests/real usage on the same Redis may have already
        // incremented it.
        let before = repo.import_count().await.unwrap();
        let after_first = repo.increment_import_count().await.unwrap();
        assert_eq!(after_first, before + 1);
        let after_second = repo.increment_import_count().await.unwrap();
        assert_eq!(after_second, before + 2);
        assert_eq!(repo.import_count().await.unwrap(), before + 2);
    }
}
