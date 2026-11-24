//! Restoring the whole database from a full Redis backup (`snapshot-*.rdb`).
//!
//! Redis owns its RDB: the file must never be edited in place. The sequence is a safety copy, an
//! atomic replacement of `<dir>/<dbfilename>`, `DEBUG RELOAD NOSAVE`, then an AOF rewrite. `NOSAVE`
//! is not optional — plain `DEBUG RELOAD` first writes the *current* dataset over the file that was
//! just replaced, which silently undoes the restore (verified against 8.2.1).
//!
//! Every mutation happens after the pre-restore full backup exists, and the previous RDB is kept
//! beside the target until the reload succeeds, because a failed reload can leave Redis with an
//! empty or partially loaded dataset.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

use crate::AppState;
use lanrurugi_storage::paths::snapshot_dir;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::ArchiveRepository;

use super::progress::{BackupProgress, NoProgress};
use super::{parse_name, take_full_backup_in, SnapshotKind};

/// An AOF rewrite of a few hundred megabytes is normal; five minutes is the point at which it is
/// not making progress any more.
const AOF_TIMEOUT: Duration = Duration::from_secs(300);
const AOF_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullRestoreErrorKind {
    InvalidName,
    NotFound,
    NotABackup,
    NotARedisBackup,
    SafetyBackupFailed,
    RedisUnavailable,
    DebugCommandDisabled,
    ReloadFailed,
    AofRewriteTimeout,
    Io,
}

/// The outcome of a successful restore. `previous_state` is the `pre-restore` full backup written
/// before anything was touched, not the temporary copy beside the live RDB (that one is deleted).
#[derive(Debug, Clone, Serialize)]
pub struct FullRestoreOutcome {
    pub restored_from: String,
    pub previous_state: String,
    /// LRR archive records present after the reload.
    pub entries: usize,
}

#[derive(Debug, Clone)]
pub struct FullRestoreError {
    kind: FullRestoreErrorKind,
    message: String,
    hint: Option<String>,
}

impl FullRestoreError {
    fn new(kind: FullRestoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn kind(&self) -> FullRestoreErrorKind {
        self.kind
    }

    /// Machine-readable reason, stable across releases (clients key off this, never the message).
    pub fn reason(&self) -> &'static str {
        match self.kind {
            FullRestoreErrorKind::InvalidName => "invalid_backup_name",
            FullRestoreErrorKind::NotFound => "not_found",
            FullRestoreErrorKind::NotABackup => "not_a_full_backup",
            FullRestoreErrorKind::NotARedisBackup => "not_a_redis_backup",
            FullRestoreErrorKind::SafetyBackupFailed => "safety_backup_failed",
            FullRestoreErrorKind::RedisUnavailable => "redis_unavailable",
            FullRestoreErrorKind::DebugCommandDisabled => "debug_command_disabled",
            FullRestoreErrorKind::ReloadFailed => "reload_failed",
            FullRestoreErrorKind::AofRewriteTimeout => "aof_rewrite_timeout",
            FullRestoreErrorKind::Io => "io_error",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self.kind {
            FullRestoreErrorKind::InvalidName
            | FullRestoreErrorKind::NotABackup
            | FullRestoreErrorKind::NotARedisBackup => StatusCode::BAD_REQUEST,
            FullRestoreErrorKind::NotFound => StatusCode::NOT_FOUND,
            // A configuration conflict the operator has to resolve, not a transient failure.
            FullRestoreErrorKind::DebugCommandDisabled => StatusCode::CONFLICT,
            FullRestoreErrorKind::RedisUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            FullRestoreErrorKind::SafetyBackupFailed
            | FullRestoreErrorKind::ReloadFailed
            | FullRestoreErrorKind::AofRewriteTimeout
            | FullRestoreErrorKind::Io => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// `{operation, success:0, error, reason, hint?}` — the shape every other endpoint's failure
    /// uses, plus the machine-readable reason this operation needs.
    pub fn into_response(self, operation: &str) -> Response {
        let mut body = json!({
            "operation": operation,
            "success": 0,
            "error": self.message,
            "reason": self.reason(),
        });
        if let Some(hint) = &self.hint {
            body["hint"] = json!(hint);
        }
        (self.status(), Json(body)).into_response()
    }
}

impl std::fmt::Display for FullRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.hint {
            Some(hint) => write!(f, "{} — {hint}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for FullRestoreError {}

/// How long a restore waits for writes that were already admitted when the gate closed.
const GATE_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Restores `name` (a full Redis backup inside `<library>/.lanrurugi-backups/`) into the live Redis.
///
/// Closes the write gate for the whole swap: `DEBUG RELOAD` replaces every key at once, so a
/// concurrent upload, scan or grouping review would otherwise write against a dataset that is about
/// to be replaced by an older one.
pub async fn restore_full(
    state: &AppState,
    name: &str,
    progress: &dyn BackupProgress,
) -> Result<FullRestoreOutcome, FullRestoreError> {
    // Releases the gate even if the restore task panics — otherwise every later write would 503
    // until the process restarted.
    struct Release(lanrurugi_core::restore_gate::RestoreGate);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.open();
        }
    }

    state.restore_gate.close();
    let _release = Release(state.restore_gate.clone());
    progress.note("Pausing new writes");
    let remaining = state.restore_gate.wait_for_writes(GATE_DRAIN_TIMEOUT).await;
    if remaining > 0 {
        tracing::warn!(
            remaining,
            "full restore: writes still in flight after the drain timeout; they may land in the \
             reload window"
        );
        progress.note(&format!(
            "{remaining} in-flight writes did not finish in {}s; continuing",
            GATE_DRAIN_TIMEOUT.as_secs()
        ));
    } else {
        progress.note("New writes paused; in-flight writes finished");
    }

    restore_full_with(&state.redis, &state.library.archive_dir, name, progress).await
}

/// [`restore_full`] without an `AppState`, for the offline `snapshots restore` CLI (which has no
/// scanner, plugin pool or job registry — it only needs Redis and the library path).
pub async fn restore_full_with(
    redis: &RedisDbs,
    archive_dir: &Path,
    name: &str,
    progress: &dyn BackupProgress,
) -> Result<FullRestoreOutcome, FullRestoreError> {
    let source = preflight(archive_dir, name)?;
    let size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
    progress.update(0, size, 5.0);

    let target = super::redis_rdb_path(redis)
        .await
        .map_err(|e| FullRestoreError::new(FullRestoreErrorKind::RedisUnavailable, e))?;
    let target_dir =
        match target.parent() {
            Some(dir) if dir.is_dir() => dir.to_path_buf(),
            _ => return Err(FullRestoreError::new(
                FullRestoreErrorKind::RedisUnavailable,
                format!(
                    "Redis reports its RDB at {}, which this process cannot read — a remote or \
                     containerised Redis cannot be restored from here",
                    target.display()
                ),
            )
            .with_hint(
                "Run the restore where Redis runs (docker exec into its container), or use the \
                 shell steps in docs/dev/deletion-safety-net.md.",
            )),
        };

    // The rollback copy is taken *before* the safety backup, because that backup's own BGSAVE
    // rewrites this very file; taking it afterwards would mean a failed reload put back bytes that
    // were never the starting state (same dataset, different file).
    let stamp = Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
    let rollback = target_dir.join(format!(".lanrurugi-restore-backup-{stamp}.rdb"));
    let staged = target_dir.join(format!(".lanrurugi-restore-{stamp}.tmp"));
    let had_existing = target.is_file();
    if had_existing {
        copy_file(&target, &rollback)
            .await
            .map_err(|e| FullRestoreError::new(FullRestoreErrorKind::Io, e))?;
    }

    // Safety copy: no snapshot, no mutations. `take_full_backup_in` also proves Redis is reachable
    // and that its RDB directory is one this process can see.
    let safety =
        match take_full_backup_in(redis, archive_dir, "pre-restore", Arc::new(NoProgress)).await {
            Ok(safety) => safety,
            Err(e) => {
                // Nothing was replaced yet, so the rollback copy is only litter.
                let _ = tokio::fs::remove_file(&rollback).await;
                return Err(FullRestoreError::new(
                    FullRestoreErrorKind::SafetyBackupFailed,
                    e,
                ));
            }
        };
    progress.update(size, size, 20.0);
    progress.note(&format!("Previous state saved as {}", safety.name));
    tracing::info!(snapshot = %safety.name, "full restore: pre-restore safety copy written");

    progress.update(size, size, 35.0);
    progress.note("Waiting for any AOF rewrite to finish");

    // An AOF rewrite forked before the reload would land afterwards and keep the old dataset in
    // the AOF, which the next start would then load over the restored RDB.
    wait_for_aof_idle(redis, AOF_TIMEOUT).await?;
    tracing::info!(target = %target.display(), "full restore: replacing Redis's RDB");
    let install = async {
        copy_file(&source, &staged).await?;
        copy_permissions(&target, &staged);
        tokio::fs::rename(&staged, &target)
            .await
            .map_err(|e| format!("could not move the restored RDB into place: {e}"))
    }
    .await;
    if let Err(e) = install {
        let rolled_back = rollback_file(&target, &rollback, had_existing).await;
        return Err(FullRestoreError::new(FullRestoreErrorKind::Io, e)
            .with_hint(rollback_hint(&rollback, rolled_back)));
    }
    progress.update(size, size, 60.0);
    progress.note("Reloading the Redis dataset");

    match debug_reload(redis).await {
        Ok(()) => {}
        Err(e) if debug_command_disabled(&e) => {
            let rolled_back = rollback_file(&target, &rollback, had_existing).await;
            return Err(FullRestoreError::new(
                FullRestoreErrorKind::DebugCommandDisabled,
                format!("Redis refused DEBUG RELOAD: {e}"),
            )
            .with_hint(format!(
                "Add `enable-debug-command local` to redis.conf and restart Redis (Debian's Redis 7+ \
                 default is `no`), or use the shell steps in docs/dev/deletion-safety-net.md. {}",
                rollback_hint(&rollback, rolled_back)
            )));
        }
        Err(e) => {
            let rolled_back = rollback_file(&target, &rollback, had_existing).await;
            // The failed reload may have flushed the in-memory dataset; load the old file back so
            // the running server matches the file that was just restored.
            let reloaded = debug_reload(redis).await;
            let mut message = format!("DEBUG RELOAD NOSAVE failed: {e}");
            if let Err(again) = reloaded {
                message.push_str(&format!(
                    "; reloading the previous RDB also failed: {again}"
                ));
            }
            return Err(
                FullRestoreError::new(FullRestoreErrorKind::ReloadFailed, message)
                    .with_hint(rollback_hint(&rollback, rolled_back)),
            );
        }
    }
    progress.update(size, size, 90.0);
    tracing::info!(
        snapshot = %safety.name,
        "full restore: RDB loaded; rewriting the AOF if enabled"
    );

    if appendonly_enabled(redis).await? {
        progress.note("Rewriting the append-only file");
        bgrewriteaof(redis).await?;
        wait_for_aof_idle(redis, AOF_TIMEOUT).await.map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::AofRewriteTimeout,
                format!(
                    "the restored dataset is loaded in memory but its AOF was not rewritten \
                         within {}s: restarting Redis now would load the pre-restore AOF and undo \
                         the restore ({})",
                    AOF_TIMEOUT.as_secs(),
                    e.message()
                ),
            )
            .with_hint(
                "Keep Redis running and retry `BGREWRITEAOF`, or run the shell steps in \
                     docs/dev/deletion-safety-net.md.",
            )
        })?;
    }

    // The pre-restore full backup above is the durable fallback; these two are only needed while
    // the swap is in flight.
    if had_existing {
        let _ = tokio::fs::remove_file(&rollback).await;
    }
    let _ = tokio::fs::remove_file(&staged).await;

    let entries = ArchiveRepository::new(redis.archive.clone())
        .list_ids()
        .await
        .map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::Io,
                format!(
                    "the database was restored but its archive records could not be counted: {e}"
                ),
            )
        })?
        .len();
    progress.update(size, size, 100.0);
    tracing::info!(
        snapshot = %safety.name,
        entries,
        "full restore: complete"
    );
    Ok(FullRestoreOutcome {
        restored_from: name.to_string(),
        previous_state: safety.name,
        entries,
    })
}

/// Resolves and validates the snapshot *without* touching Redis, so the HTTP handler can answer
/// `404`/`400` synchronously instead of accepting a job that is certain to fail.
pub fn preflight(archive_dir: &Path, name: &str) -> Result<PathBuf, FullRestoreError> {
    let parsed = parse_name(name).ok_or_else(|| {
        FullRestoreError::new(
            FullRestoreErrorKind::InvalidName,
            format!("{name} is not a snapshot file name."),
        )
    })?;
    if parsed.kind != SnapshotKind::Backup {
        return Err(FullRestoreError::new(
            FullRestoreErrorKind::NotABackup,
            format!("{name} is a JSON export, not a full Redis backup."),
        )
        .with_hint("Use the export restore path for .json snapshots."));
    }
    let path = snapshot_dir(archive_dir).join(name);
    if !path.is_file() {
        return Err(FullRestoreError::new(
            FullRestoreErrorKind::NotFound,
            format!("Snapshot {name} does not exist."),
        ));
    }
    let len = std::fs::metadata(&path)
        .map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::Io,
                format!("could not read {name}: {e}"),
            )
        })?
        .len();
    if len < 9 {
        return Err(FullRestoreError::new(
            FullRestoreErrorKind::NotARedisBackup,
            format!("{name} is {len} bytes — too small to be a Redis RDB."),
        ));
    }
    let mut header = [0u8; 9];
    std::fs::File::open(&path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::Io,
                format!("could not read {name}: {e}"),
            )
        })?;
    if &header[..5] != b"REDIS" {
        return Err(FullRestoreError::new(
            FullRestoreErrorKind::NotARedisBackup,
            format!("{name} does not start with the Redis RDB magic (REDIS)."),
        ));
    }
    Ok(path)
}

/// Copies `src` to `dst`, fsyncing before returning so the rename that follows cannot publish a
/// partially written file after a crash.
async fn copy_file(src: &Path, dst: &Path) -> Result<(), String> {
    let label = src.display().to_string();
    let src = src.to_path_buf();
    let dst = dst.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let mut input = std::fs::File::open(&src)?;
        let mut output = std::fs::File::create(&dst)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        Ok(())
    })
    .await
    .map_err(|e| format!("file copy task failed: {e}"))?
    .map_err(|e| format!("could not copy {label}: {e}"))
}

/// Redis runs as the same user in this image, but a bind-mounted data directory may not; matching
/// the old file's mode is what keeps a restart readable.
fn copy_permissions(existing: &Path, staged: &Path) {
    if let Ok(metadata) = std::fs::metadata(existing) {
        let _ = std::fs::set_permissions(staged, metadata.permissions());
    }
}

/// Puts the previous RDB back. Returns whether it did, so the caller's error can say whether the
/// rollback file is still needed.
async fn rollback_file(target: &Path, rollback: &Path, had_existing: bool) -> bool {
    if !had_existing {
        return tokio::fs::remove_file(target).await.is_ok();
    }
    let tmp = target.with_extension("rollback.tmp");
    if copy_file(rollback, &tmp).await.is_err() {
        return false;
    }
    tokio::fs::rename(&tmp, target).await.is_ok()
}

fn rollback_hint(rollback: &Path, rolled_back: bool) -> String {
    if rolled_back {
        "The previous RDB file was put back; the in-memory dataset may need a restart to match it."
            .to_string()
    } else {
        format!(
            "The previous RDB could not be put back automatically — it is at {}; copy it over the \
             configured dbfilename before restarting Redis.",
            rollback.display()
        )
    }
}

async fn redis_config(redis: &RedisDbs, key: &str) -> Result<String, FullRestoreError> {
    let mut conn = redis.config.get().await.map_err(|e| {
        FullRestoreError::new(
            FullRestoreErrorKind::RedisUnavailable,
            format!("could not reach Redis: {e}"),
        )
    })?;
    let values: Vec<String> = deadpool_redis::redis::cmd("CONFIG")
        .arg("GET")
        .arg(key)
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::RedisUnavailable,
                format!("could not read Redis's {key} setting: {e}"),
            )
        })?;
    Ok(values.get(1).cloned().unwrap_or_default())
}

async fn appendonly_enabled(redis: &RedisDbs) -> Result<bool, FullRestoreError> {
    Ok(redis_config(redis, "appendonly")
        .await?
        .eq_ignore_ascii_case("yes"))
}

async fn persistence_info(redis: &RedisDbs) -> Result<String, FullRestoreError> {
    let mut conn = redis.config.get().await.map_err(|e| {
        FullRestoreError::new(
            FullRestoreErrorKind::RedisUnavailable,
            format!("could not reach Redis: {e}"),
        )
    })?;
    deadpool_redis::redis::cmd("INFO")
        .arg("persistence")
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            FullRestoreError::new(
                FullRestoreErrorKind::RedisUnavailable,
                format!("could not read Redis's persistence info: {e}"),
            )
        })
}

fn info_field<'a>(info: &'a str, field: &str) -> Option<&'a str> {
    info.lines()
        .find_map(|line| line.trim().strip_prefix(field)?.strip_prefix(':'))
        .map(str::trim)
}

async fn aof_rewrite_running(redis: &RedisDbs) -> Result<bool, FullRestoreError> {
    let info = persistence_info(redis).await?;
    Ok(info_field(&info, "aof_rewrite_in_progress") == Some("1")
        || info_field(&info, "aof_rewrite_scheduled") == Some("1"))
}

async fn wait_for_aof_idle(redis: &RedisDbs, timeout: Duration) -> Result<(), FullRestoreError> {
    let deadline = Instant::now() + timeout;
    loop {
        if !aof_rewrite_running(redis).await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(FullRestoreError::new(
                FullRestoreErrorKind::AofRewriteTimeout,
                format!(
                    "an AOF rewrite was still running after {}s; no files were changed",
                    timeout.as_secs()
                ),
            ));
        }
        tokio::time::sleep(AOF_POLL).await;
    }
}

async fn debug_reload(redis: &RedisDbs) -> Result<(), String> {
    let mut conn = redis
        .config
        .get()
        .await
        .map_err(|e| format!("could not reach Redis: {e}"))?;
    // `NOSAVE` is what makes this load the file just installed. Plain `DEBUG RELOAD` saves the
    // current dataset over it first.
    let reply: String = deadpool_redis::redis::cmd("DEBUG")
        .arg("RELOAD")
        .arg("NOSAVE")
        .query_async(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    if reply != "OK" {
        return Err(format!("Redis answered {reply:?}"));
    }
    Ok(())
}

fn debug_command_disabled(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    // A bare `NOPERM` is an ACL denial of the one command we are running here, so it counts on its
    // own; the wording-based cases need "debug" to avoid matching unrelated refusals.
    error.contains("noperm")
        || (error.contains("debug")
            && (error.contains("not allowed")
                || error.contains("unknown command")
                || error.contains("disabled")))
}

async fn bgrewriteaof(redis: &RedisDbs) -> Result<(), FullRestoreError> {
    let mut conn = redis.config.get().await.map_err(|e| {
        FullRestoreError::new(
            FullRestoreErrorKind::RedisUnavailable,
            format!("could not reach Redis: {e}"),
        )
    })?;
    match deadpool_redis::redis::cmd("BGREWRITEAOF")
        .query_async::<String>(&mut conn)
        .await
    {
        Ok(_) => Ok(()),
        // Already running: the poll below waits for it. Any other refusal is real.
        Err(e)
            if e.to_string()
                .to_ascii_lowercase()
                .contains("already in progress") =>
        {
            Ok(())
        }
        Err(e) => Err(FullRestoreError::new(
            FullRestoreErrorKind::AofRewriteTimeout,
            format!("could not start the AOF rewrite: {e}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_snapshot(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let dir = snapshot_dir(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn rejects_a_file_that_is_not_an_rdb() {
        let tmp = temp_dir();
        let name = "snapshot-20261124T000000Z-manual.rdb";
        write_snapshot(tmp.path(), name, b"not a redis file, just text");
        let err = preflight(tmp.path(), name).unwrap_err();
        assert_eq!(err.kind(), FullRestoreErrorKind::NotARedisBackup);
        assert_eq!(err.reason(), "not_a_redis_backup");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_an_empty_file() {
        let tmp = temp_dir();
        let name = "snapshot-20261124T000000Z-manual.rdb";
        write_snapshot(tmp.path(), name, b"");
        let err = preflight(tmp.path(), name).unwrap_err();
        assert_eq!(err.kind(), FullRestoreErrorKind::NotARedisBackup);
    }

    #[test]
    fn accepts_a_redis_header() {
        let tmp = temp_dir();
        let name = "snapshot-20261124T000000Z-manual.rdb";
        write_snapshot(tmp.path(), name, b"REDIS0011\x00\x00");
        assert!(preflight(tmp.path(), name).is_ok());
    }

    #[test]
    fn rejects_a_missing_file_and_a_foreign_name() {
        let tmp = temp_dir();
        let err = preflight(tmp.path(), "snapshot-20261124T000000Z-manual.rdb").unwrap_err();
        assert_eq!(err.kind(), FullRestoreErrorKind::NotFound);
        assert_eq!(err.status(), StatusCode::NOT_FOUND);

        let err = preflight(tmp.path(), "../database.rdb").unwrap_err();
        assert_eq!(err.kind(), FullRestoreErrorKind::InvalidName);

        let name = "export-20261124T000000Z-manual.json";
        write_snapshot(tmp.path(), name, b"{}");
        let err = preflight(tmp.path(), name).unwrap_err();
        assert_eq!(err.kind(), FullRestoreErrorKind::NotABackup);
    }

    #[test]
    fn reason_and_status_mapping_is_stable() {
        let cases = [
            (
                FullRestoreErrorKind::InvalidName,
                "invalid_backup_name",
                400,
            ),
            (FullRestoreErrorKind::NotFound, "not_found", 404),
            (FullRestoreErrorKind::NotABackup, "not_a_full_backup", 400),
            (
                FullRestoreErrorKind::NotARedisBackup,
                "not_a_redis_backup",
                400,
            ),
            (
                FullRestoreErrorKind::SafetyBackupFailed,
                "safety_backup_failed",
                500,
            ),
            (
                FullRestoreErrorKind::RedisUnavailable,
                "redis_unavailable",
                503,
            ),
            (
                FullRestoreErrorKind::DebugCommandDisabled,
                "debug_command_disabled",
                409,
            ),
            (FullRestoreErrorKind::ReloadFailed, "reload_failed", 500),
            (
                FullRestoreErrorKind::AofRewriteTimeout,
                "aof_rewrite_timeout",
                500,
            ),
            (FullRestoreErrorKind::Io, "io_error", 500),
        ];
        for (kind, reason, status) in cases {
            let err = FullRestoreError::new(kind, "x");
            assert_eq!(err.reason(), reason);
            assert_eq!(err.status().as_u16(), status, "{reason}");
        }
    }

    #[test]
    fn recognises_the_ways_redis_says_debug_is_disabled() {
        assert!(debug_command_disabled(
            "ERR DEBUG command not allowed. If the enable-debug-command option is set to \"local\""
        ));
        assert!(debug_command_disabled(
            "NOPERM this user has no permissions"
        ));
        assert!(debug_command_disabled("ERR unknown command 'DEBUG'"));
        assert!(!debug_command_disabled(
            "ERR Background save already in progress"
        ));
    }
}
