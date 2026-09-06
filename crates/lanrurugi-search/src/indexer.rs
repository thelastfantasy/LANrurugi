//! Search-index maintenance on the **search** logical DB (verified:
//! `~/LANraragi/lib/LANraragi/Utils/Database.pm::update_indexes`/`set_title`/`set_isnew`,
//! `Shinobu.pm::add_archive_to_redis`). Called from `lanrurugi-scanner` on new-archive
//! cataloguing and from `lanrurugi-api` on metadata/isnew updates, so both paths keep the search
//! indexes in sync with the archive DB the same way legacy's callers do.
//!
//! Tags treated as "basic" (don't count toward `LRR_UNTAGGED` membership) match
//! `archives.rs::has_meaningful_tags`'s list exactly — kept in one place there and mirrored here
//! only for the untagged-set bookkeeping, not duplicated search logic.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use thiserror::Error;

use crate::keys::{NEW_KEY, TANKGROUPED_KEY, TITLES_KEY, UNTAGGED_KEY};

#[derive(Debug, Error)]
pub enum IndexerError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, IndexerError>;

const BASIC_NAMESPACES: &[&str] = &[
    "artist:",
    "parody:",
    "series:",
    "language:",
    "event:",
    "group:",
    "date_added:",
    "timestamp:",
    "source:",
];

fn has_meaningful_tags(tags: &str) -> bool {
    tags.split(',').any(|t| {
        let t = t.trim().to_ascii_lowercase();
        !t.is_empty() && !BASIC_NAMESPACES.iter().any(|ns| t.starts_with(ns))
    })
}

/// Registers a freshly-catalogued archive in every search-side set it should start in: title
/// index, untagged (new archives have no tags yet), not-in-a-tank, and new-flag.
///
/// Issued as a single atomic pipeline (`MULTI`/`EXEC`), not four sequential `.await`s — a
/// mid-sequence failure (a transient connection hiccup, timeout, ...) used to leave the archive
/// half-indexed: e.g. present in `LRR_UNTAGGED` but missing from `LRR_TANKGROUPED`/`LRR_NEW`,
/// which made it permanently invisible to the default (`groupby_tanks=true`) search despite
/// `ArchiveRepository::save` having already succeeded — a real "ghost record" observed in
/// practice, not just a theoretical race. All four writes now succeed or fail together.
pub async fn index_new_archive(search_pool: &Pool, id: &str, title: &str) -> Result<()> {
    let mut conn = search_pool.get().await?;
    let title_key = format!("{}\0{}", title.to_lowercase(), id);
    let _: () = deadpool_redis::redis::pipe()
        .atomic()
        .zadd(TITLES_KEY, title_key, 0)
        .sadd(UNTAGGED_KEY, id)
        .sadd(TANKGROUPED_KEY, id)
        .sadd(NEW_KEY, id)
        .query_async(&mut conn)
        .await?;
    Ok(())
}

/// Strips a deleted archive out of every search-side set/index it could be a member of: title
/// index, untagged, not-in-a-tank, new-flag, and every `INDEX_<tag>` set for its current tags.
///
/// Legacy's own `delete_archive` (`~/LANraragi/lib/LANraragi/Model/Archive.pm:375-415`) never
/// cleans these up either — it only `DEL`s the archive hash itself — so this is a real
/// improvement over legacy's own (buggy) behavior, not a parity requirement. Without it, a
/// deleted id lingers in `LRR_TANKGROUPED`/`LRR_UNTAGGED`/`LRR_NEW`/`INDEX_<tag>` forever: default
/// search silently returns a smaller `data` array than `recordsTotal` claims whenever it's paged
/// past, and endpoints that pick a candidate id at random (`/search/random`) can select the ghost
/// id, fail the `ArchiveRepository::get` lookup for it, and surface as an inexplicably empty
/// result — observed in practice, not just theoretical.
pub async fn remove_archive_index(
    search_pool: &Pool,
    id: &str,
    title: &str,
    tags: &str,
) -> Result<()> {
    let mut conn = search_pool.get().await?;
    let title_key = format!("{}\0{}", title.to_lowercase(), id);
    let mut pipe = deadpool_redis::redis::pipe();
    pipe.atomic()
        .zrem(TITLES_KEY, title_key)
        .srem(UNTAGGED_KEY, id)
        .srem(TANKGROUPED_KEY, id)
        .srem(NEW_KEY, id);
    for tag in tags.split(',') {
        let tag = tag.trim().to_ascii_lowercase();
        if !tag.is_empty() {
            pipe.srem(format!("INDEX_{tag}"), id);
        }
    }
    let _: () = pipe.query_async(&mut conn).await?;
    Ok(())
}

/// Strips a single title-index entry (legacy Tankoubon delete's own `LRR_TITLES` cleanup — an
/// archive's own delete path removes it as part of `remove_archive_index`'s bigger multi-set
/// pipeline instead, since an archive has more than just a title entry to clean up).
pub async fn remove_title_index(search_pool: &Pool, id: &str, title: &str) -> Result<()> {
    let mut conn = search_pool.get().await?;
    let key = format!("{}\0{}", title.to_lowercase(), id);
    let _: () = conn.zrem(TITLES_KEY, key).await?;
    Ok(())
}

/// Moves an archive's title-index entry when its title changes (legacy `set_title`).
pub async fn update_title_index(
    search_pool: &Pool,
    id: &str,
    old_title: &str,
    new_title: &str,
) -> Result<()> {
    if old_title == new_title {
        return Ok(());
    }
    let mut conn = search_pool.get().await?;
    let old_key = format!("{}\0{}", old_title.to_lowercase(), id);
    let new_key = format!("{}\0{}", new_title.to_lowercase(), id);
    let _: () = conn.zrem(TITLES_KEY, old_key).await?;
    let _: () = conn.zadd(TITLES_KEY, new_key, 0).await?;
    Ok(())
}

/// Updates `INDEX_<tag>` membership and `LRR_UNTAGGED` when an archive's tags change (legacy
/// `update_indexes`; `LRR_STATS` tag-popularity counters and the `source:` URL map are skipped —
/// display/statistics niceties, not required for search correctness).
pub async fn update_tag_indexes(
    search_pool: &Pool,
    id: &str,
    old_tags: &str,
    new_tags: &str,
) -> Result<()> {
    let mut conn = search_pool.get().await?;
    for tag in old_tags.split(',') {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        let _: () = conn.srem(format!("INDEX_{tag}"), id).await?;
    }
    for tag in new_tags.split(',') {
        let tag = tag.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        let _: () = conn.sadd(format!("INDEX_{tag}"), id).await?;
    }

    if has_meaningful_tags(new_tags) {
        let _: () = conn.srem(UNTAGGED_KEY, id).await?;
    } else {
        let _: () = conn.sadd(UNTAGGED_KEY, id).await?;
    }
    Ok(())
}

/// Toggles `LRR_NEW` membership (legacy `set_isnew`).
pub async fn set_isnew_index(search_pool: &Pool, id: &str, isnew: bool) -> Result<()> {
    let mut conn = search_pool.get().await?;
    if isnew {
        let _: () = conn.sadd(NEW_KEY, id).await?;
    } else {
        let _: () = conn.srem(NEW_KEY, id).await?;
    }
    Ok(())
}

/// Keeps `LRR_TANKGROUPED` consistent with a Tankoubon's own member-archive list whenever it
/// changes (bulk archive-list replace, single add, single remove — see `TANKGROUPED_KEY`'s own
/// doc comment for the set's real semantics). Does **not** touch the tank's own id — that's
/// [`add_tank_to_index`]/[`remove_tank_from_index`]'s job, called once at create/delete time only,
/// deliberately independent of membership count: a Tankoubon with zero members is still a real,
/// user-visible record (until explicitly deleted), not something that should vanish from the
/// default grouped search the moment its last archive is removed — a tank that silently disappears
/// the instant it empties out has no discoverable path back for the user to repopulate or delete
/// it, becoming unreachable garbage instead of an intentionally-empty placeholder. Matches how its
/// `LRR_TITLES` entry is also written unconditionally at creation, not conditioned on membership.
///
/// Takes pre-computed deltas rather than diffing a before/after list itself: `joined` (archives
/// pulled out of the pool because they're now folded into this tank) and `left` (archives put
/// back because they no longer are) — because "no longer a member of *this* tank" isn't the same
/// as "should rejoin the pool." An archive can belong to more than one Tankoubon at once (real,
/// observed data, not just a theoretical edge case), so a caller must first confirm an archive
/// isn't still a member of some *other* tank before including it in `left` — matches legacy's own
/// `get_tankoubons_containing_archive` guard in `delete_tankoubon` — otherwise it would wrongly
/// reappear as a standalone search result while still visually folded into its other tank(s).
pub async fn sync_tank_membership(
    search_pool: &Pool,
    joined: &[String],
    left: &[String],
) -> Result<()> {
    if joined.is_empty() && left.is_empty() {
        return Ok(());
    }
    let mut conn = search_pool.get().await?;
    let mut pipe = deadpool_redis::redis::pipe();
    pipe.atomic();
    for id in joined {
        pipe.srem(TANKGROUPED_KEY, id.as_str());
    }
    for id in left {
        pipe.sadd(TANKGROUPED_KEY, id.as_str());
    }
    let _: () = pipe.query_async(&mut conn).await?;
    Ok(())
}

/// Adds a freshly-created Tankoubon's own id to `LRR_TANKGROUPED` — called once, at creation,
/// unconditionally of whether it has any members yet (see [`sync_tank_membership`]'s own docs for
/// why membership count must never gate this). The counterpart to [`remove_tank_from_index`].
pub async fn add_tank_to_index(search_pool: &Pool, tank_id: &str) -> Result<()> {
    let mut conn = search_pool.get().await?;
    let _: () = conn.sadd(TANKGROUPED_KEY, tank_id).await?;
    Ok(())
}

/// Removes a Tankoubon's own id from `LRR_TANKGROUPED` on outright deletion — the only thing that
/// should ever make a tank disappear from the default grouped search, not merely emptying it out.
pub async fn remove_tank_from_index(search_pool: &Pool, tank_id: &str) -> Result<()> {
    let mut conn = search_pool.get().await?;
    let _: () = conn.srem(TANKGROUPED_KEY, tank_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = format!("{}/3", base.trim_end_matches('/'));
        lanrurugi_storage::test_support::test_pool_for_url(&url).await
    }

    #[tokio::test]
    async fn new_archive_lands_in_untagged_new_and_ungrouped_sets() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let id = "f".repeat(40);
        index_new_archive(&pool, &id, "My Title").await.unwrap();

        let mut conn = pool.get().await.unwrap();
        let untagged: bool = conn.sismember(UNTAGGED_KEY, &id).await.unwrap();
        let is_new: bool = conn.sismember(NEW_KEY, &id).await.unwrap();
        let ungrouped: bool = conn.sismember(TANKGROUPED_KEY, &id).await.unwrap();
        assert!(untagged);
        assert!(is_new);
        assert!(ungrouped);

        let _: () = conn.srem(UNTAGGED_KEY, &id).await.unwrap();
        let _: () = conn.srem(NEW_KEY, &id).await.unwrap();
        let _: () = conn.srem(TANKGROUPED_KEY, &id).await.unwrap();
        let _: () = conn
            .zrem(TITLES_KEY, format!("my title\0{id}"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn remove_archive_index_strips_every_set_and_tag_index() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let id = "d".repeat(40);
        index_new_archive(&pool, &id, "Ghost Title").await.unwrap();
        update_tag_indexes(&pool, &id, "", "artist:jane")
            .await
            .unwrap();

        remove_archive_index(&pool, &id, "Ghost Title", "artist:jane")
            .await
            .unwrap();

        let mut conn = pool.get().await.unwrap();
        let untagged: bool = conn.sismember(UNTAGGED_KEY, &id).await.unwrap();
        let is_new: bool = conn.sismember(NEW_KEY, &id).await.unwrap();
        let ungrouped: bool = conn.sismember(TANKGROUPED_KEY, &id).await.unwrap();
        let in_tag_index: bool = conn.sismember("INDEX_artist:jane", &id).await.unwrap();
        let title_score: Option<f64> = conn
            .zscore(TITLES_KEY, format!("ghost title\0{id}"))
            .await
            .unwrap();
        assert!(
            !untagged,
            "delete should remove the untagged membership too"
        );
        assert!(!is_new);
        assert!(!ungrouped);
        assert!(!in_tag_index);
        assert!(title_score.is_none());
    }

    #[tokio::test]
    async fn tag_update_moves_id_between_index_and_untagged_sets() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let id = "e".repeat(40);
        let mut conn = pool.get().await.unwrap();
        let _: () = conn.sadd(UNTAGGED_KEY, &id).await.unwrap();

        update_tag_indexes(&pool, &id, "", "adventure,artist:jane")
            .await
            .unwrap();

        let untagged: bool = conn.sismember(UNTAGGED_KEY, &id).await.unwrap();
        let in_adventure: bool = conn.sismember("INDEX_adventure", &id).await.unwrap();
        assert!(
            !untagged,
            "adventure is a meaningful tag, should clear untagged"
        );
        assert!(in_adventure);

        update_tag_indexes(&pool, &id, "adventure,artist:jane", "artist:jane")
            .await
            .unwrap();
        let untagged_again: bool = conn.sismember(UNTAGGED_KEY, &id).await.unwrap();
        let still_in_adventure: bool = conn.sismember("INDEX_adventure", &id).await.unwrap();
        assert!(
            untagged_again,
            "only a basic tag remains, should be untagged again"
        );
        assert!(!still_in_adventure);

        let _: () = conn.srem("INDEX_artist:jane", &id).await.unwrap();
        let _: () = conn.srem(UNTAGGED_KEY, &id).await.unwrap();
    }
}
