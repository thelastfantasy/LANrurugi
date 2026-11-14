//! 027 US5: fills in the whole-file BLAKE3 for archives catalogued before 027 recorded one. Run on
//! request from the settings page (`POST /database/blake3-backfill`), stoppable
//! (`DELETE` on the same path), visible on the Jobs page.
//!
//! Each file is read once and only the `blake3` field is written, so an edit made to a record
//! while the job runs is never overwritten. The work is disk-bound: hashing outruns any NAS disk,
//! and parallel reads on spinning disks seek against each other, so at most two files are read at
//! once, and the job still backs off when the host is busy.

use std::path::PathBuf;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::auth_context::AuthContext;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/database/blake3-backfill", post(start).delete(stop))
}

/// The running job, if any: one at a time, so a second click joins the first.
static RUNNING: tokio::sync::Mutex<Option<(String, CancellationToken)>> =
    tokio::sync::Mutex::const_new(None);

/// Files hashed per batch before progress, cancellation and load are checked again.
const BATCH: usize = 16;

async fn start(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let cancel = CancellationToken::new();
    let job_id = {
        let mut running = RUNNING.lock().await;
        if let Some((id, _)) = running.as_ref() {
            return axum::Json(json!({
                "operation": "blake3_backfill",
                "success": 1,
                "job": id,
                "already_running": true,
            }))
            .into_response();
        }
        let id = state.jobs.create("blake3_backfill").await;
        *running = Some((id.clone(), cancel.clone()));
        id
    };
    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_BLAKE3_BACKFILL,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("database".to_string()),
        },
        Outcome::Success,
        None,
        None,
    )
    .await;
    let task_state = state.clone();
    let task_job = job_id.clone();
    tokio::spawn(async move {
        run(&task_state, &task_job, &cancel).await;
        *RUNNING.lock().await = None;
    });
    axum::Json(json!({
        "operation": "blake3_backfill",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

async fn stop() -> Response {
    let running = RUNNING.lock().await.clone();
    if let Some((_, cancel)) = &running {
        cancel.cancel();
    }
    axum::Json(json!({
        "operation": "blake3_backfill_stop",
        "success": 1,
        "stopped": running.is_some(),
    }))
    .into_response()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct BackfillSummary {
    pub total: usize,
    pub filled: usize,
    pub failed: usize,
    pub cancelled: bool,
}

async fn run(state: &AppState, job_id: &str, cancel: &CancellationToken) {
    state.jobs.mark_active(job_id).await;
    match backfill(state, cancel, |done, total| {
        let jobs = state.jobs.clone();
        let job_id = job_id.to_string();
        async move {
            jobs.set_progress(&job_id, done as f32 / total.max(1) as f32)
                .await
        }
    })
    .await
    {
        Ok(summary) => {
            tracing::info!(?summary, "blake3 backfill finished");
            state
                .jobs
                .finish(
                    job_id,
                    json!({
                        "archives": summary.total,
                        "filled": summary.filled,
                        "failed": summary.failed,
                        "cancelled": summary.cancelled,
                    }),
                )
                .await;
        }
        Err(e) => state.jobs.fail(job_id, e).await,
    }
}

/// Hashes every archive that has a file but no fingerprint. Missing or unreadable files are
/// counted and skipped; a later run tries them again.
pub(crate) async fn backfill<F, Fut>(
    state: &AppState,
    cancel: &CancellationToken,
    progress: F,
) -> Result<BackfillSummary, String>
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
    let pending: Vec<(ArchiveId, PathBuf)> = archives
        .into_iter()
        .filter(|a| a.blake3.is_none() && !a.file.trim().is_empty())
        .map(|a| (a.id, PathBuf::from(a.file)))
        .collect();
    let mut summary = BackfillSummary {
        total: pending.len(),
        ..Default::default()
    };
    let threads = crate::recommend_precompute::precompute_worker_budget().min(2);
    let pool = std::sync::Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("blake3-backfill-{i}"))
            .build()
            .map_err(|e| e.to_string())?,
    );
    let mut throttle = crate::recommend_precompute::LoadThrottle::new();
    let mut done = 0;
    for batch in pending.chunks(BATCH) {
        if cancel.is_cancelled() {
            summary.cancelled = true;
            break;
        }
        let started = std::time::Instant::now();
        let batch = batch.to_vec();
        let pool = pool.clone();
        let hashed = lanrurugi_core::concurrency::run_blocking(move || {
            use rayon::prelude::*;
            pool.install(|| {
                batch
                    .into_par_iter()
                    .map(|(id, path)| {
                        let hash = lanrurugi_core::io::blake3_file(&path);
                        (id, path, hash)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .await
        .map_err(|e| e.to_string())?;
        for (id, path, hash) in hashed {
            match hash {
                Ok(hash) => match state.repos.archives.set_blake3(&id, &hash.to_hex()).await {
                    Ok(true) => summary.filled += 1,
                    // Deleted while it was being read: nothing to fill in.
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(%id, error = %e, "could not store a backfilled fingerprint");
                        summary.failed += 1;
                    }
                },
                Err(e) => {
                    tracing::warn!(%id, path = %path.display(), error = %e, "could not fingerprint an archive");
                    summary.failed += 1;
                }
            }
            done += 1;
        }
        progress(done, summary.total).await;
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

    fn record(id: &ArchiveId, file: &std::path::Path) -> lanrurugi_core::entities::Archive {
        lanrurugi_core::entities::Archive {
            id: id.clone(),
            name: "n".into(),
            title: "t".into(),
            file: file.to_string_lossy().to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 0,
            pagecount: 0,
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
        }
    }

    #[tokio::test]
    async fn fills_in_missing_fingerprints_and_stops_when_asked() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut expected = Vec::new();
        for n in 0..2u8 {
            let path = dir.path().join(format!("{n}.zip"));
            std::fs::write(&path, vec![n; 4096 + n as usize]).unwrap();
            let id = ArchiveId(format!("{:0>40}", format!("b3f{n}")));
            state
                .repos
                .archives
                .save(&record(&id, &path))
                .await
                .unwrap();
            expected.push((
                id,
                blake3::hash(&std::fs::read(&path).unwrap())
                    .to_hex()
                    .to_string(),
            ));
        }

        let stopped = CancellationToken::new();
        stopped.cancel();
        let summary = backfill(&state, &stopped, |_, _| async {}).await.unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.filled, 0);

        let summary = backfill(&state, &CancellationToken::new(), |_, _| async {})
            .await
            .unwrap();
        assert!(summary.filled >= 2, "{summary:?}");
        for (id, hash) in &expected {
            let stored = state.repos.archives.get(id).await.unwrap().unwrap();
            assert_eq!(stored.blake3.as_deref(), Some(hash.as_str()));
            state.repos.archives.delete(id).await.unwrap();
        }
    }
}
