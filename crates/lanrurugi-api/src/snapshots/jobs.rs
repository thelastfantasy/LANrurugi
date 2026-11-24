//! Backup jobs: a full RDB backup or a JSON export runs off-request, reporting progress to an SSE
//! stream (`GET /api/backups/jobs/{job_id}/events`) with a polling fallback
//! (`GET /api/backups/jobs/{job_id}`).
//!
//! The connection handling is the one already used elsewhere in this crate
//! (`tankoubon_grouping/run.rs`, `archive_split.rs`): an axum `Sse` response fed by a
//! `tokio::sync::broadcast` channel through `BroadcastStream`, with axum's own `KeepAlive` producing
//! the `: keep-alive` comment lines. What is new here is only the payload — an absolute
//! `{percent, bytes_written, bytes_expected, elapsed_ms}` snapshot per event, so a client that
//! misses events (or joins late) is never left with a delta it cannot apply.
//!
//! Jobs live in a process-local registry: they are progress bookkeeping, not data. Finished jobs are
//! kept for an hour (capped at 32) so a UI that navigates away and back can still read the result.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderName, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::{SecondsFormat, Utc};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::progress::BackupProgress;
use super::{entry_json, retention_sweep, take_export, take_full_backup, SnapshotEntry};
use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::not_found;
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

/// How long a finished job stays readable.
const KEEP_FINISHED: Duration = Duration::from_secs(60 * 60);
/// Hard cap on remembered jobs, so a client that starts one and never reads it cannot grow memory.
const MAX_JOBS: usize = 32;
/// `: keep-alive` cadence. Below the 15 s the API contract promises, with room for scheduling jitter.
const KEEP_ALIVE: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Serialize)]
pub struct BackupJobStatus {
    /// `running` / `done` / `failed`.
    pub state: &'static str,
    /// What produced this job: `backup` / `export` / `restore` / `import`. Free-form, so a new
    /// long-running operation only has to name itself.
    pub kind: &'static str,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub percent: f64,
    pub bytes_written: u64,
    pub bytes_expected: u64,
    pub error: Option<String>,
    pub entry: Option<Value>,
    /// The summary a restore/import finishes with; `null` for a backup or export.
    pub result: Option<Value>,
}

struct Inner {
    state: &'static str,
    started_at: String,
    finished_at: Option<String>,
    finished_instant: Option<Instant>,
    percent: f64,
    bytes_written: u64,
    bytes_expected: u64,
    error: Option<String>,
    entry: Option<Value>,
    /// The result of a job that has no snapshot entry (restore/import summaries). Same slot as
    /// `entry` in the terminal event, under its own key so a client never has to guess which shape
    /// it is looking at.
    result: Option<Value>,
    last_emit: Instant,
    /// Whether any progress line has been published yet. The first real change always goes out,
    /// even if it arrives inside the 100 ms throttle window — a UI that connects before any event
    /// would otherwise show nothing until the second tick.
    emitted: bool,
}

pub struct BackupJob {
    pub id: String,
    pub kind: &'static str,
    started: Instant,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<Value>,
}

impl BackupJob {
    /// A fresh job, already registered by the caller. Named `create` rather than `new` because it
    /// hands back an `Arc` (the task and the SSE handler share it).
    fn create(kind: &'static str) -> Arc<Self> {
        let (tx, _) = broadcast::channel(64);
        Arc::new(Self {
            id: format!("backup-{}", uuid::Uuid::new_v4().simple()),
            kind,
            started: Instant::now(),
            inner: Mutex::new(Inner {
                state: "running",
                started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                finished_at: None,
                finished_instant: None,
                percent: 0.0,
                bytes_written: 0,
                bytes_expected: 0,
                error: None,
                entry: None,
                result: None,
                last_emit: Instant::now(),
                emitted: false,
            }),
            tx,
        })
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn status(&self) -> BackupJobStatus {
        let inner = self.inner.lock().expect("job status lock");
        BackupJobStatus {
            state: inner.state,
            kind: self.kind,
            started_at: inner.started_at.clone(),
            finished_at: inner.finished_at.clone(),
            percent: inner.percent,
            bytes_written: inner.bytes_written,
            bytes_expected: inner.bytes_expected,
            error: inner.error.clone(),
            entry: inner.entry.clone(),
            result: inner.result.clone(),
        }
    }

    /// A progress line, already in the shape the stream emits. `percent` is clamped to
    /// `[previous, 99.9]`: the sources may re-estimate the total downward, and the contract is that
    /// the bar only ever moves forward and only the terminal event says 100.
    fn progress_event(&self, inner: &Inner) -> Value {
        json!({
            "kind": "progress",
            "percent": inner.percent,
            "bytes_written": inner.bytes_written,
            "bytes_expected": inner.bytes_expected,
            "elapsed_ms": self.elapsed_ms(),
        })
    }

    /// Publishes the terminal state for a job whose result is not a snapshot entry (restore,
    /// import): 100 %, the summary payload, and the `done` event.
    pub(crate) fn finish_with_result(&self, result: Value) {
        let event = {
            let mut inner = self.inner.lock().expect("job status lock");
            if inner.state != "running" {
                return;
            }
            inner.state = "done";
            inner.percent = 100.0;
            inner.finished_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
            inner.finished_instant = Some(Instant::now());
            inner.result = Some(result.clone());
            json!({
                "kind": "done",
                "percent": 100.0,
                "bytes_written": inner.bytes_written,
                "bytes_expected": inner.bytes_expected,
                "elapsed_ms": self.elapsed_ms(),
                "entry": serde_json::Value::Null,
                "result": result,
            })
        };
        let _ = self.tx.send(event);
    }

    fn finish(&self, entry: &SnapshotEntry) {
        let event = {
            let mut inner = self.inner.lock().expect("job status lock");
            if inner.state != "running" {
                return;
            }
            inner.state = "done";
            inner.percent = 100.0;
            inner.bytes_written = entry.size_bytes;
            inner.bytes_expected = entry.size_bytes;
            inner.finished_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
            inner.finished_instant = Some(Instant::now());
            inner.entry = Some(entry_json(entry));
            json!({
                "kind": "done",
                "percent": 100.0,
                "bytes_written": entry.size_bytes,
                "bytes_expected": entry.size_bytes,
                "elapsed_ms": self.elapsed_ms(),
                "entry": entry_json(entry),
                "result": serde_json::Value::Null,
            })
        };
        let _ = self.tx.send(event);
    }

    pub(crate) fn fail(&self, message: String) {
        let event = {
            let mut inner = self.inner.lock().expect("job status lock");
            if inner.state != "running" {
                return;
            }
            inner.state = "failed";
            inner.finished_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true));
            inner.finished_instant = Some(Instant::now());
            inner.error = Some(message.clone());
            json!({
                "kind": "error",
                "error": message,
                "percent": inner.percent,
                "elapsed_ms": self.elapsed_ms(),
            })
        };
        let _ = self.tx.send(event);
    }
}

impl BackupProgress for BackupJob {
    fn update(&self, written: u64, expected: u64, percent: f64) {
        let event = {
            let mut inner = self.inner.lock().expect("job status lock");
            if inner.state != "running" {
                return;
            }
            let percent = percent.clamp(0.0, 99.9).max(inner.percent);
            let changed =
                percent > inner.percent || written > inner.bytes_written || !inner.emitted;
            inner.percent = percent;
            inner.bytes_written = inner.bytes_written.max(written);
            inner.bytes_expected = expected.max(inner.bytes_written);
            // Sinks are already sampled every ~150 ms; this second gate keeps a chatty sink (the
            // record writer can flush many times per second) from filling the broadcast buffer.
            let due = !inner.emitted || inner.last_emit.elapsed() >= Duration::from_millis(100);
            if !changed || !due {
                return;
            }
            inner.emitted = true;
            inner.last_emit = Instant::now();
            self.progress_event(&inner)
        };
        let _ = self.tx.send(event);
    }
}

/// Starts a job for an operation that is not a backup: it is registered and streamed exactly like
/// one, and the caller drives it through the [`BackupProgress`] impl plus
/// [`BackupJob::finish_with_result`] / [`BackupJob::fail`].
pub(crate) fn start_progress_job(kind: &'static str) -> Arc<BackupJob> {
    let job = BackupJob::create(kind);
    register(&job);
    job
}

/// The percentage for a record-counted operation: `done / total`. Whole-record granularity in,
/// one-decimal precision out — a 400-archive import moves the bar in 0.25 % steps.
pub(crate) fn record_percent(done: u64, total: u64) -> f64 {
    if total == 0 {
        100.0
    } else {
        (done as f64 / total as f64 * 100.0).min(100.0)
    }
}

static JOBS: LazyLock<Mutex<HashMap<String, Arc<BackupJob>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn register(job: &Arc<BackupJob>) {
    let mut jobs = JOBS.lock().expect("job registry lock");
    // Drop expired finished jobs first, then the oldest finished ones if still over the cap.
    jobs.retain(|_, job| {
        let inner = job.inner.lock().expect("job status lock");
        inner
            .finished_instant
            .is_none_or(|at| at.elapsed() < KEEP_FINISHED)
    });
    while jobs.len() >= MAX_JOBS {
        let oldest = jobs
            .iter()
            .filter_map(|(id, job)| {
                let inner = job.inner.lock().expect("job status lock");
                inner.finished_instant.map(|at| (id.clone(), at))
            })
            .min_by_key(|(_, at)| *at)
            .map(|(id, _)| id);
        match oldest {
            Some(id) => {
                jobs.remove(&id);
            }
            // Every remembered job is still running: keep them all, the cap yields.
            None => break,
        }
    }
    jobs.insert(job.id.clone(), job.clone());
}

pub(super) fn lookup(id: &str) -> Option<Arc<BackupJob>> {
    JOBS.lock().expect("job registry lock").get(id).cloned()
}

/// `POST /api/backups` — starts a full Redis backup and answers immediately with the job id. The
/// backup itself (`BGSAVE`, wait, copy) runs on its own task; the UI follows it on
/// `/backups/jobs/{id}/events`.
pub async fn start_full_backup(state: &AppState, auth: Option<AuthContext>) -> Arc<BackupJob> {
    let job = BackupJob::create("backup");
    register(&job);
    let state = state.clone();
    let task_job = job.clone();
    tokio::spawn(async move {
        let progress: Arc<dyn BackupProgress> = task_job.clone();
        match take_full_backup(&state, "manual", progress).await {
            Ok(entry) => {
                // A fresh backup can immediately push the oldest one past `FULL_BACKUP_KEEP`.
                retention_sweep(&state).await;
                task_job.finish(&entry);
                record_manual(
                    &state,
                    auth.as_ref(),
                    action_types::BACKUP_CREATE,
                    ActivityTarget {
                        id: None,
                        label: Some(entry.name.clone()),
                        kind: Some("backup".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "kind": "backup", "size_bytes": entry.size_bytes })),
                )
                .await;
            }
            Err(e) => {
                task_job.fail(e.clone());
                record_manual(
                    &state,
                    auth.as_ref(),
                    action_types::BACKUP_CREATE,
                    ActivityTarget {
                        id: None,
                        label: None,
                        kind: Some("backup".to_string()),
                    },
                    Outcome::Failure { reason: e },
                    None,
                    None,
                )
                .await;
            }
        }
    });
    job
}

/// `POST /api/backups/export` — the JSON side, same job shape.
pub async fn start_export(state: &AppState, auth: Option<AuthContext>) -> Arc<BackupJob> {
    let job = BackupJob::create("export");
    register(&job);
    let state = state.clone();
    let task_job = job.clone();
    tokio::spawn(async move {
        let progress: Arc<dyn BackupProgress> = task_job.clone();
        match take_export(&state, "manual", progress).await {
            Ok(entry) => {
                retention_sweep(&state).await;
                task_job.finish(&entry);
                record_manual(
                    &state,
                    auth.as_ref(),
                    action_types::BACKUP_CREATE,
                    ActivityTarget {
                        id: None,
                        label: Some(entry.name.clone()),
                        kind: Some("backup".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "kind": "export", "size_bytes": entry.size_bytes })),
                )
                .await;
            }
            Err(e) => {
                task_job.fail(e.clone());
                record_manual(
                    &state,
                    auth.as_ref(),
                    action_types::BACKUP_CREATE,
                    ActivityTarget {
                        id: None,
                        label: None,
                        kind: Some("backup".to_string()),
                    },
                    Outcome::Failure { reason: e },
                    None,
                    None,
                )
                .await;
            }
        }
    });
    job
}

/// `GET /api/backups/jobs/{job_id}` — the polling fallback for clients that cannot hold an
/// `EventSource` open.
pub async fn job_status(State(_state): State<AppState>, UrlPath(id): UrlPath<String>) -> Response {
    match lookup(&id) {
        Some(job) => axum::Json(job.status()).into_response(),
        None => not_found(
            "backup_job_status",
            format!("Backup job {id} does not exist."),
        ),
    }
}

/// `GET /api/backups/jobs/{job_id}/events` — the primary path: one SSE connection per job.
///
/// The first event is the job's current state, so a client that subscribes late (or reconnects)
/// starts from an absolute snapshot rather than waiting for the next tick. A terminal first event
/// closes the stream immediately.
pub async fn job_events(State(_state): State<AppState>, UrlPath(id): UrlPath<String>) -> Response {
    let Some(job) = lookup(&id) else {
        return not_found(
            "backup_job_events",
            format!("Backup job {id} does not exist."),
        );
    };
    // Subscribe before reading the state: an event published in between would otherwise be lost.
    let rx = job.tx.subscribe();
    let status = job.status();
    let first = status_event(&status);

    let stream = futures_util::stream::once(async move { Ok(first) }).chain({
        let terminal_sent = Arc::new(AtomicBool::new(status.state != "running"));
        futures_util::stream::unfold(
            (rx, terminal_sent),
            move |(mut rx, terminal_sent)| async move {
                if terminal_sent.load(Ordering::SeqCst) {
                    return None;
                }
                loop {
                    match rx.recv().await {
                        Ok(value) => {
                            let kind = value
                                .get("kind")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if kind == "done" || kind == "error" {
                                terminal_sent.store(true, Ordering::SeqCst);
                            }
                            return Some((
                                Ok::<_, std::convert::Infallible>(to_event(&value)),
                                (rx, terminal_sent),
                            ));
                        }
                        // Every event carries absolute numbers, so a lagging client can skip
                        // straight to the next one without losing state.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            },
        )
    });

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE).text("keep-alive"))
        .into_response();
    let headers = response.headers_mut();
    // A progress stream must never be cached, and nginx must not buffer it into uselessness.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    response
}

fn status_event(status: &BackupJobStatus) -> Event {
    let value = serde_json::to_value(status).unwrap_or_default();
    let name = match status.state {
        "done" => "done",
        "failed" => "error",
        _ => "progress",
    };
    Event::default().event(name).data(value.to_string())
}

fn to_event(value: &Value) -> Event {
    // `Event::event` wants a `&'static str`, so the name is mapped, not borrowed from the payload.
    let name = match value.get("kind").and_then(Value::as_str) {
        Some("done") => "done",
        Some("error") => "error",
        _ => "progress",
    };
    Event::default().event(name).data(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::SnapshotKind;

    #[test]
    fn progress_never_goes_backwards_and_ends_at_one_hundred() {
        let job = BackupJob::create("backup");
        let progress: Arc<dyn BackupProgress> = job.clone();
        progress.update(100, 1_000, 10.0);
        progress.update(200, 1_000, 20.0);
        // A re-estimate that would put the bar backwards: clamped to where it already was.
        progress.update(150, 5_000, 3.0);
        let status = job.status();
        assert_eq!(status.percent, 20.0);
        assert_eq!(status.bytes_written, 200, "bytes never shrink either");

        let entry = entry_for("snapshot-20261123T030000Z-manual.rdb", 1_000);
        job.finish(&entry);
        let status = job.status();
        assert_eq!(status.state, "done");
        assert_eq!(status.percent, 100.0);
        assert_eq!(status.bytes_written, 1_000);
        assert!(status.finished_at.is_some());
        assert_eq!(status.entry.as_ref().unwrap()["kind"], json!("backup"));
    }

    #[test]
    fn a_failed_job_carries_an_error_and_no_entry() {
        let job = BackupJob::create("export");
        job.fail("could not ask Redis where its RDB lives".to_string());
        let status = job.status();
        assert_eq!(status.state, "failed");
        assert_eq!(
            status.error.as_deref(),
            Some("could not ask Redis where its RDB lives")
        );
        assert!(status.entry.is_none());
        assert!(status.finished_at.is_some());
        assert!(status.percent < 100.0, "a failure is not a completed bar");
    }

    /// The stream a client actually receives: the events a job publishes, in order, progress before
    /// its terminal event.
    #[test]
    fn the_broadcast_carries_progress_then_done() {
        let job = BackupJob::create("backup");
        let mut rx = job.tx.subscribe();
        let progress: Arc<dyn BackupProgress> = job.clone();
        progress.update(500, 1_000, 50.0);
        let first = rx.try_recv().expect("a progress event");
        assert_eq!(first["kind"], json!("progress"));
        assert_eq!(first["percent"], json!(50.0));
        assert_eq!(first["bytes_written"], json!(500));

        job.finish(&entry_for("snapshot-20261123T030000Z-manual.rdb", 1_000));
        let terminal = rx.try_recv().expect("the done event");
        assert_eq!(terminal["kind"], json!("done"));
        assert_eq!(terminal["percent"], json!(100.0));
        assert_eq!(
            terminal["entry"]["name"],
            json!("snapshot-20261123T030000Z-manual.rdb")
        );
    }

    #[test]
    fn the_broadcast_carries_an_error_event() {
        let job = BackupJob::create("export");
        let mut rx = job.tx.subscribe();
        job.fail("disk full".to_string());
        let event = rx.try_recv().expect("the error event");
        assert_eq!(event["kind"], json!("error"));
        assert_eq!(event["error"], json!("disk full"));
    }

    /// A record-counted operation (restore/import) drives the same job: progress lines, then a
    /// terminal `done` carrying the summary instead of a snapshot entry. Sinks are throttled to
    /// ~100 ms, so a burst of records collapses into one line — the bar is still monotonic.
    #[test]
    fn a_record_based_job_reports_monotonic_progress_and_a_summary() {
        let job = start_progress_job("restore");
        let mut rx = job.tx.subscribe();
        let sink: Arc<dyn BackupProgress> = job.clone();
        for done in 1..=4u64 {
            sink.update(done, 4, record_percent(done, 4));
        }
        let first = rx.try_recv().expect("the first change always reports");
        assert_eq!(first["kind"], json!("progress"));
        let first_percent = first["percent"].as_f64().unwrap();
        assert!(first_percent > 0.0 && first_percent <= 99.9, "{first}");

        assert_eq!(record_percent(4, 4), 100.0);
        assert_eq!(
            record_percent(0, 0),
            100.0,
            "an empty operation is complete"
        );

        job.finish_with_result(json!({ "restored": 3, "skipped": 1 }));
        let done = rx.try_recv().expect("the done event");
        assert_eq!(done["kind"], json!("done"));
        assert!(done["percent"].as_f64().unwrap() >= first_percent);
        assert_eq!(done["percent"], json!(100.0));
        assert_eq!(done["result"]["restored"], json!(3));
        assert_eq!(done["entry"], serde_json::Value::Null);
        let status = job.status();
        assert_eq!(status.state, "done");
        assert_eq!(status.kind, "restore");
        assert_eq!(status.result.as_ref().unwrap()["skipped"], json!(1));
    }

    #[test]
    fn a_record_based_job_can_fail_with_an_error() {
        let job = start_progress_job("import");
        let sink: Arc<dyn BackupProgress> = job.clone();
        sink.update(2, 10, record_percent(2, 10));
        job.fail("redis went away".to_string());
        let status = job.status();
        assert_eq!(status.state, "failed");
        assert_eq!(status.error.as_deref(), Some("redis went away"));
        assert!(status.result.is_none());
        assert!(status.percent < 100.0);
    }

    fn entry_for(name: &str, size: u64) -> SnapshotEntry {
        SnapshotEntry {
            name: name.to_string(),
            kind: SnapshotKind::Backup,
            created_at: "2026-11-23T03:00:00Z".to_string(),
            reason: "manual".to_string(),
            size_bytes: size,
            usable: true,
            summary: None,
            pinned: false,
        }
    }
}
