//! Recycle bin endpoints and the recycle/restore orchestration
//! (`specs/019-recycle-bin-safety-password`).
//!
//! Deleting an archive moves its file into `<library>/.lanrurugi-recycle/<id>/` and records a
//! recoverable entry; restore puts the file back and re-creates everything deletion destroyed.
//! Non-user deletions (cleanup, superseded revisions, split originals) call
//! `archives::purge_one_archive` directly and never create entries.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::Archive;
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::archives::{is_valid_archive_id, purge_one_archive, DeleteOneOutcome, PurgeOptions};
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::settings::{DEFAULT_RECYCLE_MAX_SIZE_GB, DEFAULT_RECYCLE_RETENTION_DAYS};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::{CONFIG_KEY, FILEMAP_KEY, SAFETY_PASSWORD_HASH_FIELD};
use lanrurugi_storage::recycle::{
    entry_dir, move_into_bin, move_out_of_bin, payload_size, remove_entry_dir, scan_entry_dirs,
    write_meta, RecycleActor, RecycleConflict, RecycleEntry, RecycleEntryView, RecycleKind,
    RecycleMemberView, RecycleRepository, RecycleSnapshot, RecycleTank, RECYCLE_SCHEMA,
};

pub fn router() -> Router<AppState> {
    // Session-only; `route_policy.csv` denies both token roles explicitly (see its own block).
    Router::new()
        .route("/recycle-bin", get(list_recycle_bin))
        .route("/recycle-bin/{id}/thumbnail", get(recycle_thumbnail))
        .route("/recycle-bin/{id}/restore", post(restore_entry))
        .route("/recycle-bin/{id}", delete(purge_entry_endpoint))
        .route("/recycle-bin", delete(empty_bin))
}

#[derive(Debug, Deserialize)]
struct SafetyBody {
    safety_password: String,
}

fn repository(state: &AppState) -> RecycleRepository {
    RecycleRepository::new(state.redis.archive.clone())
}

fn actor(auth: Option<&AuthContext>) -> RecycleActor {
    match auth {
        Some(auth) => RecycleActor {
            kind: match auth.method {
                crate::auth_context::AuthMethod::Session => "session",
                crate::auth_context::AuthMethod::Token { .. } => "token",
                _ => "guest",
            }
            .to_string(),
            label: auth.trace_label(),
        },
        None => RecycleActor {
            kind: "system".to_string(),
            label: "system".to_string(),
        },
    }
}

/// Resolved recycle policy. Read directly from `LRR_CONFIG` rather than through the settings
/// module's full payload, which builds every field.
pub(crate) struct Policy {
    pub retention_days: i64,
    pub max_bytes: u64,
}

pub(crate) async fn read_policy(state: &AppState) -> Policy {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(_) => {
            return Policy {
                retention_days: DEFAULT_RECYCLE_RETENTION_DAYS,
                max_bytes: (DEFAULT_RECYCLE_MAX_SIZE_GB as u64) * 1024 * 1024 * 1024,
            }
        }
    };
    let days: Option<String> = conn
        .hget(CONFIG_KEY, "recycle_retention_days")
        .await
        .ok()
        .flatten();
    let gib: Option<String> = conn
        .hget(CONFIG_KEY, "recycle_max_size_gb")
        .await
        .ok()
        .flatten();
    // The MiB field this shipped with for one release: an instance that saved it keeps its cap
    // (rounded up) instead of silently reverting to unlimited.
    let legacy_mib: Option<String> = conn
        .hget(CONFIG_KEY, "recycle_max_size_mb")
        .await
        .ok()
        .flatten();
    let days = days
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_RECYCLE_RETENTION_DAYS)
        .max(0);
    let gib = match gib.and_then(|v| v.parse::<i64>().ok()) {
        Some(v) => v.max(0) as u64,
        None => legacy_mib
            .and_then(|v| v.parse::<i64>().ok())
            .map(|mib| (mib.max(0) as u64).div_ceil(1024))
            .unwrap_or(DEFAULT_RECYCLE_MAX_SIZE_GB as u64),
    };
    Policy {
        retention_days: days,
        max_bytes: gib.saturating_mul(1024 * 1024 * 1024),
    }
}

async fn safety_password_configured(state: &AppState) -> bool {
    match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hexists::<_, _, bool>(CONFIG_KEY, SAFETY_PASSWORD_HASH_FIELD)
            .await
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Builds the restore snapshot: everything `delete_one_archive` would otherwise destroy.
async fn build_snapshot(state: &AppState, archive: &Archive) -> RecycleSnapshot {
    let categories = match state.repos.categories.for_archive(&archive.id).await {
        Ok(cats) => cats.into_iter().map(|c| c.catid).collect(),
        Err(e) => {
            tracing::warn!(id = %archive.id, error = %e, "failed to snapshot category membership for the bin");
            Vec::new()
        }
    };
    let bookmarks = match state.bookmarks.list_for_archive(archive.id.as_str()).await {
        Ok(list) => list
            .into_iter()
            .map(|b| lanrurugi_storage::recycle::BookmarkSnapshot {
                page: b.page,
                bookmarked_at: b.bookmarked_at,
                name: b.name,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(id = %archive.id, error = %e, "failed to snapshot bookmarks for the bin");
            Vec::new()
        }
    };
    let mut stamps = Vec::with_capacity(archive.stamp_ids.len());
    for stamp_id in &archive.stamp_ids {
        match state.repos.stamps.get(stamp_id).await {
            Ok(Some(stamp)) => stamps.push(stamp),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(id = %archive.id, stamp = %stamp_id, error = %e, "failed to snapshot a stamp for the bin")
            }
        }
    }
    let version_history = state
        .repos
        .archives
        .version_history(&archive.id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    RecycleSnapshot {
        tags: archive.tags.clone(),
        summary: archive.summary.clone(),
        isnew: archive.isnew,
        lastreadpage: archive.lastreadpage,
        lastreadtime: archive.lastreadtime,
        thumbhash: archive.thumbhash.clone(),
        toc: archive.toc.clone(),
        has_patch: archive.has_patch,
        corrupted_pages: archive.corrupted_pages.clone(),
        heal_failed_at: archive.heal_failed_at,
        version_history,
        categories,
        bookmarks,
        stamps,
    }
}

fn entry_from_record(
    archive: &Archive,
    snapshot: RecycleSnapshot,
    patch_file_name: Option<String>,
    deleted_by: RecycleActor,
) -> RecycleEntry {
    let original_path = archive.file.clone();
    RecycleEntry {
        schema: RECYCLE_SCHEMA,
        id: archive.id.clone(),
        title: archive.title.clone(),
        name: archive.name.clone(),
        file_name: Path::new(&original_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| archive.name.clone()),
        original_path,
        patch_file_name,
        size: archive.arcsize,
        page_count: archive.pagecount,
        deleted_at: lanrurugi_storage::recycle::now_secs(),
        deleted_by,
        snapshot,
    }
}

/// Recycle one archive: snapshot, move bytes, record the entry, then drop the live record.
/// Every failure before the move leaves the library untouched (FR-002).
pub(crate) async fn recycle_archive(
    state: &AppState,
    id: &ArchiveId,
    auth: Option<&AuthContext>,
) -> DeleteOneOutcome {
    // Serializes every operation on this archive (delete, restore, purge) without blocking other
    // ids. `lock_filename` waits cooperatively, so a concurrent duplicate request simply queues.
    let _entry_guard = state.lock_filename(&format!("recycle:{id}")).await;

    let archive = match state.repos.archives.get(id).await {
        Ok(Some(a)) => a,
        Ok(None) => return DeleteOneOutcome::NotFound,
        Err(e) => return DeleteOneOutcome::Error(e.to_string()),
    };

    // A record whose file is already gone (legacy leftovers, a manually deleted file) has nothing
    // to move: purge it like a cleanup would rather than failing the delete.
    if archive.file.is_empty() || !Path::new(&archive.file).is_file() {
        tracing::warn!(%id, file = %archive.file, "deleting an archive whose file is already gone");
        return purge_one_archive(state, id, auth, PurgeOptions::PERMANENT).await;
    }

    let snapshot = build_snapshot(state, &archive).await;
    let original_path = archive.file.clone();
    let patch_path = lanrurugi_scanner::patch::patch_path_for(Path::new(&original_path));
    let patch_source = patch_path.exists().then_some(patch_path.as_path());

    let patch_file_name =
        match move_into_bin(&state.library.archive_dir, id, &original_path, patch_source).await {
            Ok(name) => name,
            Err(e) => {
                return DeleteOneOutcome::Error(format!("failed to move archive into the bin: {e}"))
            }
        };
    // The file (and its patch) left this directory: remove any directory the move emptied, so a
    // deleted archive does not leave a skeleton of empty folders behind.
    prune_empty_parents(&state.library.archive_dir, Path::new(&original_path)).await;
    // The sidecar describes bytes that are no longer in the library. Restoring the archive
    // re-catalogues it, which writes a fresh one — so this is not a data loss.
    if let Err(e) = lanrurugi_storage::sidecar::remove(&state.library.archive_dir, id.as_str()) {
        tracing::warn!(%id, error = %e, "failed to remove recycled archive's sidecar");
    }

    let entry = entry_from_record(&archive, snapshot, patch_file_name, actor(auth));

    let dir = entry_dir(&state.library.archive_dir, id);
    if let Err(e) = write_meta(&dir, &entry).await {
        rollback_move(state, id, &entry, &original_path).await;
        return DeleteOneOutcome::Error(format!("failed to record the recycle entry: {e}"));
    }
    if let Err(e) = repository(state).put(&entry).await {
        rollback_move(state, id, &entry, &original_path).await;
        return DeleteOneOutcome::Error(format!("failed to record the recycle entry: {e}"));
    }

    // The file no longer lives at the original path; leaving `path -> old id` in the filemap
    // would make a later file arriving there look like a rewrite of this recycled archive.
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &original_path).await;
    }

    let outcome = purge_one_archive(state, id, auth, PurgeOptions::AFTER_RECYCLE).await;
    if let DeleteOneOutcome::Error(e) = outcome {
        // The payload is safely in the bin, but the live record could not be dropped. Report it
        // rather than claiming success; startup reconciliation finishes the job.
        return DeleteOneOutcome::Error(e);
    }

    record_manual(
        state,
        auth,
        action_types::ARCHIVE_RECYCLE,
        ActivityTarget {
            id: Some(id.to_string()),
            label: Some(archive.title.clone()),
            kind: Some("archive".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "file": original_path, "size": archive.arcsize })),
    )
    .await;

    DeleteOneOutcome::Deleted {
        filename: archive.name,
    }
}

/// Best-effort inverse of the move when the entry could not be recorded.
async fn rollback_move(
    state: &AppState,
    id: &ArchiveId,
    entry: &RecycleEntry,
    original_path: &str,
) {
    let destination = Path::new(original_path);
    let result = move_out_of_bin(
        &state.library.archive_dir,
        id,
        &entry.file_name,
        entry.patch_file_name.as_deref(),
        destination,
    )
    .await;
    if let Err(e) = result {
        tracing::warn!(%id, error = %e, "failed to roll a recycled archive back out of the bin");
        return;
    }
    let _ = repository(state).remove(id).await;
    let _ = remove_entry_dir(&state.library.archive_dir, id).await;
}

/// One entry as the UI sees it: expiry, restorability and the id conflict are computed on read.
async fn view_of(state: &AppState, entry: RecycleEntry, policy: &Policy) -> RecycleEntryView {
    let expires_at = (policy.retention_days > 0)
        .then(|| entry.deleted_at + policy.retention_days.saturating_mul(86_400));
    let now = lanrurugi_storage::recycle::now_secs();
    let remaining_seconds = expires_at.map(|at| (at - now).max(0));

    let mut conflict = None;
    if entry.schema != RECYCLE_SCHEMA {
        conflict = Some(RecycleConflict::UnsupportedSchema);
    } else {
        match payload_size(&state.library.archive_dir, &entry.id, &entry.file_name).await {
            None => conflict = Some(RecycleConflict::PayloadMissing),
            Some(len) if len != entry.size => conflict = Some(RecycleConflict::PayloadSizeMismatch),
            Some(_) => {
                if state
                    .repos
                    .archives
                    .get(&entry.id)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    conflict = Some(RecycleConflict::IdenticalArchiveExists);
                }
            }
        }
    }

    RecycleEntryView {
        id: entry.id.clone(),
        title: entry.title.clone(),
        name: entry.name.clone(),
        original_path: entry.original_path.clone(),
        file_name: entry.file_name.clone(),
        size: entry.size,
        page_count: entry.page_count,
        deleted_at: entry.deleted_at,
        deleted_by: entry.deleted_by.clone(),
        expires_at,
        remaining_seconds,
        restorable: conflict.is_none(),
        conflict,
        has_thumbnail: thumbnail_path(state, &entry.id).is_some(),
        kind: RecycleKind::Archive,
        member_count: 0,
        members: Vec::new(),
    }
}

/// A recycled Tankoubon as one row: the grouping's own title/time, with read-only member summaries
/// behind the row's expand control.
async fn tank_view_of(state: &AppState, tank: &RecycleTank, policy: &Policy) -> RecycleEntryView {
    let expires_at = (policy.retention_days > 0)
        .then(|| tank.deleted_at + policy.retention_days.saturating_mul(86_400));
    let now = lanrurugi_storage::recycle::now_secs();
    let remaining_seconds = expires_at.map(|at| (at - now).max(0));

    let mut members = Vec::with_capacity(tank.grouping.archives.len());
    let mut conflict = None;
    let mut has_thumbnail = false;
    for id in &tank.grouping.archives {
        match repository(state).get(id).await {
            Ok(Some(entry)) => {
                if !has_thumbnail {
                    has_thumbnail = thumbnail_path(state, id).is_some();
                }
                let view = view_of(state, entry.clone(), policy).await;
                if !view.restorable && conflict.is_none() {
                    conflict = view.conflict;
                }
                members.push(RecycleMemberView {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    size: entry.size,
                    page_count: entry.page_count,
                    restorable: view.restorable,
                    conflict: view.conflict,
                });
            }
            _ => {
                conflict.get_or_insert(RecycleConflict::PayloadMissing);
                members.push(RecycleMemberView {
                    id: id.clone(),
                    title: id.to_string(),
                    size: 0,
                    page_count: 0,
                    restorable: false,
                    conflict: Some(RecycleConflict::PayloadMissing),
                });
            }
        }
    }
    let member_count = members.len() as u32;

    RecycleEntryView {
        kind: RecycleKind::Tankoubon,
        id: ArchiveId(tank.tank_id.clone()),
        title: tank.grouping.name.clone(),
        name: tank.grouping.name.clone(),
        original_path: String::new(),
        file_name: String::new(),
        size: tank.size,
        page_count: tank.page_count,
        deleted_at: tank.deleted_at,
        deleted_by: tank.deleted_by.clone(),
        expires_at,
        remaining_seconds,
        restorable: conflict.is_none(),
        conflict,
        has_thumbnail,
        member_count,
        members,
    }
}

fn thumbnail_path(state: &AppState, id: &ArchiveId) -> Option<(PathBuf, &'static str)> {
    if !is_valid_archive_id(id.as_str()) {
        return None;
    }
    let shard = &id.as_str()[0..2.min(id.len())];
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state
            .library
            .thumb_dir
            .join(shard)
            .join(format!("{id}.{}", format.extension()));
        if path.is_file() {
            return Some((path, format.content_type()));
        }
    }
    None
}

async fn purge_entry_endpoint(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<SafetyBody>,
) -> Response {
    let auth_ref = auth.as_ref().map(|e| &e.0);
    // Verify before resolving the entry: a caller without the safety password must not learn
    // whether an id exists (FR-022).
    if let Err(e) = crate::safety_password::verify(&state, auth_ref, &body.safety_password).await {
        return e.into_response("purge_archive");
    }
    let id = ArchiveId(id);
    if let Ok(Some(tank)) = repository(&state).get_tank(id.as_str()).await {
        let _guard = state
            .lock_filename(&format!("recycle:{}", tank.tank_id))
            .await;
        return match purge_tankoubon(&state, &tank).await {
            Ok(removed) => {
                record_manual(
                    &state,
                    auth_ref,
                    action_types::TANKOUBON_PURGE,
                    ActivityTarget {
                        id: Some(tank.tank_id.clone()),
                        label: Some(tank.grouping.name.clone()),
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "members": removed, "reason": "manual" })),
                )
                .await;
                axum::Json(json!({
                    "operation": "purge_tankoubon",
                    "success": 1,
                    "id": tank.tank_id,
                    "kind": "tankoubon",
                    "members": removed,
                }))
                .into_response()
            }
            Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "purge_tankoubon", e),
        };
    }
    let entry = match repository(&state).get(&id).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return not_found("purge_archive", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "purge_archive",
                e.to_string(),
            )
        }
    };
    let _guard = state.lock_filename(&format!("recycle:{id}")).await;
    if let Err(e) = purge_entry(&state, &entry).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "purge_archive", e);
    }
    record_manual(
        &state,
        auth_ref,
        action_types::ARCHIVE_PURGE,
        ActivityTarget {
            id: Some(id.to_string()),
            label: Some(entry.title.clone()),
            kind: Some("archive".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "file": entry.original_path, "reason": "manual" })),
    )
    .await;
    axum::Json(json!({
        "operation": "purge_archive",
        "success": 1,
        "id": id,
    }))
    .into_response()
}

async fn empty_bin(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<SafetyBody>,
) -> Response {
    let auth_ref = auth.as_ref().map(|e| &e.0);
    if let Err(e) = crate::safety_password::verify(&state, auth_ref, &body.safety_password).await {
        return e.into_response("empty_recycle_bin");
    }
    let repo = repository(&state);
    let tanks = match repo.list_tanks().await {
        Ok(tanks) => tanks,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "empty_recycle_bin",
                e.to_string(),
            )
        }
    };
    let member_ids: std::collections::HashSet<String> = tanks
        .iter()
        .flat_map(|t| t.grouping.archives.iter().map(|a| a.0.clone()))
        .collect();
    let entries = match repo.list().await {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| !member_ids.contains(e.id.as_str()))
            .collect::<Vec<_>>(),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "empty_recycle_bin",
                e.to_string(),
            )
        }
    };
    let mut deleted = 0u64;
    let mut failed = Vec::new();
    for tank in &tanks {
        let _guard = state
            .lock_filename(&format!("recycle:{}", tank.tank_id))
            .await;
        match purge_tankoubon(&state, tank).await {
            Ok(_) => {
                deleted += 1;
                record_manual(
                    &state,
                    auth_ref,
                    action_types::TANKOUBON_PURGE,
                    ActivityTarget {
                        id: Some(tank.tank_id.clone()),
                        label: Some(tank.grouping.name.clone()),
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "reason": "empty" })),
                )
                .await;
            }
            Err(e) => failed.push(json!({ "id": tank.tank_id, "error": e })),
        }
    }
    for entry in &entries {
        let _guard = state.lock_filename(&format!("recycle:{}", entry.id)).await;
        match purge_entry(&state, entry).await {
            Ok(()) => {
                deleted += 1;
                record_manual(
                    &state,
                    auth_ref,
                    action_types::ARCHIVE_PURGE,
                    ActivityTarget {
                        id: Some(entry.id.to_string()),
                        label: Some(entry.title.clone()),
                        kind: Some("archive".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "file": entry.original_path, "reason": "empty" })),
                )
                .await;
            }
            Err(e) => failed.push(json!({ "id": entry.id, "error": e })),
        }
    }
    record_manual(
        &state,
        auth_ref,
        action_types::RECYCLE_EMPTY,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("recycle_bin".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "count": deleted, "failed": failed.len() })),
    )
    .await;
    axum::Json(json!({
        "operation": "empty_recycle_bin",
        "success": 1,
        "deleted": deleted,
        "failed": failed,
    }))
    .into_response()
}

async fn list_recycle_bin(State(state): State<AppState>) -> Response {
    let policy = read_policy(&state).await;
    let repo = repository(&state);
    let tanks = match repo.list_tanks().await {
        Ok(tanks) => tanks,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "recycle_bin",
                e.to_string(),
            )
        }
    };
    let member_ids: std::collections::HashSet<String> = tanks
        .iter()
        .flat_map(|t| t.grouping.archives.iter().map(|a| a.0.clone()))
        .collect();
    let entries = match repo.list().await {
        Ok(entries) => entries,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "recycle_bin",
                e.to_string(),
            )
        }
    };

    let mut views: Vec<RecycleEntryView> = Vec::with_capacity(entries.len() + tanks.len());
    for tank in &tanks {
        views.push(tank_view_of(&state, tank, &policy).await);
    }
    for entry in entries {
        if member_ids.contains(entry.id.as_str()) {
            continue; // shown inside its Tankoubon's row instead
        }
        views.push(view_of(&state, entry, &policy).await);
    }
    views.sort_by_key(|v| std::cmp::Reverse(v.deleted_at));
    let bytes: u64 = views.iter().map(|v| v.size).sum();
    let leftovers = leftover_count(&state).await;

    axum::Json(json!({
        "entries": views,
        "usage": { "count": views.len(), "bytes": bytes },
        "limits": { "retention_days": policy.retention_days, "max_bytes": policy.max_bytes },
        "leftovers": { "count": leftovers },
        "safety_password": { "configured": safety_password_configured(&state).await },
    }))
    .into_response()
}

/// Entry directories with no inventory record — reported so a crashed delete is visible, never
/// silently offered as a normal entry.
async fn leftover_count(state: &AppState) -> usize {
    let known: std::collections::HashSet<String> = match repository(state).list().await {
        Ok(entries) => entries.into_iter().map(|e| e.id.0).collect(),
        Err(_) => return 0,
    };
    let mut count = 0;
    for dir in scan_entry_dirs(&state.library.archive_dir).await {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !known.contains(&name) {
            count += 1;
        }
    }
    count
}

async fn recycle_thumbnail(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Response {
    let mut id = ArchiveId(id);
    if let Ok(Some(tank)) = repository(&state).get_tank(id.as_str()).await {
        match tank.grouping.archives.first() {
            Some(first) => id = first.clone(),
            None => return not_found("recycle_thumbnail", "No thumbnail for this entry."),
        }
    }
    if !is_valid_archive_id(id.as_str()) {
        return not_found("recycle_thumbnail", format!("{id} does not exist."));
    }
    match repository(&state).get(&id).await {
        Ok(Some(_)) => {}
        _ => return not_found("recycle_thumbnail", format!("{id} does not exist.")),
    }
    let Some((path, content_type)) = thumbnail_path(&state, &id) else {
        return not_found("recycle_thumbnail", "No thumbnail for this entry.");
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => return not_found("recycle_thumbnail", "No thumbnail for this entry."),
    };
    (
        [
            (
                header::CONTENT_TYPE,
                header::HeaderValue::from_static(content_type),
            ),
            (
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("private, no-cache"),
            ),
        ],
        Body::from(bytes),
    )
        .into_response()
}

pub(crate) enum RestoreOutcome {
    Restored { path: String, renamed: bool },
    RestoredTank { name: String, members: usize },
    NotFound,
    Conflict(RecycleConflict),
    Error(String),
}

async fn restore_entry(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let auth_ref = auth.as_ref().map(|e| &e.0);
    if repository(&state)
        .get_tank(&id)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        return match restore_tankoubon(&state, &id, auth_ref).await {
            RestoreOutcome::RestoredTank { name, members } => axum::Json(json!({
                "operation": "restore_tankoubon",
                "success": 1,
                "id": id,
                "kind": "tankoubon",
                "name": name,
                "members": members,
            }))
            .into_response(),
            RestoreOutcome::NotFound => {
                not_found("restore_tankoubon", format!("{id} does not exist."))
            }
            RestoreOutcome::Conflict(reason) => error(
                StatusCode::CONFLICT,
                "restore_tankoubon",
                conflict_message(reason),
            ),
            RestoreOutcome::Error(e) => {
                error(StatusCode::INTERNAL_SERVER_ERROR, "restore_tankoubon", e)
            }
            RestoreOutcome::Restored { .. } => unreachable!("tank restore returns RestoredTank"),
        };
    }
    let archive_id = ArchiveId(id.clone());
    match restore_archive(&state, &archive_id, auth_ref).await {
        RestoreOutcome::Restored { path, renamed } => axum::Json(json!({
            "operation": "restore_archive",
            "success": 1,
            "id": id,
            "path": path,
            "renamed": renamed,
        }))
        .into_response(),
        RestoreOutcome::NotFound => not_found("restore_archive", format!("{id} does not exist.")),
        RestoreOutcome::Conflict(reason) => error(
            StatusCode::CONFLICT,
            "restore_archive",
            conflict_message(reason),
        ),
        RestoreOutcome::Error(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "restore_archive", e),
        RestoreOutcome::RestoredTank { .. } => unreachable!("archive restore returns Restored"),
    }
}

fn conflict_message(reason: RecycleConflict) -> &'static str {
    match reason {
        RecycleConflict::IdenticalArchiveExists => {
            "an archive with identical content already exists in the library"
        }
        RecycleConflict::PayloadMissing => "the stored file for this entry is missing",
        RecycleConflict::PayloadSizeMismatch => {
            "the stored file no longer matches this entry's recorded size"
        }
        RecycleConflict::UnsupportedSchema => "this entry uses an unsupported format version",
    }
}

pub(crate) async fn restore_archive(
    state: &AppState,
    id: &ArchiveId,
    auth: Option<&AuthContext>,
) -> RestoreOutcome {
    let _entry_guard = state.lock_filename(&format!("recycle:{id}")).await;
    let entry = match repository(state).get(id).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return RestoreOutcome::NotFound,
        Err(e) => return RestoreOutcome::Error(e.to_string()),
    };
    if entry.schema != RECYCLE_SCHEMA {
        return RestoreOutcome::Conflict(RecycleConflict::UnsupportedSchema);
    }
    match payload_size(&state.library.archive_dir, id, &entry.file_name).await {
        None => return RestoreOutcome::Conflict(RecycleConflict::PayloadMissing),
        Some(len) if len != entry.size => {
            return RestoreOutcome::Conflict(RecycleConflict::PayloadSizeMismatch)
        }
        Some(_) => {}
    }
    match state.repos.archives.get(id).await {
        Ok(Some(_)) => return RestoreOutcome::Conflict(RecycleConflict::IdenticalArchiveExists),
        Ok(None) => {}
        Err(e) => return RestoreOutcome::Error(e.to_string()),
    }

    let (destination, renamed) = resolve_destination(state, &entry);
    let basename = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.file_name.clone());
    // Same lock the watcher's ingest takes for this basename: the rename below emits a create
    // event, and this keeps it from cataloguing the file before the record exists.
    let _file_guard = state.lock_filename(&basename).await;

    let patch_destination = entry
        .patch_file_name
        .as_ref()
        .map(|_| lanrurugi_scanner::patch::patch_path_for(&destination));

    if let Err(e) = move_out_of_bin(
        &state.library.archive_dir,
        id,
        &entry.file_name,
        entry.patch_file_name.as_deref(),
        &destination,
    )
    .await
    {
        return RestoreOutcome::Error(format!("failed to move the stored file back: {e}"));
    }

    let destination_str = destination.to_string_lossy().into_owned();
    let restore_result = restore_record(state, &entry, &destination_str).await;
    if let Err(e) = restore_result {
        // Put the payload back so the entry is not left half-restored.
        if let Err(back) = move_into_bin(
            &state.library.archive_dir,
            id,
            &destination_str,
            patch_destination.as_deref(),
        )
        .await
        {
            tracing::error!(%id, error = %back, "failed to return a half-restored archive to the bin");
        }
        return RestoreOutcome::Error(e);
    }

    if let Err(e) = repository(state).remove(id).await {
        tracing::warn!(%id, error = %e, "failed to drop a restored entry from the bin inventory");
    }
    let _ = remove_entry_dir(&state.library.archive_dir, id).await;

    record_manual(
        state,
        auth,
        action_types::ARCHIVE_RESTORE,
        ActivityTarget {
            id: Some(id.to_string()),
            label: Some(entry.title.clone()),
            kind: Some("archive".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "file": destination_str })),
    )
    .await;

    RestoreOutcome::Restored {
        path: destination_str,
        renamed,
    }
}

/// Re-creates the archive record, its user data and its search-index entries.
async fn restore_record(
    state: &AppState,
    entry: &RecycleEntry,
    destination: &str,
) -> Result<(), String> {
    let snap = &entry.snapshot;
    let arcsize = tokio::fs::metadata(destination)
        .await
        .map(|m| m.len())
        .unwrap_or(entry.size);
    let archive = Archive {
        id: entry.id.clone(),
        name: entry.name.clone(),
        title: entry.title.clone(),
        file: destination.to_string(),
        tags: snap.tags.clone(),
        summary: snap.summary.clone(),
        arcsize,
        pagecount: entry.page_count,
        isnew: snap.isnew,
        lastreadpage: snap.lastreadpage,
        lastreadtime: snap.lastreadtime,
        thumbhash: snap.thumbhash.clone(),
        toc: snap.toc.clone(),
        stamp_ids: snap.stamps.iter().map(|s| s.stamp_id.clone()).collect(),
        heal_failed_at: snap.heal_failed_at,
        corrupted_pages: snap.corrupted_pages.clone(),
        has_patch: entry.patch_file_name.is_some(),
    };
    state
        .repos
        .archives
        .save(&archive)
        .await
        .map_err(|e| e.to_string())?;

    if !snap.version_history.is_empty() {
        let _ = state
            .repos
            .archives
            .set_version_history(&entry.id, &snap.version_history)
            .await;
    }
    for category in &snap.categories {
        if let Err(e) =
            crate::categories::add_archive_to_category(state, category.as_str(), entry.id.as_str())
                .await
        {
            tracing::warn!(id = %entry.id, category = %category, error = ?e, "failed to restore a category membership");
        }
    }
    for bookmark in &snap.bookmarks {
        if let Err(e) = state
            .bookmarks
            .add(entry.id.as_str(), bookmark.page, bookmark.bookmarked_at)
            .await
        {
            tracing::warn!(id = %entry.id, page = bookmark.page, error = %e, "failed to restore a bookmark");
            continue;
        }
        if let Some(name) = &bookmark.name {
            let _ = state
                .bookmarks
                .set_name(entry.id.as_str(), bookmark.page, Some(name))
                .await;
        }
    }
    for stamp in &snap.stamps {
        if let Err(e) = state.repos.stamps.restore_raw(stamp).await {
            tracing::warn!(id = %entry.id, stamp = %stamp.stamp_id, error = %e, "failed to restore a stamp");
        }
    }

    // The file is back on disk: make the watcher treat this path as known before it sees the
    // rename, using the id the scanner would compute (equal to the stored id for size-aware
    // archives; writing a legacy id here would make the watcher re-key the record and orphan the
    // stamps/bookmarks restored above).
    let computed =
        lanrurugi_storage::id::size_aware_id(Path::new(destination)).map_err(|e| e.to_string())?;
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.hset(FILEMAP_KEY, destination, &computed).await;
    }

    let search = &state.redis.search;
    let _ = lanrurugi_search::indexer::index_new_archive(
        search,
        &state.equivalence,
        entry.id.as_str(),
        &archive.title,
    )
    .await;
    let _ = lanrurugi_search::indexer::update_tag_indexes(
        search,
        &state.equivalence,
        entry.id.as_str(),
        "",
        &archive.tags,
    )
    .await;
    let _ =
        lanrurugi_search::indexer::set_isnew_index(search, entry.id.as_str(), archive.isnew).await;
    if let Ok(tanks) = state.repos.groupings.for_archive(&entry.id).await {
        let joined: Vec<String> = tanks.into_iter().map(|g| g.tankid.0).collect();
        let _ = lanrurugi_search::indexer::sync_tank_membership(search, &joined, &[]).await;
    }

    Ok(())
}

/// Original path when it is inside the library and free; otherwise the library root. A taken path
/// keeps the archive beside the occupant under a ` (restored N)` name, never overwriting it.
/// Deletes directories emptied by moving the archive out, walking up only while each level is
/// empty and stopping at the library root. A directory that still holds anything (another archive,
/// a cover, a note) stops the walk; nothing outside the library is ever touched.
async fn prune_empty_parents(library: &Path, file_path: &Path) {
    let mut current = file_path.parent();
    while let Some(dir) = current {
        if dir == library || !dir.starts_with(library) {
            break;
        }
        let empty = match tokio::fs::read_dir(dir).await {
            Ok(mut entries) => matches!(entries.next_entry().await, Ok(None)),
            Err(_) => false,
        };
        if !empty || tokio::fs::remove_dir(dir).await.is_err() {
            break;
        }
        tracing::debug!(directory = %dir.display(), "removed a directory emptied by recycling an archive");
        current = dir.parent();
    }
}

fn resolve_destination(state: &AppState, entry: &RecycleEntry) -> (PathBuf, bool) {
    let library = &state.library.archive_dir;
    let original = Path::new(&entry.original_path);
    // Always restore to the recorded path when it is inside the library: `move_out_of_bin`
    // recreates the parent chain, so a directory the delete itself pruned comes back with it.
    // Only a path outside the current library (the library was moved/reconfigured) falls back to
    // the root.
    let candidate = if original.starts_with(library) {
        original.to_path_buf()
    } else {
        library.join(&entry.file_name)
    };
    if !candidate.exists() {
        let renamed = candidate != original;
        return (candidate, renamed);
    }
    let stem = candidate
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| entry.file_name.clone());
    let extension = candidate
        .extension()
        .map(|s| s.to_string_lossy().into_owned());
    for n in 1..1000 {
        let suffix = if n == 1 {
            " (restored)".to_string()
        } else {
            format!(" (restored {n})")
        };
        let name = match &extension {
            Some(ext) => format!("{stem}{suffix}.{ext}"),
            None => format!("{stem}{suffix}"),
        };
        let candidate = candidate.with_file_name(name);
        if !candidate.exists() {
            return (candidate, true);
        }
    }
    (candidate, true)
}

/// Moves a whole Tankoubon into the bin: every member archive becomes an ordinary bin entry, then
/// one `RecycleTank` record ties them together. The caller deletes the grouping record afterwards.
pub(crate) async fn recycle_tankoubon(
    state: &AppState,
    tank_id: &TankId,
    auth: Option<&AuthContext>,
) -> Result<(), String> {
    let grouping = state
        .repos
        .groupings
        .get(tank_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{tank_id} does not exist."))?;
    let member_ids = grouping.archives.clone();

    let mut recycled: Vec<ArchiveId> = Vec::new();
    for member in &member_ids {
        match recycle_archive(state, member, auth).await {
            DeleteOneOutcome::Deleted { .. } => recycled.push(member.clone()),
            DeleteOneOutcome::NotFound => {
                rollback_recycled(state, &recycled).await;
                return Err(format!(
                    "member archive {member} disappeared while recycling the tankoubon"
                ));
            }
            DeleteOneOutcome::Error(e) => {
                rollback_recycled(state, &recycled).await;
                return Err(e);
            }
        }
    }

    let repo = repository(state);
    let mut size = 0u64;
    let mut page_count = 0u32;
    for id in &member_ids {
        if let Ok(Some(entry)) = repo.get(id).await {
            size += entry.size;
            page_count += entry.page_count;
        }
    }
    let tank = RecycleTank {
        schema: RECYCLE_SCHEMA,
        tank_id: tank_id.0.clone(),
        grouping,
        size,
        page_count,
        deleted_at: lanrurugi_storage::recycle::now_secs(),
        deleted_by: actor(auth),
    };
    if let Err(e) = repo.put_tank(&tank).await {
        rollback_recycled(state, &recycled).await;
        return Err(format!("failed to record the recycled tankoubon: {e}"));
    }
    Ok(())
}

async fn rollback_recycled(state: &AppState, ids: &[ArchiveId]) {
    for id in ids {
        if let RestoreOutcome::Restored { .. } = restore_archive(state, id, None).await {
            tracing::warn!(%id, "rolled back a recycled member after the tankoubon recycle failed");
        }
    }
}

/// Restores a recycled Tankoubon: preflights every member, restores the archives, then re-creates
/// the grouping record and its search-index entries.
pub(crate) async fn restore_tankoubon(
    state: &AppState,
    tank_id: &str,
    auth: Option<&AuthContext>,
) -> RestoreOutcome {
    let repo = repository(state);
    let tank = match repo.get_tank(tank_id).await {
        Ok(Some(tank)) => tank,
        Ok(None) => return RestoreOutcome::NotFound,
        Err(e) => return RestoreOutcome::Error(e.to_string()),
    };
    if tank.schema != RECYCLE_SCHEMA {
        return RestoreOutcome::Conflict(RecycleConflict::UnsupportedSchema);
    }

    // Preflight the whole tank before moving anything, so a conflict cannot leave it half restored.
    for id in &tank.grouping.archives {
        match repo.get(id).await {
            Ok(Some(entry)) => match payload_size(&state.library.archive_dir, id, &entry.file_name)
                .await
            {
                None => return RestoreOutcome::Conflict(RecycleConflict::PayloadMissing),
                Some(len) if len != entry.size => {
                    return RestoreOutcome::Conflict(RecycleConflict::PayloadSizeMismatch)
                }
                Some(_) => {
                    if state.repos.archives.get(id).await.ok().flatten().is_some() {
                        return RestoreOutcome::Conflict(RecycleConflict::IdenticalArchiveExists);
                    }
                }
            },
            Ok(None) => return RestoreOutcome::Conflict(RecycleConflict::PayloadMissing),
            Err(e) => return RestoreOutcome::Error(e.to_string()),
        }
    }

    let mut restored: Vec<ArchiveId> = Vec::new();
    for id in &tank.grouping.archives {
        match restore_archive(state, id, auth).await {
            RestoreOutcome::Restored { .. } => restored.push(id.clone()),
            other => {
                rollback_recycled(state, &restored).await;
                return other;
            }
        }
    }

    if let Err(e) = state.repos.groupings.save(&tank.grouping).await {
        rollback_recycled(state, &restored).await;
        return RestoreOutcome::Error(e.to_string());
    }
    let _ =
        lanrurugi_search::indexer::add_tank_to_index(&state.redis.search, &tank.grouping.tankid.0)
            .await;
    let _ = lanrurugi_search::indexer::update_title_index(
        &state.redis.search,
        &state.equivalence,
        &tank.grouping.tankid.0,
        "",
        &tank.grouping.name,
    )
    .await;
    let joined: Vec<String> = tank.grouping.archives.iter().map(|a| a.0.clone()).collect();
    let _ =
        lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &joined, &[]).await;

    if let Err(e) = repo.remove_tank(&tank.tank_id).await {
        tracing::warn!(tank = %tank.tank_id, error = %e, "failed to drop a restored tankoubon from the bin inventory");
    }

    record_manual(
        state,
        auth,
        action_types::TANKOUBON_RESTORE,
        ActivityTarget {
            id: Some(tank.tank_id.clone()),
            label: Some(tank.grouping.name.clone()),
            kind: Some("tankoubon".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "members": restored.len() })),
    )
    .await;

    RestoreOutcome::RestoredTank {
        name: tank.grouping.name,
        members: restored.len(),
    }
}

/// Permanently deletes a recycled Tankoubon: every member entry's payload, then the tank record.
pub(crate) async fn purge_tankoubon(state: &AppState, tank: &RecycleTank) -> Result<u64, String> {
    let repo = repository(state);
    let mut removed = 0u64;
    let mut first_error: Option<String> = None;
    for id in &tank.grouping.archives {
        if let Ok(Some(entry)) = repo.get(id).await {
            match purge_entry(state, &entry).await {
                Ok(()) => removed += 1,
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
    }
    repo.remove_tank(&tank.tank_id)
        .await
        .map_err(|e| e.to_string())?;
    match first_error {
        Some(e) => Err(e),
        None => Ok(removed),
    }
}

/// Removes one bin entry and its payload/thumbnails permanently. Shared by the endpoint and the
/// sweep.
pub(crate) async fn purge_entry(state: &AppState, entry: &RecycleEntry) -> Result<(), String> {
    remove_entry_dir(&state.library.archive_dir, &entry.id)
        .await
        .map_err(|e| e.to_string())?;
    let shard = &entry.id.as_str()[0..2.min(entry.id.as_str().len())];
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state.library.thumb_dir.join(shard).join(format!(
            "{}.{}",
            entry.id,
            format.extension()
        ));
        let _ = tokio::fs::remove_file(&path).await;
    }
    let pages = state.library.thumb_dir.join(shard).join(entry.id.as_str());
    let _ = tokio::fs::remove_dir_all(&pages).await;
    repository(state)
        .remove(&entry.id)
        .await
        .map_err(|e| e.to_string())
}

/// Reconciles the bin inventory against what is on disk (research §3). Runs once at startup:
/// finishes a delete that crashed after the move, rebuilds a missing `meta.json` from a record that
/// still exists, and adopts a directory whose hash write was lost.
pub async fn reconcile(state: &AppState) -> u64 {
    let mut repaired = 0;
    for dir in scan_entry_dirs(&state.library.archive_dir).await {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let id = ArchiveId(name);
        if !is_valid_archive_id(id.as_str()) {
            continue;
        }
        let inventory = match repository(state).get(&id).await {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        match inventory {
            Some(entry) => {
                // A payload-bearing entry whose record is still live means the delete crashed
                // between the hash write and the record drop.
                if state.repos.archives.get(&id).await.ok().flatten().is_some() {
                    let _ = purge_one_archive(state, &id, None, PurgeOptions::AFTER_RECYCLE).await;
                    repaired += 1;
                }
                let _ = entry;
            }
            None => match lanrurugi_storage::recycle::read_meta(&dir).await {
                Some(entry) => {
                    if repository(state).put(&entry).await.is_ok() {
                        repaired += 1;
                        if state.repos.archives.get(&id).await.ok().flatten().is_some() {
                            let _ =
                                purge_one_archive(state, &id, None, PurgeOptions::AFTER_RECYCLE)
                                    .await;
                        }
                    }
                }
                None => {
                    // No journal: recover from the still-present record, since a crash right after
                    // the rename is the only way to get here.
                    if let Ok(Some(archive)) = state.repos.archives.get(&id).await {
                        let patch_name = archive_file_patch_name(&archive);
                        let entry = entry_from_record(
                            &archive,
                            build_snapshot(state, &archive).await,
                            patch_name,
                            RecycleActor {
                                kind: "system".to_string(),
                                label: "recovery".to_string(),
                            },
                        );
                        if write_meta(&dir, &entry).await.is_ok()
                            && repository(state).put(&entry).await.is_ok()
                        {
                            let _ =
                                purge_one_archive(state, &id, None, PurgeOptions::AFTER_RECYCLE)
                                    .await;
                            repaired += 1;
                        }
                    }
                }
            },
        }
    }
    repaired
}

fn archive_file_patch_name(archive: &Archive) -> Option<String> {
    let patch = lanrurugi_scanner::patch::patch_path_for(Path::new(&archive.file));
    patch.file_name().map(|n| n.to_string_lossy().into_owned())
}

#[derive(Debug, Default)]
pub struct SweepReport {
    pub expired: u64,
    pub over_capacity: u64,
    pub leftovers: u64,
    pub bytes: u64,
}

/// Enforces the retention window and the size cap (FR-028–FR-031). The newest entry is never
/// evicted by capacity, even when it alone exceeds the cap — the most recent mistake must stay
/// recoverable.
/// One unit = one row in the bin: a whole Tankoubon or a standalone archive. Members of a
/// recycled tank are never evicted on their own.
struct SweepUnit {
    key: String,
    deleted_at: i64,
    size: u64,
    tank: Option<RecycleTank>,
    archive: Option<RecycleEntry>,
}

async fn purge_unit(state: &AppState, unit: &SweepUnit) -> Result<(), String> {
    if let Some(tank) = &unit.tank {
        return purge_tankoubon(state, tank).await.map(|_| ());
    }
    match &unit.archive {
        Some(entry) => purge_entry(state, entry).await,
        None => Ok(()),
    }
}

pub async fn sweep(state: &AppState) -> SweepReport {
    let policy = read_policy(state).await;
    let repo = repository(state);
    let tanks = match repo.list_tanks().await {
        Ok(tanks) => tanks,
        Err(e) => {
            tracing::warn!(error = %e, "recycle sweep could not read the recycled tankoubons");
            return SweepReport::default();
        }
    };
    let member_ids: std::collections::HashSet<String> = tanks
        .iter()
        .flat_map(|t| t.grouping.archives.iter().map(|a| a.0.clone()))
        .collect();
    let entries = match repo.list().await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "recycle sweep could not read the bin");
            return SweepReport::default();
        }
    };

    let mut units: Vec<SweepUnit> = tanks
        .into_iter()
        .map(|tank| SweepUnit {
            key: tank.tank_id.clone(),
            deleted_at: tank.deleted_at,
            size: tank.size,
            tank: Some(tank),
            archive: None,
        })
        .collect();
    for entry in entries {
        if member_ids.contains(entry.id.as_str()) {
            continue;
        }
        units.push(SweepUnit {
            key: entry.id.0.clone(),
            deleted_at: entry.deleted_at,
            size: entry.size,
            tank: None,
            archive: Some(entry),
        });
    }
    units.sort_by_key(|u| std::cmp::Reverse(u.deleted_at));
    let newest = units.first().map(|u| u.key.clone());
    let now = lanrurugi_storage::recycle::now_secs();
    let mut report = SweepReport::default();
    let mut remaining: Vec<SweepUnit> = Vec::new();

    for unit in units {
        let expired = policy.retention_days > 0
            && now - unit.deleted_at >= policy.retention_days.saturating_mul(86_400);
        if expired {
            if purge_unit(state, &unit).await.is_ok() {
                report.expired += 1;
                report.bytes += unit.size;
            }
        } else {
            remaining.push(unit);
        }
    }

    if policy.max_bytes > 0 {
        let mut total: u64 = remaining.iter().map(|u| u.size).sum();
        remaining.sort_by_key(|u| u.deleted_at);
        for unit in &remaining {
            if total <= policy.max_bytes {
                break;
            }
            if Some(&unit.key) == newest.as_ref() {
                continue;
            }
            if purge_unit(state, unit).await.is_ok() {
                total = total.saturating_sub(unit.size);
                report.over_capacity += 1;
                report.bytes += unit.size;
            }
        }
    }

    // Orphan payload directories with no inventory entry and no journal are only reclaimed after
    // a 24 h grace, so a crash mid-delete is never mistaken for garbage.
    let mut known: std::collections::HashSet<String> = match repo.list().await {
        Ok(list) => list.into_iter().map(|e| e.id.0).collect(),
        Err(_) => return report,
    };
    known.extend(member_ids);
    for dir in scan_entry_dirs(&state.library.archive_dir).await {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if known.contains(&name) {
            continue;
        }
        let old_enough = tokio::fs::metadata(&dir)
            .await
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age.as_secs() >= 86_400);
        if old_enough && tokio::fs::remove_dir_all(&dir).await.is_ok() {
            report.leftovers += 1;
        }
    }

    if report.expired + report.over_capacity > 0 {
        crate::activity::record_automatic(
            state,
            "recycle",
            action_types::RECYCLE_SWEEP,
            ActivityTarget {
                id: None,
                label: None,
                kind: Some("recycle_bin".to_string()),
            },
            Outcome::Success,
            Some(json!({
                "reason": if report.expired > 0 { "expired" } else { "over_capacity" },
                "expired": report.expired,
                "over_capacity": report.over_capacity,
                "bytes": report.bytes,
            })),
            None,
        )
        .await;
    }
    report
}
