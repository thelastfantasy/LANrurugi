//! A duplicate scan's lifecycle around `run_scan`: the phase and counts the page shows, the Redis
//! marker that lets a scan cut short by a restart start again on its own, and the tidy-up of what
//! earlier scans left behind.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{ArchiveMeta, GroupRecord};
use super::{cover, decisions, group_key, notify_changed, review, run_scan};
use crate::AppState;

/// Present while a scan runs: `{threshold, attempts}`. Left behind by a restart mid-scan, which is
/// how the next start knows to scan again.
const SCAN_MARKER_KEY: &str = "LANRURUGI_DUP_SCAN_RUNNING";
/// How many archives the last scan could not compare because they have no thumbnail yet.
const NO_COVER_KEY: &str = "LANRURUGI_DUP_NO_COVER";
/// A scan that keeps dying (say, the process runs out of memory mid-scan) is not restarted forever.
const MAX_RESUME_ATTEMPTS: u32 = 2;
/// A leftover patch temp file is only removed once it is this old, so a write in flight is safe.
const STALE_TEMP_AGE: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    Preparing,
    Covers,
    Grouping,
    Patches,
    Reviewing,
    Cleanup,
}

/// What the page shows of a running scan: the phase, and `done`/`total` where the phase counts
/// (covers to hash, groups to review); zero otherwise.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub done: usize,
    pub total: usize,
}

static PROGRESS: Mutex<Option<ScanProgress>> = Mutex::new(None);

pub fn set_progress(phase: ScanPhase, done: usize, total: usize) {
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = Some(ScanProgress { phase, done, total });
}

pub fn progress() -> Option<ScanProgress> {
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner())
}

fn clear_progress() {
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[derive(Debug, Serialize, Deserialize)]
struct ScanMarker {
    threshold: u32,
    attempts: u32,
}

/// Starts a scan in the background unless one is already queued or running (`None` then).
pub async fn start_scan(state: &AppState, threshold: u32) -> Option<String> {
    start_scan_attempt(state, threshold, 0).await
}

async fn start_scan_attempt(state: &AppState, threshold: u32, attempts: u32) -> Option<String> {
    if state
        .jobs
        .by_name("find_duplicates")
        .await
        .iter()
        .any(|job| !job.state.is_terminal())
    {
        return None;
    }
    let job_id = state.jobs.create("find_duplicates").await;
    write_marker(
        state,
        &ScanMarker {
            threshold,
            attempts,
        },
    )
    .await;
    let state = state.clone();
    let job = job_id.clone();
    tokio::spawn(async move {
        state.jobs.mark_active(&job).await;
        set_progress(ScanPhase::Preparing, 0, 0);
        notify_changed();
        let result = run_scan(&state, threshold, &job).await;
        // Cleared on failure too: a scan that fails is reported, not retried on the next start.
        clear_marker(&state).await;
        match result {
            Ok(found) => {
                state
                    .jobs
                    .finish(&job, json!({ "groups_found": found }))
                    .await
            }
            Err(message) => state.jobs.fail(&job, message).await,
        }
        clear_progress();
        notify_changed();
    });
    Some(job_id)
}

/// On startup: starts again a scan the last run of the server did not finish. The cover hashes it
/// had already computed are cached, so this picks up roughly where it stopped.
pub async fn resume_interrupted(state: &AppState) {
    let Some(marker) = read_marker(state).await else {
        return;
    };
    if marker.attempts >= MAX_RESUME_ATTEMPTS {
        tracing::warn!(
            attempts = marker.attempts,
            "an interrupted duplicate scan kept failing to finish; not restarting it"
        );
        clear_marker(state).await;
        return;
    }
    tracing::info!(
        threshold = marker.threshold,
        "restarting an interrupted duplicate scan"
    );
    start_scan_attempt(state, marker.threshold, marker.attempts + 1).await;
}

/// A scan was cut short and has not been started again yet: the stored groups are incomplete.
pub async fn interrupted(state: &AppState, scanning: bool) -> bool {
    !scanning && read_marker(state).await.is_some()
}

async fn read_marker(state: &AppState) -> Option<ScanMarker> {
    let mut conn = state.redis.config.get().await.ok()?;
    let raw: Option<String> = conn.get(SCAN_MARKER_KEY).await.ok()?;
    serde_json::from_str(&raw?).ok()
}

async fn write_marker(state: &AppState, marker: &ScanMarker) {
    if let Ok(mut conn) = state.redis.config.get().await {
        let value = serde_json::to_string(marker).unwrap_or_default();
        let _: Result<(), _> = conn.set(SCAN_MARKER_KEY, value).await;
    }
}

async fn clear_marker(state: &AppState) {
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.del(SCAN_MARKER_KEY).await;
    }
}

pub async fn store_no_cover(state: &AppState, count: usize) {
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.set(NO_COVER_KEY, count).await;
    }
}

pub async fn no_cover(state: &AppState) -> usize {
    let Ok(mut conn) = state.redis.config.get().await else {
        return 0;
    };
    conn.get::<_, Option<usize>>(NO_COVER_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// After a finished scan: drops cover hashes of archives no longer in the library, and model
/// reviews and keep/discard choices of groups that no longer exist. "Not a duplicate" marks are
/// per pair and stay.
pub async fn prune(
    state: &AppState,
    library_ids: &HashSet<String>,
    groups: &[GroupRecord],
    metas: &[ArchiveMeta],
) {
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let group_keys: HashSet<String> = groups.iter().map(|g| group_key(&g.members)).collect();
    let review_keys: HashSet<String> = groups
        .iter()
        .flat_map(|g| review::keys_for(g, metas))
        .collect();

    let mut removed = 0usize;
    for (key, keep) in [
        (cover::COVER_HASH_KEY, library_ids),
        (review::REVIEW_KEY, &review_keys),
        (decisions::DECISIONS_KEY, &group_keys),
    ] {
        let fields: Vec<String> = conn.hkeys(key).await.unwrap_or_default();
        let stale: Vec<&String> = fields.iter().filter(|f| !keep.contains(*f)).collect();
        if !stale.is_empty() {
            let _: Result<(), _> = conn.hdel(key, &stale).await;
            removed += stale.len();
        }
    }
    if removed > 0 {
        tracing::info!(removed, "duplicate scan pruned stale cached entries");
    }
}

/// Removes patch temp files (`.<name>.<pid>.tmp`) a restart left beside archives. Only another
/// process's, and only ones older than an hour, so a write in progress is never touched.
pub fn sweep_patch_temps(files: &HashMap<String, String>) -> usize {
    let dirs: HashSet<PathBuf> = files
        .values()
        .filter_map(|file| Path::new(file).parent().map(Path::to_path_buf))
        .collect();
    let own = format!(".{}.tmp", std::process::id());
    let mut removed = 0;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !is_patch_temp_name(&name) || name.ends_with(&own) {
                continue;
            }
            let old_enough = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_TEMP_AGE);
            if old_enough && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// `.<file name>.<pid>.tmp`, as `patching::write_durably` names its temp files.
fn is_patch_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".tmp")) else {
        return false;
    };
    rest.rsplit_once('.').is_some_and(|(stem, pid)| {
        !stem.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_temp_names_are_recognised() {
        assert!(is_patch_temp_name(".book.patch.zip.1234.tmp"));
        assert!(!is_patch_temp_name("book.patch.zip.1234.tmp"));
        assert!(!is_patch_temp_name(".book.patch.zip.tmp"));
        assert!(!is_patch_temp_name(".book.zip.12a4.tmp"));
        assert!(!is_patch_temp_name("..1234.tmp"));
    }

    #[test]
    fn sweep_removes_only_old_foreign_temps() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("book.zip");
        std::fs::write(&archive, b"x").unwrap();
        let old = dir.path().join(".book.patch.zip.1.tmp");
        let fresh = dir.path().join(".book.patch.zip.2.tmp");
        let own = dir
            .path()
            .join(format!(".book.patch.zip.{}.tmp", std::process::id()));
        for path in [&old, &fresh, &own] {
            std::fs::write(path, b"x").unwrap();
        }
        let hours_ago = SystemTime::now() - Duration::from_secs(2 * 3600);
        for path in [&old, &own] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(hours_ago)
                .unwrap();
        }
        let files = HashMap::from([("a".to_string(), archive.to_string_lossy().into_owned())]);
        assert_eq!(sweep_patch_temps(&files), 1);
        assert!(!old.exists());
        assert!(fresh.exists(), "too recent: may still be written");
        assert!(own.exists(), "this process's own temp file");
        assert!(archive.exists());
    }
}
