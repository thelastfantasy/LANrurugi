//! `database` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Utils/Database.pm::clean_database`.

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_backup::build::BackupDocument;
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::CONFIG_KEY;

/// `Multipart`'s `DefaultBodyLimit` default (2 MB, see `upload.rs`'s own comment on the same
/// issue) is too small for a real large library's backup export — confirmed live against a real
/// 6228-archive LANraragi export at 5.4 MB, comfortably under this cap but already 2.7x the
/// default. A pure-text JSON export would need an implausibly large library to approach 100 MB.
const MAX_IMPORT_LEGACY_BYTES: usize = 100 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    // `/database/drop` — whole-library, irreversible ("数据销毁类") — no API token, admin-role or
    // not, may reach this; only a real session cookie can. Enforced by `require_api_key` itself
    // (issue #91's `route_policy.csv` `deny` rule for `token_admin`/`token_guest` against this
    // exact route) — see `procedure.rs`'s own module docs for why this used to be a separate
    // `route_layer`-gated sub-router and no longer is.
    Router::new()
        .route("/database/drop", post(drop_database))
        .route("/database/stats", get(stats))
        .route("/database/backup", get(get_backup).post(queue_backup))
        .route("/database/backup/{jobid}", get(download_backup))
        .route("/database/restore", post(queue_restore))
        .route("/database/isnew", delete(clear_new_all))
        .route("/database/clean", post(clean_database))
        .route("/database/rebuild-index", post(rebuild_index))
        .route(
            "/database/import-legacy",
            post(queue_import_legacy).layer(DefaultBodyLimit::max(MAX_IMPORT_LEGACY_BYTES)),
        )
        .route("/database/import-legacy/count", get(import_legacy_count))
        .route("/database/import-snapshots", get(list_import_snapshots))
        .route(
            "/database/import-snapshots/{id}",
            get(download_import_snapshot).delete(delete_import_snapshot),
        )
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
async fn stats(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(params): Query<StatsParams>,
) -> Response {
    let mut archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_tag_stats",
                e.to_string(),
            )
        }
    };
    // 007-guest-restricted-access: this endpoint has no `route_policy.csv` deny rule for
    // `guest_visitor` (a guest genuinely benefits from a tag cloud to browse with), but without
    // this scope filter it aggregated tags off *every* archive regardless of visibility — leaking
    // both the existence of out-of-scope archives (via their tags/weights) and producing a tag
    // cloud whose entries then 404/empty the moment a guest actually searches by one, since
    // `search_archives`'s own `restrict_to_archive_ids` correctly excludes them there. Confirmed
    // live, 2026-08-27: a guest saw out-of-scope tags (e.g. from test-only archives never marked
    // visible) rendered in the tag cloud, each returning zero search results when clicked.
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(crate::auth_context::AuthMethod::GuestVisitor)
    ) {
        match crate::search::guest_visible_archive_ids(&state).await {
            Ok(allowed) => archives.retain(|a| allowed.contains(&a.id)),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "serve_tag_stats",
                    e.to_string(),
                )
            }
        }
    }

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
        &state.bookmarks,
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
    let bookmarks = state.bookmarks.clone();
    let job_id_for_task = job_id.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        match lanrurugi_backup::build::build(
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &repos.stamps,
            &bookmarks,
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

async fn queue_restore(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    mut multipart: Multipart,
) -> Response {
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

    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_RESTORE,
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

    let job_id = state.jobs.create("restore").await;
    let jobs = state.jobs.clone();
    let repos = state.repos.clone();
    let job_id_for_task = job_id.clone();
    let state_for_task = state.clone();
    let auth_for_task = auth.as_ref().map(|e| e.0.clone());

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let result = lanrurugi_backup::restore::restore(
            &doc,
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &repos.stamps,
            &state_for_task.bookmarks,
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
                        "bookmarks_restored": summary.bookmarks_restored,
                    }),
                )
                .await;
            }
            Err(e) => {
                // The restore was actually attempted (request already accepted, task running) and
                // failed partway through — distinct from the earlier body/JSON validation
                // rejections above, which never got this far.
                record_manual(
                    &state_for_task,
                    auth_for_task.as_ref(),
                    action_types::DATABASE_RESTORE,
                    ActivityTarget {
                        id: None,
                        label: None,
                        kind: Some("database".to_string()),
                    },
                    Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    None,
                )
                .await;
                jobs.fail(&job_id_for_task, e.to_string()).await;
            }
        }
    });

    axum::Json(json!({ "operation": "queue_restore", "success": 1, "job": job_id })).into_response()
}

async fn clear_new_all(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
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
    let mut cleared = 0u32;
    for mut archive in archives {
        if archive.isnew {
            archive.isnew = false;
            let _ = state.repos.archives.save(&archive).await;
            let _ =
                lanrurugi_search::indexer::set_isnew_index(&state.redis.search, &archive.id, false)
                    .await;
            cleared += 1;
        }
    }

    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_CLEAR_NEW_FLAGS,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("database".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "cleared": cleared })),
    )
    .await;

    axum::Json(json!({ "operation": "clear_new_all", "success": 1 })).into_response()
}

/// Deliberately dangerous — see the endpoint's own legacy description ("might lock you out of the
/// server as a client"). Drops every logical DB this instance knows about, matching legacy's
/// `drop_database`'s `FLUSHALL` semantics (which is server-wide, not scoped to one DB).
async fn drop_database(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    // Collected rather than discarded (the previous `let _: Result<(), _> = ...` silently dropped
    // every pool's outcome) — this is a whole-library wipe, so a `FLUSHDB` that actually failed on
    // any of the five logical DBs must not vanish without a trace.
    let mut failures: Vec<String> = Vec::new();
    for pool in [
        &state.redis.archive,
        &state.redis.minion,
        &state.redis.config,
        &state.redis.search,
        &state.redis.metrics,
    ] {
        match pool.get().await {
            Ok(mut conn) => {
                if let Err(e) = deadpool_redis::redis::cmd("FLUSHDB")
                    .query_async::<()>(&mut conn)
                    .await
                {
                    failures.push(e.to_string());
                }
            }
            Err(e) => failures.push(e.to_string()),
        }
    }

    // Written *after* the FLUSHDB loop above — including the `config` DB this very entry would
    // otherwise land in — deliberately: `record_manual` re-reads the retention setting and
    // re-derives a fresh id/timestamp *after* the wipe, so this one record survives its own
    // triggering operation instead of being flushed along with everything else a half-second
    // earlier would have caused.
    let outcome = if failures.is_empty() {
        Outcome::Success
    } else {
        Outcome::Failure {
            reason: failures.join("; "),
        }
    };
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_DROP,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("database".to_string()),
        },
        outcome,
        None,
        None,
    )
    .await;

    axum::Json(json!({ "operation": "drop_database", "success": 1 })).into_response()
}

/// Two-phase cleanup matching the legacy contract's documented *effect* ("unlinked at first,
/// deleted on a subsequent run"): an archive whose file is missing gets its `file` field cleared
/// (unlinked) on the first pass; an archive that's *already* unlinked (empty `file`) is deleted
/// outright. This differs from legacy's exact trigger (a `LRR_FILEMAP` cross-reference mismatch,
/// verified in `Utils/Database.pm::clean_database`) but preserves the same observable two-run
/// behavior for the common case (file deleted from disk out from under LANrurugi).
async fn clean_database(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
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
            let _ = state.recommend_cache.delete_for(archive.id.as_str()).await;
            deleted += 1;
            continue;
        }
        if !std::path::Path::new(&archive.file).exists() {
            archive.file.clear();
            let _ = state.repos.archives.save(&archive).await;
            unlinked += 1;
        }
    }

    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_CLEAN,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("database".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "deleted": deleted, "unlinked": unlinked })),
    )
    .await;

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
/// Shared "recompute archive IDs + rescan the library + heal pagecounts + backfill reverse
/// indexes" sequence — originally inline in `rebuild_index` below, extracted so
/// `queue_import_legacy` can run the exact same rebuild after a legacy import lands new/changed
/// archive metadata, without either call site's copy silently drifting from the other over time.
/// Records its own `record_manual` failure entries (using `action_type`/`target` from the
/// caller, since `rebuild_index` and `queue_import_legacy` want different activity-log framing)
/// and returns `Err` with the failure reason on the first failing step — the caller is
/// responsible for calling `jobs.fail` with it; this function itself never touches `jobs.finish`/
/// `jobs.fail` so a caller that wants to fold this result into a larger combined payload (e.g.
/// alongside an import summary) can still do so.
async fn run_rebuild_sequence(
    state: &AppState,
    job_id: &str,
    action_type: &'static str,
    auth: Option<&AuthContext>,
) -> Result<serde_json::Value, String> {
    let repos = &state.repos;
    let jobs = &state.jobs;

    let rebuild_failure_target = ActivityTarget {
        id: None,
        label: None,
        kind: Some("database".to_string()),
    };

    let rekey_summary = match lanrurugi_storage::rebuild::rekey_all(
        &repos.archives,
        &repos.categories,
        &repos.groupings,
        &repos.stamps,
        jobs,
        job_id,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            record_manual(
                state,
                auth,
                action_type,
                rebuild_failure_target,
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            return Err(e.to_string());
        }
    };

    let scan_summary = lanrurugi_scanner::full_scan::full_scan(
        &state.library.archive_dir,
        &repos.archives,
        &state.redis.config,
        &state.redis.search,
        &state.library.thumb_dir,
        jobs,
        job_id,
        Some(state.new_archive_tx.clone()),
    )
    .await;

    let heal_summary = lanrurugi_scanner::full_scan::heal_pagecounts(&repos.archives).await;

    // Issue #67: unconditional, not gated behind `rekey_summary.rekeyed` being non-empty — a
    // Category/Grouping saved before the `archive_id -> [category_id]`/`[tankoubon_id]`
    // reverse index existed has never had its membership written into it at all, regardless of
    // whether this particular rebuild happened to change any archive ID.
    if let Err(e) =
        lanrurugi_storage::rebuild::backfill_reverse_indexes(&repos.categories, &repos.groupings)
            .await
    {
        record_manual(
            state,
            auth,
            action_type,
            ActivityTarget {
                id: None,
                label: None,
                kind: Some("database".to_string()),
            },
            Outcome::Failure {
                reason: e.to_string(),
            },
            None,
            None,
        )
        .await;
        return Err(e.to_string());
    }

    Ok(json!({
        "rekeyed": rekey_summary.rekeyed.len(),
        "unchanged": rekey_summary.unchanged,
        "missing_file": rekey_summary.missing_file,
        "scanned": scan_summary.scanned,
        "newly_catalogued": scan_summary.catalogued,
        "errors": scan_summary.errors,
        "pagecount_heal": {
            "checked": heal_summary.checked,
            "healed": heal_summary.healed,
            "failed": heal_summary.failed,
            "skipped_known_failed": heal_summary.skipped_known_failed,
            "details": heal_summary.details,
        },
    }))
}

async fn rebuild_index(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_REBUILD_INDEX,
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

    let job_id = state.jobs.create("rebuild_index").await;
    let jobs = state.jobs.clone();
    let job_id_for_task = job_id.clone();
    let state_for_task = state.clone();
    let auth_for_task = auth.as_ref().map(|e| e.0.clone());

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        match run_rebuild_sequence(
            &state_for_task,
            &job_id_for_task,
            action_types::DATABASE_REBUILD_INDEX,
            auth_for_task.as_ref(),
        )
        .await
        {
            Ok(result) => jobs.finish(&job_id_for_task, result).await,
            Err(e) => jobs.fail(&job_id_for_task, e).await,
        }
    });

    axum::Json(json!({
        "operation": "rebuild_index",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImportConflictModeParam {
    Overwrite,
    Merge,
    Skip,
}

impl From<ImportConflictModeParam> for lanrurugi_backup::import_legacy::ImportConflictMode {
    fn from(m: ImportConflictModeParam) -> Self {
        match m {
            ImportConflictModeParam::Overwrite => Self::Overwrite,
            ImportConflictModeParam::Merge => Self::Merge,
            ImportConflictModeParam::Skip => Self::Skip,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ImportLegacyQuery {
    on_existing: ImportConflictModeParam,
    /// See `lanrurugi_backup::import_legacy::import_from_legacy`'s own `minimize_tags` param docs.
    /// Defaults to `false` (full tag import) when the query string omits it — matches every other
    /// boolean query param in this API (e.g. `ArchiveListParams`'s own `new_only`).
    #[serde(default)]
    minimize_tags: bool,
}

/// Imports a LANraragi (or LANrurugi) backup JSON file the caller uploads directly — no live
/// connection to any remote Redis instance (see `lanrurugi_backup::import_legacy`'s own module
/// docs for why: the official LANraragi Docker image never exposes its bundled Redis outside the
/// container by default). Same multipart-file-upload shape as `queue_restore` above, but this is
/// deliberately its own independent handler/parsing/persistence path, not a variant of
/// `queue_restore` — see `import_legacy`'s own docs on why the two must not share code despite
/// superficially similar JSON shapes.
async fn queue_import_legacy(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(query): Query<ImportLegacyQuery>,
    mut multipart: Multipart,
) -> Response {
    let mut file_bytes: Option<Vec<u8>> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "queue_import_legacy",
                    e.to_string(),
                )
            }
        };
        if field.name() == Some("file") {
            file_bytes = field.bytes().await.ok().map(|b| b.to_vec());
        }
    }
    let Some(bytes) = file_bytes else {
        return error(
            StatusCode::BAD_REQUEST,
            "queue_import_legacy",
            "No file provided.",
        );
    };
    let doc: lanrurugi_backup::import_legacy::LegacyBackupDocument =
        match serde_json::from_slice(&bytes) {
            Ok(d) => d,
            Err(e) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "queue_import_legacy",
                    format!("Not a valid LANraragi backup JSON: {e}"),
                )
            }
        };

    let target = ActivityTarget {
        id: None,
        label: None,
        kind: Some("database".to_string()),
    };

    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DATABASE_IMPORT_LEGACY,
        target.clone(),
        Outcome::Success,
        None,
        None,
    )
    .await;

    let on_existing: lanrurugi_backup::import_legacy::ImportConflictMode = query.on_existing.into();
    let minimize_tags = query.minimize_tags;
    let job_id = state.jobs.create("import_legacy").await;
    let jobs = state.jobs.clone();
    let repos = state.repos.clone();
    let job_id_for_task = job_id.clone();
    let state_for_task = state.clone();
    let auth_for_task = auth.as_ref().map(|e| e.0.clone());

    let import_snapshots = state.import_snapshots.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let import_result = lanrurugi_backup::import_legacy::import_from_legacy(
            doc,
            on_existing,
            minimize_tags,
            &repos.archives,
            &repos.categories,
            &repos.groupings,
            &repos.stamps,
        )
        .await;

        // `import_from_legacy` is not transactional (see its own docs) — on `Err`, it still
        // returns whatever `summary`/`snapshot` had accumulated before the failing write, rather
        // than discarding that progress. Persist that partial snapshot the same as a full success
        // (still a valid rollback point for whatever *did* get written) and surface the error
        // alongside how much of the import actually completed, instead of just "it failed".
        let (summary, snapshot, import_error) = match import_result {
            Ok((summary, snapshot)) => (summary, snapshot, None),
            Err((e, summary, snapshot)) => (summary, snapshot, Some(e)),
        };
        if let Some(e) = &import_error {
            record_manual(
                &state_for_task,
                auth_for_task.as_ref(),
                action_types::DATABASE_IMPORT_LEGACY,
                target.clone(),
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
        }

        // A snapshot with nothing in it (nothing this import actually overwrote — e.g. every
        // record was skipped or matched nothing) is not worth a rollback point; saving it would
        // just be a no-op entry cluttering the list with nothing to undo.
        let snapshot_is_empty = snapshot.archives.is_empty()
            && snapshot.categories.is_empty()
            && snapshot.tankoubons.is_empty()
            && snapshot.stamps.is_empty();
        if !snapshot_is_empty {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock is after the Unix epoch")
                .as_secs() as i64;
            if let Err(e) = import_snapshots.save(now, snapshot).await {
                tracing::warn!(error = %e, "failed to save import rollback snapshot — import itself still succeeded");
            }
        }
        let import_count = match import_snapshots.increment_import_count().await {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!(error = %e, "failed to increment import count");
                0
            }
        };

        let summary_json = json!({
            "archives_updated": summary.archives_updated,
            "archives_skipped_already_exists": summary.archives_skipped_already_exists,
            "archives_skipped_no_match": summary.archives_skipped_no_match,
            "archives_ambiguous_match": summary.archives_ambiguous_match,
            "archives_multiple_legacy_records_same_target": summary.archives_multiple_legacy_records_same_target,
            "titles_mojibake_repaired": summary.titles_mojibake_repaired,
            "categories_restored": summary.categories_restored,
            "categories_skipped_already_exists": summary.categories_skipped_already_exists,
            "tankoubons_restored": summary.tankoubons_restored,
            "tankoubons_skipped_already_exists": summary.tankoubons_skipped_already_exists,
            "stamps_restored": summary.stamps_restored,
            "stamps_skipped_already_exists": summary.stamps_skipped_already_exists,
            "new_category_ids": summary.new_category_ids,
            "new_tankoubon_ids": summary.new_tankoubon_ids,
            "new_stamp_ids": summary.new_stamp_ids,
            "import_count": import_count,
        });

        // Skip the (workspace-wide, not cheap) rebuild entirely when nothing was actually
        // written — every archive/category/tankoubon/stamp was either skipped, unmatched, or
        // ambiguous. Mirrors `snapshot_is_empty` above (both are really asking the same "did this
        // call change anything" question), but also covers a fresh category/tankoubon creation,
        // which writes real data without necessarily populating `snapshot` (nothing pre-existed to
        // snapshot).
        let nothing_written = summary.archives_updated == 0
            && summary.categories_restored == 0
            && summary.tankoubons_restored == 0
            && summary.stamps_restored == 0;
        if nothing_written && import_error.is_none() {
            jobs.finish(&job_id_for_task, summary_json).await;
        } else {
            match run_rebuild_sequence(
                &state_for_task,
                &job_id_for_task,
                action_types::DATABASE_IMPORT_LEGACY,
                auth_for_task.as_ref(),
            )
            .await
            {
                Ok(rebuild_result) => {
                    let mut result = summary_json;
                    result["rebuild"] = json!(rebuild_result);
                    match import_error {
                        // The import itself failed partway through — still report how much of it
                        // completed (not transactional, see `import_from_legacy`'s own docs)
                        // rather than only "it failed" with no indication of the partial write.
                        Some(e) => {
                            result["partial"] = json!(true);
                            jobs.fail(
                                &job_id_for_task,
                                format!("{e} (partial progress: {result})"),
                            )
                            .await;
                        }
                        None => {
                            jobs.finish(&job_id_for_task, result).await;
                        }
                    }
                }
                Err(e) => jobs.fail(&job_id_for_task, e).await,
            }
        }
    });

    axum::Json(json!({
        "operation": "queue_import_legacy",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

/// How many times this instance has ever run a LANraragi import — read by the frontend before
/// showing the import UI, so it can warn ("you've done this before — consider a full backup
/// first") on a 2nd-or-later import without waiting for the import itself to complete. Read-only,
/// separate from `queue_import_legacy`'s own increment (which happens after a real import
/// finishes), matching `ImportSnapshotRepository::import_count`'s own read/increment split.
async fn import_legacy_count(State(state): State<AppState>) -> Response {
    match state.import_snapshots.import_count().await {
        Ok(count) => axum::Json(json!({ "import_count": count })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "import_legacy_count",
            e.to_string(),
        ),
    }
}

fn import_snapshot_metadata_json(
    m: &lanrurugi_backup::import_snapshot::ImportSnapshotMetadata,
) -> serde_json::Value {
    json!({
        "id": m.id,
        "created_at": m.created_at,
        "archive_count": m.archive_count,
        "category_count": m.category_count,
        "tankoubon_count": m.tankoubon_count,
        "stamp_count": m.stamp_count,
    })
}

/// Newest-first metadata list (no `document` payload — see
/// `ImportSnapshotRepository::list_metadata`'s own docs) for the Backup page's Time-Machine-style
/// rollback-point picker.
async fn list_import_snapshots(State(state): State<AppState>) -> Response {
    match state.import_snapshots.list_metadata().await {
        Ok(snapshots) => {
            let body: Vec<_> = snapshots
                .iter()
                .map(import_snapshot_metadata_json)
                .collect();
            axum::Json(body).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_import_snapshots",
            e.to_string(),
        ),
    }
}

/// Returns the full `BackupDocument` payload as a downloadable JSON file — same shape
/// `POST /database/restore` accepts, so the frontend's own "restore" flow can take this file
/// straight back in with zero format translation on either side.
async fn download_import_snapshot(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    match state.import_snapshots.get(&id).await {
        Ok(Some(snapshot)) => (
            [(header::CONTENT_TYPE, "application/json")],
            axum::Json(snapshot.document),
        )
            .into_response(),
        Ok(None) => not_found(
            "download_import_snapshot",
            format!("Snapshot {id} not found."),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "download_import_snapshot",
            e.to_string(),
        ),
    }
}

async fn delete_import_snapshot(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    match state.import_snapshots.get(&id).await {
        Ok(None) => {
            return not_found(
                "delete_import_snapshot",
                format!("Snapshot {id} not found."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_import_snapshot",
                e.to_string(),
            )
        }
        Ok(Some(_)) => {}
    };
    match state.import_snapshots.delete(&id).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DATABASE_IMPORT_LEGACY,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("import_snapshot".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "delete_import_snapshot", "success": 1 }))
                .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_import_snapshot",
            e.to_string(),
        ),
    }
}
