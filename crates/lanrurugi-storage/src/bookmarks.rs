//! Redis persistence for page-level reading bookmarks — additive, LANrurugi-only namespace with
//! no legacy equivalent (legacy's own "bookmark" concept was really just a category alias, see
//! `crates/lanrurugi-api/src/categories.rs`'s own history for the mechanism this replaces).
//!
//! A bookmark is `(archive_id, page)`, not just `archive_id` — a single archive can carry any
//! number of independent bookmarks, one per page, deliberately finer-grained than "this whole
//! book is bookmarked" (that's what a Category is for).
//!
//! Structural template: [`crate::ignored_group_suggestions`] (single Redis Hash on the
//! **`config`** logical DB, `thiserror` with only `Redis`/`Pool` variants since nothing here
//! round-trips through `serde_json`) — but keyed by `"{archive_id}:{page}"` instead of a
//! suggestion fingerprint, and the value is a bare Unix-seconds timestamp string rather than a
//! JSON blob, since there's nothing else worth storing per bookmark.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BookmarksError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, BookmarksError>;

const HASH_KEY: &str = "LANRURUGI_BOOKMARKS";
/// Single Redis String — the user's saved preference for how `BookmarkHoverGrid` orders pages
/// within its own popup (one of the four `HoverGridPageOrder` variants the frontend defines, e.g.
/// `"bookmarkedAtDesc"`). Stored verbatim as whatever string the frontend sends — this crate
/// doesn't validate it against a fixed enum of its own, same "server trusts the frontend's own
/// closed set of values" posture `settings.rs`'s theme field already takes.
const PAGE_ORDER_KEY: &str = "LANRURUGI_BOOKMARK_HOVER_PAGE_ORDER";
/// Separate Hash, `archive_id → Unix-seconds timestamp` of that archive's own most recent
/// bookmark *event* (add or remove) — distinct from any individual bookmark's own `bookmarked_at`
/// (which only ever moves forward on `add`, and is silently orphaned by a `remove` that happens
/// to delete the very bookmark that held the max). Written by both `add` and `remove`, and the
/// field itself is deleted (not just left stale) the moment an archive's last remaining bookmark
/// is removed — an archive with zero bookmarks has no business appearing in `/bookmarks` at all,
/// so there's nothing for a lingering timestamp to usefully mean once that happens. This is the
/// sort key `GET /bookmarks`'s default `sort=bookmarked_at` actually orders by (despite the query
/// param's own name, kept as-is for API back-compat — see `bookmarks.rs` in `lanrurugi-api`).
const UPDATED_AT_HASH_KEY: &str = "LANRURUGI_BOOKMARKS_UPDATED_AT";

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub archive_id: String,
    pub page: u32,
    pub bookmarked_at: u64,
}

fn field(archive_id: &str, page: u32) -> String {
    format!("{archive_id}:{page}")
}

/// Splits a `"{archive_id}:{page}"` field back into its parts — `rsplit_once` (not
/// `split_once`) so the page number always comes from the *last* `:`-delimited segment,
/// regardless of what an archive id itself could ever contain.
fn parse_field(raw: &str) -> Option<(String, u32)> {
    let (archive_id, page) = raw.rsplit_once(':')?;
    Some((archive_id.to_string(), page.parse().ok()?))
}

#[derive(Clone)]
pub struct BookmarksRepository {
    pool: Pool,
}

impl BookmarksRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Every bookmark across every archive — malformed individual entries (should never happen
    /// outside manual Redis tampering) are skipped rather than failing the whole read, same
    /// posture as `ignored_group_suggestions::list_all`.
    pub async fn list_all(&self) -> Result<Vec<Bookmark>> {
        let mut conn = self.pool.get().await?;
        let raw: HashMap<String, String> = conn.hgetall(HASH_KEY).await?;
        Ok(raw
            .into_iter()
            .filter_map(|(f, v)| {
                let (archive_id, page) = parse_field(&f)?;
                let bookmarked_at = v.parse().ok()?;
                Some(Bookmark {
                    archive_id,
                    page,
                    bookmarked_at,
                })
            })
            .collect())
    }

    pub async fn is_bookmarked(&self, archive_id: &str, page: u32) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        Ok(conn.hexists(HASH_KEY, field(archive_id, page)).await?)
    }

    /// A single archive's own bookmarks (ascending isn't guaranteed here — callers that need
    /// sorted `page` numbers, e.g. the `GET /bookmarks/{id}` endpoint, sort after calling this).
    /// No separate index by archive_id exists — same full-Hash-scan-then-filter cost as
    /// `list_all`, acceptable at this data's expected scale.
    pub async fn list_for_archive(&self, archive_id: &str) -> Result<Vec<Bookmark>> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(|b| b.archive_id == archive_id)
            .collect())
    }

    /// Each archive_id's own `bookmarks_updated_at` — the sort key for the default
    /// (`sort=bookmarked_at`) ordering of `GET /bookmarks`. A direct read of `UPDATED_AT_HASH_KEY`
    /// (not derived from `list_all()`, unlike the individual-bookmark data this repository
    /// otherwise groups on the fly) — see that constant's own docs for why a per-archive event
    /// timestamp has to be tracked separately from any individual bookmark's own `bookmarked_at`.
    pub async fn latest_bookmark_per_archive(&self) -> Result<HashMap<String, u64>> {
        let mut conn = self.pool.get().await?;
        let raw: HashMap<String, String> = conn.hgetall(UPDATED_AT_HASH_KEY).await?;
        Ok(raw
            .into_iter()
            .filter_map(|(archive_id, v)| Some((archive_id, v.parse().ok()?)))
            .collect())
    }

    pub async fn add(&self, archive_id: &str, page: u32, bookmarked_at: u64) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .hset(HASH_KEY, field(archive_id, page), bookmarked_at.to_string())
            .await?;
        let _: () = conn
            .hset(UPDATED_AT_HASH_KEY, archive_id, bookmarked_at.to_string())
            .await?;
        Ok(())
    }

    /// Idempotent — un-bookmarking a page that isn't currently bookmarked is a no-op, not an
    /// error (matches `ignored_group_suggestions::unignore`'s own contract). `removed_at` still
    /// updates `UPDATED_AT_HASH_KEY` even on that no-op path — a caller invoking `remove` at all
    /// (regardless of whether the specific page was actually still bookmarked) is itself evidence
    /// of activity worth bumping the archive's sort position for, and idempotency only promises
    /// "the *bookmark* ends up absent either way", not "an already-absent bookmark leaves no
    /// trace anywhere".
    ///
    /// Deletes `UPDATED_AT_HASH_KEY`'s own field for this archive (rather than leaving
    /// `removed_at` stored there) once this was the archive's *last* remaining bookmark — an
    /// archive with zero bookmarks has no reason to appear in `/bookmarks` at all, so there's
    /// nothing for a lingering timestamp to usefully order there; checked by a fresh
    /// `list_for_archive` call *after* the deletion, not before, so it reflects the post-removal
    /// state.
    pub async fn remove(&self, archive_id: &str, page: u32, removed_at: u64) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(HASH_KEY, field(archive_id, page)).await?;
        if self.list_for_archive(archive_id).await?.is_empty() {
            let _: () = conn.hdel(UPDATED_AT_HASH_KEY, archive_id).await?;
        } else {
            let _: () = conn
                .hset(UPDATED_AT_HASH_KEY, archive_id, removed_at.to_string())
                .await?;
        }
        Ok(())
    }

    /// `None` when never set — the frontend's own `useHoverGridPageOrder` falls back to its
    /// default (`"pageAsc"`) in that case, matching this repository's other "absent means default"
    /// conventions rather than this crate needing to know what that default actually is.
    pub async fn hover_page_order(&self) -> Result<Option<String>> {
        let mut conn = self.pool.get().await?;
        Ok(conn.get(PAGE_ORDER_KEY).await?)
    }

    pub async fn set_hover_page_order(&self, order: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(PAGE_ORDER_KEY, order).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    #[test]
    fn field_round_trips_through_parse_field() {
        assert_eq!(
            parse_field(&field("abc123", 42)),
            Some(("abc123".to_string(), 42))
        );
    }

    #[test]
    fn parse_field_rejects_missing_colon() {
        assert_eq!(parse_field("no-colon-here"), None);
    }

    #[test]
    fn parse_field_rejects_non_numeric_page() {
        assert_eq!(parse_field("abc123:not-a-number"), None);
    }

    /// Clears both Hashes' fields for the given archives — every test needs this at the start (a
    /// previous failed run can leave stale fields behind) and it's simplest to always leave both
    /// clean at the end too, rather than relying on each test's own `remove` calls to do it
    /// (`remove`'s own `UPDATED_AT_HASH_KEY` cleanup only fires once an archive's bookmarks are
    /// *actually* empty, which a test that intentionally leaves some behind, if any ever did,
    /// wouldn't trigger).
    async fn clear_test_archives(pool: &Pool, archive_ids: &[&str], pages: &[(&str, u32)]) {
        let mut conn = pool.get().await.unwrap();
        let fields: Vec<String> = pages.iter().map(|(a, p)| field(a, *p)).collect();
        let _: () = deadpool_redis::redis::AsyncCommands::hdel(&mut conn, HASH_KEY, fields)
            .await
            .unwrap();
        let _: () = deadpool_redis::redis::AsyncCommands::hdel(
            &mut conn,
            UPDATED_AT_HASH_KEY,
            archive_ids.to_vec(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn round_trips_add_list_and_remove() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = BookmarksRepository::new(pool.clone());
        let pages = [("test-archive", 5), ("test-archive", 12)];
        clear_test_archives(&pool, &["test-archive"], &pages).await;

        repo.add("test-archive", 5, 1_700_000_000).await.unwrap();
        repo.add("test-archive", 12, 1_700_000_100).await.unwrap();

        assert!(repo.is_bookmarked("test-archive", 5).await.unwrap());
        assert!(!repo.is_bookmarked("test-archive", 999).await.unwrap());

        let all = repo.list_all().await.unwrap();
        let mut ours: Vec<_> = all
            .into_iter()
            .filter(|b| b.archive_id == "test-archive")
            .collect();
        ours.sort_by_key(|b| b.page);
        assert_eq!(ours.len(), 2);
        assert_eq!(ours[0].page, 5);
        assert_eq!(ours[0].bookmarked_at, 1_700_000_000);
        assert_eq!(ours[1].page, 12);

        repo.remove("test-archive", 5, 1_700_000_200).await.unwrap();
        assert!(!repo.is_bookmarked("test-archive", 5).await.unwrap());

        // Idempotent.
        repo.remove("test-archive", 5, 1_700_000_300).await.unwrap();

        repo.remove("test-archive", 12, 1_700_000_400)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_for_archive_only_returns_that_archives_bookmarks() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = BookmarksRepository::new(pool.clone());
        let pages = [("archive-a", 1), ("archive-a", 2), ("archive-b", 1)];
        clear_test_archives(&pool, &["archive-a", "archive-b"], &pages).await;

        repo.add("archive-a", 1, 1_700_000_000).await.unwrap();
        repo.add("archive-a", 2, 1_700_000_100).await.unwrap();
        repo.add("archive-b", 1, 1_700_000_200).await.unwrap();

        let mut a_pages: Vec<u32> = repo
            .list_for_archive("archive-a")
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.page)
            .collect();
        a_pages.sort_unstable();
        assert_eq!(a_pages, vec![1, 2]);

        let b_pages: Vec<u32> = repo
            .list_for_archive("archive-b")
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.page)
            .collect();
        assert_eq!(b_pages, vec![1]);

        repo.remove("archive-a", 1, 1_700_000_300).await.unwrap();
        repo.remove("archive-a", 2, 1_700_000_400).await.unwrap();
        repo.remove("archive-b", 1, 1_700_000_500).await.unwrap();
    }

    #[tokio::test]
    async fn latest_bookmark_per_archive_reads_bookmarks_updated_at_not_individual_bookmarked_at() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = BookmarksRepository::new(pool.clone());
        let pages = [("archive-c", 1), ("archive-c", 2)];
        clear_test_archives(&pool, &["archive-c"], &pages).await;

        // The individual bookmark carries an *earlier* timestamp than the archive-level
        // `bookmarks_updated_at` this same `add` call also writes — proving the sort key comes
        // from the latter, not (as an earlier version of this method worked) the max of every
        // surviving bookmark's own `bookmarked_at`.
        repo.add("archive-c", 1, 1_700_000_100).await.unwrap();
        repo.add("archive-c", 2, 1_700_000_500).await.unwrap();

        let latest = repo.latest_bookmark_per_archive().await.unwrap();
        assert_eq!(latest.get("archive-c"), Some(&1_700_000_500));

        repo.remove("archive-c", 1, 1_700_000_600).await.unwrap();
        repo.remove("archive-c", 2, 1_700_000_700).await.unwrap();
    }

    #[tokio::test]
    async fn remove_bumps_bookmarks_updated_at_even_though_the_deleted_bookmark_was_older() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = BookmarksRepository::new(pool.clone());
        let pages = [("archive-d", 1), ("archive-d", 2)];
        clear_test_archives(&pool, &["archive-d"], &pages).await;

        repo.add("archive-d", 1, 1_700_000_100).await.unwrap();
        repo.add("archive-d", 2, 1_700_000_200).await.unwrap();

        // Deleting the *older* bookmark still bumps the archive's own `bookmarks_updated_at` past
        // both bookmarks' own `bookmarked_at` values — this is the actual bug report this feature
        // fixes: a `remove` used to leave the sort key untouched (or, if it happened to delete the
        // bookmark holding the max, make it go *backwards*) instead of reflecting that the archive
        // was just interacted with.
        repo.remove("archive-d", 1, 1_700_000_900).await.unwrap();
        let latest = repo.latest_bookmark_per_archive().await.unwrap();
        assert_eq!(latest.get("archive-d"), Some(&1_700_000_900));

        repo.remove("archive-d", 2, 1_700_001_000).await.unwrap();
    }

    #[tokio::test]
    async fn removing_the_last_bookmark_deletes_bookmarks_updated_at_entirely() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = BookmarksRepository::new(pool.clone());
        let pages = [("archive-e", 1)];
        clear_test_archives(&pool, &["archive-e"], &pages).await;

        repo.add("archive-e", 1, 1_700_000_100).await.unwrap();
        assert!(repo
            .latest_bookmark_per_archive()
            .await
            .unwrap()
            .contains_key("archive-e"));

        repo.remove("archive-e", 1, 1_700_000_200).await.unwrap();
        assert!(!repo
            .latest_bookmark_per_archive()
            .await
            .unwrap()
            .contains_key("archive-e"));
    }
}
