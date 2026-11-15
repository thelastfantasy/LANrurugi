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
}
