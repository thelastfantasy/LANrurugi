//! Memory of on-demand thumbnail generation failures, split by what the failure means.
//!
//! A thumbnail is generated lazily the first time a page (or the cover) is requested, and a
//! *failure* is not cached anywhere by the generator itself — so a file that can never produce a
//! thumbnail is re-opened and re-parsed on **every** page load that lists it. In the dev library
//! one truncated EPUB did exactly that: ~1 s of parsing per page load, five concurrent copies of
//! it during a single search page (seen in the tracker as a burst of
//! `on-demand thumbnail generation failed` warnings, each around the same second other endpoints
//! went from p50 single-digit ms to 1–2 s), and 124 warnings over three hours.
//!
//! [`lanrurugi_scanner::thumbnail::ThumbnailError::is_permanent`] draws the line between the two
//! cases, and this module stores each kind differently:
//!
//! - **Permanent** — the archive's own bytes are the problem (corrupt/truncated/encrypted
//!   container, unsupported format, a page that is not a decodable image, a book the worker calls
//!   unreadable). Remembered in [`THUMBNAIL_BLACKLIST_KEY`] *until the file itself changes*: the
//!   entry carries the size and mtime the failure was observed at, so a re-import, a repaired
//!   download or a replaced file silently retries, while an untouched broken file is opened once
//!   and never again. This is deliberately not a plain TTL: retrying a truncated ZIP every ten
//!   minutes forever is what made the burst.
//!
//! That fingerprint is not optional bookkeeping: an archive id is `SHA-1(first 512 KB [+ size])`
//! (`lanrurugi_storage::id`), so a file whose *tail* was truncated and later re-downloaded keeps
//! the same id and therefore the same key — the id alone cannot tell "broken" from "fixed". (The
//! same property is why [`clear_archive`] also runs when an archive is deleted: an id can come
//! back attached to different bytes.)
//! - **Transient** — a worker that is missing/restarting/crashed, a momentary I/O error, an
//!   encoder that ran out of memory. Remembered for [`THUMBNAIL_RETRY_TTL_SECS`] only, because the
//!   very next attempt plausibly works and must not need an admin action.
//!
//! Both layers are best-effort: a Redis outage makes them report "not blocked" so generation
//! behaves exactly as it did before this module existed.

use std::path::Path;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_scanner::thumbnail::ThumbnailError;
use lanrurugi_storage::keys::{
    thumbnail_retry_key, THUMBNAIL_BLACKLIST_KEY, THUMBNAIL_RETRY_TTL_SECS,
};
// The identity of one generate-able thumbnail (`"{id}:{page}"`, `0` = cover, generated from page 1)
// and the three "forget it" operations live in `lanrurugi_storage`, so every crate that deletes an
// archive record — this one, the scanner's overwrite policy, the server's startup repair — can drop
// the same entries.
use lanrurugi_storage::thumbnail_failures::{forget_all, forget_archive, forget_page, slot};

use crate::AppState;

/// `reason|size|mtime_ms` — the failure text plus the file fingerprint it was observed at. Split
/// from the right when decoding so an error message containing `|` still round-trips.
fn encode_entry(reason: &str, size: u64, mtime_ms: Option<u128>) -> String {
    match mtime_ms {
        Some(mtime) => format!("{reason}|{size}|{mtime}"),
        None => format!("{reason}|{size}|"),
    }
}

fn decode_entry(raw: &str) -> Option<(&str, u64, Option<u128>)> {
    let (rest, mtime) = raw.rsplit_once('|')?;
    let (reason, size) = rest.rsplit_once('|')?;
    let size: u64 = size.parse().ok()?;
    let mtime = if mtime.is_empty() {
        None
    } else {
        Some(mtime.parse().ok()?)
    };
    Some((reason, size, mtime))
}

/// The file's current size and mtime in milliseconds — the "did this file change?" fingerprint.
async fn fingerprint(file: &Path) -> Option<(u64, Option<u128>)> {
    let metadata = tokio::fs::metadata(file).await.ok()?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis());
    Some((metadata.len(), mtime))
}

fn fingerprint_matches(
    stored_size: u64,
    stored_mtime: Option<u128>,
    current: (u64, Option<u128>),
) -> bool {
    let (size, mtime) = current;
    match (stored_mtime, mtime) {
        // Both sides know the mtime: require both to be unchanged.
        (Some(stored), Some(current)) => stored_size == size && stored == current,
        // The filesystem did not report an mtime for one of them: size alone is the only signal.
        _ => stored_size == size,
    }
}

/// Why generation should be skipped right now, if it should. `None` means "try it".
///
/// The returned text is for logs/diagnostics — callers treat any `Some` as "serve the placeholder".
pub async fn blocked(state: &AppState, archive_id: &str, page: u32, file: &Path) -> Option<String> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return None;
    };

    let permanent: Option<String> = conn
        .hget(THUMBNAIL_BLACKLIST_KEY, slot(archive_id, page))
        .await
        .ok()
        .flatten();
    if let Some(raw) = permanent {
        if let Some((reason, stored_size, stored_mtime)) = decode_entry(&raw) {
            let current = fingerprint(file).await;
            let unchanged = match current {
                Some(current) => fingerprint_matches(stored_size, stored_mtime, current),
                // The file is gone entirely: this is no longer a "the bytes are broken" case, so
                // do not speak for it — the generator's own I/O error decides.
                None => false,
            };
            if unchanged {
                return Some(reason.to_string());
            }
            // The file changed since the failure (re-imported, repaired, replaced): its entry no
            // longer describes it. Drop it and let this attempt be the deciding one.
            let _: Result<(), _> = conn
                .hdel(THUMBNAIL_BLACKLIST_KEY, slot(archive_id, page))
                .await;
        }
    }

    let retry: Option<String> = conn
        .get(thumbnail_retry_key(archive_id, page))
        .await
        .ok()
        .flatten();
    retry
}

/// Records a failed generation in the layer its classification calls for.
pub async fn note_failure(
    state: &AppState,
    archive_id: &str,
    page: u32,
    file: &Path,
    error: &ThumbnailError,
) {
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    if error.is_permanent() {
        let (size, mtime) = fingerprint(file).await.unwrap_or((0, None));
        let entry = encode_entry(&error.to_string(), size, mtime);
        let _: Result<(), _> = conn
            .hset(THUMBNAIL_BLACKLIST_KEY, slot(archive_id, page), entry)
            .await;
        let _: Result<(), _> = conn.del(thumbnail_retry_key(archive_id, page)).await;
        tracing::debug!(
            id = %archive_id,
            page,
            error = %error,
            "thumbnail generation can never succeed for this file; remembering it until it changes"
        );
    } else {
        let _: Result<(), _> = conn
            .set_ex(
                thumbnail_retry_key(archive_id, page),
                error.to_string(),
                THUMBNAIL_RETRY_TTL_SECS as u64,
            )
            .await;
    }
}

/// Forgets every remembered failure for one archive, so the next request tries again — the escape
/// hatch an explicit regeneration (set-as-cover, the regen job) has to offer, and what every path
/// that deletes an archive record calls, since an id can later name different bytes.
pub async fn clear_archive(state: &AppState, archive_id: &str) {
    let _ = forget_archive(&state.redis.config, archive_id).await;
}

/// Forgets both layers for one page — the narrow form used when a single page is regenerated (the
/// reader's "set as cover" acts on one page, not the whole archive).
pub async fn clear_page(state: &AppState, archive_id: &str, page: u32) {
    let _ = forget_page(&state.redis.config, archive_id, page).await;
}

/// Forgets every permanent entry in the library — what a library-wide regeneration job does before
/// it starts, since it re-attempts every archive anyway.
pub async fn clear_all(state: &AppState) {
    let _ = forget_all(&state.redis.config).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_round_trips_even_when_the_reason_contains_a_pipe() {
        let raw = encode_entry("libarchive error: a|b|c", 42, Some(1_700_000_000_123));
        let (reason, size, mtime) = decode_entry(&raw).expect("decodes");
        assert_eq!(reason, "libarchive error: a|b|c");
        assert_eq!(size, 42);
        assert_eq!(mtime, Some(1_700_000_000_123));
    }

    #[test]
    fn an_entry_without_an_mtime_still_decodes() {
        let raw = encode_entry("I/O error: denied", 7, None);
        let (reason, size, mtime) = decode_entry(&raw).expect("decodes");
        assert_eq!(reason, "I/O error: denied");
        assert_eq!(size, 7);
        assert_eq!(mtime, None);
    }

    #[test]
    fn a_garbage_entry_decodes_to_nothing_rather_than_panicking() {
        assert!(decode_entry("no separators").is_none());
        assert!(decode_entry("reason|not-a-number|1").is_none());
        assert!(decode_entry("reason|1|not-a-number").is_none());
    }

    #[test]
    fn a_changed_file_no_longer_matches_its_remembered_failure() {
        assert!(fingerprint_matches(10, Some(100), (10, Some(100))));
        assert!(
            !fingerprint_matches(10, Some(100), (10, Some(101))),
            "a re-imported file (same size, new mtime) must be retried"
        );
        assert!(
            !fingerprint_matches(10, Some(100), (11, Some(100))),
            "a file that grew (a finished download) must be retried"
        );
        // No stored mtime: size is the only comparable signal.
        assert!(fingerprint_matches(10, None, (10, Some(100))));
        assert!(!fingerprint_matches(10, None, (11, Some(100))));
    }

    /// The two layers against a real Redis: this is the part the pure tests above cannot cover —
    /// the actual `HSET`/`HGET`/`SETEX`/`DEL` round trips, and that a permanent entry really does
    /// stop blocking once the file it describes has changed.
    #[tokio::test]
    async fn a_permanent_failure_blocks_until_the_file_changes_and_a_transient_one_is_cleared() {
        use lanrurugi_scanner::archive_format::ArchiveFormatError;

        let Some(state) = crate::plugins::tests::test_state().await else {
            return; // no test Redis configured in this environment
        };
        // A fixed, unique id: other tests in this binary share the same Redis hash.
        let id = "00000000000000000000000000000000000000f1";
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.zip");
        std::fs::write(&file, b"PK\x05\x06 truncated").unwrap();
        clear_archive(&state, id).await;

        // --- permanent: blocks, and survives a plain retry
        note_failure(
            &state,
            id,
            0,
            &file,
            &ThumbnailError::Archive(ArchiveFormatError::Libarchive("corrupt".into())),
        )
        .await;
        assert!(
            blocked(&state, id, 0, &file).await.is_some(),
            "a corrupt container must not be re-parsed on the next page load"
        );
        assert!(
            blocked(&state, id, 0, &file).await.is_some(),
            "asking again must not clear it"
        );
        // A *different* page of the same archive is its own slot.
        assert!(blocked(&state, id, 1, &file).await.is_none());

        // --- and stops blocking once the file itself changes (a repaired download)
        std::fs::write(&file, b"PK\x05\x06 truncated, but longer now").unwrap();
        assert!(
            blocked(&state, id, 0, &file).await.is_none(),
            "a file whose size/mtime changed must be retried"
        );

        // --- transient: blocks, then is cleared by an explicit regeneration
        note_failure(
            &state,
            id,
            2,
            &file,
            &ThumbnailError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        )
        .await;
        assert!(blocked(&state, id, 2, &file).await.is_some());
        clear_page(&state, id, 2).await;
        assert!(
            blocked(&state, id, 2, &file).await.is_none(),
            "an explicit regeneration must be able to clear the transient memory"
        );

        // --- deleting the archive forgets everything about it
        note_failure(&state, id, 3, &file, &ThumbnailError::NoPages).await;
        assert!(blocked(&state, id, 3, &file).await.is_some());
        clear_archive(&state, id).await;
        assert!(blocked(&state, id, 3, &file).await.is_none());
    }
}
