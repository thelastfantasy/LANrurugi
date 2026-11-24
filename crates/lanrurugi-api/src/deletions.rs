//! Deletion journal (`specs/028-deletion-safety-net`).
//!
//! Every archive record that is permanently deleted is copied into a Redis hash *before* the
//! delete commits, so the record (tags, reading progress, timestamps — the whole legacy hash) can
//! be put back afterwards. The hook lives in `archives::purge_one_archive`, the single exit every
//! delete path shares, so Clean Database, the reader's own delete, superseded revisions and splits
//! are all covered without each caller remembering to journal.
//!
//! What is *not* in the entry: category/tankoubon memberships, bookmarks and stamps. Those are
//! separate keys that `purge_one_archive` also drops, and `research.md` D2/D8 deliberately keep
//! the journal to the archive record itself. The library-wide snapshot is what covers the rest.

use std::collections::HashMap;
use std::path::Path as FsPath;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{SecondsFormat, TimeZone, Utc};
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::Archive;
use lanrurugi_core::ids::ArchiveId;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::settings::read_deletion_journal_days;
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::{DELETION_JOURNAL_KEY, FILEMAP_KEY, RESTORED_MISSING_KEY};

/// Bump only when the entry shape changes incompatibly; readers skip what they cannot parse.
pub const JOURNAL_VERSION: u32 = 1;

/// Hard cap on journal size, independent of the retention window. A clean that drops 100k records
/// must not push 100k archive records into Redis; the oldest are evicted first.
pub const MAX_ENTRIES: usize = 20_000;

/// One deleted archive, as it looked immediately before the delete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub record: Archive,
    pub deleted_at_ms: i64,
    /// `clean` / `single` / `superseded` / `split` / `other` — see [`PurgeSource`].
    pub source: String,
    /// The record's `file` at deletion time, kept outside `record` so a listing never has to
    /// deserialize a whole record just to answer "is it still on disk".
    pub path: String,
    #[serde(default = "current_version")]
    pub journal_version: u32,
}

fn current_version() -> u32 {
    JOURNAL_VERSION
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn rfc3339(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// What a journal write did, so the delete path can tell "recorded" from "recording is off" from
/// "the write failed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalWrite {
    Recorded,
    /// `deletion_journal_days = 0`: the journal is deliberately off.
    Disabled,
    Failed,
}

/// Writes one entry. `Ok(false)` means recording is turned off (`deletion_journal_days = 0`);
/// `Err` means the caller must tell the user this delete cannot be undone.
pub(crate) async fn journal_put(
    state: &AppState,
    archive: &Archive,
    source: &str,
) -> Result<bool, String> {
    if read_deletion_journal_days(state).await <= 0 {
        return Ok(false);
    }
    let entry = JournalEntry {
        record: archive.clone(),
        deleted_at_ms: now_ms(),
        source: source.to_string(),
        path: archive.file.clone(),
        journal_version: JOURNAL_VERSION,
    };
    let value = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    let mut conn = state.redis.archive.get().await.map_err(|e| e.to_string())?;
    let _: () = conn
        .hset(DELETION_JOURNAL_KEY, archive.id.as_str(), value)
        .await
        .map_err(|e| e.to_string())?;
    trim(&mut conn, MAX_ENTRIES).await?;
    Ok(true)
}

/// Evicts the oldest entries once the hash is over `limit`. Kept as an explicit limit/key
/// parameter so the eviction rule can be exercised without writing twenty thousand entries in a
/// test (or touching a real instance's journal).
async fn trim_key(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    limit: usize,
) -> Result<(), String> {
    let len: usize = conn.hlen(key).await.map_err(|e| e.to_string())?;
    if len <= limit {
        return Ok(());
    }
    let all: HashMap<String, String> = conn.hgetall(key).await.map_err(|e| e.to_string())?;
    let mut ordered: Vec<(String, i64)> = all
        .iter()
        .map(|(id, raw)| {
            // An unparsable entry sorts oldest, so corruption is evicted before real data.
            let deleted_at = serde_json::from_str::<JournalEntry>(raw)
                .map(|e| e.deleted_at_ms)
                .unwrap_or(0);
            (id.clone(), deleted_at)
        })
        .collect();
    ordered.sort_by_key(|(_, deleted_at)| *deleted_at);
    let victims: Vec<String> = ordered
        .into_iter()
        .take(len - limit)
        .map(|(id, _)| id)
        .collect();
    let _: () = conn.hdel(key, &victims).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn trim(conn: &mut deadpool_redis::Connection, limit: usize) -> Result<(), String> {
    trim_key(conn, DELETION_JOURNAL_KEY, limit).await
}

/// Every readable entry, newest first. Corrupt entries are logged and skipped rather than failing
/// the whole listing — one bad byte must not hide the other 200 entries.
pub async fn journal_list(state: &AppState) -> Result<Vec<JournalEntry>, String> {
    let mut conn = state.redis.archive.get().await.map_err(|e| e.to_string())?;
    let all: HashMap<String, String> = conn
        .hgetall(DELETION_JOURNAL_KEY)
        .await
        .map_err(|e| e.to_string())?;
    let mut entries = Vec::with_capacity(all.len());
    let mut corrupt = 0usize;
    for (id, raw) in all {
        match serde_json::from_str::<JournalEntry>(&raw) {
            Ok(entry) if entry.record.id.as_str() == id => entries.push(entry),
            _ => {
                corrupt += 1;
                tracing::warn!(%id, "skipping a corrupt deletion-journal entry");
            }
        }
    }
    if corrupt > 0 {
        tracing::warn!(corrupt, "deletion journal contained unreadable entries");
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.deleted_at_ms));
    Ok(entries)
}

async fn journal_get(state: &AppState, id: &str) -> Result<Option<JournalEntry>, String> {
    let mut conn = state.redis.archive.get().await.map_err(|e| e.to_string())?;
    let raw: Option<String> = conn
        .hget(DELETION_JOURNAL_KEY, id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    match serde_json::from_str::<JournalEntry>(&raw) {
        Ok(entry) if entry.record.id.as_str() == id => Ok(Some(entry)),
        Ok(_) => {
            tracing::warn!(%id, "deletion-journal entry's record id does not match its field");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(%id, error = %e, "unreadable deletion-journal entry");
            Ok(None)
        }
    }
}

/// Drops one entry. Public within the crate for the delete path's own rollback: a delete whose
/// record survived must not leave a journal entry claiming it did not.
pub(crate) async fn forget_journal_entry(state: &AppState, id: &str) -> Result<(), String> {
    let mut conn = state.redis.archive.get().await.map_err(|e| e.to_string())?;
    let _: () = conn
        .hdel(DELETION_JOURNAL_KEY, id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn clear_key(state: &AppState) -> Result<(), String> {
    let mut conn = state.redis.archive.get().await.map_err(|e| e.to_string())?;
    let _: () = conn
        .del(DELETION_JOURNAL_KEY)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Outcome of restoring a single entry.
#[derive(Debug)]
pub enum RestoreOne {
    Restored {
        file_missing: bool,
    },
    /// A record with this id exists again — never overwritten (`research.md` D8).
    AlreadyExists,
    NotFound,
    Failed(String),
}

/// Puts one journaled record back, verbatim, then drops its journal entry.
///
/// The record is written with `create_or_replace`, which puts the record back as a whole
/// (tags/progress/etc.) the way a re-catalogue would. Search-side indexes were removed by the
/// delete, so they are re-added here; without that the record exists but is invisible to search.
pub async fn restore_one(state: &AppState, id: &str) -> RestoreOne {
    if !crate::archives::is_valid_archive_id(id) {
        return RestoreOne::NotFound;
    }
    let entry = match journal_get(state, id).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return RestoreOne::NotFound,
        Err(e) => return RestoreOne::Failed(e),
    };
    let archive_id = ArchiveId(id.to_string());
    match state.repos.archives.get(&archive_id).await {
        Ok(Some(_)) => return RestoreOne::AlreadyExists,
        Ok(None) => {}
        Err(e) => return RestoreOne::Failed(e.to_string()),
    }
    // A user delete moves the bytes into the recycle bin before the record is purged, so for that
    // path the bin's own restore is both richer (categories, bookmarks, stamps) and what actually
    // puts the file back where it was. `NotFound` here just means "not recycled" — Clean Database
    // and the other purge paths leave no bin entry.
    match crate::recycle::restore_archive(state, &archive_id, None).await {
        crate::recycle::RestoreOutcome::Restored { .. } => {
            if let Err(e) = forget_journal_entry(state, id).await {
                tracing::warn!(%id, error = %e, "restored from the bin but could not drop the journal entry");
            }
            return RestoreOne::Restored {
                file_missing: false,
            };
        }
        crate::recycle::RestoreOutcome::NotFound => {}
        other => {
            let reason = match other {
                crate::recycle::RestoreOutcome::Error(e) => e,
                _ => "the recycle entry is unusable".to_string(),
            };
            tracing::warn!(%id, error = %reason, "could not restore from the recycle bin; restoring the journal record only");
        }
    }
    if let Err(e) = state.repos.archives.create_or_replace(&entry.record).await {
        return RestoreOne::Failed(e.to_string());
    }
    if let Err(e) = forget_journal_entry(state, id).await {
        tracing::warn!(%id, error = %e, "restored the record but could not drop its journal entry");
    };
    reindex_restored(state, &entry.record).await;
    let file_missing = entry.record.file.is_empty() || !FsPath::new(&entry.record.file).is_file();
    if file_missing {
        mark_restored_missing(state, id).await;
    }
    RestoreOne::Restored { file_missing }
}

/// Exempts a restored-but-fileless record from `repair_zombie_archives` for the journal's own
/// retention window. Without this the startup sweep deletes it again on the very next restart,
/// which is the opposite of what restoring a record for a not-yet-remounted library is for.
pub(crate) async fn mark_restored_missing(state: &AppState, id: &str) {
    match state.redis.archive.get().await {
        Ok(mut conn) => {
            let stored: Result<(), _> = conn.hset(RESTORED_MISSING_KEY, id, now_ms()).await;
            if let Err(e) = stored {
                // Without the marker the zombie sweep deletes this restored record on the next
                // restart, which is precisely what restoring it for a missing library avoids.
                tracing::warn!(%id, error = %e, "could not record the restored-missing marker");
            }
        }
        Err(e) => tracing::warn!(%id, error = %e, "could not record the restored-missing marker"),
    }
}

/// Whether this record was restored from the journal and its file still has not come back.
pub async fn is_restored_missing(state: &AppState, id: &str) -> bool {
    match state.redis.archive.get().await {
        Ok(mut conn) => conn
            .hexists::<_, _, bool>(RESTORED_MISSING_KEY, id)
            .await
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// Drops the marker once the record is no longer a zombie candidate (file back, or record gone).
pub async fn clear_restored_missing(state: &AppState, id: &str) {
    if let Ok(mut conn) = state.redis.archive.get().await {
        let _: Result<(), _> = conn.hdel(RESTORED_MISSING_KEY, id).await;
    }
}

/// Re-adds a restored record to the search index and the filemap. Best-effort, matching every
/// other derived-index write: the record itself is already safely back.
async fn reindex_restored(state: &AppState, record: &Archive) {
    let search = &state.redis.search;
    if let Err(e) = lanrurugi_search::indexer::index_new_archive(
        search,
        &state.equivalence,
        record.id.as_str(),
        &record.title,
    )
    .await
    {
        tracing::warn!(id = %record.id, error = %e, "failed to re-index a restored archive");
    }
    if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
        search,
        &state.equivalence,
        record.id.as_str(),
        "",
        &record.tags,
    )
    .await
    {
        tracing::warn!(id = %record.id, error = %e, "failed to re-index a restored archive's tags");
    }
    if let Err(e) =
        lanrurugi_search::indexer::set_isnew_index(search, record.id.as_str(), record.isnew).await
    {
        tracing::warn!(id = %record.id, error = %e, "failed to restore the new flag index");
    }
    // The path was dropped from the filemap when the record was deleted; restoring it before a
    // scan sees the file avoids re-hashing bytes that are already catalogued.
    if !record.file.is_empty() && FsPath::new(&record.file).is_file() {
        if let Ok(mut conn) = state.redis.config.get().await {
            let _: Result<(), _> = conn
                .hset(FILEMAP_KEY, &record.file, record.id.as_str())
                .await;
        }
    }
}

#[derive(Debug, Default)]
pub struct RestoreAllReport {
    pub restored: usize,
    pub skipped: usize,
    pub missing_files: usize,
    pub failed: usize,
}

/// Restores every entry, one at a time; one failure never stops the rest (a 20k-entry incident
/// should not be all-or-nothing).
pub async fn restore_all(state: &AppState) -> Result<RestoreAllReport, String> {
    let entries = journal_list(state).await?;
    let mut report = RestoreAllReport::default();
    for entry in entries {
        match restore_one(state, entry.record.id.as_str()).await {
            RestoreOne::Restored { file_missing: true } => {
                report.restored += 1;
                report.missing_files += 1;
            }
            RestoreOne::Restored { .. } => report.restored += 1,
            RestoreOne::AlreadyExists => report.skipped += 1,
            RestoreOne::NotFound => {}
            RestoreOne::Failed(e) => {
                tracing::warn!(id = %entry.record.id, error = %e, "failed to restore a journaled archive");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Drops entries past the retention window. Returns how many were removed.
///
/// Called from the hourly `recycle::sweep`; a non-empty result first takes a `pre-journal-purge`
/// snapshot, so the last copy of "what was deleted recently" is not lost to the sweep itself.
pub async fn purge_expired(state: &AppState) -> u64 {
    let days = read_deletion_journal_days(state).await;
    let now = now_ms();
    let cutoff = days.saturating_mul(86_400_000);
    let expired: Vec<String> = {
        let mut conn = match state.redis.archive.get().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "deletion journal sweep could not reach Redis");
                return 0;
            }
        };
        let all: HashMap<String, String> = match conn.hgetall(DELETION_JOURNAL_KEY).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "deletion journal sweep could not read the journal");
                return 0;
            }
        };
        all.into_iter()
            .filter(|(_, raw)| {
                // `0` means "keep nothing": the journal is off, so what an earlier setting left
                // behind is cleared too rather than lingering forever.
                if days <= 0 {
                    return true;
                }
                let deleted_at = serde_json::from_str::<JournalEntry>(raw)
                    .map(|e| e.deleted_at_ms)
                    .unwrap_or(0);
                now - deleted_at > cutoff
            })
            .map(|(id, _)| id)
            .collect()
    };
    if expired.is_empty() {
        return 0;
    }
    if let Err(e) = crate::snapshots::take_export(
        state,
        "pre-journal-purge",
        std::sync::Arc::new(crate::snapshots::NoProgress),
    )
    .await
    {
        tracing::warn!(error = %e, "could not snapshot before purging expired deletions");
    }
    match state.redis.archive.get().await {
        Ok(mut conn) => match conn.hdel::<_, _, ()>(DELETION_JOURNAL_KEY, &expired).await {
            Ok(()) => {
                // The restored-missing exemption only lasts as long as the journal does: once the
                // window is over, a record whose file never came back is a genuine zombie again.
                let markers: HashMap<String, String> =
                    conn.hgetall(RESTORED_MISSING_KEY).await.unwrap_or_default();
                let stale: Vec<String> = markers
                    .into_iter()
                    .filter(|(_, raw)| {
                        raw.parse::<i64>()
                            .map(|ts| now - ts > cutoff)
                            .unwrap_or(true)
                    })
                    .map(|(id, _)| id)
                    .collect();
                if !stale.is_empty() {
                    let _: Result<(), _> = conn.hdel(RESTORED_MISSING_KEY, &stale).await;
                }
                tracing::info!(removed = expired.len(), "deletion journal expiry swept");
                expired.len() as u64
            }
            Err(e) => {
                tracing::warn!(error = %e, "deletion journal sweep could not remove entries");
                0
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "deletion journal sweep lost its Redis connection");
            0
        }
    }
}

/// Journal write for a delete path, converted into what the caller must report. Never returns an
/// error: the delete itself always proceeds (`research.md` D9).
pub(crate) async fn journal_put_best_effort(
    state: &AppState,
    archive: &Archive,
    source: &str,
) -> JournalWrite {
    match journal_put(state, archive, source).await {
        Ok(true) => JournalWrite::Recorded,
        Ok(false) => JournalWrite::Disabled,
        Err(e) => {
            tracing::warn!(
                id = %archive.id,
                error = %e,
                "failed to write the deletion journal; this delete cannot be undone"
            );
            JournalWrite::Failed
        }
    }
}

pub fn router() -> Router<AppState> {
    // Session-only; `route_policy.csv` denies both token roles (see its own specs/028 block).
    Router::new()
        .route("/deletions", get(list_deletions).delete(clear_deletions))
        .route("/deletions/restore-all", post(restore_all_deletions))
        .route("/deletions/{id}", delete(purge_deletion))
        .route("/deletions/{id}/restore", post(restore_deletion))
}

async fn list_deletions(State(state): State<AppState>) -> Response {
    let entries = match journal_list(&state).await {
        Ok(entries) => entries,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "list_deletions", e),
    };
    let retention_days = read_deletion_journal_days(&state).await;
    let total = entries.len();
    let entries: Vec<_> = entries
        .into_iter()
        .map(|entry| {
            let file_exists = !entry.path.is_empty() && FsPath::new(&entry.path).is_file();
            json!({
                "id": entry.record.id.as_str(),
                "title": entry.record.title,
                "deleted_at": rfc3339(entry.deleted_at_ms),
                "source": entry.source,
                "path": entry.path,
                "file_exists": file_exists,
            })
        })
        .collect();
    axum::Json(json!({
        "operation": "list_deletions",
        "success": 1,
        "retention_days": retention_days,
        "total": total,
        "entries": entries,
    }))
    .into_response()
}

async fn restore_deletion(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    let response = match restore_one(&state, &id).await {
        RestoreOne::Restored { file_missing } => {
            record_manual(
                &state,
                auth,
                action_types::DELETION_RESTORE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("archive".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({ "file_missing": file_missing })),
            )
            .await;
            axum::Json(json!({
                "operation": "restore_deletion",
                "success": 1,
                "id": id,
                "restored": 1,
                "file_missing": file_missing,
            }))
            .into_response()
        }
        RestoreOne::AlreadyExists => (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "operation": "restore_deletion",
                "success": 0,
                "error": "An archive with this id already exists; it was not overwritten.",
                "reason": "already_exists",
                "id": id,
            })),
        )
            .into_response(),
        RestoreOne::NotFound => not_found("restore_deletion", "No journal entry for that id."),
        RestoreOne::Failed(e) => {
            record_manual(
                &state,
                auth,
                action_types::DELETION_RESTORE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("archive".to_string()),
                },
                Outcome::Failure { reason: e.clone() },
                None,
                None,
            )
            .await;
            error(StatusCode::INTERNAL_SERVER_ERROR, "restore_deletion", e)
        }
    };
    response
}

async fn restore_all_deletions(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    match restore_all(&state).await {
        Ok(report) => {
            record_manual(
                &state,
                auth,
                action_types::DELETION_RESTORE,
                ActivityTarget {
                    id: None,
                    label: Some("all".to_string()),
                    kind: Some("deletion_journal".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({
                    "restored": report.restored,
                    "skipped": report.skipped,
                    "missing_files": report.missing_files,
                    "failed": report.failed,
                })),
            )
            .await;
            axum::Json(json!({
                "operation": "restore_all_deletions",
                "success": 1,
                "restored": report.restored,
                "skipped": report.skipped,
                "missing_files": report.missing_files,
                "failed": report.failed,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "restore_all_deletions",
            e,
        ),
    }
}

async fn purge_deletion(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    let exists = matches!(journal_get(&state, &id).await, Ok(Some(_)));
    if !exists {
        return not_found("purge_deletion", "No journal entry for that id.");
    }
    if let Err(e) = forget_journal_entry(&state, &id).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "purge_deletion", e);
    }
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DELETION_PURGE,
        ActivityTarget {
            id: Some(id.clone()),
            label: None,
            kind: Some("deletion_journal".to_string()),
        },
        Outcome::Success,
        None,
        None,
    )
    .await;
    axum::Json(json!({
        "operation": "purge_deletion",
        "success": 1,
        "purged": 1,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct ClearParams {
    #[serde(default)]
    confirm: Option<String>,
}

fn truthy(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

async fn clear_deletions(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(params): Query<ClearParams>,
) -> Response {
    if !truthy(params.confirm.as_deref()) {
        return error(
            StatusCode::BAD_REQUEST,
            "clear_deletions",
            "Pass confirm=1 to empty the deletion journal.",
        );
    }
    let entries = match journal_list(&state).await {
        Ok(entries) => entries,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "clear_deletions", e),
    };
    let purged = entries.len();
    // A bulk purge is destructive enough to deserve the same pre-operation snapshot as a clean.
    if purged > 0 {
        if let Err(e) = crate::snapshots::take_export(
            &state,
            "pre-journal-purge",
            std::sync::Arc::new(crate::snapshots::NoProgress),
        )
        .await
        {
            tracing::warn!(error = %e, "could not snapshot before clearing the deletion journal");
        }
    }
    if let Err(e) = clear_key(&state).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "clear_deletions", e);
    }
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DELETION_CLEAR,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("deletion_journal".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "purged": purged })),
    )
    .await;
    axum::Json(json!({
        "operation": "clear_deletions",
        "success": 1,
        "purged": purged,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, deleted_at_ms: i64) -> JournalEntry {
        JournalEntry {
            record: Archive {
                id: ArchiveId(id.to_string()),
                name: "n".to_string(),
                title: "t".to_string(),
                file: format!("/library/{id}.zip"),
                tags: "artist:x".to_string(),
                summary: String::new(),
                arcsize: 1,
                pagecount: 1,
                isnew: false,
                lastreadpage: 7,
                lastreadtime: 123,
                thumbhash: None,
                toc: vec![],
                stamp_ids: vec![],
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
                blake3: None,
                primary_source: None,
                progress_pages: None,
            },
            deleted_at_ms,
            source: "clean".to_string(),
            path: format!("/library/{id}.zip"),
            journal_version: JOURNAL_VERSION,
        }
    }

    /// T014: an entry survives a full JSON round trip, progress included.
    #[test]
    fn journal_entry_round_trips() {
        let original = entry("a".repeat(40).as_str(), 1_795_412_345_678);
        let raw = serde_json::to_string(&original).unwrap();
        let parsed: JournalEntry = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.record.id, original.record.id);
        assert_eq!(parsed.record.lastreadpage, 7);
        assert_eq!(parsed.deleted_at_ms, 1_795_412_345_678);
        assert_eq!(parsed.source, "clean");
        assert_eq!(parsed.journal_version, JOURNAL_VERSION);
        assert!(raw.contains("\"journal_version\":1"));
    }

    /// A record written by an older build (no `journal_version`) still reads.
    #[test]
    fn an_entry_without_a_version_still_parses() {
        let mut value = serde_json::to_value(entry("b".repeat(40).as_str(), 5)).unwrap();
        value.as_object_mut().unwrap().remove("journal_version");
        let parsed: JournalEntry = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.journal_version, JOURNAL_VERSION);
    }

    #[test]
    fn the_oldest_entries_are_evicted_first() {
        // `trim` only sorts; this mirrors its ordering rule so the eviction direction is pinned
        // without needing a Redis round trip here (the Redis half is covered below).
        let mut entries: Vec<(String, i64)> = vec![
            ("mid".to_string(), 20),
            ("old".to_string(), 10),
            ("new".to_string(), 30),
        ];
        entries.sort_by_key(|(_, at)| *at);
        let victims: Vec<String> = entries.into_iter().take(2).map(|(id, _)| id).collect();
        assert_eq!(victims, vec!["old".to_string(), "mid".to_string()]);
    }

    #[tokio::test]
    async fn journal_round_trip_eviction_and_restore_conflict() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let id = format!("{}", uuid::Uuid::new_v4().simple());
        let id = format!("{:0<40}", &id[..40.min(id.len())]);
        let record = entry(&id, now_ms()).record;
        assert!(journal_put(&state, &record, "single").await.unwrap());
        let listed = journal_list(&state).await.unwrap();
        assert!(listed.iter().any(|e| e.record.id.as_str() == id.as_str()));

        // Restoring when the record is already back is a conflict, and leaves the entry alone.
        state
            .repos
            .archives
            .create_or_replace(&record)
            .await
            .unwrap();
        assert!(matches!(
            restore_one(&state, id.as_str()).await,
            RestoreOne::AlreadyExists
        ));
        assert!(journal_get(&state, id.as_str()).await.unwrap().is_some());

        // Delete the record and the journal entry restores it for real.
        state.repos.archives.delete(&record.id).await.unwrap();
        assert!(matches!(
            restore_one(&state, id.as_str()).await,
            RestoreOne::Restored { .. }
        ));
        assert!(journal_get(&state, id.as_str()).await.unwrap().is_none());
        assert_eq!(
            state
                .repos
                .archives
                .get(&record.id)
                .await
                .unwrap()
                .unwrap()
                .lastreadpage,
            7
        );
        state.repos.archives.delete(&record.id).await.unwrap();
    }

    #[tokio::test]
    async fn a_corrupt_entry_is_skipped_and_the_rest_still_list() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let mut conn = state.redis.archive.get().await.unwrap();
        let good = entry(&"c".repeat(40), 1);
        let good_id = good.record.id.clone();
        let _: () = conn
            .hset(
                DELETION_JOURNAL_KEY,
                good_id.as_str(),
                serde_json::to_string(&good).unwrap(),
            )
            .await
            .unwrap();
        let _: () = conn
            .hset(DELETION_JOURNAL_KEY, "corrupt", "{not json")
            .await
            .unwrap();
        let listed = journal_list(&state).await.unwrap();
        assert!(listed.iter().any(|e| e.record.id == good_id));
        let _: () = conn
            .hdel(DELETION_JOURNAL_KEY, good_id.as_str())
            .await
            .unwrap();
        let _: () = conn.hdel(DELETION_JOURNAL_KEY, "corrupt").await.unwrap();
    }

    /// The eviction rule as `trim` implements it, with a small limit and a throwaway key so a real
    /// journal is never the one being pruned by a test run.
    #[tokio::test]
    async fn trim_evicts_the_oldest_past_the_limit() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let key = format!("LANRURUGI_TEST_JOURNAL_TRIM_{}", uuid::Uuid::new_v4());
        let mut conn = state.redis.archive.get().await.unwrap();
        let ids: Vec<String> = (0..5)
            .map(|i| {
                let suffix = i.to_string();
                format!("trim{}{}", "e".repeat(34 - suffix.len()), suffix)
            })
            .collect();
        for (i, id) in ids.iter().enumerate() {
            let _: () = conn
                .hset(
                    key.as_str(),
                    id.as_str(),
                    serde_json::to_string(&entry(id, i as i64)).unwrap(),
                )
                .await
                .unwrap();
        }
        trim_key(&mut conn, &key, 2).await.unwrap();
        let remaining: HashMap<String, String> = conn.hgetall(&key).await.unwrap();
        assert_eq!(
            remaining.len(),
            2,
            "only the newest two of the five survive"
        );
        assert!(remaining.contains_key(&ids[4]));
        assert!(remaining.contains_key(&ids[3]));
        let _: () = conn.del(&key).await.unwrap();
    }

    /// T011: the listing handler returns the documented envelope, and clearing requires the
    /// explicit confirmation parameter.
    #[tokio::test]
    async fn list_and_clear_handlers_follow_the_contract() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let listed = list_deletions(State(state.clone())).await;
        assert_eq!(listed.status(), StatusCode::OK);
        let body = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["operation"], "list_deletions");
        assert_eq!(body["success"], 1);
        assert!(body["retention_days"].is_number());
        assert!(body["entries"].is_array());

        let refused =
            clear_deletions(State(state.clone()), None, Query(ClearParams::default())).await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn confirm_and_force_accept_the_documented_spellings() {
        assert!(truthy(Some("1")));
        assert!(truthy(Some("true")));
        assert!(!truthy(Some("0")));
        assert!(!truthy(None));
    }
}
