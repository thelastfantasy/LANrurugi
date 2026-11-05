//! Structured logging setup. One process, one Tokio runtime (constitution Principle III) — so one
//! global `tracing` subscriber is enough; no per-component logger wiring needed.
//!
//! `init(Some(log_dir))` (the `serve` subcommand) additionally splits events into the same five
//! category files legacy's Settings → Logs page reads (`Utils/Logging.pm::get_logger`'s
//! `$logfile` argument, `Controller/Logging.pm`'s five `print_*` actions): `general.log`
//! (anything not matched below — legacy calls this one `lanraragi.log`, renamed here since
//! "lanraragi" is the legacy project's own name, not a fitting label for this process's own
//! catch-all events), `shinobu.log` (file-watcher/scanner activity), `plugins.log` (plugin
//! execution), `redis.log` (Redis client activity), `http.log` (`tower_http`/`axum`'s own
//! request-lifecycle tracing — legacy called the equivalent category `mojo`, after its underlying
//! web framework Mojolicious; renamed here since this project runs Axum, not Mojolicious, and
//! nothing about this project's own log category should stay pinned to a framework it doesn't
//! use — issue #86, done pre-release with no real deployment's `mojo.log` to migrate). `init(None)`
//! (`rebuild-index`/`bench`) keeps the original stderr-only behavior — those are one-shot CLI
//! invocations, not something with an equivalent "categories" concept.
//!
//! Files are on by default and rotate — daily, or earlier when a category's file outgrows
//! `LANRURUGI_LOG_MAX_FILE_MB` — keeping `LANRURUGI_LOG_MAX_FILES` files per category. Without a
//! cap these five files grew for the life of the instance: nothing rotated them, so a long-running
//! deployment accumulated one unbounded `general.log`. The pattern (daily + size + fixed count) is
//! copied from Jellyfin's own file sink (`logging.json`: `rollingInterval: Day`,
//! `retainedFileCountLimit: 3`, `rollOnFileSizeLimit`), which also logs to the console *and* to
//! disk rather than choosing. Only the active file is named `<category>.log` — that is what
//! `lanrurugi_api::logs` and the diagnostics bundle read — and rotated ones are
//! `<category>-<YYYYMMDD>.log`, with `-2`, `-3`, … appended if a day rolls more than once.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

/// Jellyfin's own file sink keeps 3 files and caps each at 100 MB. The count is copied as-is; the
/// size cap is smaller here because this process writes five category files, not one.
const DEFAULT_MAX_FILE_MB: u64 = 16;
const DEFAULT_MAX_FILES: usize = 3;

pub fn init(log_dir: Option<&Path>) {
    let env_filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            // `ort=warn`: ONNX Runtime logs every BFCArena reservation / CUDA node assignment / Memcpy insertion
            // at INFO; on this model set that was ~1700 lines per worker cold start and tens of MB per
            // session in `general.log` for zero diagnostic value. Warning+ keeps real ORT failures visible
            // while dropping the per-allocation noise. Everything `lanrurugi_*` stays at info.
            EnvFilter::new("info,ort=warn,ort_sys=warn")
        })
    };

    // Stderr, not stdout: `lanrurugi bench` prints its final JSON report to stdout, and log
    // lines interleaved into that same stream would corrupt it for any caller piping the
    // output into a JSON parser.
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(env_filter());

    // Error-level log events also become diagnostics events (specs/018): plugin failures and
    // background-job errors are logged, not raised, so this is the only way they reach the store.
    //
    // `ErrorLayer` (sentry-tracing ≥0.49) replaces the deprecated combined `layer()` +
    // `EventFilter`: the old `event_filter` callback is now an ordinary layer filter, which is
    // what `filter_fn` below is. The level filter stays separate so a warning that
    // `capture_log_event` rejects is dropped before any event is built.
    let capture_layer = sentry_tracing::ErrorLayer::new()
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            lanrurugi_api::diagnostics::report::capture_log_event(
                metadata.level().as_str(),
                metadata.target(),
            )
        }))
        .with_filter(tracing_subscriber::filter::LevelFilter::WARN);

    // Structured logs for the tracker's Logs page. Volume is controlled by one env var:
    // `LANRURUGI_TRACKER_LOG_LEVEL=info` sends `lanrurugi_*` INFO and everything WARN+; unset sends
    // WARN+ only. The level filter alone is not enough — `ort` logs every BFCArena reservation at
    // INFO, which is why the target filter below also excludes it unless explicitly asked for.
    let log_level = std::env::var("LANRURUGI_TRACKER_LOG_LEVEL")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let info_targets = log_level == "info" || log_level == "debug";
    let log_layer = sentry_tracing::LogLayer::new().with_filter(
        tracing_subscriber::filter::filter_fn(move |metadata| {
            let level = metadata.level();
            let target = metadata.target();
            if !target.starts_with("lanrurugi") {
                return *level == tracing::Level::WARN || *level == tracing::Level::ERROR;
            }
            if info_targets {
                *level <= tracing::Level::INFO
            } else {
                *level == tracing::Level::WARN || *level == tracing::Level::ERROR
            }
        }),
    );

    let registry = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(capture_layer)
        .with(log_layer);

    match log_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).ok();
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(CategorizedMakeWriter::new(dir.to_path_buf()))
                .with_filter(env_filter());
            registry.with(file_layer).init();
        }
        None => registry.init(),
    }
}

fn categorize(target: &str) -> &'static str {
    if target.starts_with("lanrurugi_scanner") {
        "shinobu"
    } else if target.starts_with("lanrurugi_plugin") {
        "plugins"
    } else if target.starts_with("redis") || target.starts_with("deadpool_redis") {
        "redis"
    } else if target.starts_with("tower_http") || target.starts_with("axum") {
        "http"
    } else {
        "general"
    }
}

/// When a category's active file is rolled over and how many files that category keeps in total.
#[derive(Debug, Clone, Copy)]
struct Rotation {
    max_file_bytes: u64,
    /// Including the active `<category>.log`.
    keep_files: usize,
}

impl Rotation {
    fn from_env() -> Self {
        let mb = positive_env("LANRURUGI_LOG_MAX_FILE_MB").unwrap_or(DEFAULT_MAX_FILE_MB);
        let keep_files = positive_env("LANRURUGI_LOG_MAX_FILES")
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_FILES);
        Self {
            max_file_bytes: mb.saturating_mul(1024 * 1024),
            keep_files,
        }
    }
}

fn local_date(at: std::time::SystemTime) -> chrono::NaiveDate {
    chrono::DateTime::<chrono::Local>::from(at).date_naive()
}

fn positive_env(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

/// One category's active file plus the state rotation needs.
struct CategoryFile {
    file: Arc<Mutex<File>>,
    /// Bytes written to the active file, seeded from its length when opened.
    bytes: Arc<AtomicU64>,
    day: chrono::NaiveDate,
}

struct CategorizedMakeWriter {
    log_dir: PathBuf,
    rotation: Rotation,
    files: Mutex<HashMap<&'static str, Arc<CategoryFile>>>,
}

impl CategorizedMakeWriter {
    fn new(log_dir: PathBuf) -> Self {
        Self::with_rotation(log_dir, Rotation::from_env())
    }

    fn with_rotation(log_dir: PathBuf, rotation: Rotation) -> Self {
        Self {
            log_dir,
            rotation,
            files: Mutex::new(HashMap::new()),
        }
    }

    fn active_path(&self, category: &str) -> PathBuf {
        self.log_dir.join(format!("{category}.log"))
    }

    fn open(&self, category: &'static str) -> SharedFile {
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let today = chrono::Local::now().date_naive();
        if let Some(state) = files.get(category) {
            let new_day = state.day != today;
            let too_big = state.bytes.load(Ordering::Relaxed) >= self.rotation.max_file_bytes;
            if !new_day && !too_big {
                return SharedFile {
                    state: state.clone(),
                };
            }
        }
        // Reaching here with a state means the day changed or the file outgrew the cap: always a
        // roll. Only a first open in this process may adopt today's file — so a restart does not
        // throw away the history someone is about to read.
        let rolling = files.contains_key(category);
        let state = Arc::new(if !rolling && self.continues_today(category, today) {
            self.append_to(category, today)
        } else {
            self.roll(category, today)
        });
        files.insert(category, state.clone());
        SharedFile { state }
    }

    /// Whether the active file already belongs to today and still has room under the size cap.
    fn continues_today(&self, category: &str, today: chrono::NaiveDate) -> bool {
        let Ok(meta) = std::fs::metadata(self.active_path(category)) else {
            return false;
        };
        let day = meta.modified().ok().map(local_date).unwrap_or(today);
        day == today && meta.len() < self.rotation.max_file_bytes
    }

    fn append_to(&self, category: &str, day: chrono::NaiveDate) -> CategoryFile {
        let path = self.active_path(category);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed to open log file {path:?}: {e}"));
        let bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        CategoryFile {
            file: Arc::new(Mutex::new(file)),
            bytes: Arc::new(AtomicU64::new(bytes)),
            day,
        }
    }

    /// Moves the active file aside (if it has anything in it) and starts a new one, then drops
    /// whatever this category no longer needs. Called with `self.files` locked.
    fn roll(&self, category: &str, today: chrono::NaiveDate) -> CategoryFile {
        let active = self.active_path(category);
        // An empty file (a previous run's first open, or a truncated one) has nothing worth
        // keeping — moving it aside would just litter the directory with empty rotated files.
        if let Ok(meta) = std::fs::metadata(&active) {
            if meta.len() > 0 {
                // Named after the day its content belongs to, not today's: a file left over from
                // yesterday must not masquerade as today's.
                let day = meta.modified().ok().map(local_date).unwrap_or(today);
                let target = self.rotated_path(category, day);
                if let Err(e) = std::fs::rename(&active, &target) {
                    // Better a file that keeps growing than a log line lost in the writer.
                    eprintln!("lanrurugi: failed to rotate {active:?} to {target:?}: {e}");
                }
            }
        }
        self.prune(category);
        self.append_to(category, today)
    }

    /// `<category>-<YYYYMMDD>.log`, plus `-2`, `-3`, … when a single day rolls more than once.
    fn rotated_path(&self, category: &str, today: chrono::NaiveDate) -> PathBuf {
        let stem = format!("{category}-{}", today.format("%Y%m%d"));
        for n in 1.. {
            let name = if n == 1 {
                format!("{stem}.log")
            } else {
                format!("{stem}-{n}.log")
            };
            let path = self.log_dir.join(name);
            if !path.exists() {
                return path;
            }
        }
        unreachable!("the loop above only stops when it returns")
    }

    /// Deletes this category's oldest rotated files so that the active one plus what remains fits
    /// in `keep_files`.
    fn prune(&self, category: &str) {
        let keep = self.rotation.keep_files.saturating_sub(1);
        let prefix = format!("{category}-");
        let Ok(entries) = std::fs::read_dir(&self.log_dir) else {
            return;
        };
        let mut rotated: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(".log")
            })
            .filter_map(|entry| {
                let modified = entry.metadata().ok()?.modified().ok()?;
                Some((modified, entry.path()))
            })
            .collect();
        rotated.sort();
        let excess = rotated.len().saturating_sub(keep);
        for (_, path) in rotated.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A `File` shared behind a mutex so every event routed to the same category appends to the same
/// open handle rather than re-opening the file per event.
struct SharedFile {
    state: Arc<CategoryFile>,
}

impl io::Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut file = self
            .state
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let written = file.write(buf)?;
        self.state
            .bytes
            .fetch_add(written as u64, Ordering::Relaxed);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .flush()
    }
}

impl<'a> MakeWriter<'a> for CategorizedMakeWriter {
    type Writer = SharedFile;

    fn make_writer(&'a self) -> Self::Writer {
        self.open("general")
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        self.open(categorize(meta.target()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorizes_known_targets() {
        assert_eq!(categorize("lanrurugi_scanner::watcher"), "shinobu");
        assert_eq!(categorize("lanrurugi_plugin::pool"), "plugins");
        assert_eq!(categorize("redis::connection"), "redis");
        assert_eq!(categorize("deadpool_redis::pool"), "redis");
        assert_eq!(categorize("tower_http::trace"), "http");
        assert_eq!(categorize("lanrurugi_api::archives"), "general");
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_that_outgrows_the_size_cap_is_rolled_and_the_oldest_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let writer = CategorizedMakeWriter::with_rotation(
            dir.path().to_path_buf(),
            Rotation {
                max_file_bytes: 100,
                keep_files: 3,
            },
        );

        // 40 bytes per event, so every third write crosses the 100-byte cap.
        for _ in 0..80 {
            let mut file = writer.open("general");
            io::Write::write_all(&mut file, &[b'x'; 40]).unwrap();
        }

        let names = names(dir.path());
        assert_eq!(
            names.iter().filter(|n| n.starts_with("general-")).count(),
            2,
            "keep_files = 3 counts the active file, so only two rotated ones survive: {names:?}"
        );
        assert!(names.contains(&"general.log".to_string()), "{names:?}");
        let active = std::fs::metadata(dir.path().join("general.log"))
            .unwrap()
            .len();
        assert!(
            active <= 100,
            "the active file rolled before growing past the cap: {active}"
        );
    }

    #[test]
    fn a_new_day_rolls_the_file_even_when_it_is_small() {
        let dir = tempfile::tempdir().unwrap();
        let writer = CategorizedMakeWriter::with_rotation(
            dir.path().to_path_buf(),
            Rotation {
                max_file_bytes: 1024 * 1024,
                keep_files: 5,
            },
        );
        let yesterday = chrono::Local::now().date_naive() - chrono::Duration::days(1);
        std::fs::write(dir.path().join("shinobu.log"), b"yesterday\n").unwrap();
        let previous = Arc::new(CategoryFile {
            file: Arc::new(Mutex::new(
                OpenOptions::new()
                    .append(true)
                    .open(dir.path().join("shinobu.log"))
                    .unwrap(),
            )),
            bytes: Arc::new(AtomicU64::new(10)),
            day: yesterday,
        });
        writer.files.lock().unwrap().insert("shinobu", previous);

        let mut file = writer.open("shinobu");
        io::Write::write_all(&mut file, b"today\n").unwrap();
        drop(file);

        let names = names(dir.path());
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("shinobu-") && name.ends_with(".log")),
            "yesterday's content moved aside: {names:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("shinobu.log")).unwrap(),
            "today\n",
            "the active file starts fresh rather than appending to yesterday's"
        );
    }

    #[test]
    fn a_restart_continues_todays_file_instead_of_rolling_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("general.log"), b"before restart\n").unwrap();
        let writer = CategorizedMakeWriter::with_rotation(
            dir.path().to_path_buf(),
            Rotation {
                max_file_bytes: 1024 * 1024,
                keep_files: 3,
            },
        );

        let mut file = writer.open("general");
        io::Write::write_all(&mut file, b"after restart\n").unwrap();
        drop(file);

        assert_eq!(
            names(dir.path()),
            vec!["general.log".to_string()],
            "a same-day restart appends to the file instead of rolling it — the log someone is \
             restarting to read must survive the restart"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("general.log")).unwrap(),
            "before restart\nafter restart\n"
        );
    }
}
