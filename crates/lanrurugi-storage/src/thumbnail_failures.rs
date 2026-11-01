//! Redis side of the thumbnail failure memory: the keys it lives in, and how a caller forgets it
//! when the archive it describes stops existing.
//!
//! The *policy* (which failures are permanent, how long a transient one is remembered) is
//! `lanrurugi_api::thumbnail_failures`'s, since it needs the generator's own error classification.
//! What lives here is only the part several crates must be able to do without depending on the API
//! crate: every code path that deletes an archive record has to drop its entries, and one of those
//! paths (`lanrurugi_scanner::pipeline`'s overwrite policy) runs in the scanner. Leaving an entry
//! behind is not harmless — an archive id is `SHA-1(first 512 KB [+ size])`
//! (`crate::id`), so a later file can reuse the id with different bytes and inherit a blacklist
//! written about a different file.
//!
//! Best-effort by contract: every function returns `Ok(())` when the key was already absent, and
//! callers are deleting a record that is already gone — a Redis hiccup must not undo that.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;

use crate::keys::{thumbnail_retry_key, THUMBNAIL_BLACKLIST_KEY};

/// The hash field naming one generate-able thumbnail: `0` is the cover, `n` a page. Shared with
/// `lanrurugi_api::thumbnail_failures`, which writes the entries this module drops.
pub fn slot(archive_id: &str, page: u32) -> String {
    format!("{archive_id}:{page}")
}

/// Forgets every remembered failure — permanent and transient — for one archive.
pub async fn forget_archive(
    pool: &Pool,
    archive_id: &str,
) -> Result<(), deadpool_redis::PoolError> {
    let mut conn = pool.get().await?;
    let fields: Vec<String> = conn
        .hkeys(THUMBNAIL_BLACKLIST_KEY)
        .await
        .unwrap_or_default();
    let prefix = format!("{archive_id}:");
    let stale: Vec<String> = fields
        .into_iter()
        .filter(|field| field.starts_with(&prefix))
        .collect();
    if !stale.is_empty() {
        let _: Result<(), _> = conn.hdel(THUMBNAIL_BLACKLIST_KEY, stale).await;
    }
    Ok(())
}

/// Forgets both layers for one page — used when a single page is regenerated on request.
pub async fn forget_page(
    pool: &Pool,
    archive_id: &str,
    page: u32,
) -> Result<(), deadpool_redis::PoolError> {
    let mut conn = pool.get().await?;
    let _: Result<(), _> = conn
        .hdel(THUMBNAIL_BLACKLIST_KEY, slot(archive_id, page))
        .await;
    let _: Result<(), _> = conn.del(thumbnail_retry_key(archive_id, page)).await;
    Ok(())
}

/// Forgets every permanent entry in the library — what a library-wide regeneration does first,
/// since it re-attempts every archive anyway.
pub async fn forget_all(pool: &Pool) -> Result<(), deadpool_redis::PoolError> {
    let mut conn = pool.get().await?;
    let _: Result<(), _> = conn.del(THUMBNAIL_BLACKLIST_KEY).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_names_the_archive_and_the_page() {
        assert_eq!(slot("abc", 0), "abc:0");
        assert_eq!(slot("abc", 12), "abc:12");
    }
}
