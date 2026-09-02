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

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

pub fn init(log_dir: Option<&Path>) {
    let env_filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Stderr, not stdout: `lanrurugi bench` prints its final JSON report to stdout, and log
    // lines interleaved into that same stream would corrupt it for any caller piping the
    // output into a JSON parser.
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(env_filter());

    let registry = tracing_subscriber::registry().with(stderr_layer);

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

/// A `File` shared behind a mutex so every event routed to the same category appends to the same
/// open handle rather than re-opening the file per event.
struct SharedFile(Arc<Mutex<File>>);

impl io::Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .flush()
    }
}

struct CategorizedMakeWriter {
    log_dir: PathBuf,
    files: Mutex<HashMap<&'static str, Arc<Mutex<File>>>>,
}

impl CategorizedMakeWriter {
    fn new(log_dir: PathBuf) -> Self {
        Self {
            log_dir,
            files: Mutex::new(HashMap::new()),
        }
    }

    fn open(&self, category: &'static str) -> SharedFile {
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = files
            .entry(category)
            .or_insert_with(|| {
                let path = self.log_dir.join(format!("{category}.log"));
                let handle = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .unwrap_or_else(|e| panic!("failed to open log file {path:?}: {e}"));
                Arc::new(Mutex::new(handle))
            })
            .clone();
        SharedFile(file)
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
}
