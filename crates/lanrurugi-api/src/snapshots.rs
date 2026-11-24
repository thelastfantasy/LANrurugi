//! Library backups (`specs/028-deletion-safety-net`).
//!
//! Two different things live in `<library>/.lanrurugi-backups/`, distinguished by
//! [`SnapshotKind`] and by file extension:
//!
//! - **export** (`<stamp>-<reason>.json`) — the same JSON document `GET /database/backup`
//!   produces: archives, tags, categories, tankoubons, stamps, bookmarks. It is selective data,
//!   restorable from inside the app (`POST /api/backups/{name}/restore`), and it is what the
//!   schedule and every `pre-*` safety snapshot write.
//! - **backup** (`<stamp>-backup.rdb`) — a byte copy of Redis's own RDB, i.e. *everything* in every
//!   logical database. It is a full backup, written only on demand
//!   (`POST /api/backups/full`), and it is **not** restorable in-app: restoring means stopping
//!   Redis, replacing the RDB file and starting it again (see `docs/dev/deletion-safety-net.md`).
//!
//! Both live inside the library, like the recycle bin, so containers need no extra volume and they
//! travel with the content. Nothing here may ever block or fail an operation it is protecting: a
//! snapshot failure is a warning plus an `ok:false` entry in the operation's own result
//! (`research.md` D9).

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use std::io::Write;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Datelike, Local, SecondsFormat, TimeZone, Utc};
use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::settings::{
    read_snapshot_cron, read_snapshot_cron_preset, read_snapshot_daily_keep,
    read_snapshot_schedule_enabled, read_snapshot_week_keep,
};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::{SNAPSHOT_PINS_KEY, SNAPSHOT_SCHEDULE_KEY};
use lanrurugi_storage::paths::snapshot_dir;

pub mod cron;
mod jobs;
pub mod progress;

use self::cron::CronSchedule;
pub(crate) use self::jobs::{record_percent, start_progress_job, BackupJob};
use self::progress::write_document;
pub use self::progress::{BackupProgress, NoProgress};

/// How many `pre-*` snapshots survive retention, independent of the daily/weekly tiers.
pub const PRE_KEEP: usize = 5;
/// User-requested snapshots ("立即生成快照") have no time window either, so they get their own
/// count cap. Without it they would accumulate forever: the sweep only knows the reasons it is
/// told about, and everything else used to fall through to "keep".
pub const MANUAL_KEEP: usize = 5;
/// Full Redis backups are big and are taken deliberately, so only the newest few are kept. Not a
/// setting: a knob nobody asked for, and one that could silently leave a library with no full
/// backup at all. Pinning is the escape hatch for "keep this one forever".
pub const FULL_BACKUP_KEEP: usize = 3;
const NAME_LEN: usize = 16; // yyyymmddThhmmssZ
const USABLE_PROBE_BYTES: u64 = 4096;
/// Every Redis RDB starts with this magic; checking it makes a truncated or foreign `.rdb` show up
/// as `usable: false` instead of as a restorable-looking backup.
const RDB_MAGIC: [u8; 5] = *b"REDIS";

/// Which of the two things in the backup directory an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotKind {
    /// `<stamp>-backup.rdb`: Redis's own RDB, i.e. every logical database. Ops-only restore.
    Backup,
    /// `<stamp>-<reason>.json`: the selective JSON document, restorable in-app.
    Export,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotEntry {
    pub name: String,
    /// Which of the two this is — a client must never offer the export restore path for a full
    /// Redis backup, and only this field distinguishes them besides the extension.
    pub kind: SnapshotKind,
    /// RFC 3339, UTC — derived from the filename, so a file moved into the directory by hand is
    /// either well-formed or not listed at all.
    pub created_at: String,
    /// `daily` / `weekly` / `manual` / `pre-*` for an export, or `backup` for a full Redis backup.
    pub reason: String,
    pub size_bytes: u64,
    /// Cheap syntactic check (readable, braced, carries an `archives` array for an export; the RDB
    /// magic for a full backup). `false` marks a crash-truncated file; the real schema check
    /// happens when an export is actually restored.
    pub usable: bool,
    /// Contents of the sibling `<name>.summary.json`, or `null` when it is missing/unreadable
    /// (a snapshot taken by a build that predates summaries, or a moved-in file). Never derived by
    /// opening the snapshot itself — that would cost as much as a restore just to draw a list.
    /// Always `null` for a full Redis backup, which has no JSON summary.
    pub summary: Option<SnapshotSummary>,
    /// Pinned by the user: retention never evicts it. Filled in from Redis by the list/sweep
    /// callers — the filesystem on its own knows nothing about pins.
    pub pinned: bool,
}

/// What a snapshot holds, written next to it as `<name>.summary.json` so the backups list can show
/// it without parsing a multi-megabyte document. `bytes` is the snapshot file's own size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub archives: u64,
    pub archives_with_tags: u64,
    pub categories: u64,
    pub tankoubons: u64,
    pub stamps: u64,
    pub bookmarks: u64,
    pub bytes: u64,
}

/// Counts the document's own vectors. Pure (no filesystem, no Redis) so the counting rule —
/// "with tags" means at least one non-blank tag — is unit-testable; `bytes` is filled in by
/// [`take_export`] once the file is actually written.
pub fn summarize(doc: &lanrurugi_backup::build::BackupDocument) -> SnapshotSummary {
    SnapshotSummary {
        archives: doc.archives.len() as u64,
        archives_with_tags: doc
            .archives
            .iter()
            .filter(|archive| !archive.tags.trim().is_empty())
            .count() as u64,
        categories: doc.categories.len() as u64,
        tankoubons: doc.tankoubons.len() as u64,
        stamps: doc.stamps.len() as u64,
        bookmarks: doc.bookmarks.len() as u64,
        bytes: 0,
    }
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

// ---------------------------------------------------------------------------------------------
// Configurable schedule (a crontab, in the server's local timezone).
//
// One tick every 5 seconds asks whether a matching second has come due since the last one served.
// Recording that instant in Redis makes a restart harmless (no duplicate) and a missed firing owed
// exactly once after downtime (catch-up, however many were missed), and makes a saved settings
// change apply on the next tick.
// ---------------------------------------------------------------------------------------------

/// The schedule as configured right now.
#[derive(Debug, Clone)]
pub struct SnapshotSchedule {
    pub enabled: bool,
    pub cron: CronSchedule,
    /// The expression as stored, so the list response can echo exactly what was configured (even
    /// when it is the fallback, which the server-side warning then explains).
    pub cron_text: String,
    /// The settings UI's preset echo (`hourly` / `daily` / `weekly` / `monthly` / `custom`).
    pub preset: String,
}

impl SnapshotSchedule {
    pub async fn load(state: &AppState) -> Self {
        let cron_text = read_snapshot_cron(state).await;
        // Only a hand-edited Redis can hold something unparseable — `PUT /settings` validates it.
        // Falling back to the default beats silently switching the schedule off.
        let cron = cron_text.parse::<CronSchedule>().unwrap_or_else(|e| {
            tracing::warn!(
                cron = %cron_text,
                error = %e,
                "stored snapshot cron does not parse; falling back to the default"
            );
            CronSchedule::default()
        });
        Self {
            enabled: read_snapshot_schedule_enabled(state).await,
            cron,
            cron_text,
            preset: read_snapshot_cron_preset(state).await,
        }
    }

    /// The next firings, or none at all while the schedule is off — a disabled schedule has no next
    /// occurrence, and reporting one would suggest a snapshot is coming.
    pub fn next_runs(&self, now: DateTime<Local>, count: usize) -> Vec<DateTime<Local>> {
        if !self.enabled {
            return Vec::new();
        }
        self.cron.next_runs(now, count)
    }
}

/// The Redis field remembering the last instant served.
const LAST_FIRE_FIELD: &str = "last_cron_fire";

/// How many upcoming firings `GET /api/backups` and the preview endpoint report.
pub const NEXT_RUNS: usize = 10;

/// The instant whose automatic attempt already failed in this process, so a full disk does not
/// retry — and keep warning — every 5 seconds until the next firing (`research.md` D9). The next
/// firing has a different timestamp, which is what re-arms it.
static FAILED_CRON_SLOT: AtomicI64 = AtomicI64::new(0);

async fn read_last_fire(state: &AppState) -> Option<DateTime<Utc>> {
    let raw: Option<String> = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget(SNAPSHOT_SCHEDULE_KEY, LAST_FIRE_FIELD)
            .await
            .ok()?,
        Err(_) => return None,
    };
    DateTime::parse_from_rfc3339(raw.as_deref()?)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

pub async fn read_schedule_state(state: &AppState) -> Option<DateTime<Utc>> {
    read_last_fire(state).await
}

/// Returns `false` when Redis refused the write, so the caller can stop re-running the same firing.
async fn record_last_fire(state: &AppState, at: DateTime<Utc>) -> bool {
    let Ok(mut conn) = state.redis.config.get().await else {
        return false;
    };
    let stored = at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let result: Result<i64, _> = conn
        .hset(SNAPSHOT_SCHEDULE_KEY, LAST_FIRE_FIELD, stored)
        .await;
    match result {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "could not record the automatic snapshot firing");
            false
        }
    }
}

/// Which firing, if any, is owed at `now`. Pure, so the restart/catch-up rules are unit-testable:
///
/// - **restart never repeats**: a candidate at or before the recorded `last_fire` is not due;
/// - **downtime catches up exactly once**: the first firing after the record is owed — however
///   many were missed, the next tick writes one snapshot and records the serve time;
/// - **no record means "one is owed now"**: a fresh install (or a first upgrade to the cron
///   schedule) takes one snapshot immediately, exactly like the pre-cron build did at startup.
///
/// That last rule is also what makes sub-minute expressions work at all: requiring the *current
/// second* to be a hit only works when the tick period divides the cron's period, and a 5-second
/// tick against `*/10` can step over every hit forever. Firing once on the first tick side-steps
/// the alignment problem and leaves a record for the real cadence to work from.
pub fn due_firing(
    cron: &CronSchedule,
    now: DateTime<Local>,
    last_fire: Option<DateTime<Utc>>,
) -> Option<DateTime<Local>> {
    match last_fire {
        Some(last) => cron
            .next_after(last.with_timezone(&Local))
            .filter(|at| *at <= now),
        None => Some(now),
    }
}

/// The decision + watermark half of a tick, with the snapshot write injected.
///
/// Split out so the scheduling policy (cron hit, restart-no-duplicate, catch-up-once, failed-firing
/// memory) can be tested without a real Redis RDB — the one environment that can read Redis's RDB
/// is not the one the test suite runs in.
pub(crate) async fn tick_with<F>(
    state: &AppState,
    now: DateTime<Local>,
    run: F,
) -> Option<SnapshotEntry>
where
    F: FnOnce(
        DateTime<Local>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SnapshotEntry, String>> + Send>,
    >,
{
    let schedule = SnapshotSchedule::load(state).await;
    if !schedule.enabled {
        return None;
    }
    let last_fire = read_last_fire(state).await;
    let Some(slot) = due_firing(&schedule.cron, now, last_fire) else {
        tracing::debug!(
            cron = %schedule.cron_text,
            last_fire = ?last_fire,
            "snapshot schedule tick: nothing due"
        );
        return None;
    };
    let instant = slot.timestamp();
    // A firing whose automatic attempt already failed in this process is not retried every tick;
    // the next firing has a different timestamp, which re-arms it.
    if FAILED_CRON_SLOT.load(Ordering::SeqCst) == instant {
        return None;
    }
    tracing::info!(
        cron = %schedule.cron_text,
        slot = %slot.to_rfc3339(),
        first = last_fire.is_none(),
        "snapshot schedule tick: firing"
    );
    match run(slot).await {
        Ok(entry) => {
            // The *serve* time, not the slot: recording the slot would leave every firing missed
            // during a long downtime still ahead of the record, and the schedule would fire once
            // per missed firing instead of once in total.
            if record_last_fire(state, Utc::now()).await {
                FAILED_CRON_SLOT.store(0, Ordering::SeqCst);
            } else {
                FAILED_CRON_SLOT.store(instant, Ordering::SeqCst);
            }
            tracing::info!(snapshot = %entry.name, "scheduled automatic snapshot written");
            Some(entry)
        }
        Err(e) => {
            FAILED_CRON_SLOT.store(instant, Ordering::SeqCst);
            tracing::warn!(error = %e, "scheduled automatic snapshot failed; not retrying this firing");
            None
        }
    }
}

/// One scheduler tick. `lanrurugi-server` calls this every 5 seconds, and an interval's first tick
/// is immediate — which is also the first firing a fresh install takes.
pub async fn schedule_tick(state: &AppState) {
    let state_for_task = state.clone();
    tick_with(state, Local::now(), move |_slot| {
        Box::pin(async move {
            let entry =
                take_full_backup(&state_for_task, "scheduled", Arc::new(NoProgress)).await?;
            // A fresh snapshot can immediately push the oldest one past `FULL_BACKUP_KEEP`.
            retention_sweep(&state_for_task).await;
            Ok(entry)
        })
    })
    .await;
}

/// One instant as the API reports it: ISO 8601 with the server's own UTC offset, so a client never
/// has to guess which zone a `next_runs` entry is in (the `timezone` field names it as well).
fn iso_local(at: DateTime<Local>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// The server's zone for display: `Asia/Tokyo (+09:00)` when the IANA name can be read, else just
/// the offset. Never used for arithmetic — that is always `chrono::Local`.
pub fn local_timezone_label() -> String {
    let offset = Local::now().offset().to_string();
    match iana_time_zone::get_timezone() {
        Ok(name) if !name.trim().is_empty() => format!("{} ({offset})", name.trim()),
        _ => offset,
    }
}

/// The `schedule` object on `GET /api/backups`. Pure, so "disabled means no next runs" and the
/// reported shape have a unit test.
fn schedule_json(
    schedule: &SnapshotSchedule,
    last_fire: Option<DateTime<Utc>>,
    now: DateTime<Local>,
) -> serde_json::Value {
    json!({
        "enabled": schedule.enabled,
        "cron": schedule.cron_text,
        "preset": schedule.preset,
        "timezone": local_timezone_label(),
        "last_fire_at": last_fire.map(|at| iso_local(at.with_timezone(&Local))),
        "next_runs": schedule
            .next_runs(now, NEXT_RUNS)
            .into_iter()
            .map(iso_local)
            .collect::<Vec<_>>(),
    })
}

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

// ---------------------------------------------------------------------------------------------
// Pins.
//
// A pin is the user's "keep this one" answer. It lives in Redis rather than in a sidecar file so
// the backups directory stays plain snapshots (a file moved in by hand is still listed), and it is
// cleared whenever the snapshot itself is deleted, so a retired name cannot linger.
// ---------------------------------------------------------------------------------------------

/// The pinned names, or `None` when Redis could not be read. [`retention_sweep`] treats `None` as
/// "cannot tell" and skips that sweep: silently evicting a pinned snapshot is worse than a late
/// cleanup. The read-only list path treats it as "nothing pinned" — it deletes nothing.
async fn read_pins(state: &AppState) -> Option<BTreeSet<String>> {
    let mut conn = state.redis.config.get().await.ok()?;
    let members: Vec<String> = conn.smembers(SNAPSHOT_PINS_KEY).await.ok()?;
    Some(members.into_iter().collect())
}

/// Adds or removes one pin. `SADD`/`SREM` are already idempotent, which is exactly the endpoint
/// contract: pinning a pinned snapshot and unpinning an unpinned one both succeed.
pub async fn set_pin(state: &AppState, name: &str, pinned: bool) -> Result<(), String> {
    let mut conn = state
        .redis
        .config
        .get()
        .await
        .map_err(|e| format!("could not reach Redis: {e}"))?;
    let result: Result<i64, _> = if pinned {
        conn.sadd(SNAPSHOT_PINS_KEY, name).await
    } else {
        conn.srem(SNAPSHOT_PINS_KEY, name).await
    };
    result
        .map(|_| ())
        .map_err(|e| format!("could not update the snapshot pin: {e}"))
}

/// One parsed backup-directory file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub created: DateTime<Utc>,
    pub kind: SnapshotKind,
    /// `daily` / `weekly` / `manual` / `pre-*` for an export; `daily` / `weekly` / `manual` for a
    /// full Redis backup.
    pub reason: String,
}

/// `snapshot-<stamp>-<reason>.rdb` for a full Redis backup, `export-<stamp>-<reason>.json` for a
/// JSON export. The prefix, not the extension, is what the rest of the module keys `kind` off.
fn format_name(at: DateTime<Utc>, kind: SnapshotKind, reason: &str) -> String {
    let stamp = at.format("%Y%m%dT%H%M%SZ");
    match kind {
        SnapshotKind::Backup => format!("snapshot-{stamp}-{reason}.rdb"),
        SnapshotKind::Export => format!("export-{stamp}-{reason}.json"),
    }
}

/// Reasons an *export* may carry: the scheduled/manual ones plus every `pre-*` safety snapshot.
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

/// Reasons a *full Redis backup* may carry: `scheduled` for a cron firing, `manual` for
/// `POST /api/backups`, plus the `daily` / `weekly` an earlier build wrote (kept parseable so those
/// files still list and still count against the three-backup cap). No `pre-*`: a destructive
/// operation gets a cheap JSON export, never a multi-hundred-megabyte RDB write in its critical
/// path.
fn is_known_backup_reason(reason: &str) -> bool {
    matches!(reason, "scheduled" | "manual" | "daily" | "weekly")
}

/// Parses a filename this module wrote. The `snapshot-` / `export-` prefix decides the kind;
/// a prefix-less `<stamp>-<reason>.json` is a legacy export from before the two kinds were split and
/// keeps listing (as `kind=export`) so old backups do not silently disappear from the list.
/// Anything else — a traversal attempt, a hand-made name, a prefix that contradicts its extension —
/// yields `None` and is never joined into a path.
pub fn parse_name(name: &str) -> Option<ParsedName> {
    if name.contains(['/', '\\']) {
        return None;
    }
    let (kind, rest) = if let Some(rest) = name.strip_prefix("snapshot-") {
        (SnapshotKind::Backup, rest)
    } else if let Some(rest) = name.strip_prefix("export-") {
        (SnapshotKind::Export, rest)
    } else {
        (SnapshotKind::Export, name)
    };
    let stamp = rest.get(..NAME_LEN)?;
    let rest = rest.get(NAME_LEN..)?.strip_prefix('-')?;
    let (reason, known) = match kind {
        SnapshotKind::Backup => {
            let reason = rest.strip_suffix(".rdb")?;
            (reason, is_known_backup_reason(reason))
        }
        SnapshotKind::Export => {
            let reason = rest.strip_suffix(".json")?;
            (reason, is_known_reason(reason))
        }
    };
    if !known {
        return None;
    }
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ").ok()?;
    Some(ParsedName {
        created: Utc.from_utc_datetime(&naive),
        kind,
        reason: reason.to_string(),
    })
}

/// Listing: every well-formed export and full backup in the directory, newest first. Files that do
/// not parse are ignored rather than reported — the directory is meant to be read-only to humans.
pub fn list_in(dir: &Path) -> std::io::Result<Vec<SnapshotEntry>> {
    let mut entries = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(e) => return Err(e),
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_name(name) else {
            continue;
        };
        let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push(SnapshotEntry {
            name: name.to_string(),
            kind: parsed.kind,
            created_at: parsed.created.to_rfc3339_opts(SecondsFormat::Secs, true),
            reason: parsed.reason,
            size_bytes,
            usable: match parsed.kind {
                SnapshotKind::Export => looks_like_export(&path),
                SnapshotKind::Backup => looks_like_rdb(&path),
            },
            // A full Redis backup has no JSON summary; its size is already on the entry.
            summary: match parsed.kind {
                SnapshotKind::Export => read_summary(dir, name),
                SnapshotKind::Backup => None,
            },
            pinned: false,
        });
    }
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(entries)
}

/// Does this look like a complete RDB? The magic is the first thing `redis-server` writes, so a
/// truncated copy (a crash mid-`copy`, a disk that filled) fails here rather than at restore time.
/// Only the first five bytes are read — an RDB is routinely hundreds of megabytes and this runs on
/// every list.
fn looks_like_rdb(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 5];
    file.read_exact(&mut magic).is_ok() && magic == RDB_MAGIC
}

/// Is this an export document, judged cheaply? A crash-truncated write stops at an arbitrary byte,
/// so it almost never ends on `}`; a foreign JSON file has no `archives` near the top (the field
/// order is the struct's own). Deliberately heuristic: the restore path parses in full and answers
/// 400 for whatever this lets through, and reading every snapshot fully just to draw a list would
/// cost as much as a backup itself.
fn looks_like_export(path: &Path) -> bool {
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

/// Which files retention would remove. Pure so the rules ("7 daily + 4 weekly + 5 `pre-*` + 5
/// `manual` exports, 3 full backups, never the newest usable file, never a pinned one") can be
/// tested without a filesystem. `pins` is the pinned file names: a pinned entry is exempt from
/// every tier *and* does not consume one of its tier's keep slots — the counts describe how many
/// unpinned files that tier keeps.
pub fn retention_plan(
    entries: &[SnapshotEntry],
    daily_keep: usize,
    week_keep: usize,
    pins: &BTreeSet<String>,
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
    let mut backup_seen = 0usize;
    for entry in &sorted {
        let Some(parsed) = parse_name(&entry.name) else {
            continue;
        };
        // Pinned: never a victim, and skipped before any tier bookkeeping so it does not push an
        // unpinned file out of the tier's budget either.
        if pins.contains(&entry.name) {
            continue;
        }
        // Full Redis backups are one flat tier: the newest `FULL_BACKUP_KEEP` survive whatever
        // their reason was (a scheduled `daily` RDB is still a full backup).
        if entry.kind == SnapshotKind::Backup {
            if backup_seen < FULL_BACKUP_KEEP {
                backup_seen += 1;
            } else {
                victims.push(entry.name.clone());
            }
            continue;
        }
        match entry.reason.as_str() {
            "daily" => {
                let day = parsed
                    .created
                    .with_timezone(&Local)
                    .date_naive()
                    .to_string();
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
                let iso = parsed.created.iso_week();
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

/// Writes the current library as a JSON **export**. Serializes and renames off the async runtime, so
/// a large library does not stall request handling for the duration of the write. This is what the
/// `pre-*` safety snapshots and `POST /api/backups/export` produce; the schedule and
/// `POST /api/backups` write full Redis backups instead ([`take_full_backup`]).
pub async fn take_export(
    state: &AppState,
    reason: &str,
    progress: Arc<dyn BackupProgress>,
) -> Result<SnapshotEntry, String> {
    if !is_known_reason(reason) {
        return Err(format!("unknown export reason: {reason}"));
    }
    progress.update(0, 0, 0.0);
    let doc = crate::database::build_backup_document_result(state).await?;
    // Reading every record out of Redis is the part before serialization; it has its own slice of
    // the bar so a slow library does not look frozen at 0%.
    progress.update(0, 0, 5.0);
    let dir = snapshot_dir(&state.library.archive_dir);
    let created = now_utc();
    let name = format_name(created, SnapshotKind::Export, reason);
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!("{name}.tmp"));

    let size = lanrurugi_storage::fsutil::in_dir(&final_path, || {
        let doc = doc.clone();
        let tmp_path = tmp_path.clone();
        let final_path = final_path.clone();
        let progress = progress.clone();
        async move { write_atomic(&tmp_path, &final_path, doc, progress).await }
    })
    .await
    .map_err(|e| format!("could not write export {name}: {e}"))?;

    // The summary is a convenience for the list view, so a failure here must not fail the export
    // itself — the entry simply reports `summary: null`, exactly like an export written before
    // summaries existed.
    let summary = SnapshotSummary {
        bytes: size,
        ..summarize(&doc)
    };
    let summary = if let Err(e) = write_summary_file(&dir, &name, &summary).await {
        tracing::warn!(name, error = %e, "could not write the export summary");
        None
    } else {
        Some(summary)
    };

    Ok(SnapshotEntry {
        name,
        kind: SnapshotKind::Export,
        created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
        reason: reason.to_string(),
        size_bytes: size,
        usable: true,
        summary,
        pinned: false,
    })
}

/// `<name>.summary.json` — the sibling holding [`SnapshotSummary`].
fn summary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.summary.json"))
}

// ---------------------------------------------------------------------------------------------
// Full Redis backups (`snapshot-<stamp>-<reason>.rdb`).
//
// The RDB is the only complete backup: every logical database, not just the library records. It is
// reached by asking Redis for a fresh save and copying the file it wrote — never by copying a
// running server's file mid-save. Two things can make that impossible, and both must be explicit
// errors rather than an empty backup: `CONFIG` disabled on a managed instance, and a server whose
// data directory is not on this host (the normal remote/containerised case).
// ---------------------------------------------------------------------------------------------

/// `CONFIG GET dir` + `CONFIG GET dbfilename`, i.e. where `redis-server` keeps its RDB.
async fn redis_rdb_path(state: &AppState) -> Result<PathBuf, String> {
    let mut conn = state
        .redis
        .config
        .get()
        .await
        .map_err(|e| format!("could not reach Redis: {e}"))?;
    // RESP2 flattens `CONFIG GET` to `[key, value]`; a server that answers with a map, or refuses
    // `CONFIG` outright (many managed offerings), lands in the error arm instead of yielding a path.
    let dir: Vec<String> = deadpool_redis::redis::cmd("CONFIG")
        .arg("GET")
        .arg("dir")
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            format!(
                "could not ask Redis where its RDB lives ({e}); a Redis that disables CONFIG cannot \
                 be backed up this way"
            )
        })?;
    let file: Vec<String> = deadpool_redis::redis::cmd("CONFIG")
        .arg("GET")
        .arg("dbfilename")
        .query_async(&mut conn)
        .await
        .map_err(|e| format!("could not ask Redis for its RDB filename ({e})"))?;
    let dir = dir.get(1).map(String::as_str).unwrap_or_default().trim();
    if dir.is_empty() {
        return Err(
            "Redis reported an empty RDB directory; no full backup was written".to_string(),
        );
    }
    let file = file.get(1).map(String::as_str).unwrap_or("dump.rdb").trim();
    Ok(Path::new(dir).join(file))
}

/// Takes a full Redis backup: `BGSAVE`, wait for the save, copy the RDB into the backup directory
/// atomically. `reason` is `daily` / `weekly` / `manual` — the same vocabulary the exports use, so
/// a listing reads the same way for both kinds.
pub async fn take_full_backup(
    state: &AppState,
    reason: &str,
    progress: Arc<dyn BackupProgress>,
) -> Result<SnapshotEntry, String> {
    if !is_known_backup_reason(reason) {
        return Err(format!("unknown backup reason: {reason}"));
    }
    let source = redis_rdb_path(state).await?;

    let mut conn = state
        .redis
        .config
        .get()
        .await
        .map_err(|e| format!("could not reach Redis: {e}"))?;
    // The denominator for the BGSAVE phase: what Redis says it is holding, or (when `INFO` is not
    // available) last RDB's size plus a little headroom. The RDB is compressed, so the real file is
    // usually *smaller* than this — the bar then runs ahead and pauses, which is the honest shape;
    // the copy phase below still ends at exactly 100.
    let expected = expected_rdb_bytes(&mut conn, &source).await;
    progress.update(0, expected, 0.0);
    let before: i64 = deadpool_redis::redis::cmd("LASTSAVE")
        .query_async(&mut conn)
        .await
        .map_err(|e| format!("could not read Redis's last-save time: {e}"))?;
    // A refused `BGSAVE` means a save is already running; the LASTSAVE poll below covers that case
    // too, so the reply itself is not interesting.
    let save_issued = std::time::Instant::now();
    let _: Result<String, _> = deadpool_redis::redis::cmd("BGSAVE")
        .query_async(&mut conn)
        .await;
    let temp_dir = source.parent().map(Path::to_path_buf);
    // A completed save renames a freshly written file over the RDB, so its modification time moves
    // — a signal with sub-second resolution. `LASTSAVE` alone is not enough: it is whole seconds,
    // and a save that finished in the same second we sampled `before` (or a refused `BGSAVE`
    // because another one was already running) leaves it equal, which used to spin here until the
    // 120 s deadline.
    let before_mtime = std::fs::metadata(&source).and_then(|m| m.modified()).ok();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let after: i64 = deadpool_redis::redis::cmd("LASTSAVE")
            .query_async(&mut conn)
            .await
            .map_err(|e| format!("could not read Redis's last-save time: {e}"))?;
        // `redis-server` writes the save into `temp-*.rdb` before renaming it over the real file, so
        // that file's size is a direct read of how far the save has got.
        if let Some(dir) = temp_dir.as_deref() {
            let written = rdb_temp_size(dir).min(expected);
            progress.update(written, expected, 90.0 * written as f64 / expected as f64);
        }
        let saved = std::fs::metadata(&source).and_then(|m| m.modified()).ok();
        let fresh_file = match (before_mtime, saved) {
            (Some(before), Some(after)) => after > before,
            // The file did not exist when we started: its appearance is the save.
            (None, Some(_)) => true,
            _ => false,
        };
        // Third signal, for an RDB this process cannot stat (a containerised Redis) and a save
        // that landed inside the same whole second as `LASTSAVE`: `INFO persistence` says no save
        // is running. Only trusted once the fork has had time to start, so it cannot accept the
        // pre-save state and skip copying a file that is about to be replaced mid-read.
        let settled = if save_issued.elapsed() >= std::time::Duration::from_millis(500) {
            let info: String = deadpool_redis::redis::cmd("INFO")
                .arg("persistence")
                .query_async(&mut conn)
                .await
                .unwrap_or_default();
            !info
                .lines()
                .find_map(|line| line.trim().strip_prefix("rdb_bgsave_in_progress:"))
                .is_some_and(|value| value.trim() == "1")
        } else {
            false
        };
        if after > before || fresh_file || settled {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "Redis did not finish its background save within 120s; no full backup was written"
                    .to_string(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    progress.update(expected, expected, 90.0);

    // Checked only after the save: a Redis that has never saved has no RDB file yet, so an earlier
    // check would refuse a perfectly valid first backup. A remote server's path is readable to *it*
    // and not to us, which is the case this message exists for.
    if !source.is_file() {
        return Err(format!(
            "Redis finished saving but its RDB at {} is not readable from this host — a remote or \
             containerised Redis cannot be backed up this way (share its data directory with this \
             service, or back it up on the Redis host)",
            source.display()
        ));
    }

    let dir = snapshot_dir(&state.library.archive_dir);
    let created = now_utc();
    let name = format_name(created, SnapshotKind::Backup, reason);
    let final_path = dir.join(&name);
    let tmp_path = dir.join(format!("{name}.tmp"));
    let size = copy_rdb(&source, &tmp_path, &final_path, progress).await?;

    Ok(SnapshotEntry {
        name,
        kind: SnapshotKind::Backup,
        created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
        reason: reason.to_string(),
        size_bytes: size,
        usable: true,
        summary: None,
        pinned: false,
    })
}

/// Copies the RDB to `tmp`, proves it is a complete RDB, then renames it onto `final_path`. Off the
/// async runtime: the file is routinely hundreds of megabytes.
async fn copy_rdb(
    source: &Path,
    tmp: &Path,
    final_path: &Path,
    progress: Arc<dyn BackupProgress>,
) -> Result<u64, String> {
    let label = source.display().to_string();
    let source = source.to_path_buf();
    let tmp = tmp.to_path_buf();
    let final_path = final_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
        if let Some(parent) = tmp.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A hand-rolled chunked copy rather than `std::fs::copy`: this is the 90→100 slice of the
        // bar, and `fs::copy` reports nothing until it is done.
        let total = std::fs::metadata(&source)?.len().max(1);
        let mut input = std::fs::File::open(&source)?;
        let mut output = std::fs::File::create(&tmp)?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut copied = 0u64;
        let mut last_sample = std::time::Instant::now();
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
            copied += read as u64;
            if last_sample.elapsed() >= std::time::Duration::from_millis(150) || copied == total {
                progress.update(copied, total, 90.0 + 10.0 * copied as f64 / total as f64);
                last_sample = std::time::Instant::now();
            }
        }
        output.flush()?;
        output.sync_all()?;
        drop(output);
        let mut file = std::fs::File::open(&tmp)?;
        let mut magic = [0u8; 5];
        file.read_exact(&mut magic)?;
        if magic != RDB_MAGIC {
            std::fs::remove_file(&tmp).ok();
            return Err(std::io::Error::other(
                "Redis reported this path but its contents are not an RDB",
            ));
        }
        file.sync_all()?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(std::fs::metadata(&final_path)?.len())
    })
    .await
    .map_err(|e| format!("could not copy the Redis RDB: {e}"))?
    .map_err(|e| format!("could not copy the Redis RDB from {label}: {e}"))
}

/// Redis's own idea of how big the RDB will be, for the BGSAVE phase's denominator.
async fn expected_rdb_bytes(conn: &mut deadpool_redis::Connection, source: &Path) -> u64 {
    let info: String = deadpool_redis::redis::cmd("INFO")
        .arg("memory")
        .query_async(conn)
        .await
        .unwrap_or_default();
    let used = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("used_memory:"))
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let previous = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
    used.max((previous as f64 * 1.05).round() as u64).max(1)
}

/// Size of the save Redis is currently writing (`temp-*.rdb` in its data directory), or 0.
fn rdb_temp_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.starts_with("temp-") && name.ends_with(".rdb") {
                entry.metadata().ok().map(|meta| meta.len())
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0)
}

/// Reads a snapshot's summary. Small file, never the snapshot itself; anything unreadable or
/// unparsable is simply "no summary".
fn read_summary(dir: &Path, name: &str) -> Option<SnapshotSummary> {
    let bytes = std::fs::read(summary_path(dir, name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes the summary atomically, the same `.tmp` + rename as the snapshot, so a crash cannot leave
/// a half-written summary next to a complete snapshot.
async fn write_summary_file(
    dir: &Path,
    name: &str,
    summary: &SnapshotSummary,
) -> std::io::Result<()> {
    let final_path = summary_path(dir, name);
    let tmp_path = dir.join(format!("{name}.summary.json.tmp"));
    let summary = summary.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&tmp_path)?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &summary).map_err(std::io::Error::other)?;
        std::io::Write::flush(&mut writer)?;
        drop(writer);
        std::fs::File::open(&tmp_path)?.sync_all()?;
        std::fs::rename(&tmp_path, &final_path)
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Removes a snapshot together with its summary. A missing summary is not an error: snapshots
/// written before summaries existed have none.
async fn remove_snapshot_files(dir: &Path, name: &str) -> std::io::Result<()> {
    tokio::fs::remove_file(dir.join(name)).await?;
    match tokio::fs::remove_file(summary_path(dir, name)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Serializes `doc` to `tmp`, flushes it to disk, then renames onto `final_path`. Atomic from a
/// reader's point of view: `.json` either does not exist or is a complete document, and a crash
/// leaves at most a `.json.tmp` that `list_in` ignores.
async fn write_atomic(
    tmp: &Path,
    final_path: &Path,
    doc: lanrurugi_backup::build::BackupDocument,
    progress: Arc<dyn BackupProgress>,
) -> std::io::Result<u64> {
    let tmp = tmp.to_path_buf();
    let final_path = final_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);
        write_document(&mut writer, &doc, progress.as_ref())?;
        writer.flush()?;
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
    match take_export(state, reason, Arc::new(NoProgress)).await {
        Ok(entry) => SnapshotOutcome::success(entry.name),
        Err(e) => {
            tracing::warn!(reason, error = %e, "could not take a pre-operation backup snapshot");
            SnapshotOutcome::failure(e)
        }
    }
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
    // A pin read that fails must not turn into "nothing is pinned" here: that would evict a
    // snapshot the user asked to keep. Skipping one sweep is the cheap, safe answer.
    let Some(pins) = read_pins(state).await else {
        tracing::warn!("could not read pinned snapshots; skipping this retention sweep");
        return 0;
    };
    let victims = retention_plan(&entries, daily_keep, week_keep, &pins);
    let mut removed = 0usize;
    for name in victims {
        // `retention_plan` only ever returns parsed names, but re-checking here keeps the removal
        // path itself incapable of joining anything that is not a plain file name.
        if parse_name(&name).is_none() {
            continue;
        }
        match remove_snapshot_files(&dir, &name).await {
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
    //
    // `POST /backups` writes a full Redis backup; `POST /backups/export` writes the JSON export.
    // Everything about one file (download, pin, delete) is name-scoped under `/backups/{name}`, so
    // the same sub-routes serve both kinds and only the kind-specific handling differs.
    Router::new()
        .route("/backups", get(list_backups).post(create_backup))
        .route("/backups/export", post(create_export))
        .route("/backups/schedule/preview", post(preview_schedule))
        .route("/backups/jobs/{job_id}", get(jobs::job_status))
        .route("/backups/jobs/{job_id}/events", get(jobs::job_events))
        .route(
            "/backups/{name}",
            get(download_backup).delete(delete_backup),
        )
        .route("/backups/{name}/restore", post(restore_backup))
        .route("/backups/{name}/pin", post(pin_backup).delete(unpin_backup))
}

#[derive(Debug, Deserialize)]
struct CronPreviewRequest {
    cron: String,
}

/// `POST /api/backups/schedule/preview` — validate an expression and show the next firings before
/// it is saved. Answers `200` with `valid: false` for a bad expression: this is a form-validation
/// helper, not a failed request, and the UI shows `error` inline.
async fn preview_schedule(axum::Json(body): axum::Json<CronPreviewRequest>) -> Response {
    let now = Local::now();
    match body.cron.parse::<CronSchedule>() {
        Ok(cron) => axum::Json(json!({
            "valid": true,
            "timezone": local_timezone_label(),
            "next_runs": cron
                .next_runs(now, NEXT_RUNS)
                .into_iter()
                .map(iso_local)
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => axum::Json(json!({
            "valid": false,
            "error": e.to_string(),
            "timezone": local_timezone_label(),
            "next_runs": [],
        }))
        .into_response(),
    }
}

fn entry_json(entry: &SnapshotEntry) -> serde_json::Value {
    json!({
        "name": entry.name,
        // "backup" (full Redis RDB) or "export" (JSON document). The one field a client must branch
        // on: only an export can be restored from inside the app.
        "kind": entry.kind,
        "created_at": entry.created_at,
        "reason": entry.reason,
        "size_bytes": entry.size_bytes,
        "usable": entry.usable,
        // `null` (never omitted) when the sibling summary is missing, so a client can rely on the
        // key existing. Always null for a full backup, which has no JSON summary.
        "summary": entry.summary.as_ref(),
        // Always present, so a client never has to treat "missing" and "not pinned" differently.
        "pinned": entry.pinned,
    })
}

async fn list_backups(State(state): State<AppState>) -> Response {
    let dir = snapshot_dir(&state.library.archive_dir);
    match list_in(&dir) {
        Ok(mut entries) => {
            // A failed read means "no pins shown" rather than a 500: the list is still correct
            // about everything else, and nothing is deleted here.
            let pins = read_pins(&state).await.unwrap_or_default();
            for entry in &mut entries {
                entry.pinned = pins.contains(&entry.name);
            }
            let schedule = SnapshotSchedule::load(&state).await;
            let served = read_schedule_state(&state).await;
            axum::Json(json!({
                "operation": "list_backups",
                "success": 1,
                "daily_keep": read_snapshot_daily_keep(&state).await,
                "week_keep": read_snapshot_week_keep(&state).await,
                "schedule": schedule_json(&schedule, served, Local::now()),
                "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_backups",
            format!("could not read the backup directory: {e}"),
        ),
    }
}

/// `POST /api/backups` — starts a full Redis backup and answers immediately with the job id. The
/// backup (`BGSAVE`, wait, copy) runs on its own task and reports progress on
/// `GET /api/backups/jobs/{job_id}/events`; a deployment whose Redis is remote or disallows `CONFIG`
/// fails there with an explicit error rather than writing an empty file.
async fn create_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let job = jobs::start_full_backup(&state, auth.map(|e| e.0)).await;
    job_started_response("create_backup", &job)
}

/// `POST /api/backups/export` — the selective JSON export, same job shape.
async fn create_export(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let job = jobs::start_export(&state, auth.map(|e| e.0)).await;
    job_started_response("create_export", &job)
}

fn job_started_response(operation: &str, job: &jobs::BackupJob) -> Response {
    axum::Json(json!({
        "operation": operation,
        "success": 1,
        "job_id": job.id,
        "started_at": job.status().started_at,
        // The SSE URL, so a client does not have to build it from the id.
        "events": format!("/api/backups/jobs/{}/events", job.id),
    }))
    .into_response()
}

/// Resolves a caller-supplied name to a real file, refusing anything that is not exactly a
/// snapshot filename this module wrote. `None` is answered as a 400 by each caller.
fn resolve_name(state: &AppState, name: &str) -> Option<PathBuf> {
    parse_name(name)?;
    Some(snapshot_dir(&state.library.archive_dir).join(name))
}

/// `GET /api/backups/{name}` — the file's raw bytes, with the name's own content type
/// (`application/json` for an export, `application/octet-stream` for an RDB). An **export** also
/// accepts `?format=zip` for a one-entry `.zip` holding exactly that file; a full backup is always
/// served as the single `.rdb`, because that is what an operator replaces. Any other `format`
/// value keeps the raw default rather than erroring: the download is the primary contract, and a
/// stray query string must not break it.
async fn download_backup(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    axum::extract::Query(query): axum::extract::Query<DownloadQuery>,
) -> Response {
    let Some(parsed) = parse_name(&name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "serve_backup",
            "Invalid backup name.",
        );
    };
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

    let wants_zip = parsed.kind == SnapshotKind::Export && query.format.as_deref() == Some("zip");
    if !wants_zip {
        let content_type = match parsed.kind {
            SnapshotKind::Export => "application/json",
            SnapshotKind::Backup => "application/octet-stream",
        };
        return (
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            body,
        )
            .into_response();
    }

    // The zip is built off the async runtime: compressing a multi-megabyte document would
    // otherwise stall request handling. The member keeps the snapshot's own file name, so
    // unzipping it gives back exactly the file `GET` without `format` would have served.
    let zip_name = format!("{name}.zip");
    let member = name.clone();
    let zipped = tokio::task::spawn_blocking(move || zip_snapshot(&member, &body)).await;
    let zip_bytes = match zipped {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_backup",
                format!("could not package {name} as a zip: {e}"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_backup",
                format!("could not package {name} as a zip: {e}"),
            )
        }
    };
    (
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{zip_name}\""),
            ),
        ],
        zip_bytes,
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
struct DownloadQuery {
    format: Option<String>,
}

/// One-member zip holding `document` under `name`. Pure (no I/O, no `AppState`) so the "readable
/// back, exactly one member, same bytes" rule has a unit test.
fn zip_snapshot(name: &str, document: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buf);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file(name, options)?;
        std::io::Write::write_all(&mut writer, document)?;
        writer.finish()?;
    }
    Ok(buf.into_inner())
}

/// `POST /api/backups/{name}/restore` — restores a JSON **export**. A full Redis backup is
/// deliberately not restorable here: replacing a live server's RDB is an operator action (stop
/// Redis, replace the file, start it), and doing it from inside a running process would corrupt the
/// server it is talking to.
async fn restore_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    if parse_name(&name).is_some_and(|parsed| parsed.kind == SnapshotKind::Backup) {
        return (
            StatusCode::CONFLICT,
            axum::Json(json!({
                "operation": "restore_backup",
                "success": 0,
                "error": "A full Redis backup cannot be restored from inside the app. Download the \
                          .rdb, stop Redis, replace its RDB file with this one, then start Redis again.",
                "reason": "full_backup_not_restorable",
            })),
        )
            .into_response();
    }
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
    let (job, progress_job) =
        crate::database::start_restore_job(&state, doc, auth.as_ref().map(|e| e.0.clone()), safety)
            .await;
    axum::Json(json!({
        "operation": "restore_backup",
        "success": 1,
        "restored_from": name,
        "safety": safety_name,
        // `job` stays for the Jobs console; `job_id` keys the progress stream.
        "job": job,
        "job_id": progress_job.id,
        "started_at": progress_job.status().started_at,
        "events": format!("/api/backups/jobs/{}/events", progress_job.id),
    }))
    .into_response()
}

async fn delete_backup(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    UrlPath(name): UrlPath<String>,
) -> Response {
    // Name validation only — the files themselves are removed by name below, together with each
    // summary, so no caller-supplied string is ever joined into a path here.
    if resolve_name(&state, &name).is_none() {
        return error(
            StatusCode::BAD_REQUEST,
            "serve_backup",
            "Invalid backup name.",
        );
    }
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
    // The path was checked above; removing by name (rather than by that path) lets the summary go
    // with it. A pin only ever referred to this file, so it goes with it too — failing to clear it
    // is a warning, not a failed delete: the timestamped name can never be re-created.
    if let Err(e) = remove_snapshot_files(&dir, &name).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_backup",
            e.to_string(),
        );
    }
    if let Err(e) = set_pin(&state, &name, false).await {
        tracing::warn!(name, error = %e, "could not clear the deleted snapshot's pin");
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

/// Shared body of the two pin endpoints. Both are idempotent (Redis `SADD`/`SREM`), but the name
/// must still be a real snapshot: pinning something that does not exist would report success and
/// pin nothing. No activity entry is recorded — a pin is a retention preference, like a setting,
/// not a change to the library.
async fn set_pin_response(state: AppState, name: String, pinned: bool) -> Response {
    let operation = if pinned { "pin_backup" } else { "unpin_backup" };
    let Some(path) = resolve_name(&state, &name) else {
        return error(StatusCode::BAD_REQUEST, operation, "Invalid backup name.");
    };
    if !path.is_file() {
        return not_found(operation, format!("Backup {name} does not exist."));
    }
    if let Err(e) = set_pin(&state, &name, pinned).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, operation, e);
    }
    axum::Json(json!({
        "operation": operation,
        "success": 1,
        "name": name,
        "pinned": pinned,
    }))
    .into_response()
}

async fn pin_backup(State(state): State<AppState>, UrlPath(name): UrlPath<String>) -> Response {
    set_pin_response(state, name, true).await
}

async fn unpin_backup(State(state): State<AppState>, UrlPath(name): UrlPath<String>) -> Response {
    set_pin_response(state, name, false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(name: &str, usable: bool) -> SnapshotEntry {
        let parsed = parse_name(name).expect("test name is well formed");
        SnapshotEntry {
            name: name.to_string(),
            kind: parsed.kind,
            created_at: parsed.created.to_rfc3339_opts(SecondsFormat::Secs, true),
            reason: parsed.reason,
            size_bytes: 10,
            usable,
            summary: None,
            pinned: false,
        }
    }

    fn no_pins() -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn pins(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn download_query(format: Option<&str>) -> axum::extract::Query<DownloadQuery> {
        axum::extract::Query(DownloadQuery {
            format: format.map(str::to_string),
        })
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    async fn list_json(state: &AppState) -> serde_json::Value {
        response_json(list_backups(State(state.clone())).await).await
    }

    /// Waits for a job started by the create handlers to reach a terminal state and returns its
    /// status JSON. Bounded, so a hung job fails the test instead of hanging the suite.
    async fn wait_for_job(response: Response) -> serde_json::Value {
        let body = response_json(response).await;
        let id = body["job_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the create call returns a job id: {body}"))
            .to_string();
        assert!(
            body["events"]
                .as_str()
                .unwrap()
                .ends_with(&format!("/{id}/events")),
            "{body}"
        );
        for _ in 0..1_200 {
            let job = jobs::lookup(&id).expect("the job is registered");
            let status = job.status();
            if status.state != "running" {
                return serde_json::to_value(status).unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("job {id} never finished");
    }

    async fn listed_entry(state: &AppState, name: &str) -> serde_json::Value {
        list_json(state).await["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .cloned()
            .expect("the entry is listed")
    }

    #[test]
    fn names_round_trip_and_reject_traversal() {
        let parsed = parse_name("export-20261123T050000Z-daily.json").unwrap();
        assert_eq!(parsed.reason, "daily");
        assert_eq!(parsed.kind, SnapshotKind::Export);
        assert_eq!(
            parsed.created.to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-11-23T05:00:00Z"
        );

        let backup = parse_name("snapshot-20261123T050000Z-manual.rdb").unwrap();
        assert_eq!(backup.kind, SnapshotKind::Backup);
        assert_eq!(backup.reason, "manual");

        // The schedule's own reason, and the two an earlier build wrote (still listed).
        let scheduled = parse_name("snapshot-20261123T030000Z-scheduled.rdb").unwrap();
        assert_eq!(scheduled.kind, SnapshotKind::Backup);
        assert_eq!(scheduled.reason, "scheduled");
        assert!(parse_name("snapshot-20261123T030000Z-weekly.rdb").is_some());

        // A file written before the two kinds were split keeps parsing, as an export.
        let legacy = parse_name("20261123T050000Z-daily.json").unwrap();
        assert_eq!(legacy.kind, SnapshotKind::Export);
        assert_eq!(legacy.reason, "daily");

        assert!(parse_name("export-20261123T050000Z-pre-journal-purge.json").is_some());
        assert!(parse_name("../etc/passwd").is_none());
        assert!(parse_name("export-20261123T050000Z-unknown.json").is_none());
        assert!(parse_name("export-20261123T050000Z-daily.json.tmp").is_none());
        assert!(parse_name("nope.json").is_none());
        // The prefix decides the kind, so a mismatched extension is not a file this module wrote.
        assert!(parse_name("snapshot-20261123T050000Z-daily.json").is_none());
        assert!(parse_name("export-20261123T050000Z-daily.rdb").is_none());
        // A full backup is never a pre-* safety snapshot.
        assert!(parse_name("snapshot-20261123T050000Z-pre-clean.rdb").is_none());
    }

    #[test]
    fn retention_keeps_seven_daily_four_weekly_and_five_pre() {
        let mut entries = Vec::new();
        for day in 1..=10 {
            entries.push(entry(
                &format!("export-202611{:02}T050000Z-daily.json", day),
                true,
            ));
        }
        for (month, day) in [(11, 2), (11, 9), (11, 16), (11, 23), (11, 30), (12, 7)] {
            entries.push(entry(
                &format!("export-2026{month:02}{day:02}T050000Z-weekly.json"),
                true,
            ));
        }
        for i in 0..8 {
            entries.push(entry(
                &format!("export-2026111{}T060000Z-pre-clean.json", i),
                true,
            ));
        }
        let victims = retention_plan(&entries, 7, 4, &no_pins());
        let removed_daily = victims.iter().filter(|n| n.contains("-daily")).count();
        let removed_weekly = victims.iter().filter(|n| n.contains("-weekly")).count();
        let removed_pre = victims.iter().filter(|n| n.contains("-pre-")).count();
        assert_eq!(removed_daily, 3, "10 daily -> 7 kept");
        assert_eq!(removed_weekly, 2, "6 weekly -> 4 kept");
        assert_eq!(removed_pre, 3, "8 pre -> 5 kept");
        assert!(!victims.contains(&"export-20261110T050000Z-daily.json".to_string()));
    }

    #[test]
    fn retention_caps_manual_exports_too() {
        // A manual export has no time window, so only a count cap bounds it; without one these
        // accumulate forever and the total budget is unbounded.
        let entries: Vec<SnapshotEntry> = (1..=8)
            .map(|d| entry(&format!("export-202611{d:02}T050000Z-manual.json"), true))
            .collect();
        let victims = retention_plan(&entries, 7, 4, &no_pins());
        let removed_manual = victims.iter().filter(|n| n.contains("manual")).count();
        assert_eq!(removed_manual, 3, "8 manual -> 5 kept");
        assert!(!victims.contains(&"export-20261108T050000Z-manual.json".to_string()));
    }

    /// Full backups are their own flat tier: three regardless of reason, and independent of the
    /// export tiers happening in the same directory.
    #[test]
    fn retention_keeps_only_the_newest_three_full_backups() {
        let mut entries = Vec::new();
        for day in 1..=5 {
            entries.push(entry(
                &format!("snapshot-202611{:02}T050000Z-daily.rdb", day),
                true,
            ));
        }
        entries.push(entry("snapshot-20261106T050000Z-manual.rdb", true));
        // An export in the same directory must not be counted as a backup, nor vice versa.
        entries.push(entry("export-20261107T050000Z-manual.json", true));

        let victims = retention_plan(&entries, 7, 4, &no_pins());
        assert_eq!(victims.len(), 3, "6 backups -> 3 kept");
        assert!(!victims.contains(&"snapshot-20261106T050000Z-manual.rdb".to_string()));
        assert!(victims.contains(&"snapshot-20261101T050000Z-daily.rdb".to_string()));
        assert!(!victims.contains(&"export-20261107T050000Z-manual.json".to_string()));

        // Pinned: exempt, and it does not consume one of the three slots either, so only the two
        // unpinned stragglers go.
        let pinned = pins(&["snapshot-20261101T050000Z-daily.rdb"]);
        let victims = retention_plan(&entries, 7, 4, &pinned);
        assert_eq!(victims.len(), 2);
        assert!(!victims.contains(&"snapshot-20261101T050000Z-daily.rdb".to_string()));
    }

    /// The whole point of a pin: a file the tier caps would otherwise drop is never selected.
    #[test]
    fn a_pinned_export_is_never_evicted_and_does_not_consume_a_slot() {
        // 9 dailies with keep=7: two go, oldest first. Pinning the oldest keeps it, and because a
        // pin takes no slot, only the second oldest is now a victim.
        let mut entries = Vec::new();
        for day in 1..=9 {
            entries.push(entry(
                &format!("export-202611{:02}T050000Z-daily.json", day),
                true,
            ));
        }
        let oldest = "export-20261101T050000Z-daily.json";
        let victims = retention_plan(&entries, 7, 4, &no_pins());
        assert!(victims.contains(&oldest.to_string()));

        let victims = retention_plan(&entries, 7, 4, &pins(&[oldest]));
        assert_eq!(
            victims,
            vec!["export-20261102T050000Z-daily.json".to_string()]
        );
    }

    /// The skip happens before the tier match, so it applies to `pre-*` and `manual` too, not just
    /// the daily tier.
    #[test]
    fn a_pin_exempts_every_tier() {
        let entries: Vec<SnapshotEntry> = (1..=8)
            .map(|i| entry(&format!("export-2026110{}T060000Z-pre-clean.json", i), true))
            .collect();
        let oldest = "export-20261101T060000Z-pre-clean.json";
        assert!(retention_plan(&entries, 7, 4, &no_pins()).contains(&oldest.to_string()));
        let victims = retention_plan(&entries, 7, 4, &pins(&[oldest]));
        assert_eq!(victims.len(), 2, "8 pre -> 5 kept, minus the pinned one");
        assert!(!victims.contains(&oldest.to_string()));
    }

    #[test]
    fn retention_never_deletes_the_newest_usable_snapshot() {
        // A single pre-* export that is also the newest usable one, with more pre-* than the cap:
        // the newest must survive even though its own tier would drop it eventually.
        let mut entries: Vec<SnapshotEntry> = (0..8)
            .map(|i| {
                entry(
                    &format!("export-2026110{}T050000Z-pre-clean.json", i + 1),
                    true,
                )
            })
            .collect();
        entries[7].usable = false; // newest is corrupt, so the protected one is the one before it
        let victims = retention_plan(&entries, 7, 4, &no_pins());
        assert!(!victims.contains(&"export-20261107T050000Z-pre-clean.json".to_string()));
        assert!(victims.contains(&"export-20261101T050000Z-pre-clean.json".to_string()));
    }

    #[test]
    fn a_second_daily_export_for_the_same_local_day_is_dropped() {
        // 00:30 and 01:30 UTC are the same local calendar day under every possible UTC offset,
        // so this assertion does not depend on the test machine's timezone.
        let entries = vec![
            entry("export-20261123T003000Z-daily.json", true),
            entry("export-20261123T013000Z-daily.json", true),
            entry("export-20261124T120000Z-daily.json", true),
        ];
        let victims = retention_plan(&entries, 7, 4, &no_pins());
        assert_eq!(
            victims,
            vec!["export-20261123T003000Z-daily.json".to_string()]
        );
    }

    /// Listing tells the two kinds apart, still shows a pre-split export, and marks truncated files
    /// of either kind unusable.
    #[test]
    fn listing_reports_both_kinds_and_marks_truncated_files_unusable() {
        let tmp = tempfile::tempdir().unwrap();
        let good_export = "export-20261123T050000Z-daily.json";
        let legacy = "20261122T050000Z-daily.json";
        let truncated = "export-20261124T050000Z-daily.json";
        let foreign = "export-20261125T050000Z-daily.json";
        let good_backup = "snapshot-20261126T050000Z-manual.rdb";
        let truncated_backup = "snapshot-20261127T050000Z-manual.rdb";
        write(
            tmp.path(),
            good_export,
            "{\"archives\":[],\"categories\":[]}",
        );
        write(tmp.path(), legacy, "{\"archives\":[],\"categories\":[]}");
        // A write cut off mid-token, which is what a crash or a full disk actually leaves behind.
        write(
            tmp.path(),
            truncated,
            "{\"archives\":[{\"arcid\":\"a\",\"ti",
        );
        write(tmp.path(), foreign, "{\"nope\":1}");
        write(tmp.path(), good_backup, "REDIS0011\u{0}\u{0}\u{0}");
        write(tmp.path(), truncated_backup, "REDI");

        let entries = list_in(tmp.path()).unwrap();
        assert_eq!(entries.len(), 6);
        let by_name = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
        assert!(by_name(good_export).usable);
        assert_eq!(by_name(good_export).kind, SnapshotKind::Export);
        assert_eq!(by_name(legacy).kind, SnapshotKind::Export);
        assert!(!by_name(truncated).usable);
        assert!(!by_name(foreign).usable);
        assert!(by_name(good_backup).usable);
        assert_eq!(by_name(good_backup).kind, SnapshotKind::Backup);
        assert_eq!(by_name(good_backup).reason, "manual");
        assert!(by_name(good_backup).summary.is_none());
        assert!(!by_name(truncated_backup).usable);
        assert_eq!(entry_json(by_name(good_export))["kind"], json!("export"));
        assert_eq!(entry_json(by_name(good_backup))["kind"], json!("backup"));

        // A `.tmp` left by a crash is not listed at all.
        write(tmp.path(), "export-20261128T050000Z-daily.json.tmp", "{}");
        assert_eq!(list_in(tmp.path()).unwrap().len(), 6);
    }

    /// T021: the write leaves exactly one complete `.json` and no `.tmp` behind, and the result
    /// parses back as the same document.
    #[tokio::test]
    async fn an_atomic_write_leaves_no_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let name = "export-20261123T050000Z-daily.json";
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
        let size = write_atomic(&tmp_path, &final_path, doc, Arc::new(NoProgress))
            .await
            .unwrap();
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

    /// The export handlers write a real file, list it with its kind, serve exactly those bytes both
    /// raw and as a one-entry zip, and refuse to delete the newest usable one.
    #[tokio::test]
    async fn export_handlers_create_list_serve_zip_and_protect_the_last_usable() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();

        let created = create_export(State(state.clone()), None).await;
        assert_eq!(created.status(), StatusCode::OK);
        let status = wait_for_job(created).await;
        assert_eq!(status["state"], json!("done"), "{status}");
        assert_eq!(status["percent"], json!(100.0));
        assert_eq!(status["kind"], json!("export"));
        let entry = &status["entry"];
        let name = entry["name"].as_str().unwrap().to_string();
        assert_eq!(entry["kind"], json!("export"));
        assert!(entry["usable"].as_bool().unwrap());
        assert!(
            name.starts_with("export-") && name.ends_with("-manual.json"),
            "{name}"
        );
        assert!(entry["summary"]["archives"].is_number());

        let listed = listed_entry(&state, &name).await;
        assert_eq!(listed["kind"], json!("export"));
        assert_eq!(listed["pinned"], json!(false));

        let on_disk = std::fs::read(snapshot_dir(tmp.path()).join(&name)).unwrap();
        let downloaded = download_backup(
            State(state.clone()),
            UrlPath(name.clone()),
            download_query(None),
        )
        .await;
        assert_eq!(downloaded.status(), StatusCode::OK);
        assert_eq!(
            downloaded.headers()[header::CONTENT_TYPE],
            "application/json"
        );
        let bytes = axum::body::to_bytes(downloaded.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), on_disk.as_slice());

        let zipped = download_backup(
            State(state.clone()),
            UrlPath(name.clone()),
            download_query(Some("zip")),
        )
        .await;
        assert_eq!(zipped.status(), StatusCode::OK);
        assert_eq!(zipped.headers()[header::CONTENT_TYPE], "application/zip");
        let disposition = zipped.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            disposition.contains(&format!("{name}.zip")),
            "{disposition}"
        );
        let zip_bytes = axum::body::to_bytes(zipped.into_body(), usize::MAX)
            .await
            .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes.to_vec())).unwrap();
        assert_eq!(archive.len(), 1, "exactly one member");
        {
            let mut member = archive.by_index(0).unwrap();
            assert_eq!(member.name().unwrap().as_ref(), name);
            let mut contents = Vec::new();
            std::io::Read::read_to_end(&mut member, &mut contents).unwrap();
            assert_eq!(contents, on_disk);
        }

        // The only export is the newest usable one: it may not be deleted.
        let deleted = delete_backup(State(state.clone()), None, UrlPath(name.clone())).await;
        assert_eq!(deleted.status(), StatusCode::CONFLICT);

        // A path-ish name is rejected before it is ever joined into a path.
        let bad = download_backup(
            State(state.clone()),
            UrlPath("../../etc/passwd".to_string()),
            download_query(None),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    /// A full Redis backup, against this test deployment's own Redis: the file is a real RDB, it is
    /// listed as `kind=backup`, `?format=zip` deliberately keeps serving the raw `.rdb`, and the
    /// in-app restore answers 409 with the operator instructions. A Redis this process cannot reach
    /// through `CONFIG` (remote/managed) must fail with the explicit explanation instead.
    #[tokio::test]
    async fn a_full_backup_is_created_and_is_not_restorable_in_app() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();

        let created = create_backup(State(state.clone()), None).await;
        assert_eq!(created.status(), StatusCode::OK);
        let status = wait_for_job(created).await;
        if status["state"] != json!("done") {
            let message = status["error"].as_str().unwrap_or_default().to_string();
            assert!(
                message.contains("cannot be backed up this way")
                    || message.contains("could not ask Redis"),
                "an unsupported Redis must fail with the explicit explanation, got {message:?}"
            );
            eprintln!("skipping the rest: this Redis cannot be backed up ({message})");
            return;
        }

        assert_eq!(status["percent"], json!(100.0));
        assert_eq!(status["kind"], json!("backup"));
        let entry = &status["entry"];
        let name = entry["name"].as_str().unwrap().to_string();
        assert_eq!(entry["kind"], json!("backup"));
        assert!(
            name.starts_with("snapshot-") && name.ends_with("-manual.rdb"),
            "{name}"
        );
        assert_eq!(
            entry["summary"],
            serde_json::Value::Null,
            "an RDB has no JSON summary"
        );
        let path = snapshot_dir(tmp.path()).join(&name);
        let on_disk = std::fs::read(&path).unwrap();
        assert!(on_disk.starts_with(b"REDIS"), "the file is a real RDB");
        assert_eq!(entry["size_bytes"], json!(on_disk.len() as u64));

        let listed = listed_entry(&state, &name).await;
        assert_eq!(listed["kind"], json!("backup"));
        assert_eq!(listed["pinned"], json!(false));

        let downloaded = download_backup(
            State(state.clone()),
            UrlPath(name.clone()),
            download_query(Some("zip")),
        )
        .await;
        assert_eq!(downloaded.status(), StatusCode::OK);
        assert_eq!(
            downloaded.headers()[header::CONTENT_TYPE],
            "application/octet-stream",
            "a full backup is always the single .rdb, never a zip"
        );
        let bytes = axum::body::to_bytes(downloaded.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), on_disk.as_slice());

        let restored = restore_backup(State(state.clone()), None, UrlPath(name.clone())).await;
        assert_eq!(restored.status(), StatusCode::CONFLICT);
        assert_eq!(
            response_json(restored).await["reason"],
            json!("full_backup_not_restorable")
        );
    }

    /// Pinning round-trips through the handlers, is idempotent both ways, and the pin is cleared
    /// when the file is deleted rather than left pointing at nothing.
    #[tokio::test]
    async fn pinning_round_trips_and_is_cleared_on_delete() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let older = "snapshot-20261122T030000Z-daily.rdb";
        let newer = "snapshot-20261123T030000Z-daily.rdb";
        write(&dir, older, "REDIS0011\u{0}\u{0}\u{0}");
        write(&dir, newer, "REDIS0011\u{0}\u{0}\u{0}");
        // Start from a clean slate even if an earlier failed run left the pin behind.
        set_pin(&state, older, false).await.unwrap();

        let pinned = pin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        assert_eq!(pinned.status(), StatusCode::OK);
        assert_eq!(response_json(pinned).await["pinned"], json!(true));
        assert_eq!(listed_entry(&state, older).await["pinned"], json!(true));

        // Idempotent.
        let again = pin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        assert_eq!(again.status(), StatusCode::OK);

        let unpinned = unpin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        assert_eq!(unpinned.status(), StatusCode::OK);
        assert_eq!(listed_entry(&state, older).await["pinned"], json!(false));
        let again = unpin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        assert_eq!(again.status(), StatusCode::OK);

        // Pinned again, then deleted: the file and its pin go together.
        pin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        let deleted = delete_backup(State(state.clone()), None, UrlPath(older.to_string())).await;
        assert_eq!(deleted.status(), StatusCode::OK);
        assert!(!read_pins(&state).await.unwrap().contains(older));
        assert!(!dir.join(older).exists());
        // With the file gone, pinning is a 404 rather than a silent success.
        let gone = pin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    /// The zip builder on its own: readable back, exactly one member, byte-identical contents.
    #[test]
    fn a_zip_holds_exactly_the_export_document() {
        let name = "export-20261123T030000Z-daily.json";
        let document = br#"{"archives":[],"categories":[]}"#;
        let zipped = zip_snapshot(name, document).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zipped)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut member = archive.by_index(0).unwrap();
        assert_eq!(member.name().unwrap().as_ref(), name);
        let mut contents = Vec::new();
        std::io::Read::read_to_end(&mut member, &mut contents).unwrap();
        assert_eq!(contents, document);
    }

    // -----------------------------------------------------------------------------------------
    // Configurable schedule. The parser's own behaviour (segment ranges, day-of-month/weekday
    // ORing, names, year field, next_runs) is tested in `cron.rs`; what is tested here is the
    // scheduling policy around it: when a firing is owed, and what the list reports.
    // -----------------------------------------------------------------------------------------

    fn local_at(date: &str, hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        let date =
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("test date is well formed");
        date.and_hms_opt(hour, minute, second)
            .expect("test time is valid")
            .and_local_timezone(Local)
            .single()
            .expect("test instant exists in this timezone")
    }

    fn cron(expression: &str) -> CronSchedule {
        expression.parse().expect("test expression parses")
    }

    fn test_schedule(enabled: bool, expression: &str) -> SnapshotSchedule {
        SnapshotSchedule {
            enabled,
            cron: cron(expression),
            cron_text: expression.to_string(),
            preset: "custom".to_string(),
        }
    }

    #[test]
    fn a_fresh_install_fires_once_immediately() {
        // No record yet: one snapshot now, like the pre-cron build's startup snapshot. This is what
        // makes a sub-minute expression reliable — requiring the tick to land exactly on a hit
        // second can miss every hit forever when the periods do not divide each other.
        let every_ten = cron("*/10 * * * * *");
        for second in [0, 3, 7, 13, 59] {
            assert_eq!(
                due_firing(&every_ten, local_at("2026-11-23", 12, 0, second), None),
                Some(local_at("2026-11-23", 12, 0, second)),
                "a fresh install fires on the first tick, whatever second it lands on"
            );
        }
    }

    #[test]
    fn a_served_firing_is_never_served_twice() {
        let every_ten = cron("*/10 * * * * *");
        let served = Some(local_at("2026-11-23", 12, 0, 10).with_timezone(&Utc));
        // The very second that was served: not due, even though it is still "now".
        assert_eq!(
            due_firing(&every_ten, local_at("2026-11-23", 12, 0, 10), served),
            None
        );
        // Nothing else is due until the next firing actually arrives.
        assert_eq!(
            due_firing(&every_ten, local_at("2026-11-23", 12, 0, 12), served),
            None
        );
        // Then the next one is served, not the already-served one.
        assert_eq!(
            due_firing(&every_ten, local_at("2026-11-23", 12, 0, 22), served),
            Some(local_at("2026-11-23", 12, 0, 20))
        );
    }

    #[test]
    fn a_firing_missed_between_ticks_is_still_served() {
        // Ticks are every 5 seconds, and a slow tick (or jitter) can step over the exact second:
        // the `*/10` firing at :10 is still picked up by a tick at :12, because the record only
        // says what was served *before* it.
        let every_ten = cron("*/10 * * * * *");
        let served = Some(local_at("2026-11-23", 12, 0, 0).with_timezone(&Utc));
        assert_eq!(
            due_firing(&every_ten, local_at("2026-11-23", 12, 0, 12), served),
            Some(local_at("2026-11-23", 12, 0, 10))
        );
    }

    #[test]
    fn downtime_catches_up_exactly_once() {
        let daily = cron("0 0 3 * * *");
        let last = Some(local_at("2026-11-20", 3, 0, 0).with_timezone(&Utc));
        let now = local_at("2026-11-23", 12, 0, 0);
        // Three days were missed; exactly one firing is owed.
        assert_eq!(
            due_firing(&daily, now, last),
            Some(local_at("2026-11-21", 3, 0, 0))
        );
        // Once that tick records the serve time, nothing else is owed — the *serve* time is what
        // gets recorded, not the missed slot, which is what stops the schedule re-firing for every
        // slot it missed.
        assert_eq!(due_firing(&daily, now, Some(now.with_timezone(&Utc))), None);
    }

    #[test]
    fn the_default_expression_is_monday_three_am() {
        let schedule = test_schedule(true, "0 0 3 * * 1");
        let runs = schedule.next_runs(local_at("2026-11-17", 12, 0, 0), 3);
        assert_eq!(
            runs,
            vec![
                local_at("2026-11-23", 3, 0, 0),
                local_at("2026-11-30", 3, 0, 0),
                local_at("2026-12-07", 3, 0, 0),
            ]
        );
    }

    #[test]
    fn a_disabled_schedule_has_no_next_runs() {
        let enabled = test_schedule(true, "0 0 3 * * 1");
        assert_eq!(
            enabled.next_runs(local_at("2026-11-17", 12, 0, 0), 5).len(),
            5
        );
        assert!(test_schedule(false, "0 0 3 * * 1")
            .next_runs(local_at("2026-11-17", 12, 0, 0), 5)
            .is_empty());
    }

    /// The scheduler actually fires: a `*/5` cron, a cleared watermark, and repeated ticks must
    /// leave a `snapshot-*-scheduled.rdb` behind and a non-null `last_fire_at`. This is the
    /// regression for "the plan never fires", where the first firing was only served if the 5 s
    /// tick happened to land exactly on a hit second and could therefore never fire at all.
    ///
    /// The write itself is injected: a Redis's RDB is only readable from the process that shares
    /// its data directory, which the test container does not, so a real `BGSAVE` here would skip
    /// on every run and prove nothing.
    #[tokio::test]
    async fn the_scheduler_writes_a_snapshot_on_a_real_redis() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());

        // Save the cron exactly as `PUT /settings` would store it, then clear the watermark so the
        // instance looks fresh.
        {
            let mut conn = state.redis.config.get().await.unwrap();
            let stored: () = deadpool_redis::redis::cmd("HSET")
                .arg(lanrurugi_storage::keys::CONFIG_KEY)
                .arg("snapshot_cron")
                .arg("*/5 * * * * *")
                .arg("snapshot_schedule_enabled")
                .arg("1")
                .query_async(&mut conn)
                .await
                .unwrap();
            assert_eq!(stored, ());
            let cleared: () = deadpool_redis::redis::cmd("HDEL")
                .arg(SNAPSHOT_SCHEDULE_KEY)
                .arg(LAST_FIRE_FIELD)
                .query_async(&mut conn)
                .await
                .unwrap();
            assert_eq!(cleared, ());
        }

        let mut fired = 0usize;
        for _ in 0..4 {
            let dir_for_task = dir.clone();
            let entry = tick_with(&state, Local::now(), move |_slot| {
                Box::pin(async move {
                    tokio::fs::create_dir_all(&dir_for_task)
                        .await
                        .map_err(|e| e.to_string())?;
                    let created = now_utc();
                    let name = format_name(created, SnapshotKind::Backup, "scheduled");
                    tokio::fs::write(dir_for_task.join(&name), b"REDIS0011stub")
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(SnapshotEntry {
                        name,
                        kind: SnapshotKind::Backup,
                        created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
                        reason: "scheduled".to_string(),
                        size_bytes: 13,
                        usable: true,
                        summary: None,
                        pinned: false,
                    })
                })
            })
            .await;
            if entry.is_some() {
                fired += 1;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        // The very first tick of a fresh install fires, so at least one snapshot must exist; the
        // watermark must be recorded, and the list must show it as a real `scheduled` backup.
        assert!(fired >= 1, "the scheduler never fired");
        assert!(
            read_last_fire(&state).await.is_some(),
            "the watermark is recorded once a firing is served"
        );
        let entries = list_in(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.reason == "scheduled" && entry.kind == SnapshotKind::Backup),
            "{entries:?}"
        );

        // Restart-no-duplicate: the same watermark immediately afterwards owes nothing, so the
        // injected writer must not even be called.
        let again = tick_with(&state, Local::now(), move |_slot| {
            Box::pin(async move {
                panic!("a served firing must not be served twice");
                #[allow(unreachable_code)]
                Err::<SnapshotEntry, String>("unreachable".to_string())
            })
        })
        .await;
        assert!(again.is_none());
    }

    #[test]
    fn schedule_json_reports_the_cron_shape() {
        let schedule = test_schedule(true, "0 0 3 * * 1");
        let last_fire = Some(
            Utc.with_ymd_and_hms(2026, 11, 23, 3, 0, 0)
                .single()
                .expect("unambiguous UTC instant"),
        );
        let now = local_at("2026-11-23", 5, 0, 0);
        let value = schedule_json(&schedule, last_fire, now);
        assert_eq!(value["enabled"], json!(true));
        assert_eq!(value["cron"], json!("0 0 3 * * 1"));
        assert_eq!(value["preset"], json!("custom"));
        assert!(value["timezone"].is_string());
        assert!(
            value["last_fire_at"].is_string(),
            "ISO with the local offset"
        );
        assert_eq!(value["next_runs"].as_array().unwrap().len(), NEXT_RUNS);
        assert!(
            value["next_runs"][0].as_str().unwrap().contains("03:00:00"),
            "{}",
            value["next_runs"][0]
        );

        // Disabled: no next runs suggested, but what already fired is still reported honestly.
        let value = schedule_json(&test_schedule(false, "0 0 3 * * 1"), last_fire, now);
        assert_eq!(value["next_runs"], json!([]));
        assert!(value["last_fire_at"].is_string());
        assert_eq!(
            schedule_json(&schedule, None, now)["last_fire_at"],
            json!(null)
        );
    }

    fn backup_archive(id: &str, tags: &str) -> lanrurugi_backup::build::BackupArchive {
        lanrurugi_backup::build::BackupArchive {
            arcid: id.to_string(),
            title: id.to_string(),
            tags: tags.to_string(),
            summary: None,
            thumbhash: None,
            filename: format!("{id}.zip"),
        }
    }

    fn backup_doc_with(
        archives: Vec<lanrurugi_backup::build::BackupArchive>,
    ) -> lanrurugi_backup::build::BackupDocument {
        lanrurugi_backup::build::BackupDocument {
            archives,
            categories: vec![],
            tankoubons: vec![],
            stamps: vec![],
            bookmarks: vec![],
            terminology_glossaries: vec![],
            volume_font_patterns: vec![],
            text_regions: vec![],
            share_node_secret: None,
        }
    }

    #[test]
    fn summarize_counts_archives_carrying_at_least_one_tag() {
        let doc = backup_doc_with(vec![
            backup_archive("a", "artist:x"),
            backup_archive("b", "   "),
            backup_archive("c", ""),
        ]);
        let summary = summarize(&doc);
        assert_eq!(summary.archives, 3);
        assert_eq!(
            summary.archives_with_tags, 1,
            "blank and empty tag strings are not tags"
        );
        assert_eq!(summary.bytes, 0, "filled in by `take_export` from the file");
        assert_eq!(
            (
                summary.categories,
                summary.tankoubons,
                summary.stamps,
                summary.bookmarks
            ),
            (0, 0, 0, 0)
        );
    }

    #[tokio::test]
    async fn a_summary_round_trips_beside_its_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let name = "20261123T030000Z-daily.json";
        write(tmp.path(), name, "{\"archives\":[],\"categories\":[]}");
        let summary = SnapshotSummary {
            archives: 384,
            archives_with_tags: 360,
            categories: 12,
            tankoubons: 5,
            stamps: 3,
            bookmarks: 8,
            bytes: 412_345,
        };
        write_summary_file(tmp.path(), name, &summary)
            .await
            .unwrap();

        // The `...summary.json` sibling is never listed as a snapshot of its own.
        let entries = list_in(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].summary.as_ref(), Some(&summary));
        // The API's own `entry_json` carries it (and `null` rather than omitting the key).
        assert_eq!(entry_json(&entries[0])["summary"]["archives"], json!(384));

        // A missing summary is `null`; so is one that does not parse. Neither is an error.
        let other = "20261124T030000Z-daily.json";
        write(tmp.path(), other, "{\"archives\":[],\"categories\":[]}");
        let entries = list_in(tmp.path()).unwrap();
        let by_name = |n: &str| entries.iter().find(|entry| entry.name == n).unwrap();
        assert!(by_name(other).summary.is_none());
        assert_eq!(
            entry_json(by_name(other))["summary"],
            serde_json::Value::Null
        );

        write(tmp.path(), &format!("{other}.summary.json"), "not json");
        let entries = list_in(tmp.path()).unwrap();
        assert!(entries
            .iter()
            .find(|e| e.name == other)
            .unwrap()
            .summary
            .is_none());

        // Removing a snapshot takes its summary with it; a missing summary is not an error.
        remove_snapshot_files(tmp.path(), name).await.unwrap();
        assert!(!tmp.path().join(name).exists());
        assert!(!tmp.path().join(format!("{name}.summary.json")).exists());
        remove_snapshot_files(tmp.path(), other).await.unwrap();
    }
}
