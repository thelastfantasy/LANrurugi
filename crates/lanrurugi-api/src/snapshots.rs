//! Automatic backup snapshots (`specs/028-deletion-safety-net`).
//!
//! A snapshot is the same JSON document `GET /database/backup` produces, written atomically into
//! `<library>/.lanrurugi-backups/` — inside the library, like the recycle bin, so containers need
//! no extra volume and the backups travel with the content. They are taken at startup (once a
//! day, plus once a week for the weekly tier), before every destructive operation, and on demand.
//!
//! Nothing here may ever block or fail an operation it is protecting: a snapshot failure is a
//! warning plus an `ok:false` entry in the operation's own result (`research.md` D9).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Datelike, Local, SecondsFormat, TimeZone, Utc};
use serde::Serialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::settings::{read_snapshot_daily_keep, read_snapshot_week_keep};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::paths::snapshot_dir;

/// How many `pre-*` snapshots survive retention, independent of the daily/weekly tiers.
pub const PRE_KEEP: usize = 5;
/// User-requested snapshots ("立即生成快照") have no time window either, so they get their own
/// count cap. Without it they would accumulate forever: the sweep only knows the reasons it is
/// told about, and everything else used to fall through to "keep".
pub const MANUAL_KEEP: usize = 5;
const NAME_LEN: usize = 16; // yyyymmddThhmmssZ
const USABLE_PROBE_BYTES: u64 = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotEntry {
    pub name: String,
    /// RFC 3339, UTC — derived from the filename, so a file moved into the directory by hand is
    /// either well-formed or not listed at all.
    pub created_at: String,
    pub reason: String,
    pub size_bytes: u64,
    /// Cheap syntactic check (readable, braced, carries an `archives` array). `false` disables the
    /// UI's restore button; the real schema check happens when a restore actually parses it.
    pub usable: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SnapshotOutcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SnapshotOutcome {
    pub fn success(name: String) -> Self {
        Self {
            ok: true,
            name: Some(name),
            error: None,
        }
    }

    pub fn failure(error: String) -> Self {
        Self {
            ok: false,
            name: None,
            error: Some(error),
        }
    }
}

/// One process-wide marker: the local date whose automatic snapshot was already attempted. A
/// failed attempt must not be retried every hour for the rest of the day (`research.md` D9).
static ATTEMPTED_DAY: AtomicI64 = AtomicI64::new(0);

fn day_ordinal() -> i64 {
    Local::now().date_naive().num_days_from_ce() as i64
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// `<ts>-<reason>.json`, reason restricted to the reasons this module itself writes.
pub fn is_known_reason(reason: &str) -> bool {
    if matches!(reason, "daily" | "weekly" | "manual") {
        return true;
    }
    reason.strip_prefix("pre-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

fn format_name(at: DateTime<Utc>, reason: &str) -> String {
    format!("{}-{reason}.json", at.format("%Y%m%dT%H%M%SZ"))
}

/// Parses a filename this module wrote. Anything else — a traversal attempt, a hand-made name —
/// yields `None` and is never joined into a path.
pub fn parse_name(name: &str) -> Option<(DateTime<Utc>, String)> {
    if name.len() <= NAME_LEN + 6 || name.contains(['/', '\\']) {
        return None;
    }
    let stamp = name.get(..NAME_LEN)?;
    let rest = name.get(NAME_LEN..)?;
    let reason = rest.strip_prefix('-')?.strip_suffix(".json")?;
    if !is_known_reason(reason) {
        return None;
    }
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ").ok()?;
    Some((Utc.from_utc_datetime(&naive), reason.to_string()))
}

/// Listing: every well-formed `.json` in the directory, newest first. Files that do not parse are
/// ignored rather than reported — the directory is meant to be read-only to humans.
pub fn list_in(dir: &Path) -> std::io::Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(e) => return Err(e),
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((created, reason)) = parse_name(name) else {
            continue;
        };
        let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push(SnapshotEntry {
            name: name.to_string(),
            created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
            reason,
            size_bytes,
            usable: looks_like_backup(&path),
        });
    }
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(entries)
}

/// Is this a backup document, judged cheaply? A crash-truncated write stops at an arbitrary byte,
/// so it almost never ends on `}`; a foreign JSON file has no `archives` near the top (the field
/// order is the struct's own). Deliberately heuristic: the restore path parses in full and answers
/// 400 for whatever this lets through, and reading every snapshot fully just to draw a list would
/// cost as much as a backup itself.
fn looks_like_backup(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return false;
    };
    if len < 8 {
        return false;
    }
    let mut head = vec![0u8; USABLE_PROBE_BYTES.min(len) as usize];
    if file.read_exact(&mut head).is_err() {
        return false;
    }
    if file.seek(SeekFrom::Start(len - 1)).is_err() {
        return false;
    }
    let mut last = [0u8; 1];
    if file.read_exact(&mut last).is_err() {
        return false;
    }
    let first = head.iter().find(|b| !b.is_ascii_whitespace());
    let tail = if last[0].is_ascii_whitespace() {
        // A file ending in whitespace is unusual; fall back to the head's last non-space byte,
        // which is only wrong for a >4 KiB document with trailing spaces (never written here).
        head.iter().rev().find(|b| !b.is_ascii_whitespace())
    } else {
        Some(&last[0])
    };
    matches!(first, Some(&b'{')) && matches!(tail, Some(&b'}')) && contains(&head, b"\"archives\"")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Which files retention would remove. Pure so the rule ("7 daily + 4 weekly + 5 `pre-*`, never
/// the newest usable snapshot") can be tested without a filesystem.
pub fn retention_plan(
    entries: &[SnapshotEntry],
    daily_keep: usize,
    week_keep: usize,
) -> Vec<String> {
    let mut sorted: Vec<&SnapshotEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let protected = sorted
        .iter()
        .find(|e| e.usable)
        .or_else(|| sorted.first())
        .map(|e| e.name.clone());

    let mut victims: Vec<String> = Vec::new();
    let mut day_seen: Vec<String> = Vec::new();
    let mut week_seen: Vec<(i32, u32)> = Vec::new();
    let mut pre_seen = 0usize;
    let mut manual_seen = 0usize;
    for entry in &sorted {
        let Some((created, _)) = parse_name(&entry.name) else {
            continue;
        };
        match entry.reason.as_str() {
            "daily" => {
                let day = created.with_timezone(&Local).date_naive().to_string();
                if day_seen.contains(&day) {
                    // A second snapshot for the same local day: the newest one is already kept.
                    victims.push(entry.name.clone());
                } else if day_seen.len() < daily_keep {
                    day_seen.push(day);
                } else {
                    victims.push(entry.name.clone());
                }
            }
            "weekly" => {
                let iso = created.iso_week();
                let key = (iso.year(), iso.week());
                if week_seen.contains(&key) {
                    victims.push(entry.name.clone());
                } else if week_seen.len() < week_keep {
                    week_seen.push(key);
                } else {
                    victims.push(entry.name.clone());
                }
            }
            reason if reason.starts_with("pre-") => {
                if pre_seen < PRE_KEEP {
                    pre_seen += 1;
                } else {
                    victims.push(entry.name.clone());
                }
            }
            "manual" => {
                if manual_seen < MANUAL_KEEP {
                    manual_seen += 1;
                } else {
                    victims.push(entry.name.clone());
                }
            }
            // An unknown reason is left alone on purpose: a newer version may have written it, and
            // deleting files this version does not understand is worse than keeping them.
            _ => {}
        }
    }
    if let Some(protected) = protected {
        victims.retain(|name| *name != protected);
    }
    victims
}

/// Writes the current library as a snapshot. Serializes and renames off the async runtime, so a
/// large library does not stall request handling for the duration of the write.
pub async fn take_snapshot(state: &AppState, reason: &str) -> Result<SnapshotEntry, String> {
    if !is_known_reason(reason) {
        return Err(format!("unknown snapshot reason: {reason}"));
    }
    let doc = crate::database::build_backup_document_result(state).await?;
    let dir = snapshot_dir(&state.library.archive_dir);
    let created = now_utc();
    let name = format_name(created, reason);
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!("{name}.tmp"));

    let size = lanrurugi_storage::fsutil::in_dir(&final_path, || {
        let doc = doc.clone();
        let tmp_path = tmp_path.clone();
        let final_path = final_path.clone();
        async move { write_atomic(&tmp_path, &final_path, doc).await }
    })
    .await
    .map_err(|e| format!("could not write snapshot {name}: {e}"))?;

    Ok(SnapshotEntry {
        name,
        created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
        reason: reason.to_string(),
        size_bytes: size,
        usable: true,
    })
}

/// Serializes `doc` to `tmp`, flushes it to disk, then renames onto `final_path`. Atomic from a
/// reader's point of view: `.json` either does not exist or is a complete document, and a crash
/// leaves at most a `.json.tmp` that `list_in` ignores.
async fn write_atomic(
    tmp: &Path,
    final_path: &Path,
    doc: lanrurugi_backup::build::BackupDocument,
) -> std::io::Result<u64> {
    let tmp = tmp.to_path_buf();
    let final_path = final_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &doc).map_err(std::io::Error::other)?;
        std::io::Write::flush(&mut writer)?;
        drop(writer);
        let file = std::fs::File::open(&tmp)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(std::fs::metadata(&final_path)?.len())
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Takes a snapshot for an operation that is about to destroy something. Never returns an error:
/// the operation proceeds, and its result carries `{ok:false, error}` instead.
pub async fn take_for_operation(state: &AppState, reason: &str) -> SnapshotOutcome {
    match take_snapshot(state, reason).await {
        Ok(entry) => SnapshotOutcome::success(entry.name),
        Err(e) => {
            tracing::warn!(reason, error = %e, "could not take a pre-operation backup snapshot");
            SnapshotOutcome::failure(e)
        }
    }
}

/// Once per local day (and once per ISO week for the weekly tier): make sure today has a snapshot.
/// A failure is not retried until tomorrow.
pub async fn daily_on_startup(state: &AppState) -> SnapshotOutcome {
    let today = day_ordinal();
    if ATTEMPTED_DAY.swap(today, Ordering::SeqCst) == today {
        return SnapshotOutcome::default();
    }
    let dir = snapshot_dir(&state.library.archive_dir);
    let entries = list_in(&dir).unwrap_or_default();
    let now = now_utc();
    let today_local = Local::now().date_naive();
    let has_today = entries.iter().any(|entry| {
        entry.reason == "daily"
            && entry.usable
            && parse_name(&entry.name)
                .map(|(at, _)| at.with_timezone(&Local).date_naive() == today_local)
                .unwrap_or(false)
    });
    let iso = now.iso_week();
    let week_key = (iso.year(), iso.week());
    let has_week = entries.iter().any(|entry| {
        entry.reason == "weekly"
            && entry.usable
            && parse_name(&entry.name)
                .map(|(at, _)| {
                    let iso = at.iso_week();
                    (iso.year(), iso.week()) == week_key
                })
                .unwrap_or(false)
    });

    let mut last = SnapshotOutcome::default();
    if !has_today {
        last = take_for_operation(state, "daily").await;
        if !last.ok {
            return last;
        }
    }
    if !has_week {
        last = take_for_operation(state, "weekly").await;
    }
    // A freshly written daily/weekly may push an older one out immediately.
    retention_sweep(state).await;
    last
}

/// Applies [`retention_plan`]. Returns how many files were removed.
pub async fn retention_sweep(state: &AppState) -> usize {
    let dir = snapshot_dir(&state.library.archive_dir);
    let entries = match list_in(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "could not list automatic backups for retention");
            return 0;
        }
    };
    let daily_keep = read_snapshot_daily_keep(state).await.max(0) as usize;
    let week_keep = read_snapshot_week_keep(state).await.max(0) as usize;
    let victims = retention_plan(&entries, daily_keep, week_keep);
    let mut removed = 0usize;
    for name in victims {
        // `retention_plan` only ever returns parsed names, but re-checking here keeps the removal
        // path itself incapable of joining anything that is not a plain file name.
        if parse_name(&name).is_none() {
            continue;
        }
        match tokio::fs::remove_file(dir.join(&name)).await {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(name, error = %e, "could not remove an expired snapshot"),
        }
    }
    if removed > 0 {
        tracing::info!(
            removed,
            "automatic backup retention removed expired snapshots"
        );
    }
    removed
}

pub fn router() -> Router<AppState> {
    // Session-only; `route_policy.csv` denies both token roles (see its own specs/028 block).
    Router::new()
        .route("/backups", get(list_backups).post(create_backup))
        .route(
            "/backups/{name}",
            get(download_backup).delete(delete_backup),
        )
        .route("/backups/{name}/restore", post(restore_backup))
}

fn entry_json(entry: &SnapshotEntry) -> serde_json::Value {
    json!({
        "name": entry.name,
        "created_at": entry.created_at,
        "reason": entry.reason,
        "size_bytes": entry.size_bytes,
        "usable": entry.usable,
    })
}

async fn list_backups(State(state): State<AppState>) -> Response {
    let dir = snapshot_dir(&state.library.archive_dir);
    match list_in(&dir) {
        Ok(entries) => axum::Json(json!({
            "operation": "list_backups",
            "success": 1,
            "daily_keep": read_snapshot_daily_keep(&state).await,
            "week_keep": read_snapshot_week_keep(&state).await,
            "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_backups",
            format!("could not read the backup directory: {e}"),
        ),
    }
}

async fn create_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    match take_snapshot(&state, "manual").await {
        Ok(entry) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BACKUP_CREATE,
                ActivityTarget {
                    id: None,
                    label: Some(entry.name.clone()),
                    kind: Some("backup".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({ "size_bytes": entry.size_bytes })),
            )
            .await;
            axum::Json(json!({
                "operation": "create_backup",
                "success": 1,
                "entry": entry_json(&entry),
            }))
            .into_response()
        }
        Err(e) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BACKUP_CREATE,
                ActivityTarget {
                    id: None,
                    label: None,
                    kind: Some("backup".to_string()),
                },
                Outcome::Failure { reason: e.clone() },
                None,
                None,
            )
            .await;
            error(StatusCode::INTERNAL_SERVER_ERROR, "create_backup", e)
        }
    }
}

/// Resolves a caller-supplied name to a real file, refusing anything that is not exactly a
/// snapshot filename this module wrote. `None` is answered as a 400 by each caller.
fn resolve_name(state: &AppState, name: &str) -> Option<PathBuf> {
    parse_name(name)?;
    Some(snapshot_dir(&state.library.archive_dir).join(name))
}

async fn download_backup(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let Some(path) = resolve_name(&state, &name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "serve_backup",
            "Invalid backup name.",
        );
    };
    if !path.is_file() {
        return not_found("serve_backup", format!("Backup {name} does not exist."));
    }
    let body = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_backup",
                format!("could not read {name}: {e}"),
            )
        }
    };
    (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        body,
    )
        .into_response()
}

async fn restore_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let Some(path) = resolve_name(&state, &name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "serve_backup",
            "Invalid backup name.",
        );
    };
    if !path.is_file() {
        return not_found("restore_backup", format!("Backup {name} does not exist."));
    }
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "restore_backup",
                format!("could not read {name}: {e}"),
            )
        }
    };
    let doc: lanrurugi_backup::build::BackupDocument = match serde_json::from_slice(&bytes) {
        Ok(doc) => doc,
        Err(e) => {
            return error(
                StatusCode::BAD_REQUEST,
                "restore_backup",
                format!("Backup {name} is not a valid backup document: {e}"),
            )
        }
    };
    let safety = take_for_operation(&state, "pre-restore").await;
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::BACKUP_RESTORE,
        ActivityTarget {
            id: None,
            label: Some(name.clone()),
            kind: Some("backup".to_string()),
        },
        Outcome::Success,
        None,
        None,
    )
    .await;
    let safety_name = safety.name.clone();
    let job =
        crate::database::start_restore_job(&state, doc, auth.as_ref().map(|e| e.0.clone()), safety)
            .await;
    axum::Json(json!({
        "operation": "restore_backup",
        "success": 1,
        "restored_from": name,
        "safety": safety_name,
        "job": job,
    }))
    .into_response()
}

async fn delete_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    let Some(path) = resolve_name(&state, &name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "serve_backup",
            "Invalid backup name.",
        );
    };
    let dir = snapshot_dir(&state.library.archive_dir);
    let entries = list_in(&dir).unwrap_or_default();
    if !entries.iter().any(|entry| entry.name == name) {
        return not_found("delete_backup", format!("Backup {name} does not exist."));
    }
    let newest_usable = entries
        .iter()
        .find(|entry| entry.usable)
        .map(|e| e.name.clone());
    if newest_usable.as_deref() == Some(name.as_str()) {
        return (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "operation": "delete_backup",
                "success": 0,
                "error": "The newest usable backup cannot be deleted.",
                "reason": "last_usable",
            })),
        )
            .into_response();
    }
    if let Err(e) = tokio::fs::remove_file(&path).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_backup",
            e.to_string(),
        );
    }
    record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::BACKUP_DELETE,
        ActivityTarget {
            id: None,
            label: Some(name.clone()),
            kind: Some("backup".to_string()),
        },
        Outcome::Success,
        None,
        None,
    )
    .await;
    axum::Json(json!({
        "operation": "delete_backup",
        "success": 1,
        "deleted": name,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, usable: bool) -> SnapshotEntry {
        let (created, reason) = parse_name(name).expect("test name is well formed");
        SnapshotEntry {
            name: name.to_string(),
            created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
            reason,
            size_bytes: 10,
            usable,
        }
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn names_round_trip_and_reject_traversal() {
        let (at, reason) = parse_name("20261123T050000Z-daily.json").unwrap();
        assert_eq!(reason, "daily");
        assert_eq!(
            at.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-11-23T05:00:00Z"
        );
        assert!(parse_name("20261123T050000Z-pre-journal-purge.json").is_some());
        assert!(parse_name("../etc/passwd").is_none());
        assert!(parse_name("20261123T050000Z-unknown.json").is_none());
        assert!(parse_name("20261123T050000Z-daily.json.tmp").is_none());
        assert!(parse_name("nope.json").is_none());
    }

    #[test]
    fn retention_keeps_seven_daily_four_weekly_and_five_pre() {
        let mut entries = Vec::new();
        for day in 1..=10 {
            entries.push(entry(&format!("202611{:02}T050000Z-daily.json", day), true));
        }
        for (month, day) in [(11, 2), (11, 9), (11, 16), (11, 23), (11, 30), (12, 7)] {
            entries.push(entry(
                &format!("2026{month:02}{day:02}T050000Z-weekly.json"),
                true,
            ));
        }
        for i in 0..8 {
            entries.push(entry(&format!("2026111{}T060000Z-pre-clean.json", i), true));
        }
        let victims = retention_plan(&entries, 7, 4);
        let removed_daily = victims.iter().filter(|n| n.contains("-daily")).count();
        let removed_weekly = victims.iter().filter(|n| n.contains("-weekly")).count();
        let removed_pre = victims.iter().filter(|n| n.contains("-pre-")).count();
        assert_eq!(removed_daily, 3, "10 daily -> 7 kept");
        assert_eq!(removed_weekly, 2, "6 weekly -> 4 kept");
        assert_eq!(removed_pre, 3, "8 pre -> 5 kept");
        assert!(!victims.contains(&"20261110T050000Z-daily.json".to_string()));
    }

    #[test]
    fn retention_caps_manual_snapshots_too() {
        // "立即生成快照" has no time window, so only a count cap bounds it; without one these
        // accumulate forever and the total snapshot budget is unbounded.
        let entries: Vec<SnapshotEntry> = (1..=8)
            .map(|d| SnapshotEntry {
                name: format!("202611{d:02}T050000Z-manual.json"),
                created_at: format!("2026-11-{d:02}T05:00:00Z"),
                reason: "manual".to_string(),
                size_bytes: 1,
                usable: true,
            })
            .collect();
        let victims = retention_plan(&entries, 7, 4);
        let removed_manual = victims.iter().filter(|n| n.contains("manual")).count();
        assert_eq!(removed_manual, 3, "8 manual -> 5 kept");
        assert!(!victims.contains(&"20261108T050000Z-manual.json".to_string()));
    }

    #[test]
    fn retention_never_deletes_the_newest_usable_snapshot() {
        // A single pre-* snapshot that is also the newest usable one, with more pre-* than the
        // cap: the newest must survive even though its own tier would drop it eventually.
        let mut entries: Vec<SnapshotEntry> = (0..8)
            .map(|i| entry(&format!("2026110{}T050000Z-pre-clean.json", i + 1), true))
            .collect();
        entries[7].usable = false; // newest is corrupt, so the protected one is the one before it
        let victims = retention_plan(&entries, 7, 4);
        assert!(!victims.contains(&"20261107T050000Z-pre-clean.json".to_string()));
        assert!(victims.contains(&"20261101T050000Z-pre-clean.json".to_string()));
    }

    #[test]
    fn a_second_daily_snapshot_for_the_same_local_day_is_dropped() {
        // 00:30 and 01:30 UTC are the same local calendar day under every possible UTC offset,
        // so this assertion does not depend on the test machine's timezone.
        let entries = vec![
            entry("20261123T003000Z-daily.json", true),
            entry("20261123T013000Z-daily.json", true),
            entry("20261124T120000Z-daily.json", true),
        ];
        let victims = retention_plan(&entries, 7, 4);
        assert_eq!(victims, vec!["20261123T003000Z-daily.json".to_string()]);
    }

    #[test]
    fn listing_marks_truncated_files_unusable() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "20261123T050000Z-daily.json",
            "{\"archives\":[],\"categories\":[]}",
        );
        // A write cut off mid-token, which is what a crash or a full disk actually leaves behind.
        write(
            tmp.path(),
            "20261124T050000Z-daily.json",
            "{\"archives\":[{\"arcid\":\"a\",\"ti",
        );
        write(tmp.path(), "20261125T050000Z-daily.json", "{\"nope\":1}");
        let entries = list_in(tmp.path()).unwrap();
        assert_eq!(entries.len(), 3);
        let by_name = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
        assert!(by_name("20261123T050000Z-daily.json").usable);
        assert!(!by_name("20261124T050000Z-daily.json").usable);
        assert!(!by_name("20261125T050000Z-daily.json").usable);
        // A `.tmp` left by a crash is not listed at all.
        write(
            tmp.path(),
            "20261126T050000Z-daily.json.tmp",
            "{\"archives\":[]}",
        );
        assert_eq!(list_in(tmp.path()).unwrap().len(), 3);
    }

    /// T021: the write leaves exactly one complete `.json` and no `.tmp` behind, and the result
    /// parses back as the same document.
    #[tokio::test]
    async fn an_atomic_write_leaves_no_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let name = "20261123T050000Z-daily.json";
        let final_path = tmp.path().join(name);
        let tmp_path = tmp.path().join(format!("{name}.tmp"));
        let doc = lanrurugi_backup::build::BackupDocument {
            archives: vec![],
            categories: vec![],
            tankoubons: vec![],
            stamps: vec![],
            bookmarks: vec![],
            terminology_glossaries: vec![],
            volume_font_patterns: vec![],
            text_regions: vec![],
            share_node_secret: None,
        };
        let size = write_atomic(&tmp_path, &final_path, doc).await.unwrap();
        assert!(size > 0);
        assert!(final_path.is_file());
        assert!(
            !tmp_path.exists(),
            "the temporary file must be renamed over"
        );
        let parsed: lanrurugi_backup::build::BackupDocument =
            serde_json::from_slice(&std::fs::read(&final_path).unwrap()).unwrap();
        assert!(parsed.archives.is_empty());
        let entries = list_in(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].usable);
    }

    /// T019: the HTTP handlers actually create a snapshot on disk, list it back, serve the exact
    /// bytes, and refuse to delete the newest usable one. (The router itself is wired in
    /// `lib.rs::router`; this exercises the handlers with a real `AppState` on a throwaway
    /// library directory.)
    #[tokio::test]
    async fn backup_handlers_create_list_serve_and_protect_the_last_usable() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();

        let created = create_backup(State(state.clone()), None).await;
        assert_eq!(created.status(), StatusCode::OK);
        let body = axum::body::to_bytes(created.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let name = body["entry"]["name"].as_str().unwrap().to_string();
        assert!(body["entry"]["usable"].as_bool().unwrap());
        assert!(name.contains("-manual.json"));

        let listed = list_backups(State(state.clone())).await;
        let body = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == name.as_str()));

        // The download returns the bytes that were written.
        let downloaded = download_backup(State(state.clone()), UrlPath(name.clone())).await;
        assert_eq!(downloaded.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(downloaded.into_body(), usize::MAX)
            .await
            .unwrap();
        let on_disk = std::fs::read(snapshot_dir(tmp.path()).join(&name)).unwrap();
        assert_eq!(bytes.as_ref(), on_disk.as_slice());

        // The only snapshot is the newest usable one: it may not be deleted.
        let deleted = delete_backup(State(state.clone()), None, UrlPath(name.clone())).await;
        assert_eq!(deleted.status(), StatusCode::CONFLICT);

        // A path-ish name is rejected before it is ever joined into a path.
        let bad = download_backup(
            State(state.clone()),
            UrlPath("../../etc/passwd".to_string()),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_in(&tmp.path().join("nope")).unwrap().is_empty());
    }
}
