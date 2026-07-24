//! `database` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Utils/Database.pm::clean_database`.

use std::collections::HashMap;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_backup::build::BackupDocument;
use serde::Deserialize;
use serde_json::json;

use crate::common::{error, not_found};
use crate::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/database/stats", get(stats))
        .route("/database/backup", get(get_backup).post(queue_backup))
        .route("/database/backup/{jobid}", get(download_backup))
        .route("/database/restore", post(queue_restore))
        .route("/database/isnew", delete(clear_new_all))
        .route("/database/drop", post(drop_database))
        .route("/database/clean", post(clean_database))
        .route("/database/rebuild-index", post(rebuild_index))
}

#[derive(Debug, Deserialize, Default)]
pub struct StatsParams {
    minweight: Option<i64>,
    #[serde(default)]
    hide_excluded_namespaces: bool,
}

/// Computed on demand by scanning every archive's tags (`LRR_STATS` popularity counters aren't
/// maintained incrementally in Phase 1 — see `lanrurugi-search::indexer` module docs) — correct,
/// just not index-accelerated; acceptable at the target library scale for an operator-facing
/// stats page rather than a hot request path.
async fn stats(State(state): State<AppState>, Query(params): Query<StatsParams>) -> Response {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_tag_stats",
                e.to_string(),
            )
        }
    };

    let excluded_namespaces: Vec<String> = if params.hide_excluded_namespaces {
        let mut conn = match state.redis.config.get().await {
            Ok(c) => c,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "serve_tag_stats",
                    e.to_string(),
                )
            }
        };
        let raw: String = conn
            .hget(CONFIG_KEY, "excludednamespaces")
            .await
            .unwrap_or_else(|_| "source, date_added".to_string());
        raw.split(',').map(|s| s.trim().to_string()).collect()
    } else {
        Vec::new()
    };

    let mut counts: HashMap<String, i64> = HashMap::new();
    for archive in &archives {
        for tag in archive.tags.split(',') {
            let tag = tag.trim();
            if !tag.is_empty() {
                *counts.entry(tag.to_string()).or_insert(0) += 1;
            }
        }
    }

    let min_weight = params.minweight.unwrap_or(1);
    let result: Vec<_> = counts
        .into_iter()
        .filter(|(_, weight)| *weight >= min_weight)
        .filter_map(|(tag, weight)| {
            let (namespace, text) = match tag.split_once(':') {
                Some((ns, t)) => (Some(ns.to_string()), t.to_string()),
                None => (None, tag.clone()),
            };
            if let Some(ns) = &namespace {
                if excluded_namespaces.iter().any(|e| e == ns) {
                    return None;
                }
            }
            Some(json!({ "namespace": namespace, "text": text, "weight": weight }))
        })
        .collect();

    axum::Json(result).into_response()
}

async fn build_backup_document(state: &AppState) -> Result<BackupDocument, Response> {
    lanrurugi_backup::build::build(
        &state.repos.archives,
        &state.repos.categories,
        &state.repos.groupings,
        &state.repos.stamps,
    )
    .await
    .map_err(|e| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "serve_backup",
            e.to_string(),
        )
    })
}

async fn get_backup(State(state): State<AppState>) -> Response {
    match build_backup_document(&state).await {
        Ok(doc) => axum::Json(doc).into_response(),
        Err(resp) => resp,
    }
}

async fn queue_backup(State(state): State<AppState>) -> Response {
    let job_id = state.jobs.create("backup").await;
    let jobs = state.jobs.clone();
    let repos = state.repos.clone();
    let job_id_for_task = job_id.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        match lanrurugi_backup::build::build(
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &repos.stamps,
        )
        .await
        {
            Ok(doc) => {
                let value = serde_json::to_value(doc).unwrap_or(serde_json::Value::Null);
                jobs.finish(&job_id_for_task, value).await;
            }
            Err(e) => jobs.fail(&job_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({ "operation": "queue_backup", "success": 1, "job": job_id })).into_response()
}

async fn download_backup(State(state): State<AppState>, Path(jobid): Path<String>) -> Response {
    match state.jobs.get(&jobid).await {
        Some(status) if status.result.is_some() => (
            [(header::CONTENT_TYPE, "application/json")],
            axum::Json(status.result),
        )
            .into_response(),
        Some(_) => error(
            StatusCode::BAD_REQUEST,
            "download_backup",
            "Job not complete yet.",
        ),
        None => not_found("download_backup", format!("Job {jobid} not found.")),
    }
}

async fn queue_restore(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return error(StatusCode::BAD_REQUEST, "queue_restore", e.to_string()),
        };
        if field.name() == Some("file") {
            file_bytes = field.bytes().await.ok().map(|b| b.to_vec());
        }
    }
    let Some(bytes) = file_bytes else {
        return error(
            StatusCode::BAD_REQUEST,
            "queue_restore",
            "No file provided.",
        );
    };
    let doc: BackupDocument = match serde_json::from_slice(&bytes) {
        Ok(d) => d,
        Err(e) => {
            return error(
                StatusCode::BAD_REQUEST,
                "queue_restore",
                format!("Invalid backup JSON: {e}"),
            )
        }
    };

    let job_id = state.jobs.create("restore").await;
    let jobs = state.jobs.clone();
    let repos = state.repos.clone();
    let job_id_for_task = job_id.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let result = lanrurugi_backup::restore::restore(
            &doc,
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &repos.stamps,
        )
        .await;
        match result {
            Ok(summary) => {
                let _ = lanrurugi_backup::restore::relink_stamp_ids(&doc, &repos.archives).await;
                jobs.finish(
                    &job_id_for_task,
                    json!({
                        "archives_updated": summary.archives_updated,
                        "archives_skipped_missing": summary.archives_skipped_missing,
                        "categories_restored": summary.categories_restored,
                        "tankoubons_restored": summary.tankoubons_restored,
                        "stamps_restored": summary.stamps_restored,
                    }),
                )
                .await;
            }
            Err(e) => jobs.fail(&job_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({ "operation": "queue_restore", "success": 1, "job": job_id })).into_response()
}

async fn clear_new_all(State(state): State<AppState>) -> Response {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "clear_new_all",
                e.to_string(),
            )
        }
    };
    for mut archive in archives {
        if archive.isnew {
            archive.isnew = false;
            let _ = state.repos.archives.save(&archive).await;
            let _ =
                lanrurugi_search::indexer::set_isnew_index(&state.redis.search, &archive.id, false)
                    .await;
        }
    }
    axum::Json(json!({ "operation": "clear_new_all", "success": 1 })).into_response()
}

/// Deliberately dangerous — see the endpoint's own legacy description ("might lock you out of the
/// server as a client"). Drops every logical DB this instance knows about, matching legacy's
/// `drop_database`'s `FLUSHALL` semantics (which is server-wide, not scoped to one DB).
async fn drop_database(State(state): State<AppState>) -> Response {
    for pool in [
        &state.redis.archive,
        &state.redis.minion,
        &state.redis.config,
        &state.redis.search,
        &state.redis.metrics,
    ] {
        if let Ok(mut conn) = pool.get().await {
            let _: Result<(), _> = deadpool_redis::redis::cmd("FLUSHDB")
                .query_async(&mut conn)
                .await;
        }
    }
    axum::Json(json!({ "operation": "drop_database", "success": 1 })).into_response()
}

/// Two-phase cleanup matching the legacy contract's documented *effect* ("unlinked at first,
/// deleted on a subsequent run"): an archive whose file is missing gets its `file` field cleared
/// (unlinked) on the first pass; an archive that's *already* unlinked (empty `file`) is deleted
/// outright. This differs from legacy's exact trigger (a `LRR_FILEMAP` cross-reference mismatch,
/// verified in `Utils/Database.pm::clean_database`) but preserves the same observable two-run
/// behavior for the common case (file deleted from disk out from under LANrurugi).
async fn clean_database(State(state): State<AppState>) -> Response {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "clean_database",
                e.to_string(),
            )
        }
    };

    let mut deleted = 0u32;
    let mut unlinked = 0u32;
    for mut archive in archives {
        if archive.file.is_empty() {
            let _ = state.repos.archives.delete(&archive.id).await;
            deleted += 1;
            continue;
        }
        if !std::path::Path::new(&archive.file).exists() {
            archive.file.clear();
            let _ = state.repos.archives.save(&archive).await;
            unlinked += 1;
        }
    }

    axum::Json(json!({
        "operation": "clean_database",
        "success": 1,
        "deleted": deleted,
        "unlinked": unlinked,
    }))
    .into_response()
}

/// `POST /database/rebuild-index` (User Story 6, additive endpoint per `contracts/rest-api.md` —
/// no legacy equivalent). Runs as a background job, pollable the same way as other jobs
/// (`/minion/{jobid}`-shaped, per contract): first re-keys every already-tracked archive whose
/// on-disk content now hashes differently (T074/T075), then does a full directory scan to
/// discover and catalogue any previously-invisible sibling files the historical false-merge
/// defect had hidden (T074, completing data-model.md's "Rebuild/Reindex operation").
async fn rebuild_index(State(state): State<AppState>) -> Response {
    let job_id = state.jobs.create("rebuild_index").await;
    let jobs = state.jobs.clone();
    let repos = state.repos.clone();
    let redis = state.redis.clone();
    let library_path = state.library.archive_dir.clone();
    let thumb_dir = state.library.thumb_dir.clone();
    let job_id_for_task = job_id.clone();
    // Same long-lived "自动运行" auto-plugin consumer `serve`'s own `main.rs` spawns once at
    // startup — a manually-triggered rebuild discovering previously-invisible files should
    // auto-tag them exactly the same way the watcher/startup scan would, so this just hands that
    // same consumer a clone of its sender rather than spawning a redundant one of its own.
    let new_archive_tx = state.new_archive_tx.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;

        let rekey_summary = match lanrurugi_storage::rebuild::rekey_all(
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &jobs,
            &job_id_for_task,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };

        let scan_summary = lanrurugi_scanner::full_scan::full_scan(
            &library_path,
            &repos.archives,
            &redis.config,
            &redis.search,
            &thumb_dir,
            &jobs,
            &job_id_for_task,
            Some(new_archive_tx),
        )
        .await;

        jobs.finish(
            &job_id_for_task,
            json!({
                "rekeyed": rekey_summary.rekeyed.len(),
                "unchanged": rekey_summary.unchanged,
                "missing_file": rekey_summary.missing_file,
                "scanned": scan_summary.scanned,
                "newly_catalogued": scan_summary.catalogued,
                "errors": scan_summary.errors,
            }),
        )
        .await;
    });

    axum::Json(json!({
        "operation": "rebuild_index",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}
