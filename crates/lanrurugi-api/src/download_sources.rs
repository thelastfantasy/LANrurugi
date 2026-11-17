//! Which source URLs have already produced an archive.
//!
//! Content addressing (`020`) makes a re-download of an unchanged gallery land on the same
//! archive id, but only *after* the bytes are fetched and hashed again — a full re-download the
//! user did not ask for. This registry remembers the first archive a URL produced, so a second
//! attempt is refused before the plugin runs. It is deliberately advisory: a URL can legitimately
//! serve different bytes later (a live gallery gaining pages), which is what the queue row's
//! "download anyway" action is for.

use deadpool_redis::redis::AsyncCommands;

use crate::AppState;

/// Hash of `source URL -> archive id`, in the config database next to the queue itself.
pub const SOURCES_KEY: &str = "LANRURUGI_DOWNLOAD_SOURCES";

/// Remembers `url` as producing `archive_id`. `HSETNX`, so a later revision of the same URL does
/// not rewrite what the row will link to; empty URLs (local uploads) are ignored.
pub async fn record(state: &AppState, url: &str, archive_id: &str) {
    if url.is_empty() || !url.starts_with("http") {
        return;
    }
    let mut conn = match state.redis.config.get().await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(error = %e, "could not record the download source");
            return;
        }
    };
    let written: Result<bool, _> = conn.hset_nx(SOURCES_KEY, url, archive_id).await;
    if let Err(e) = written {
        tracing::warn!(error = %e, "could not record the download source");
    }
}

/// The archive `url` already produced, if that archive is still in the library (a deleted one
/// must not block a re-download).
pub async fn lookup(state: &AppState, url: &str) -> Option<String> {
    if url.is_empty() || !url.starts_with("http") {
        return None;
    }
    let mut conn = state.redis.config.get().await.ok()?;
    let found: Option<String> = conn.hget(SOURCES_KEY, url).await.ok()?;
    let id = found?;
    let archive = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId::from(id.clone()))
        .await
        .ok()
        .flatten()?;
    (!archive.file.trim().is_empty()).then_some(id)
}

/// Set once [`backfill_primary_sources`] has run, so later startups skip it.
const PRIMARY_BACKFILLED_KEY: &str = "LANRURUGI_PRIMARY_SOURCE_BACKFILLED";

/// Gives archives downloaded before `primary_source` existed the URL they were downloaded from,
/// as this registry recorded it — when that URL is still one of their `source:` tags and they have
/// no primary source yet. Runs once; returns how many archives it marked.
pub async fn backfill_primary_sources(state: &AppState) -> usize {
    let Ok(mut conn) = state.redis.config.get().await else {
        return 0;
    };
    let done: bool = conn.exists(PRIMARY_BACKFILLED_KEY).await.unwrap_or(false);
    if done {
        return 0;
    }
    let entries: std::collections::HashMap<String, String> =
        conn.hgetall(SOURCES_KEY).await.unwrap_or_default();
    drop(conn);
    let mut marked = 0;
    for (url, id) in entries {
        let source = crate::plugins::trim_url(&url);
        let tag = format!("source:{source}");
        let mut set = false;
        let _ = state
            .repos
            .archives
            .update(&lanrurugi_core::ids::ArchiveId::from(id), |a| {
                if a.primary_source.is_none() && a.tags.split(',').any(|t| t.trim() == tag) {
                    a.primary_source = Some(source);
                    set = true;
                }
            })
            .await;
        marked += usize::from(set);
    }
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.set(PRIMARY_BACKFILLED_KEY, 1).await;
    }
    marked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_non_http_source_is_never_recorded() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        record(&state, "local_upload", "abc").await;
        assert!(lookup(&state, "local_upload").await.is_none());
    }

    /// #302: the registry gives an already-downloaded archive its primary source, but only while
    /// that URL is still one of its `source:` tags, and never replaces one it already has.
    #[tokio::test]
    async fn the_backfill_marks_the_url_an_archive_was_downloaded_from() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        let archive =
            |id: &str, tags: &str, primary: Option<&str>| lanrurugi_core::entities::Archive {
                id: lanrurugi_core::ids::ArchiveId(id.to_string()),
                name: "n".into(),
                title: "t".into(),
                file: "/x.zip".into(),
                tags: tags.into(),
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
                primary_source: primary.map(str::to_string),
            };
        let downloaded = "302c".repeat(10);
        let retagged = "302d".repeat(10);
        let marked = "302e".repeat(10);
        for a in [
            archive(
                &downloaded,
                "source:panda.chaika.moe/archive/1,source:e-hentai.org/g/1/a",
                None,
            ),
            archive(&retagged, "source:e-hentai.org/g/2/b", None),
            archive(
                &marked,
                "source:panda.chaika.moe/archive/3",
                Some("e-hentai.org/g/3/c"),
            ),
        ] {
            state.repos.archives.create_or_replace(&a).await.unwrap();
        }
        record(&state, "https://panda.chaika.moe/archive/1/", &downloaded).await;
        record(&state, "https://panda.chaika.moe/archive/2/", &retagged).await;
        record(&state, "https://panda.chaika.moe/archive/3/", &marked).await;
        let mut conn = state.redis.config.get().await.unwrap();
        let _: () = conn.del(PRIMARY_BACKFILLED_KEY).await.unwrap();
        drop(conn);

        backfill_primary_sources(&state).await;
        let primary = |id: &str| {
            let state = state.clone();
            let id = id.to_string();
            async move {
                state
                    .repos
                    .archives
                    .get(&lanrurugi_core::ids::ArchiveId(id))
                    .await
                    .unwrap()
                    .unwrap()
                    .primary_source
            }
        };
        assert_eq!(
            primary(&downloaded).await.as_deref(),
            Some("panda.chaika.moe/archive/1")
        );
        assert_eq!(primary(&retagged).await, None);
        assert_eq!(
            primary(&marked).await.as_deref(),
            Some("e-hentai.org/g/3/c")
        );
        // Once only.
        assert_eq!(backfill_primary_sources(&state).await, 0);
        for id in [&downloaded, &retagged, &marked] {
            let _ = state
                .repos
                .archives
                .delete(&lanrurugi_core::ids::ArchiveId(id.clone()))
                .await;
        }
    }
}
