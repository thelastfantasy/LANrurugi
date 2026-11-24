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
use std::sync::OnceLock;

use std::io::Write;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{DateTime, Datelike, Local, Offset, SecondsFormat, TimeZone, Utc};
use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::settings::{
    read_snapshot_cron, read_snapshot_cron_preset, read_snapshot_daily_keep,
    read_snapshot_schedule_enabled, read_snapshot_schedules, read_snapshot_week_keep,
    read_timezone,
};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::{BACKUP_NOTES_KEY, SNAPSHOT_PINS_KEY, SNAPSHOT_SCHEDULE_KEY};
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
    /// `daily` / `weekly` / `manual` / `pre-*` for an export, or `scheduled` / `manual` for a full
    /// Redis backup.
    pub reason: String,
    /// The schedule that wrote it, so the list can group by schedule. `None` for a manual snapshot,
    /// a `pre-*` export, and the single-schedule files older builds wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    /// That schedule's configured name, resolved against the settings when the list is served;
    /// `None` when the schedule has since been deleted (the snapshot still lists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_label: Option<String>,
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
    /// The user's free-text note for this file, filled in from Redis like `pinned`. `None` when no
    /// note is set; the JSON omits the field rather than emitting `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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

/// One scheduled snapshot, as configured right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSchedule {
    /// Filename-safe slug, carried into the snapshot's own name so the list can group by it.
    pub id: String,
    /// Operator-facing name; empty means the UI supplies a fallback.
    pub label: String,
    pub enabled: bool,
    pub cron: CronSchedule,
    /// The zone the expression is evaluated in, from the app's `timezone` setting (never the
    /// process's own `TZ`: a container running in UTC would otherwise fire a "03:00" schedule at
    /// local noon for a JST operator).
    pub tz: chrono_tz::Tz,
    /// The expression as stored, so the list response can echo exactly what was configured (even
    /// when it is the fallback, which the server-side warning then explains).
    pub cron_text: String,
    /// The settings UI's preset echo (`hourly` / `daily` / `weekly` / `monthly` /
    /// `monthly-first-weekday` / `custom`).
    pub preset: String,
}

/// The id the migrated single-schedule configuration gets: an untouched deployment that upgrades
/// into this build keeps exactly one schedule, under a stable name.
pub const LEGACY_SCHEDULE_ID: &str = "plan-1";

impl SnapshotSchedule {
    /// Every configured schedule, newest configuration wins over the legacy single one. Never
    /// empty: a deployment that has never saved the array gets one schedule built from the three
    /// legacy keys, which is what it was running before this field existed.
    pub async fn load_all(state: &AppState) -> Vec<Self> {
        let tz = read_timezone(state).await;
        let configured = parse_schedules(&read_snapshot_schedules(state).await, tz);
        if !configured.is_empty() {
            return configured;
        }
        vec![Self {
            id: LEGACY_SCHEDULE_ID.to_string(),
            label: String::new(),
            enabled: read_snapshot_schedule_enabled(state).await,
            cron: parse_cron_or_default(&read_snapshot_cron(state).await),
            cron_text: read_snapshot_cron(state).await,
            preset: read_snapshot_cron_preset(state).await,
            tz,
        }]
    }

    /// The next firings, or none at all while the schedule is off — a disabled schedule has no next
    /// occurrence, and reporting one would suggest a snapshot is coming.
    pub fn next_runs(
        &self,
        now: DateTime<chrono_tz::Tz>,
        count: usize,
    ) -> Vec<DateTime<chrono_tz::Tz>> {
        if !self.enabled {
            return Vec::new();
        }
        self.cron.next_runs(now, count)
    }

    /// The reason a snapshot this schedule writes carries: `scheduled-<id>`, so every snapshot
    /// names the schedule that wrote it and the list can group by it. Files written before
    /// multiple schedules existed say just `scheduled` and stay unattributed.
    pub fn reason(&self) -> String {
        format!("scheduled-{}", self.id)
    }
}

/// Parses the stored JSON array. Pure, so "a broken entry is skipped, not fatal" is testable.
fn parse_schedules(text: &str, tz: chrono_tz::Tz) -> Vec<SnapshotSchedule> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let items: Vec<crate::settings::SnapshotScheduleSetting> = match serde_json::from_str(text) {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "stored snapshot schedules do not parse; using the default");
            return Vec::new();
        }
    };
    items
        .into_iter()
        .filter_map(|item| {
            let cron = item.cron.parse::<CronSchedule>().ok().or_else(|| {
                tracing::warn!(cron = %item.cron, "stored snapshot cron does not parse; skipped");
                None
            })?;
            Some(SnapshotSchedule {
                id: item.id,
                label: item.label,
                enabled: item.enabled,
                cron,
                tz,
                cron_text: item.cron,
                preset: item.preset,
            })
        })
        .collect()
}

fn parse_cron_or_default(text: &str) -> CronSchedule {
    // Only a hand-edited Redis can hold something unparseable — `PUT /settings` validates it.
    // Falling back to the default beats silently switching the schedule off.
    text.parse::<CronSchedule>().unwrap_or_else(|e| {
        tracing::warn!(
            cron = %text,
            error = %e,
            "stored snapshot cron does not parse; falling back to the default"
        );
        CronSchedule::default()
    })
}

/// The Redis field remembering the last instant served, per schedule id (`last_cron_fire:<id>`).
/// The plain `last_cron_fire` field is the single-schedule build's, still read for the legacy id.
const LAST_FIRE_FIELD: &str = "last_cron_fire";

/// How many upcoming firings `GET /api/backups` and the preview endpoint report.
pub const NEXT_RUNS: usize = 10;

/// Per schedule id, the instant whose automatic attempt already failed in this process, so a full
/// disk does not retry — and keep warning — every 5 seconds until the next firing (`research.md`
/// D9). The next firing has a different timestamp, which is what re-arms it.
static FAILED_CRON_SLOTS: OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, i64>>> =
    OnceLock::new();

fn failed_cron_slots() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, i64>> {
    FAILED_CRON_SLOTS.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

fn failed_slot(id: &str) -> i64 {
    failed_cron_slots()
        .lock()
        .map(|m| *m.get(id).unwrap_or(&0))
        .unwrap_or(0)
}

fn set_failed_slot(id: &str, instant: i64) {
    if let Ok(mut slots) = failed_cron_slots().lock() {
        if instant == 0 {
            slots.remove(id);
        } else {
            slots.insert(id.to_string(), instant);
        }
    }
}

fn last_fire_field(id: &str) -> String {
    format!("{LAST_FIRE_FIELD}:{id}")
}

/// A schedule's stored watermark. "The store could not be read" is deliberately its own state:
/// collapsing it into [`Watermark::Never`] makes an unreachable Redis look like a fresh install,
/// which fires — and warns — on every tick for as long as the outage lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watermark {
    /// No firing has ever been recorded: one is owed immediately.
    Never,
    /// The instant of the last served firing.
    At(DateTime<Utc>),
    /// Redis refused the read; this tick must not act on a guess.
    Unreadable,
}

impl Watermark {
    fn at(self) -> Option<DateTime<Utc>> {
        match self {
            Watermark::At(at) => Some(at),
            Watermark::Never | Watermark::Unreadable => None,
        }
    }
}

async fn read_last_fire(state: &AppState, id: &str) -> Watermark {
    let Ok(mut conn) = state.redis.config.get().await else {
        return Watermark::Unreadable;
    };
    let mut raw: Option<String> = match conn.hget(SNAPSHOT_SCHEDULE_KEY, last_fire_field(id)).await
    {
        Ok(raw) => raw,
        Err(_) => return Watermark::Unreadable,
    };
    if raw.is_none() && id == LEGACY_SCHEDULE_ID {
        // A build from before multiple schedules wrote the un-suffixed field.
        raw = match conn.hget(SNAPSHOT_SCHEDULE_KEY, LAST_FIRE_FIELD).await {
            Ok(raw) => raw,
            Err(_) => return Watermark::Unreadable,
        };
    }
    let Some(raw) = raw else {
        return Watermark::Never;
    };
    // A value that does not parse is treated as "never fired" (today's behaviour), not as an
    // unreadable store: one corrupt field must not freeze the schedule forever.
    DateTime::parse_from_rfc3339(&raw)
        .map(|at| Watermark::At(at.with_timezone(&Utc)))
        .unwrap_or(Watermark::Never)
}

pub async fn read_schedule_state(state: &AppState) -> Option<DateTime<Utc>> {
    let mut latest: Option<DateTime<Utc>> = None;
    for schedule in SnapshotSchedule::load_all(state).await {
        if let Some(at) = read_last_fire(state, &schedule.id).await.at() {
            latest = Some(latest.map_or(at, |current| current.max(at)));
        }
    }
    latest
}

/// Returns `false` when Redis refused the write, so the caller can stop re-running the same firing.
async fn record_last_fire(state: &AppState, id: &str, at: DateTime<Utc>) -> bool {
    let Ok(mut conn) = state.redis.config.get().await else {
        return false;
    };
    let stored = at.to_rfc3339_opts(SecondsFormat::Secs, true);
    let result: Result<i64, _> = conn
        .hset(SNAPSHOT_SCHEDULE_KEY, last_fire_field(id), stored)
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
    tz: chrono_tz::Tz,
    now: DateTime<chrono_tz::Tz>,
    last_fire: Watermark,
) -> Option<DateTime<chrono_tz::Tz>> {
    match last_fire {
        // The watermark is stored in UTC; the comparison happens in the configured zone, which is
        // the same instant either way.
        Watermark::At(last) => cron
            .next_after(last.with_timezone(&tz))
            .filter(|at| *at <= now),
        Watermark::Never => Some(now),
        // Nothing is owed on a guess. The caller also skips before calling this, so the rule is
        // stated in both places on purpose: it is the one that stops the alert storm.
        Watermark::Unreadable => None,
    }
}

/// The decision + watermark half of a tick, with the snapshot write injected. Every enabled
/// schedule is evaluated independently: each has its own watermark, its own failed-slot memory and
/// its own files, so a broken one never silences the others.
///
/// Split out so the scheduling policy (cron hit, restart-no-duplicate, catch-up-once, failed-firing
/// memory) can be tested without a real Redis RDB — the one environment that can read Redis's RDB
/// is not the one the test suite runs in.
pub(crate) async fn tick_with<F>(state: &AppState, now: DateTime<chrono_tz::Tz>, run: F) -> usize
where
    F: Fn(
        &SnapshotSchedule,
        DateTime<chrono_tz::Tz>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SnapshotEntry, String>> + Send>,
    >,
{
    let mut fired = 0usize;
    for schedule in SnapshotSchedule::load_all(state).await {
        if !schedule.enabled {
            continue;
        }
        let last_fire = read_last_fire(state, &schedule.id).await;
        if last_fire == Watermark::Unreadable {
            // An unreadable watermark is not "never fired": treating it as such is what made an
            // unreachable Redis fire — and warn — on every 5 s tick. Skip this schedule for this
            // tick without logging; the read failure is already reported by the Redis layer.
            continue;
        }
        let Some(slot) = due_firing(&schedule.cron, schedule.tz, now, last_fire) else {
            tracing::debug!(
                schedule = %schedule.id,
                cron = %schedule.cron_text,
                last_fire = ?last_fire,
                "snapshot schedule tick: nothing due"
            );
            continue;
        };
        let instant = slot.timestamp();
        // A firing whose automatic attempt already failed in this process is not retried every
        // tick; the next firing has a different timestamp, which re-arms it.
        if failed_slot(&schedule.id) == instant {
            continue;
        }
        tracing::info!(
            schedule = %schedule.id,
            cron = %schedule.cron_text,
            slot = %slot.to_rfc3339(),
            first = last_fire == Watermark::Never,
            "snapshot schedule tick: firing"
        );
        match run(&schedule, slot).await {
            Ok(entry) => {
                // The *serve* time, not the slot: recording the slot would leave every firing
                // missed during a long downtime still ahead of the record, and the schedule would
                // fire once per missed firing instead of once in total.
                if record_last_fire(state, &schedule.id, Utc::now()).await {
                    set_failed_slot(&schedule.id, 0);
                } else {
                    set_failed_slot(&schedule.id, instant);
                }
                tracing::info!(
                    snapshot = %entry.name,
                    schedule = %schedule.id,
                    "scheduled automatic snapshot written"
                );
                fired += 1;
            }
            Err(e) => {
                set_failed_slot(&schedule.id, instant);
                tracing::warn!(
                    schedule = %schedule.id,
                    error = %e,
                    "scheduled automatic snapshot failed; not retrying this firing"
                );
            }
        }
    }
    fired
}

/// One scheduler tick. `lanrurugi-server` calls this every 5 seconds, and an interval's first tick
/// is immediate — which is also the first firing a fresh install takes.
pub async fn schedule_tick(state: &AppState) {
    let state_for_task = state.clone();
    // Wall-clock now in the operator's zone; the cron expression is evaluated there.
    let tz = read_timezone(state).await;
    tick_with(
        state,
        Utc::now().with_timezone(&tz),
        move |schedule, _slot| {
            let reason = schedule.reason();
            let state_for_task = state_for_task.clone();
            Box::pin(async move {
                let entry =
                    take_full_backup(&state_for_task, &reason, Arc::new(NoProgress)).await?;
                // A fresh snapshot can immediately push the oldest one past `FULL_BACKUP_KEEP`.
                retention_sweep(&state_for_task).await;
                Ok(entry)
            })
        },
    )
    .await;
}

/// One instant as the API reports it: ISO 8601 with the configured zone's UTC offset, so a client
/// never has to guess which zone a `next_runs` entry is in (the `timezone` field names it too).
fn iso_local(at: DateTime<chrono_tz::Tz>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// The configured zone for display: `Asia/Tokyo (+09:00)`. The offset is the zone's current one,
/// so a DST-observing zone shows the offset in force right now (`-05:00` vs `-04:00`).
pub fn timezone_label(tz: chrono_tz::Tz) -> String {
    // `TzOffset`'s own `Display` is the abbreviation (`JST`); the numeric offset is what the UI
    // and the contract promise.
    let offset = Utc::now().with_timezone(&tz).offset().fix().to_string();
    format!("{tz} ({offset})")
}

/// The `schedule` object on `GET /api/backups`: the zone every cron is read in, plus one entry per
/// configured schedule. Pure, so "a disabled schedule reports no next runs" and the reported shape
/// have a unit test.
fn schedule_json(
    schedules: &[SnapshotSchedule],
    last_fires: &std::collections::BTreeMap<String, Option<DateTime<Utc>>>,
    now: DateTime<chrono_tz::Tz>,
    tz: chrono_tz::Tz,
) -> serde_json::Value {
    let items: Vec<serde_json::Value> = schedules
        .iter()
        .map(|schedule| {
            let last_fire = last_fires.get(&schedule.id).copied().flatten();
            json!({
                "id": schedule.id,
                "label": schedule.label,
                "enabled": schedule.enabled,
                "cron": schedule.cron_text,
                "preset": schedule.preset,
                "last_fire_at": last_fire
                    .map(|at| iso_local(at.with_timezone(&schedule.tz))),
                "next_runs": schedule
                    .next_runs(now.with_timezone(&schedule.tz), NEXT_RUNS)
                    .into_iter()
                    .map(iso_local)
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "timezone": timezone_label(tz),
        // `timezone` carries the human label (`Asia/Tokyo (+09:00)`), which is not an id any date
        // library accepts; `Intl.DateTimeFormat` needs this one.
        "timezone_name": tz.name(),
        "items": items,
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

// ---------------------------------------------------------------------------------------------
// Notes.
//
// A note is the user's "why this snapshot exists" — an alias no other column can carry. Like a
// pin it lives in Redis rather than in a sidecar file, so the backups directory stays plain
// snapshots and a file moved in by hand still lists; unlike a pin it affects neither retention nor
// restores. Clearing it whenever the file goes keeps the hash from growing without bound.
// ---------------------------------------------------------------------------------------------

/// Every note, or `None` when Redis could not be read. The read-only list treats that as "no
/// notes": it is still correct about everything else and deletes nothing.
async fn read_notes(state: &AppState) -> Option<std::collections::HashMap<String, String>> {
    let mut conn = state.redis.config.get().await.ok()?;
    let notes: std::collections::HashMap<String, String> =
        conn.hgetall(BACKUP_NOTES_KEY).await.ok()?;
    Some(notes)
}

/// Sets (`Some`) or clears (`None`) one note. `HSET`/`HDEL` are already idempotent, so repeating
/// either is harmless.
async fn set_note(state: &AppState, name: &str, note: Option<&str>) -> Result<(), String> {
    let mut conn = state
        .redis
        .config
        .get()
        .await
        .map_err(|e| format!("could not reach Redis: {e}"))?;
    let result: Result<(), _> = match note {
        Some(text) => conn.hset(BACKUP_NOTES_KEY, name, text).await,
        None => conn.hdel(BACKUP_NOTES_KEY, name).await,
    };
    result.map_err(|e| format!("could not update the snapshot note: {e}"))
}

/// One parsed backup-directory file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedName {
    pub created: DateTime<Utc>,
    pub kind: SnapshotKind,
    /// `daily` / `weekly` / `manual` / `pre-*` for an export; `scheduled` / `daily` / `weekly` /
    /// `manual` for a full Redis backup.
    pub reason: String,
    /// The schedule that wrote it (`scheduled-<id>` in the file name), or `None` for a manual one
    /// and for the single `scheduled` files an earlier build wrote.
    pub schedule_id: Option<String>,
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
    if matches!(reason, "scheduled" | "manual" | "daily" | "weekly") {
        return true;
    }
    // `scheduled-<id>`: which schedule fired (see `SnapshotSchedule::reason`).
    reason.strip_prefix("scheduled-").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 32
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
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
    // `scheduled-<id>` names the schedule; everything else has no schedule of its own.
    let (reason, schedule_id) = match reason.strip_prefix("scheduled-") {
        Some(id) => ("scheduled".to_string(), Some(id.to_string())),
        None => (reason.to_string(), None),
    };
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ").ok()?;
    Some(ParsedName {
        created: Utc.from_utc_datetime(&naive),
        kind,
        reason,
        schedule_id,
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
            schedule_id: parsed.schedule_id,
            schedule_label: None,
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
            note: None,
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
    let mut backup_seen: std::collections::BTreeMap<Option<String>, usize> =
        std::collections::BTreeMap::new();
    for entry in &sorted {
        let Some(parsed) = parse_name(&entry.name) else {
            continue;
        };
        // Pinned: never a victim, and skipped before any tier bookkeeping so it does not push an
        // unpinned file out of the tier's budget either.
        if pins.contains(&entry.name) {
            continue;
        }
        // Full Redis backups keep `FULL_BACKUP_KEEP` each *per schedule*, so adding a second
        // schedule does not evict the first one's history — and the manual ones (`None`) form one
        // bucket of their own.
        if entry.kind == SnapshotKind::Backup {
            let count = backup_seen.entry(entry.schedule_id.clone()).or_insert(0);
            if *count < FULL_BACKUP_KEEP {
                *count += 1;
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
        schedule_id: None,
        schedule_label: None,
        size_bytes: size,
        usable: true,
        summary,
        pinned: false,
        note: None,
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
    if let Err(e) = deadpool_redis::redis::cmd("BGSAVE")
        .query_async::<String>(&mut conn)
        .await
    {
        // A refusal (MISCONF, disk full) is also why no save will ever finish, so without this the
        // only symptom is a misleading "did not finish in 120s" two minutes later.
        tracing::warn!(error = %e, "BGSAVE was refused");
    }
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

    // `scheduled-<id>` in the file name, `scheduled` in the entry: the id is what the list groups
    // by, and it has to survive a file moved between installs.
    let schedule_id = reason.strip_prefix("scheduled-").map(|id| id.to_string());
    Ok(SnapshotEntry {
        name,
        kind: SnapshotKind::Backup,
        created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
        reason: reason.split('-').next().unwrap_or(reason).to_string(),
        schedule_id,
        schedule_label: None,
        size_bytes: size,
        usable: true,
        summary: None,
        pinned: false,
        note: None,
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
            Ok(()) => {
                removed += 1;
                // The file is gone, so its note can never be shown again; dropping it here is what
                // keeps the hash bounded. A failure is a warning — the sweep's job is the file.
                if let Err(e) = set_note(state, &name, None).await {
                    tracing::warn!(name, error = %e, "could not clear a pruned snapshot's note");
                }
            }
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
            get(download_backup)
                .delete(delete_backup)
                .patch(update_note),
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
async fn preview_schedule(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CronPreviewRequest>,
) -> Response {
    // Preview in the same zone the schedule will actually run in.
    let tz = read_timezone(&state).await;
    let now = Utc::now().with_timezone(&tz);
    match body.cron.parse::<CronSchedule>() {
        Ok(cron) => axum::Json(json!({
            "valid": true,
            "timezone": timezone_label(tz),
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
            "timezone": timezone_label(tz),
            "next_runs": [],
        }))
        .into_response(),
    }
}

fn entry_json(entry: &SnapshotEntry) -> serde_json::Value {
    let mut value = json!({
        "name": entry.name,
        // "backup" (full Redis RDB) or "export" (JSON document). The one field a client must branch
        // on: only an export can be restored from inside the app.
        "kind": entry.kind,
        "created_at": entry.created_at,
        "reason": entry.reason,
        // Which schedule wrote it, and that schedule's current name — `null` for a manual
        // snapshot, a `pre-*` export, and a snapshot whose schedule has since been deleted (the
        // client groups those as unattributed).
        "schedule_id": entry.schedule_id,
        "schedule_label": entry.schedule_label,
        "size_bytes": entry.size_bytes,
        "usable": entry.usable,
        // `null` (never omitted) when the sibling summary is missing, so a client can rely on the
        // key existing. Always null for a full backup, which has no JSON summary.
        "summary": entry.summary.as_ref(),
        // Always present, so a client never has to treat "missing" and "not pinned" differently.
        "pinned": entry.pinned,
    });
    // Omitted when empty (like `schedule_id`), so "no note" has one representation rather than
    // `null` and `""` both standing for it.
    if let Some(note) = &entry.note {
        value["note"] = json!(note);
    }
    value
}

/// Reads the backup directory and fills in the state that lives in Redis (pin, note). The list and
/// the note endpoint share this so a client never sees the two disagree about an entry's shape.
async fn load_entries(state: &AppState) -> std::io::Result<Vec<SnapshotEntry>> {
    let dir = snapshot_dir(&state.library.archive_dir);
    let mut entries = list_in(&dir)?;
    // A failed read means "no pins/notes shown" rather than a 500: the list is still correct about
    // everything else, and nothing is deleted here.
    let pins = read_pins(state).await.unwrap_or_default();
    let notes = read_notes(state).await.unwrap_or_default();
    for entry in &mut entries {
        entry.pinned = pins.contains(&entry.name);
        entry.note = notes
            .get(&entry.name)
            .filter(|note| !note.is_empty())
            .cloned();
    }
    Ok(entries)
}

async fn list_backups(State(state): State<AppState>) -> Response {
    match load_entries(&state).await {
        Ok(mut entries) => {
            let schedules = SnapshotSchedule::load_all(&state).await;
            let tz = read_timezone(&state).await;
            let mut last_fires = std::collections::BTreeMap::new();
            let labels: std::collections::BTreeMap<String, String> = schedules
                .iter()
                .map(|schedule| (schedule.id.clone(), schedule.label.clone()))
                .collect();
            for schedule in &schedules {
                last_fires.insert(
                    schedule.id.clone(),
                    read_last_fire(&state, &schedule.id).await.at(),
                );
            }
            // A snapshot whose schedule no longer exists keeps listing, with no label: the UI
            // collects those (and manual ones) into its unattributed group rather than hiding them.
            for entry in &mut entries {
                entry.schedule_label = entry
                    .schedule_id
                    .as_ref()
                    .and_then(|id| labels.get(id).cloned());
            }
            axum::Json(json!({
                "operation": "list_backups",
                "success": 1,
                "daily_keep": read_snapshot_daily_keep(&state).await,
                "week_keep": read_snapshot_week_keep(&state).await,
                "schedule": schedule_json(&schedules, &last_fires, Utc::now().with_timezone(&tz), tz),
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
    let safety_name = safety.name.clone();
    let (job, progress_job) = crate::database::start_restore_job(
        &state,
        doc,
        auth.as_ref().map(|e| e.0.clone()),
        safety,
        crate::database::RestoreActivity {
            action_type: action_types::BACKUP_RESTORE,
            target: ActivityTarget {
                id: None,
                label: Some(name.clone()),
                kind: Some("backup".to_string()),
            },
        },
    )
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
    if let Err(e) = set_note(&state, &name, None).await {
        tracing::warn!(name, error = %e, "could not clear the deleted snapshot's note");
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

/// Longest note accepted. Short enough that a client can render it on one row next to the file
/// name, which is what a note is for; the reason field already carries the machine-readable why.
const NOTE_MAX_CHARS: usize = 200;

#[derive(Debug, Deserialize)]
struct NoteRequest {
    /// `null` (or an absent field) clears the note; so does an all-whitespace string.
    note: Option<String>,
}

/// Validates and normalizes one note: surrounding whitespace is not part of it, empty means
/// "cleared", and control characters are refused — they corrupt both the UI row and the log line.
fn normalize_note(raw: Option<&str>) -> Result<Option<String>, String> {
    let text = raw.unwrap_or_default().trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > NOTE_MAX_CHARS {
        return Err(format!(
            "A note cannot be longer than {NOTE_MAX_CHARS} characters."
        ));
    }
    if text.chars().any(char::is_control) {
        return Err("A note cannot contain control characters.".to_string());
    }
    Ok(Some(text.to_string()))
}

/// `PATCH /api/backups/{name}` — sets or clears the free-text note on one snapshot or export. The
/// note is a label for humans, nothing more: it never affects retention, a restore, or `usable`.
async fn update_note(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    axum::Json(body): axum::Json<NoteRequest>,
) -> Response {
    let Some(path) = resolve_name(&state, &name) else {
        return error(
            StatusCode::BAD_REQUEST,
            "update_note",
            "Invalid backup name.",
        );
    };
    if !path.is_file() {
        return not_found("update_note", format!("Backup {name} does not exist."));
    }
    let note = match normalize_note(body.note.as_deref()) {
        Ok(note) => note,
        Err(message) => return error(StatusCode::BAD_REQUEST, "update_note", message),
    };
    if let Err(e) = set_note(&state, &name, note.as_deref()).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "update_note", e);
    }
    // Answer with the entry exactly as the list renders it, so a client can drop the response into
    // its cached list instead of re-fetching the whole thing for one changed field.
    let entries = match load_entries(&state).await {
        Ok(entries) => entries,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_note",
                format!("could not read the backup directory: {e}"),
            )
        }
    };
    match entries.into_iter().find(|entry| entry.name == name) {
        Some(entry) => axum::Json(entry_json(&entry)).into_response(),
        None => not_found("update_note", format!("Backup {name} does not exist.")),
    }
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
            schedule_id: parsed.schedule_id,
            schedule_label: None,
            size_bytes: 10,
            usable,
            summary: None,
            pinned: false,
            note: None,
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

    /// The note hash field for `name`, read straight from Redis so a test can tell "cleared" from
    /// "cleared in the response but left in the hash".
    async fn stored_note(state: &AppState, name: &str) -> Option<String> {
        let mut conn = state.redis.config.get().await.unwrap();
        conn.hget::<_, _, Option<String>>(BACKUP_NOTES_KEY, name)
            .await
            .unwrap()
    }

    /// A note is a human alias, and both kinds carry one: a backup and an export are the same
    /// entry shape everywhere else, so they must not differ here.
    #[tokio::test]
    async fn a_note_round_trips_through_the_list() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let backup = "snapshot-20261121T030000Z-manual.rdb";
        let export = "export-20261121T040000Z-daily.json";
        write(&dir, backup, "REDIS0011\u{0}\u{0}\u{0}");
        write(&dir, export, r#"{"archives":[],"categories":[]}"#);
        // An earlier failed run of this test may have left its note behind.
        set_note(&state, backup, None).await.unwrap();
        set_note(&state, export, None).await.unwrap();

        let response = update_note(
            State(state.clone()),
            UrlPath(backup.to_string()),
            axum::Json(NoteRequest {
                // Surrounding whitespace is not part of the note.
                note: Some("  before the reindex run  ".to_string()),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["name"], json!(backup));
        assert_eq!(body["note"], json!("before the reindex run"));
        assert_eq!(
            listed_entry(&state, backup).await["note"],
            json!("before the reindex run")
        );

        let exported = update_note(
            State(state.clone()),
            UrlPath(export.to_string()),
            axum::Json(NoteRequest {
                note: Some("kept for the migration".to_string()),
            }),
        )
        .await;
        assert_eq!(exported.status(), StatusCode::OK);
        assert_eq!(
            listed_entry(&state, export).await["note"],
            json!("kept for the migration")
        );

        set_note(&state, backup, None).await.unwrap();
        set_note(&state, export, None).await.unwrap();
    }

    /// Clearing is `""` (or whitespace) or `null`, and either way the field disappears from the
    /// list rather than turning into `"note": null`.
    #[tokio::test]
    async fn clearing_a_note_removes_the_field() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let name = "snapshot-20261120T030000Z-manual.rdb";
        write(&dir, name, "REDIS0011\u{0}\u{0}\u{0}");
        set_note(&state, name, None).await.unwrap();

        let set = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some("temporary".to_string()),
            }),
        )
        .await;
        assert_eq!(set.status(), StatusCode::OK);
        assert_eq!(
            stored_note(&state, name).await.as_deref(),
            Some("temporary")
        );

        let blank = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some("   ".to_string()),
            }),
        )
        .await;
        assert_eq!(blank.status(), StatusCode::OK);
        assert!(response_json(blank).await.get("note").is_none());
        assert!(stored_note(&state, name).await.is_none());
        assert!(listed_entry(&state, name).await.get("note").is_none());

        // `null` clears the same way.
        update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some("again".to_string()),
            }),
        )
        .await;
        let nulled = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest { note: None }),
        )
        .await;
        assert_eq!(nulled.status(), StatusCode::OK);
        assert!(response_json(nulled).await.get("note").is_none());
        assert!(stored_note(&state, name).await.is_none());
    }

    /// The limit is 200 **characters**, not bytes: 200 two-byte characters are accepted, 201 are
    /// not, and a rejected note is never stored. Control characters are refused outright.
    #[tokio::test]
    async fn a_note_is_limited_to_two_hundred_characters() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let name = "snapshot-20261119T030000Z-manual.rdb";
        write(&dir, name, "REDIS0011\u{0}\u{0}\u{0}");
        set_note(&state, name, None).await.unwrap();

        let rejected = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some("é".repeat(201)),
            }),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let body = response_json(rejected).await;
        assert!(
            body["error"].as_str().unwrap_or_default().contains("200"),
            "the message names the limit: {body}"
        );
        assert!(stored_note(&state, name).await.is_none());

        let accepted_note = "é".repeat(200);
        let accepted = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some(accepted_note.clone()),
            }),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::OK);
        assert_eq!(response_json(accepted).await["note"], json!(accepted_note));

        let control = update_note(
            State(state.clone()),
            UrlPath(name.to_string()),
            axum::Json(NoteRequest {
                note: Some("two\nlines".to_string()),
            }),
        )
        .await;
        assert_eq!(control.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            listed_entry(&state, name).await["note"],
            json!(accepted_note),
            "a rejected note leaves the stored one alone"
        );

        set_note(&state, name, None).await.unwrap();
    }

    /// The note goes with the file: a deleted snapshot's note must not linger in the hash.
    #[tokio::test]
    async fn deleting_a_snapshot_removes_its_note() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let older = "snapshot-20261118T030000Z-manual.rdb";
        let newer = "snapshot-20261118T040000Z-manual.rdb";
        write(&dir, older, "REDIS0011\u{0}\u{0}\u{0}");
        write(&dir, newer, "REDIS0011\u{0}\u{0}\u{0}");
        set_note(&state, older, Some("about to be deleted"))
            .await
            .unwrap();

        let deleted = delete_backup(State(state.clone()), None, UrlPath(older.to_string())).await;
        assert_eq!(deleted.status(), StatusCode::OK);
        assert!(!dir.join(older).exists());
        assert!(stored_note(&state, older).await.is_none());

        // A name that no longer exists cannot carry a note.
        let gone = update_note(
            State(state.clone()),
            UrlPath(older.to_string()),
            axum::Json(NoteRequest {
                note: Some("too late".to_string()),
            }),
        )
        .await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    /// Retention pruning drops the note of what it pruned, so the hash cannot grow with every
    /// evicted snapshot.
    #[tokio::test]
    async fn retention_pruning_removes_the_note() {
        use lanrurugi_storage::keys::CONFIG_KEY;

        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let older = "export-20260101T000000Z-daily.json";
        let newer = "export-20260102T000000Z-daily.json";
        write(&dir, older, "{}");
        write(&dir, newer, "{}");
        set_note(&state, older, Some("pruned soon")).await.unwrap();
        set_note(&state, newer, Some("kept")).await.unwrap();

        // Keep one daily and no weekly, so the older daily is over its tier's budget. The previous
        // values are read first and restored before any assertion, so a failure here cannot leak
        // these keep counts into another test sharing this Redis.
        let mut conn = state.redis.config.get().await.unwrap();
        let old_daily: Option<String> = conn.hget(CONFIG_KEY, "snapshot_daily_keep").await.unwrap();
        let old_week: Option<String> = conn.hget(CONFIG_KEY, "snapshot_week_keep").await.unwrap();
        let _: () = conn
            .hset(CONFIG_KEY, "snapshot_daily_keep", "1")
            .await
            .unwrap();
        let _: () = conn
            .hset(CONFIG_KEY, "snapshot_week_keep", "0")
            .await
            .unwrap();
        drop(conn);

        let removed = retention_sweep(&state).await;

        let mut conn = state.redis.config.get().await.unwrap();
        for (field, previous) in [
            ("snapshot_daily_keep", old_daily),
            ("snapshot_week_keep", old_week),
        ] {
            match previous {
                Some(value) => {
                    let _: () = conn.hset(CONFIG_KEY, field, value).await.unwrap();
                }
                None => {
                    let _: () = conn.hdel(CONFIG_KEY, field).await.unwrap();
                }
            }
        }
        drop(conn);

        assert_eq!(removed, 1, "only the older daily is over budget");
        assert!(!dir.join(older).exists());
        assert!(stored_note(&state, older).await.is_none());
        assert_eq!(stored_note(&state, newer).await.as_deref(), Some("kept"));
        set_note(&state, newer, None).await.unwrap();
    }

    /// A note is an annotation only: it must not change `usable`, and it must not act as a pin in
    /// either direction.
    #[tokio::test]
    async fn a_note_neither_pins_nor_unpins_a_snapshot() {
        let Some(mut state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        state.library.archive_dir = tmp.path().to_path_buf();
        let dir = snapshot_dir(tmp.path());
        let older = "snapshot-20261117T030000Z-manual.rdb";
        let newer = "snapshot-20261117T040000Z-manual.rdb";
        write(&dir, older, "REDIS0011\u{0}\u{0}\u{0}");
        write(&dir, newer, "REDIS0011\u{0}\u{0}\u{0}");
        set_pin(&state, older, false).await.unwrap();
        set_note(&state, older, None).await.unwrap();
        let before = listed_entry(&state, older).await;
        assert_eq!(before["pinned"], json!(false));

        update_note(
            State(state.clone()),
            UrlPath(older.to_string()),
            axum::Json(NoteRequest {
                note: Some("still just an old backup".to_string()),
            }),
        )
        .await;
        let after = listed_entry(&state, older).await;
        assert_eq!(after["usable"], before["usable"]);
        assert_eq!(after["pinned"], json!(false), "a note is not a pin");

        // A pin and a note are independent: pinning keeps the note, clearing the note keeps the pin.
        pin_backup(State(state.clone()), UrlPath(older.to_string())).await;
        let pinned = listed_entry(&state, older).await;
        assert_eq!(pinned["pinned"], json!(true));
        assert_eq!(pinned["note"], json!("still just an old backup"));
        assert_eq!(pinned["usable"], before["usable"]);

        update_note(
            State(state.clone()),
            UrlPath(older.to_string()),
            axum::Json(NoteRequest { note: None }),
        )
        .await;
        let cleared = listed_entry(&state, older).await;
        assert_eq!(
            cleared["pinned"],
            json!(true),
            "clearing a note keeps the pin"
        );
        assert!(cleared.get("note").is_none());

        unpin_backup(State(state.clone()), UrlPath(older.to_string())).await;
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

    /// An instant on the given zone's wall clock. Every schedule test names its zone explicitly —
    /// the whole point of the setting is that the process's own `TZ` does not decide this.
    fn at(
        tz: chrono_tz::Tz,
        date: &str,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<chrono_tz::Tz> {
        let date =
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("test date is well formed");
        date.and_hms_opt(hour, minute, second)
            .expect("test time is valid")
            .and_local_timezone(tz)
            .single()
            .expect("test instant exists in this timezone")
    }

    fn cron(expression: &str) -> CronSchedule {
        expression.parse().expect("test expression parses")
    }

    fn test_schedule(enabled: bool, expression: &str) -> SnapshotSchedule {
        test_schedule_in(chrono_tz::UTC, enabled, expression)
    }

    fn test_schedule_in(tz: chrono_tz::Tz, enabled: bool, expression: &str) -> SnapshotSchedule {
        SnapshotSchedule {
            id: LEGACY_SCHEDULE_ID.to_string(),
            label: String::new(),
            enabled,
            cron: cron(expression),
            tz,
            cron_text: expression.to_string(),
            preset: "custom".to_string(),
        }
    }

    /// `scheduled-<id>` names the schedule that wrote a snapshot; everything else has none. The
    /// id has to be a filename-safe slug, which is what the write-side validator enforces.
    #[test]
    fn a_scheduled_name_carries_its_schedule_id() {
        let parsed = parse_name("snapshot-20261124T004942Z-scheduled-plan-1.rdb").unwrap();
        assert_eq!(parsed.kind, SnapshotKind::Backup);
        assert_eq!(parsed.reason, "scheduled");
        assert_eq!(parsed.schedule_id.as_deref(), Some("plan-1"));
        // Files from the single-schedule build, and manual ones, belong to no schedule.
        assert_eq!(
            parse_name("snapshot-20261124T004942Z-scheduled.rdb")
                .unwrap()
                .schedule_id,
            None
        );
        assert_eq!(
            parse_name("snapshot-20261124T004942Z-manual.rdb")
                .unwrap()
                .schedule_id,
            None
        );
        // A slug with characters the validator rejects is not a snapshot name at all.
        assert!(parse_name("snapshot-20261124T004942Z-scheduled-Plan_1.rdb").is_none());
        assert!(parse_name("snapshot-20261124T004942Z-scheduled-.rdb").is_none());
    }

    /// One unparseable entry is skipped, not fatal: a hand-edited Redis (or a downgrade) must not
    /// silence every other schedule.
    #[test]
    fn a_broken_schedule_entry_is_skipped() {
        let text = r#"[
            {"id":"a","label":"A","enabled":true,"preset":"daily","cron":"0 0 3 * * *"},
            {"id":"b","label":"B","enabled":false,"preset":"custom","cron":"not a cron"},
            {"id":"c","label":"C","enabled":true,"preset":"weekly","cron":"0 0 3 * * 1"}
        ]"#;
        let parsed = parse_schedules(text, chrono_tz::UTC);
        assert_eq!(
            parsed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "c"]
        );
        // The broken middle entry is gone; the enabled flag survives for the ones that parsed.
        assert!(parsed[0].enabled);
        assert_eq!(parsed[0].reason(), "scheduled-a");
        // Every scheduled snapshot names its schedule, including the migrated single one.
        assert_eq!(
            test_schedule(true, "0 0 3 * * 1").reason(),
            "scheduled-plan-1"
        );
        // Nothing configured at all parses to no schedules, which makes `load_all` fall back.
        assert!(parse_schedules("", chrono_tz::UTC).is_empty());
        assert!(parse_schedules("}{", chrono_tz::UTC).is_empty());
    }

    /// The expression is wall-clock in the instance's own timezone, so the same text fires at a
    /// different instant after the timezone setting changes — that is the "schedules follow the
    /// timezone" behaviour, and it needs no rewrite of the stored cron.
    #[test]
    fn the_same_expression_moves_with_the_timezone_setting() {
        let text = r#"[{"id":"plan-1","label":"","enabled":true,"preset":"weekly","cron":"0 0 3 * * 0 *"}]"#;
        let tokyo = parse_schedules(text, chrono_tz::Asia::Tokyo);
        let los_angeles = parse_schedules(text, chrono_tz::America::Los_Angeles);
        let now_utc = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, 17, 12, 0, 0)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let first_in = |tz: chrono_tz::Tz| {
            tokyo_or(tz, text)
                .next_runs(now_utc.with_timezone(&tz), 1)
                .first()
                .map(|at| at.with_timezone(&chrono::Utc))
        };
        // 03:00 Sunday in Tokyo is 18:00Z on Saturday; the same wall clock in Los Angeles is
        // 11:00Z on Sunday. Same stored expression, different instants.
        assert_eq!(
            first_in(chrono_tz::Asia::Tokyo),
            Some(chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, 21, 18, 0, 0).unwrap())
        );
        assert_eq!(
            first_in(chrono_tz::America::Los_Angeles),
            Some(chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, 22, 11, 0, 0).unwrap())
        );
        // Both parsed one enabled plan, so the assertions above are about the timezone alone.
        assert!(tokyo.len() == 1 && los_angeles.len() == 1);
    }

    fn tokyo_or(tz: chrono_tz::Tz, text: &str) -> SnapshotSchedule {
        parse_schedules(text, tz).into_iter().next().unwrap()
    }

    /// Retention is per schedule: adding a second schedule must not evict the first one's history.
    #[test]
    fn retention_keeps_three_backups_per_schedule() {
        let entries = vec![
            entry("snapshot-20261123T030000Z-scheduled-a.rdb", true),
            entry("snapshot-20261122T030000Z-scheduled-a.rdb", true),
            entry("snapshot-20261121T030000Z-scheduled-a.rdb", true),
            entry("snapshot-20261120T030000Z-scheduled-a.rdb", true),
            entry("snapshot-20261119T030000Z-scheduled-b.rdb", true),
            entry("snapshot-20261118T030000Z-manual.rdb", true),
        ];
        assert_eq!(
            retention_plan(&entries, 7, 4, &no_pins()),
            vec!["snapshot-20261120T030000Z-scheduled-a.rdb".to_string()],
            "only the fourth snapshot of schedule a goes; b and the manual one are their own buckets"
        );
    }

    /// Two schedules are reported side by side, each with its own next runs.
    #[test]
    fn schedule_json_lists_every_schedule() {
        let mut daily = test_schedule(true, "0 0 3 * * *");
        daily.id = "daily".to_string();
        daily.label = "每天".to_string();
        let mut weekly = test_schedule(false, "0 0 3 * * 1");
        weekly.id = "weekly".to_string();
        let mut last_fires = std::collections::BTreeMap::new();
        last_fires.insert("daily".to_string(), None);
        last_fires.insert("weekly".to_string(), None);
        let now = at(chrono_tz::UTC, "2026-11-23", 5, 0, 0);
        let value = schedule_json(&[daily, weekly], &last_fires, now, chrono_tz::UTC);
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
        assert_eq!(value["items"][0]["id"], json!("daily"));
        assert_eq!(value["items"][0]["label"], json!("每天"));
        assert_eq!(
            value["items"][0]["next_runs"].as_array().unwrap().len(),
            NEXT_RUNS
        );
        // A disabled schedule reports no next runs but still lists.
        assert_eq!(value["items"][1]["id"], json!("weekly"));
        assert_eq!(value["items"][1]["next_runs"], json!([]));
    }

    /// A `schedule` block for one schedule and one watermark.
    fn schedule_block(
        schedule: &SnapshotSchedule,
        last_fire: Option<DateTime<Utc>>,
        now: DateTime<chrono_tz::Tz>,
    ) -> serde_json::Value {
        let mut last_fires = std::collections::BTreeMap::new();
        last_fires.insert(schedule.id.clone(), last_fire);
        schedule_json(
            std::slice::from_ref(schedule),
            &last_fires,
            now.with_timezone(&schedule.tz),
            schedule.tz,
        )
    }

    #[test]
    fn a_fresh_install_fires_once_immediately() {
        // No record yet: one snapshot now, like the pre-cron build's startup snapshot. This is what
        // makes a sub-minute expression reliable — requiring the tick to land exactly on a hit
        // second can miss every hit forever when the periods do not divide each other.
        let every_ten = cron("*/10 * * * * *");
        let utc = chrono_tz::UTC;
        for second in [0, 3, 7, 13, 59] {
            assert_eq!(
                due_firing(
                    &every_ten,
                    utc,
                    at(utc, "2026-11-23", 12, 0, second),
                    Watermark::Never
                ),
                Some(at(utc, "2026-11-23", 12, 0, second)),
                "a fresh install fires on the first tick, whatever second it lands on"
            );
        }
    }

    #[test]
    fn an_unreadable_watermark_never_fires() {
        // An unreadable store is not a fresh install. Firing "because we could not check" is the
        // bug that produced one warn per tick for as long as Redis was down; nothing is owed on a
        // guess, whatever the cron expression says.
        for expression in ["*/5 * * * * *", "0 0 3 * * *"] {
            assert_eq!(
                due_firing(
                    &cron(expression),
                    chrono_tz::UTC,
                    at(chrono_tz::UTC, "2026-11-23", 12, 0, 0),
                    Watermark::Unreadable,
                ),
                None,
                "an unreadable watermark must skip the schedule, not fire it"
            );
        }
    }

    #[test]
    fn a_served_firing_is_never_served_twice() {
        let every_ten = cron("*/10 * * * * *");
        let utc = chrono_tz::UTC;
        let served = Watermark::At(at(utc, "2026-11-23", 12, 0, 10).with_timezone(&Utc));
        // The very second that was served: not due, even though it is still "now".
        assert_eq!(
            due_firing(&every_ten, utc, at(utc, "2026-11-23", 12, 0, 10), served),
            None
        );
        // Nothing else is due until the next firing actually arrives.
        assert_eq!(
            due_firing(&every_ten, utc, at(utc, "2026-11-23", 12, 0, 12), served),
            None
        );
        // Then the next one is served, not the already-served one.
        assert_eq!(
            due_firing(&every_ten, utc, at(utc, "2026-11-23", 12, 0, 22), served),
            Some(at(utc, "2026-11-23", 12, 0, 20))
        );
    }

    #[test]
    fn a_firing_missed_between_ticks_is_still_served() {
        // Ticks are every 5 seconds, and a slow tick (or jitter) can step over the exact second:
        // the `*/10` firing at :10 is still picked up by a tick at :12, because the record only
        // says what was served *before* it.
        let every_ten = cron("*/10 * * * * *");
        let utc = chrono_tz::UTC;
        let served = Watermark::At(at(utc, "2026-11-23", 12, 0, 0).with_timezone(&Utc));
        assert_eq!(
            due_firing(&every_ten, utc, at(utc, "2026-11-23", 12, 0, 12), served),
            Some(at(utc, "2026-11-23", 12, 0, 10))
        );
    }

    #[test]
    fn downtime_catches_up_exactly_once() {
        let daily = cron("0 0 3 * * *");
        let utc = chrono_tz::UTC;
        let last = Watermark::At(at(utc, "2026-11-20", 3, 0, 0).with_timezone(&Utc));
        let now = at(utc, "2026-11-23", 12, 0, 0);
        // Three days were missed; exactly one firing is owed.
        assert_eq!(
            due_firing(&daily, utc, now, last),
            Some(at(utc, "2026-11-21", 3, 0, 0))
        );
        // Once that tick records the serve time, nothing else is owed — the *serve* time is what
        // gets recorded, not the missed slot, which is what stops the schedule re-firing for every
        // slot it missed.
        assert_eq!(
            due_firing(&daily, utc, now, Watermark::At(now.with_timezone(&Utc))),
            None
        );
    }

    #[test]
    fn the_default_expression_is_monday_three_am() {
        let schedule = test_schedule(true, "0 0 3 * * 1");
        let utc = chrono_tz::UTC;
        let runs = schedule.next_runs(at(utc, "2026-11-17", 12, 0, 0), 3);
        assert_eq!(
            runs,
            vec![
                at(utc, "2026-11-23", 3, 0, 0),
                at(utc, "2026-11-30", 3, 0, 0),
                at(utc, "2026-12-07", 3, 0, 0),
            ]
        );
    }

    #[test]
    fn a_disabled_schedule_has_no_next_runs() {
        let utc = chrono_tz::UTC;
        let enabled = test_schedule(true, "0 0 3 * * 1");
        assert_eq!(
            enabled.next_runs(at(utc, "2026-11-17", 12, 0, 0), 5).len(),
            5
        );
        assert!(test_schedule(false, "0 0 3 * * 1")
            .next_runs(at(utc, "2026-11-17", 12, 0, 0), 5)
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

        // Save the plan exactly as `PUT /settings` would store it, then clear both watermark fields
        // so the instance looks fresh. `snapshot_schedules` is what a managed install reads; the
        // legacy bare field stays cleared too because the plan below reuses the legacy id and would
        // otherwise inherit whatever a previous run of this test recorded.
        {
            let mut conn = state.redis.config.get().await.unwrap();
            let stored: () = deadpool_redis::redis::cmd("HSET")
                .arg(lanrurugi_storage::keys::CONFIG_KEY)
                .arg("snapshot_schedules")
                .arg(
                    // A once-a-year expression on purpose: "no watermark fires once, then not again"
                    // is what this test proves, and a every-5-seconds plan can cross a real slot
                    // between two ticks and make the "not twice" half flaky.
                    r#"[{"id":"plan-1","label":"","enabled":true,"preset":"custom","cron":"0 0 0 1 1 * *"}]"#,
                )
                .query_async(&mut conn)
                .await
                .unwrap();
            assert_eq!(stored, ());
            let cleared: () = deadpool_redis::redis::cmd("HDEL")
                .arg(SNAPSHOT_SCHEDULE_KEY)
                .arg(last_fire_field(LEGACY_SCHEDULE_ID))
                .arg(LAST_FIRE_FIELD)
                .query_async(&mut conn)
                .await
                .unwrap();
            assert_eq!(cleared, ());
        }

        let mut fired = 0usize;
        for _ in 0..4 {
            let dir_for_task = dir.clone();
            let entry = tick_with(
                &state,
                Utc::now().with_timezone(&chrono_tz::UTC),
                move |schedule, _slot| {
                    let reason = schedule.reason();
                    // `Fn`, not `FnOnce`: the same tick is driven several times.
                    let dir_for_task = dir_for_task.clone();
                    Box::pin(async move {
                        tokio::fs::create_dir_all(&dir_for_task)
                            .await
                            .map_err(|e| e.to_string())?;
                        let created = now_utc();
                        let name = format_name(created, SnapshotKind::Backup, &reason);
                        tokio::fs::write(dir_for_task.join(&name), b"REDIS0011stub")
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(SnapshotEntry {
                            name,
                            kind: SnapshotKind::Backup,
                            created_at: created.to_rfc3339_opts(SecondsFormat::Secs, true),
                            reason: "scheduled".to_string(),
                            schedule_id: None,
                            schedule_label: None,
                            size_bytes: 13,
                            usable: true,
                            summary: None,
                            pinned: false,
                            note: None,
                        })
                    })
                },
            )
            .await;
            fired += entry;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        // The very first tick of a fresh install fires, so at least one snapshot must exist; the
        // watermark must be recorded, and the list must show it as a real `scheduled` backup.
        assert!(fired >= 1, "the scheduler never fired");
        assert!(
            read_last_fire(&state, LEGACY_SCHEDULE_ID)
                .await
                .at()
                .is_some(),
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
        let again = tick_with(
            &state,
            Utc::now().with_timezone(&chrono_tz::UTC),
            move |_schedule, _slot| {
                Box::pin(async move {
                    panic!("a served firing must not be served twice");
                    #[allow(unreachable_code)]
                    Err::<SnapshotEntry, String>("unreachable".to_string())
                })
            },
        )
        .await;
        assert_eq!(again, 0, "a served firing must not be served twice");
    }

    /// The schedule runs on the operator's zone, not the process's. A container running with
    /// `TZ=UTC` must still fire "Monday 03:00" at 03:00 **JST**.
    #[test]
    fn the_schedule_runs_in_the_configured_timezone() {
        let tokyo: chrono_tz::Tz = "Asia/Tokyo".parse().unwrap();
        let schedule = test_schedule_in(tokyo, true, "0 0 3 * * 1");
        // 2026-11-17 12:00 JST is a Tuesday; the next Monday 03:00 JST is the 23rd.
        let runs = schedule.next_runs(at(tokyo, "2026-11-17", 12, 0, 0), 2);
        assert_eq!(runs[0], at(tokyo, "2026-11-23", 3, 0, 0));
        assert_eq!(
            runs[0].with_timezone(&Utc),
            at(chrono_tz::UTC, "2026-11-22", 18, 0, 0).with_timezone(&Utc),
            "03:00 JST is 18:00 UTC the day before"
        );

        // The reported strings carry the zone's offset, and the label names the zone.
        let value = schedule_block(
            &schedule,
            Some(runs[0].with_timezone(&Utc)),
            at(tokyo, "2026-11-17", 12, 0, 0),
        );
        assert_eq!(value["timezone"], json!("Asia/Tokyo (+09:00)"));
        assert_eq!(value["timezone_name"], json!("Asia/Tokyo"));
        assert_eq!(
            value["items"][0]["next_runs"][0],
            json!("2026-11-23T03:00:00+09:00")
        );
        assert_eq!(
            value["items"][0]["last_fire_at"],
            json!("2026-11-23T03:00:00+09:00")
        );

        // The same expression under the default UTC setting is a different instant.
        let utc = test_schedule(true, "0 0 3 * * 1");
        assert_eq!(
            utc.next_runs(at(chrono_tz::UTC, "2026-11-17", 12, 0, 0), 1)[0],
            at(chrono_tz::UTC, "2026-11-23", 3, 0, 0)
        );
        let value = schedule_block(&utc, None, at(chrono_tz::UTC, "2026-11-17", 12, 0, 0));
        assert_eq!(value["timezone"], json!("UTC (+00:00)"));
        assert_eq!(
            value["items"][0]["next_runs"][0],
            json!("2026-11-23T03:00:00+00:00")
        );
    }

    /// A DST-observing zone is evaluated on its own wall clock: the spring-forward gap has no
    /// 02:30 (so that day is skipped), the displayed offset follows the season, and the wall-clock
    /// time stays what the expression says.
    #[test]
    fn a_dst_zone_uses_its_own_wall_clock() {
        let ny: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let rfc = |at: DateTime<chrono_tz::Tz>| at.to_rfc3339_opts(SecondsFormat::Secs, false);

        let gap = test_schedule_in(ny, true, "0 30 2 * * *");
        // 2026-03-08 is the US spring-forward date: 02:30 does not exist that day.
        let next = gap.next_runs(at(ny, "2026-03-08", 0, 0, 0), 1)[0];
        assert_eq!(next, at(ny, "2026-03-09", 2, 30, 0));
        assert!(rfc(next).ends_with("-04:00"), "{}", rfc(next));

        let weekly = test_schedule_in(ny, true, "0 0 3 * * 1");
        let winter = weekly.next_runs(at(ny, "2026-01-05", 12, 0, 0), 1)[0];
        assert!(rfc(winter).ends_with("-05:00"), "{}", rfc(winter));
        let summer = weekly.next_runs(at(ny, "2026-07-06", 12, 0, 0), 1)[0];
        assert!(rfc(summer).ends_with("-04:00"), "{}", rfc(summer));
        assert_eq!(&rfc(summer)[11..19], "03:00:00", "still 03:00 wall clock");
    }

    #[test]
    fn schedule_json_reports_the_cron_shape() {
        let schedule = test_schedule(true, "0 0 3 * * 1");
        let last_fire = Some(
            Utc.with_ymd_and_hms(2026, 11, 23, 3, 0, 0)
                .single()
                .expect("unambiguous UTC instant"),
        );
        let now = at(chrono_tz::UTC, "2026-11-23", 5, 0, 0);
        let value = schedule_block(&schedule, last_fire, now);
        assert_eq!(value["items"][0]["enabled"], json!(true));
        assert_eq!(value["items"][0]["cron"], json!("0 0 3 * * 1"));
        assert_eq!(value["items"][0]["preset"], json!("custom"));
        assert!(value["timezone"].is_string());
        assert!(
            value["items"][0]["last_fire_at"].is_string(),
            "ISO with the local offset"
        );
        assert_eq!(
            value["items"][0]["next_runs"].as_array().unwrap().len(),
            NEXT_RUNS
        );
        assert!(
            value["items"][0]["next_runs"][0]
                .as_str()
                .unwrap()
                .contains("03:00:00"),
            "{}",
            value["items"][0]["next_runs"][0]
        );

        // Disabled: no next runs suggested, but what already fired is still reported honestly.
        let value = schedule_block(&test_schedule(false, "0 0 3 * * 1"), last_fire, now);
        assert_eq!(value["items"][0]["next_runs"], json!([]));
        assert!(value["items"][0]["last_fire_at"].is_string());
        assert_eq!(
            schedule_block(&schedule, None, now)["items"][0]["last_fire_at"],
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
