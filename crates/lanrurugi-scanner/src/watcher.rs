//! `notify`-based file watcher (Shinobu replacement, research.md §6).
//!
//! **Verified against source** (`~/LANraragi/lib/Shinobu.pm`): legacy's watcher polls
//! `new_events()` once per second (not an explicit multi-second debounce timer as one might
//! assume from the name "Shinobu debounce") and filters on the extension list below, excluding
//! the `thumb` directory. The actual "wait for a still-being-written file" behavior (FR-006)
//! lives in `wait_until_stable` below, matching legacy's `add_to_filemap` exactly: retry opening
//! the file (unbounded — bounded in practice by the per-file ingestion timeout, T037), then wait
//! for the file to reach 512000 bytes (the hash sample size) or give up after 5 one-second
//! attempts, whichever comes first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::mpsc;

const WATCHED_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "lzma", "xz", "cbz", "cbr", "cb7", "cbt", "pdf", "epub", "zst",
];

pub fn is_watched_archive_path(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == "thumb") {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| WATCHED_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("failed to start filesystem watcher: {0}")]
    Notify(#[from] notify::Error),
}

/// Starts watching `library_path` for archive create/modify events, forwarding matching paths on
/// the returned channel. The `RecommendedWatcher` must be kept alive for as long as watching
/// should continue (dropping it stops the watch) — callers hold onto it in their app state.
pub fn watch(
    library_path: &Path,
) -> Result<(RecommendedWatcher, mpsc::UnboundedReceiver<PathBuf>), WatcherError> {
    let (tx, rx) = mpsc::unbounded_channel();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        use notify::EventKind;
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        for path in event.paths {
            if is_watched_archive_path(&path) {
                let _ = tx.send(path);
            }
        }
    })?;

    watcher.watch(library_path, RecursiveMode::Recursive)?;
    Ok((watcher, rx))
}

/// Legacy's sample size — a file must reach this many bytes before it's considered stable enough
/// to hash (verified: `Shinobu.pm::add_to_filemap`).
const STABILITY_SIZE_THRESHOLD: u64 = 512_000;
const STABILITY_MAX_ATTEMPTS: u32 = 5;
const STABILITY_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum StabilityError {
    #[error("file disappeared while waiting for it to stabilize")]
    Disappeared,
}

/// Waits for `path` to be openable and to reach the hashing sample size (or 5 one-second
/// attempts), matching legacy's partial-write handling (FR-006) exactly.
pub async fn wait_until_stable(path: &Path) -> Result<(), StabilityError> {
    loop {
        if !path.exists() {
            return Err(StabilityError::Disappeared);
        }
        if std::fs::File::open(path).is_ok() {
            break;
        }
        tokio::time::sleep(STABILITY_POLL_INTERVAL).await;
    }

    for attempt in 0..STABILITY_MAX_ATTEMPTS {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size >= STABILITY_SIZE_THRESHOLD || attempt == STABILITY_MAX_ATTEMPTS - 1 {
            break;
        }
        tokio::time::sleep(STABILITY_POLL_INTERVAL).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_extensions_match_legacy_and_exclude_thumb_dir() {
        assert!(is_watched_archive_path(Path::new("/library/foo.zip")));
        assert!(is_watched_archive_path(Path::new("/library/foo.CBZ")));
        assert!(!is_watched_archive_path(Path::new("/library/foo.txt")));
        assert!(!is_watched_archive_path(Path::new(
            "/library/thumb/foo.zip"
        )));
    }

    #[tokio::test]
    async fn wait_until_stable_returns_quickly_for_a_small_complete_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        write!(f, "small file, well under the sample threshold").unwrap();

        let start = std::time::Instant::now();
        wait_until_stable(f.path()).await.unwrap();
        // Still has to run through the up-to-5 size-check attempts since the file never reaches
        // 512000 bytes, but each attempt is 1s and it should bail after the 5th, not hang forever.
        assert!(start.elapsed() < Duration::from_secs(6));
    }

    #[tokio::test]
    async fn wait_until_stable_errors_if_file_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.zip");
        let err = wait_until_stable(&path).await.unwrap_err();
        assert!(matches!(err, StabilityError::Disappeared));
    }
}
