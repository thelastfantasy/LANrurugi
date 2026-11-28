//! `lanrurugi-server snapshots list|restore` — the operator's way back when the UI is unusable.
//!
//! Both commands talk to Redis and the library directly (no HTTP server, no scanner, no plugin
//! pool), so they work with `docker exec` on an instance whose API is down. The restore path is the
//! same `restore_full_with` the HTTP job uses — there is one implementation of the unsafe part.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use lanrurugi_api::snapshots::progress::BackupProgress;
use lanrurugi_api::snapshots::{restore_full_with, FullRestoreOutcome};
use lanrurugi_storage::paths::snapshot_dir;
use lanrurugi_storage::redis::RedisDbs;

#[derive(Args)]
pub struct SnapshotsArgs {
    /// Bare `redis://host:port` (no database index — this connects to every logical DB itself).
    #[arg(long, env = "LANRURUGI_REDIS_URL")]
    pub redis_url: String,

    /// Archive library folder; snapshots live in `<library>/.lanrurugi-backups/`.
    #[arg(long, env = "LANRURUGI_LIBRARY_PATH", default_value = "./library")]
    pub library_path: PathBuf,

    #[command(subcommand)]
    pub command: SnapshotsCommand,
}

#[derive(Subcommand)]
pub enum SnapshotsCommand {
    /// List the snapshot directory. Works without Redis: it only reads the files.
    List,
    /// Replace the whole Redis dataset with one full `.rdb` snapshot.
    Restore(RestoreArgs),
}

#[derive(Args)]
pub struct RestoreArgs {
    /// Snapshot file name, e.g. `snapshot-20261124T120000Z-manual.rdb`.
    pub name: String,
    /// Skip the interactive confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Progress on stdout: there is no SSE client here, so one line per milestone/note.
struct StdoutProgress {
    last_note: std::sync::Mutex<String>,
}

impl BackupProgress for StdoutProgress {
    fn update(&self, _written: u64, _expected: u64, percent: f64) {
        println!("  {percent:5.1}%");
    }

    fn note(&self, text: &str) {
        let mut last = self.last_note.lock().expect("stdout progress lock");
        if last.as_str() == text {
            return;
        }
        *last = text.to_string();
        println!("  · {text}");
    }
}

pub async fn run(args: SnapshotsArgs) -> anyhow::Result<()> {
    match args.command {
        SnapshotsCommand::List => list(&args.library_path),
        SnapshotsCommand::Restore(restore) => {
            restore_snapshot(&args.redis_url, &args.library_path, &restore).await
        }
    }
}

fn list(library: &Path) -> anyhow::Result<()> {
    let dir = snapshot_dir(library);
    let entries = lanrurugi_api::snapshots::list_in(&dir)?;
    if entries.is_empty() {
        println!("no snapshots in {}", dir.display());
        return Ok(());
    }
    println!("{} snapshot(s) in {}:", entries.len(), dir.display());
    println!(
        "{:<44} {:>12}  {:<20} {:<12} REASON",
        "NAME", "SIZE", "CREATED (UTC)", "KIND"
    );
    for entry in entries {
        println!(
            "{:<44} {:>10} KiB  {:<20} {:<12} {}{}",
            entry.name,
            entry.size_bytes / 1024,
            entry.created_at,
            match entry.kind {
                lanrurugi_api::snapshots::SnapshotKind::Backup => "rdb",
                lanrurugi_api::snapshots::SnapshotKind::Export => "export",
            },
            entry.reason,
            if entry.usable { "" } else { "  [unusable]" },
        );
    }
    Ok(())
}

async fn restore_snapshot(
    redis_url: &str,
    library: &Path,
    restore: &RestoreArgs,
) -> anyhow::Result<()> {
    // Validate before the scary prompt: a typo should not read as "this will overwrite everything".
    let source = lanrurugi_api::snapshots::preflight_full_restore(library, &restore.name)
        .map_err(|e| anyhow::anyhow!("{}: {}", e.reason(), e.message()))?;
    let size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
    println!(
        "About to replace the entire Redis dataset with {} ({} KiB).",
        restore.name,
        size / 1024
    );
    println!("This overwrites the entire database (这会覆盖整个数据库).");
    if !restore.yes {
        println!(
            "A pre-restore full backup is written first; if anything fails the previous RDB file is \
             put back. Type Enter to continue, Ctrl-C to abort."
        );
        print!("> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
    }

    let redis = RedisDbs::connect(redis_url)?;
    let progress = StdoutProgress {
        last_note: std::sync::Mutex::new(String::new()),
    };
    match restore_full_with(&redis, library, &restore.name, &progress).await {
        Ok(FullRestoreOutcome {
            restored_from,
            previous_state,
            entries,
        }) => {
            println!(
                "restored {restored_from}: {entries} archive record(s) loaded; previous state kept \
                 as {previous_state}"
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("restore failed [{}]: {e}", e.reason());
            Err(anyhow::anyhow!("restore failed: {}", e.reason()))
        }
    }
}
