//! Finds archives whose file is damaged (#292): a background job that opens every archive's
//! container structure without decompressing any page, plus the list of what it found.
//!
//! The check is `list_pages` — for libarchive containers a header-only walk, for e-books the book
//! worker's own inspection — which is exactly what the reader needs to open an archive at all, so a
//! file that fails here is one the reader cannot show. Results are kept per archive in one Redis
//! hash, so a later scan only replaces what it re-checked. Archives whose self-heal already failed
//! (`heal_failed_at`) are listed too, since they are known broken without a scan.
//!
//! A finding also sets that same `heal_failed_at` marker, which is what the library's badge and
//! `is:damaged` read (#291); a file that opens again clears it and gets its page count back.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::ids::ArchiveId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/database/integrity-scan",
            get(status).post(start).delete(stop),
        )
        .route("/database/integrity-scan/archives/{id}", post(recheck))
}

/// `archive id -> Finding` for every archive the last scans found damaged.
const FINDINGS_KEY: &str = "LANRURUGI_INTEGRITY_FINDINGS";
/// Summary of the last finished scan.
const LAST_RUN_KEY: &str = "LANRURUGI_INTEGRITY_LAST_RUN";
/// Files checked per batch before progress, cancellation and load are checked again.
const BATCH: usize = 16;

struct Running {
    job: String,
    cancel: CancellationToken,
}

/// The running scan, if any: one at a time, so a second click joins the first.
static RUNNING: tokio::sync::Mutex<Option<Running>> = tokio::sync::Mutex::const_new(None);
static DONE: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Problem {
    /// The file is gone from disk.
    Missing,
    /// The container (or book) cannot be opened: truncated, corrupt, encrypted, unsupported.
    Unreadable,
    /// It opens, but holds no page at all.
    NoPages,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub problem: Problem,
    /// The reader-facing error text, untranslated.
    pub detail: String,
    pub checked_at: i64,
    /// File size when checked, next to the catalogued size: a shrunken file is the usual sign of
    /// a write that never finished.
    pub size_on_disk: Option<u64>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What one check of a file found.
pub(crate) enum Inspection {
    /// It opens and has this many pages.
    Readable {
        pages: usize,
        size: u64,
    },
    Damaged(Finding),
    /// A momentary I/O failure: says nothing about the file, so the next scan tries again.
    Unknown,
}

pub(crate) fn inspect(path: &std::path::Path) -> Inspection {
    let size_on_disk = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Inspection::Damaged(Finding {
                problem: Problem::Missing,
                detail: e.to_string(),
                checked_at: now_secs(),
                size_on_disk: None,
            })
        }
        Err(_) => return Inspection::Unknown,
    };
    let (problem, detail) = match lanrurugi_scanner::archive_format::list_pages(path) {
        Ok(pages) if pages.is_empty() => (Problem::NoPages, String::new()),
        Ok(pages) => {
            return Inspection::Readable {
                pages: pages.len(),
                size: size_on_disk,
            }
        }
        Err(e) if !e.is_permanent() => return Inspection::Unknown,
        Err(e) => (Problem::Unreadable, e.to_string()),
    };
    Inspection::Damaged(Finding {
        problem,
        detail,
        checked_at: now_secs(),
        size_on_disk: Some(size_on_disk),
    })
}

/// Checks one file; `None` means it opened and has pages, or could not be checked right now.
#[cfg(test)]
pub(crate) fn check_file(path: &std::path::Path) -> Option<Finding> {
    match inspect(path) {
        Inspection::Damaged(f) => Some(f),
        _ => None,
    }
}

/// Brings the archive record in line with a check: a finding sets `heal_failed_at` (if not set
/// already), a file that opens clears it and restores the page count and size.
async fn apply_to_record(state: &AppState, id: &ArchiveId, inspection: &Inspection) {
    let Ok(Some(mut archive)) = state.repos.archives.get(id).await else {
        return;
    };
    match inspection {
        Inspection::Damaged(_) if archive.heal_failed_at.is_none() => {
            archive.heal_failed_at = Some(now_secs().max(0) as u64);
        }
        Inspection::Readable { pages, size }
            if archive.heal_failed_at.is_some() || archive.pagecount == 0 =>
        {
            archive.heal_failed_at = None;
            archive.pagecount = *pages as u32;
            archive.arcsize = *size;
        }
        _ => return,
    }
    if let Err(e) = state.repos.archives.save(&archive).await {
        tracing::warn!(%id, error = %e, "could not update an archive's damaged marker");
    }
}

async fn store_finding(
    conn: &mut deadpool_redis::Connection,
    id: &ArchiveId,
    inspection: &Inspection,
) -> Result<(), deadpool_redis::redis::RedisError> {
    match inspection {
        Inspection::Damaged(f) => {
            conn.hset(
                FINDINGS_KEY,
                id.as_str(),
                serde_json::to_string(f).unwrap_or_default(),
            )
            .await
        }
        Inspection::Readable { .. } => conn.hdel(FINDINGS_KEY, id.as_str()).await,
        Inspection::Unknown => Ok(()),
    }
}

/// `POST /database/integrity-scan/archives/{id}` — checks one archive again, typically after its
/// file was replaced, and clears the damaged mark when it now opens.
async fn recheck(State(state): State<AppState>, Path(id): Path<ArchiveId>) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return crate::common::not_found("integrity_recheck", format!("{id} does not exist."))
        }
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_recheck",
                e.to_string(),
            )
        }
    };
    let path = PathBuf::from(&archive.file);
    let inspection = match lanrurugi_core::concurrency::run_blocking(move || inspect(&path)).await {
        Ok(i) => i,
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_recheck",
                e.to_string(),
            )
        }
    };
    if let Ok(mut conn) = state.redis.config.get().await {
        if let Err(e) = store_finding(&mut conn, &id, &inspection).await {
            tracing::warn!(%id, error = %e, "could not store an integrity finding");
        }
    }
    apply_to_record(&state, &id, &inspection).await;
    let (damaged, finding) = match &inspection {
        Inspection::Damaged(f) => (true, Some(f)),
        Inspection::Unknown => (true, None),
        Inspection::Readable { .. } => (false, None),
    };
    axum::Json(json!({ "success": 1, "damaged": damaged, "finding": finding })).into_response()
}

async fn start(State(state): State<AppState>) -> Response {
    let cancel = CancellationToken::new();
    let job = {
        let mut running = RUNNING.lock().await;
        if let Some(r) = running.as_ref() {
            return axum::Json(json!({ "success": 1, "job": r.job, "already_running": true }))
                .into_response();
        }
        let job = state.jobs.create("integrity_scan").await;
        *running = Some(Running {
            job: job.clone(),
            cancel: cancel.clone(),
        });
        job
    };
    let task_state = state.clone();
    let task_job = job.clone();
    tokio::spawn(async move {
        run(&task_state, &task_job, &cancel).await;
        *RUNNING.lock().await = None;
    });
    axum::Json(json!({ "success": 1, "job": job })).into_response()
}

async fn stop() -> Response {
    let running = RUNNING.lock().await;
    if let Some(r) = running.as_ref() {
        r.cancel.cancel();
    }
    axum::Json(json!({ "success": 1, "stopped": running.is_some() })).into_response()
}

async fn status(State(state): State<AppState>) -> Response {
    let running = RUNNING.lock().await.as_ref().map(|r| {
        json!({
            "job": r.job,
            "done": DONE.load(Ordering::Relaxed),
            "total": TOTAL.load(Ordering::Relaxed),
        })
    });
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "integrity_scan",
                e.to_string(),
            )
        }
    };
    let (findings, last_run) = match state.redis.config.get().await {
        Ok(mut conn) => {
            let raw: std::collections::HashMap<String, String> =
                conn.hgetall(FINDINGS_KEY).await.unwrap_or_default();
            let last: Option<String> = conn.get(LAST_RUN_KEY).await.unwrap_or_default();
            (
                raw.into_iter()
                    .filter_map(|(id, v)| serde_json::from_str::<Finding>(&v).ok().map(|f| (id, f)))
                    .collect::<std::collections::HashMap<_, _>>(),
                last.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
            )
        }
        Err(_) => Default::default(),
    };
    let mut items: Vec<serde_json::Value> = archives
        .iter()
        .filter_map(|a| {
            let finding = findings.get(a.id.as_str());
            if finding.is_none() && a.heal_failed_at.is_none() {
                return None;
            }
            Some(json!({
                "id": a.id.as_str(),
                "title": a.title,
                "file": a.file,
                "arcsize": a.arcsize,
                "heal_failed_at": a.heal_failed_at,
                "finding": finding,
            }))
        })
        .collect();
    items.sort_by(|a, b| a["title"].as_str().cmp(&b["title"].as_str()));
    axum::Json(json!({
        "running": running,
        "last_run": last_run,
        "archives": items,
    }))
    .into_response()
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct ScanSummary {
    pub checked: usize,
    pub damaged: usize,
    pub cancelled: bool,
}

async fn run(state: &AppState, job: &str, cancel: &CancellationToken) {
    state.jobs.mark_active(job).await;
    match scan(state, cancel, |done, total| {
        DONE.store(done, Ordering::Relaxed);
        TOTAL.store(total, Ordering::Relaxed);
        let jobs = state.jobs.clone();
        let job = job.to_string();
        async move {
            jobs.set_progress(&job, done as f32 / total.max(1) as f32)
                .await
        }
    })
    .await
    {
        Ok(summary) => {
            tracing::info!(?summary, "integrity scan finished");
            if let Ok(mut conn) = state.redis.config.get().await {
                let last = json!({
                    "finished_at": now_secs(),
                    "checked": summary.checked,
                    "damaged": summary.damaged,
                    "cancelled": summary.cancelled,
                });
                let _: Result<(), _> = conn.set(LAST_RUN_KEY, last.to_string()).await;
            }
            state
                .jobs
                .finish(job, serde_json::to_value(&summary).unwrap_or_default())
                .await;
        }
        Err(e) => state.jobs.fail(job, e).await,
    }
    DONE.store(0, Ordering::Relaxed);
    TOTAL.store(0, Ordering::Relaxed);
}

/// Checks every catalogued archive and records the damaged ones, clearing earlier findings for
/// archives that now open (a replaced file) or no longer exist.
pub(crate) async fn scan<F, Fut>(
    state: &AppState,
    cancel: &CancellationToken,
    progress: F,
) -> Result<ScanSummary, String>
where
    F: Fn(usize, usize) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let archives = state
        .repos
        .archives
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    let known: std::collections::HashSet<String> =
        archives.iter().map(|a| a.id.as_str().to_string()).collect();
    // Whether the record needs touching is decided from these two fields, so an archive that was
    // fine and still is costs no write.
    let needs_record = |marked: bool, pagecount: u32, inspection: &Inspection| match inspection {
        Inspection::Damaged(_) => !marked,
        Inspection::Readable { .. } => marked || pagecount == 0,
        Inspection::Unknown => false,
    };
    let pending: Vec<(ArchiveId, PathBuf, bool, u32)> = archives
        .into_iter()
        .filter(|a| !a.file.trim().is_empty())
        .map(|a| {
            (
                a.id,
                PathBuf::from(a.file),
                a.heal_failed_at.is_some(),
                a.pagecount,
            )
        })
        .collect();
    let mut summary = ScanSummary::default();
    let total = pending.len();
    progress(0, total).await;

    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let stale: Vec<String> = conn.hkeys(FINDINGS_KEY).await.unwrap_or_default();
    let stale: Vec<String> = stale.into_iter().filter(|id| !known.contains(id)).collect();
    if !stale.is_empty() {
        let _: () = conn
            .hdel(FINDINGS_KEY, &stale)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Disk-bound like the BLAKE3 backfill: parallel reads on spinning disks seek against each
    // other, so two at most, and the job backs off when the host is busy.
    let threads = crate::recommend_precompute::precompute_worker_budget().min(2);
    let pool = std::sync::Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("integrity-scan-{i}"))
            .build()
            .map_err(|e| e.to_string())?,
    );
    let mut throttle = crate::recommend_precompute::LoadThrottle::new();
    for batch in pending.chunks(BATCH) {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let started = std::time::Instant::now();
        let batch = batch.to_vec();
        let pool = pool.clone();
        let checked = lanrurugi_core::concurrency::run_blocking(move || {
            use rayon::prelude::*;
            pool.install(|| {
                batch
                    .into_par_iter()
                    .map(|(id, path, marked, pagecount)| (id, marked, pagecount, inspect(&path)))
                    .collect::<Vec<_>>()
            })
        })
        .await
        .map_err(|e| e.to_string())?;
        for (id, marked, pagecount, inspection) in checked {
            summary.checked += 1;
            if matches!(inspection, Inspection::Damaged(_)) {
                summary.damaged += 1;
            }
            if let Err(e) = store_finding(&mut conn, &id, &inspection).await {
                tracing::warn!(%id, error = %e, "could not store an integrity finding");
            }
            if needs_record(marked, pagecount, &inspection) {
                apply_to_record(state, &id, &inspection).await;
            }
        }
        progress(summary.checked, total).await;
        let delay = throttle.sample_and_backoff(started.elapsed());
        if !delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel.cancelled() => {}
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with_pages(path: &std::path::Path, pages: usize) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for n in 0..pages {
            zip.start_file(format!("{n:03}.png"), options).unwrap();
            std::io::Write::write_all(&mut zip, &vec![n as u8; 8192]).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn a_whole_archive_passes_and_a_truncated_one_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        zip_with_pages(&path, 4);
        assert_eq!(check_file(&path), None);

        // What an interrupted copy leaves behind: the head of the file, cut on a block boundary.
        let len = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(len / 2 / 4096 * 4096 + 4096)
            .unwrap();
        let finding = check_file(&path).expect("a truncated zip is a finding");
        assert_eq!(finding.problem, Problem::Unreadable);
        assert!(finding.size_on_disk.unwrap() < len);
    }

    #[test]
    fn a_missing_file_and_an_empty_archive_are_findings() {
        let dir = tempfile::tempdir().unwrap();
        let missing = check_file(&dir.path().join("gone.zip")).unwrap();
        assert_eq!(missing.problem, Problem::Missing);

        let empty = dir.path().join("empty.zip");
        zip_with_pages(&empty, 0);
        assert_eq!(check_file(&empty).unwrap().problem, Problem::NoPages);
    }
}
