//! End-to-end tests for `restore_full_with` against a **throwaway** `redis-server`.
//!
//! These deliberately never touch the suite's shared scratch Redis (`LANRURUGI_TEST_REDIS_URL`):
//! the whole point of the feature is replacing a dataset, and doing that to a database other tests
//! are using would be indistinguishable from the bug this is meant to prevent. Each test spawns its
//! own server on an ephemeral port with its own temp directory, and kills it afterwards.
//!
//! A missing `redis-server` binary (a host without Redis, but running the tests outside the dev
//! image) skips with a printed explanation rather than failing.

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use deadpool_redis::redis;
use lanrurugi_api::snapshots::progress::NoProgress;
use lanrurugi_api::snapshots::{restore_full_with, FullRestoreErrorKind};
use lanrurugi_storage::redis::RedisDbs;

fn redis_binary() -> Option<PathBuf> {
    for candidate in [
        "redis-server",
        "/usr/local/bin/redis-server",
        "/usr/bin/redis-server",
    ] {
        if let Ok(output) = Command::new(candidate).arg("--version").output()
            && output.status.success()
        {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

fn free_port() -> u16 {
    // Bind and immediately release: the kernel will not hand the same port to the redis server
    // while the listener is still open, and a race after release is vanishingly unlikely for a
    // test that starts one server.
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn start(binary: &Path, dir: &Path, port: u16, debug: bool) -> Child {
    let mut command = Command::new(binary);
    command
        .arg("--port")
        .arg(port.to_string())
        .arg("--bind")
        .arg("127.0.0.1")
        .arg("--dir")
        .arg(dir)
        .arg("--dbfilename")
        .arg("database.rdb")
        .arg("--appendonly")
        .arg("yes")
        .arg("--appendfilename")
        .arg("appendonly.aof")
        .arg("--save")
        .arg("")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if debug {
        command.arg("--enable-debug-command").arg("yes");
    }
    command.spawn().expect("spawn redis-server")
}

async fn redis_cmd(port: u16, args: &[&str]) -> redis::RedisResult<redis::Value> {
    let client = redis::Client::open(format!("redis://127.0.0.1:{port}"))?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    redis::cmd(args[0])
        .arg(&args[1..])
        .query_async(&mut conn)
        .await
}

async fn get(port: u16, key: &str) -> Option<String> {
    redis::cmd("GET")
        .arg(key)
        .query_async::<Option<String>>(
            &mut redis::Client::open(format!("redis://127.0.0.1:{port}"))
                .unwrap()
                .get_multiplexed_async_connection()
                .await
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn set(port: u16, key: &str, value: &str) {
    let client = redis::Client::open(format!("redis://127.0.0.1:{port}")).unwrap();
    let mut conn = client.get_multiplexed_async_connection().await.unwrap();
    let _: () = redis::cmd("SET")
        .arg(key)
        .arg(value)
        .query_async(&mut conn)
        .await
        .unwrap();
}

async fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(client) = redis::Client::open(format!("redis://127.0.0.1:{port}"))
            && let Ok(mut conn) = client.get_multiplexed_async_connection().await
            && redis::cmd("PING")
                .query_async::<String>(&mut conn)
                .await
                .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("redis-server on port {port} did not become ready");
}

fn library_with_snapshot(dir: &Path, snapshot_name: &str, rdb: &Path) -> PathBuf {
    let library = dir.join("library");
    let backups = library.join(".lanrurugi-backups");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::copy(rdb, backups.join(snapshot_name)).unwrap();
    library
}

fn snapshot_names(library: &Path) -> Vec<String> {
    std::fs::read_dir(library.join(".lanrurugi-backups"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The whole point of the feature: the snapshot's value comes back, survives a Redis restart, and
/// a `pre-restore` copy of the state being replaced exists on disk.
#[tokio::test]
async fn restore_replaces_the_dataset_and_survives_a_restart() {
    let Some(binary) = redis_binary() else {
        eprintln!("skipping: no redis-server binary on PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(&binary, dir.path(), port, true);
    wait_ready(port).await;

    set(port, "lrr:probe", "old").await;
    let _ = redis_cmd(port, &["SAVE"]).await.unwrap();
    let library = library_with_snapshot(
        dir.path(),
        "snapshot-20261124T120000Z-manual.rdb",
        &dir.path().join("database.rdb"),
    );
    set(port, "lrr:probe", "new").await;

    let redis_db = RedisDbs::connect(&format!("redis://127.0.0.1:{port}")).unwrap();
    let outcome = restore_full_with(
        &redis_db,
        &library,
        "snapshot-20261124T120000Z-manual.rdb",
        &NoProgress,
    )
    .await
    .expect("restore should succeed against a debug-enabled Redis");
    assert_eq!(get(port, "lrr:probe").await.as_deref(), Some("old"));
    assert!(outcome.previous_state.contains("pre-restore"));
    assert!(
        snapshot_names(&library)
            .iter()
            .any(|n| n.contains("pre-restore") && n.ends_with(".rdb")),
        "the pre-restore full backup must exist: {:?}",
        snapshot_names(&library)
    );
    let persistence = redis_cmd(port, &["INFO", "persistence"]).await.unwrap();
    let text = String::from_utf8_lossy(match &persistence {
        redis::Value::BulkString(bytes) => bytes,
        other => panic!("unexpected INFO reply: {other:?}"),
    })
    .into_owned();
    assert!(
        text.contains("aof_rewrite_in_progress:0"),
        "the AOF rewrite must have finished:\n{text}"
    );

    // SIGKILL, not a graceful shutdown: with `appendonly yes` a restart loads the AOF, so this
    // proves the rewrite really replaced the pre-restore AOF rather than only the in-memory data.
    let _ = child.kill();
    let _ = child.wait();
    let mut restarted = start(&binary, dir.path(), port, true);
    wait_ready(port).await;
    assert_eq!(get(port, "lrr:probe").await.as_deref(), Some("old"));
    let _ = restarted.kill();
    let _ = restarted.wait();
}

/// Without `enable-debug-command`, the reload cannot happen — and the file on disk must be exactly
/// what it was before the attempt.
#[tokio::test]
async fn a_debug_disabled_redis_is_refused_and_the_original_rdb_is_put_back() {
    let Some(binary) = redis_binary() else {
        eprintln!("skipping: no redis-server binary on PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(&binary, dir.path(), port, false);
    wait_ready(port).await;

    set(port, "lrr:probe", "old").await;
    let _ = redis_cmd(port, &["SAVE"]).await.unwrap();
    let original = std::fs::read(dir.path().join("database.rdb")).unwrap();
    let library = library_with_snapshot(
        dir.path(),
        "snapshot-20261124T120000Z-manual.rdb",
        &dir.path().join("database.rdb"),
    );
    set(port, "lrr:probe", "new").await;
    // Make the on-disk file differ from the snapshot, so "put back" is observable.
    let _ = redis_cmd(port, &["SAVE"]).await.unwrap();
    let before = std::fs::read(dir.path().join("database.rdb")).unwrap();
    assert_ne!(before, original, "the test needs two distinct RDB files");

    let redis_db = RedisDbs::connect(&format!("redis://127.0.0.1:{port}")).unwrap();
    let error = restore_full_with(
        &redis_db,
        &library,
        "snapshot-20261124T120000Z-manual.rdb",
        &NoProgress,
    )
    .await
    .expect_err("a Redis with DEBUG disabled must refuse the restore");
    assert_eq!(error.kind(), FullRestoreErrorKind::DebugCommandDisabled);
    assert_eq!(
        std::fs::read(dir.path().join("database.rdb")).unwrap(),
        before,
        "the RDB that was in place before the attempt must be restored byte for byte"
    );

    let _ = child.kill();
    let _ = child.wait();
}

/// A file that is not an RDB is refused before anything is written.
#[tokio::test]
async fn a_file_that_is_not_an_rdb_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    let backups = library.join(".lanrurugi-backups");
    std::fs::create_dir_all(&backups).unwrap();
    let mut file =
        std::fs::File::create(backups.join("snapshot-20261124T130000Z-manual.rdb")).unwrap();
    file.write_all(b"definitely not redis").unwrap();

    // No Redis needed: `preflight` runs first.
    let redis_db = RedisDbs::connect("redis://127.0.0.1:1").unwrap();
    let error = restore_full_with(
        &redis_db,
        &library,
        "snapshot-20261124T130000Z-manual.rdb",
        &NoProgress,
    )
    .await
    .expect_err("a non-RDB must be refused");
    assert_eq!(error.kind(), FullRestoreErrorKind::NotARedisBackup);
}

/// The real ordering a restore runs into: a scan is already cataloguing files when the gate
/// closes. The scan must stop before writing more rows, and the restore must then succeed against
/// the same server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scan_aborts_before_a_restore_and_the_restore_still_succeeds() {
    let Some(binary) = redis_binary() else {
        eprintln!("skipping: no redis-server binary on PATH");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut child = start(&binary, dir.path(), port, true);
    wait_ready(port).await;

    set(port, "lrr:probe", "old").await;
    let _ = redis_cmd(port, &["SAVE"]).await.unwrap();
    let library = library_with_snapshot(
        dir.path(),
        "snapshot-20261124T140000Z-manual.rdb",
        &dir.path().join("database.rdb"),
    );
    set(port, "lrr:probe", "new").await;

    let scan_dir = tempfile::tempdir().unwrap();
    let thumbs = tempfile::tempdir().unwrap();
    for name in ["a.zip", "b.zip", "c.zip"] {
        std::fs::write(scan_dir.path().join(name), b"not a real archive").unwrap();
    }

    let redis_db = RedisDbs::connect(&format!("redis://127.0.0.1:{port}")).unwrap();
    let pools = lanrurugi_storage::redis::ScannerPools {
        archive: redis_db.archive.clone(),
        config: redis_db.config.clone(),
        search: redis_db.search.clone(),
    };
    let gate = lanrurugi_core::restore_gate::RestoreGate::new();
    gate.close();
    let jobs = lanrurugi_core::jobs::JobRegistry::new();
    let job_id = jobs.create("scan").await;
    let summary = lanrurugi_scanner::full_scan::full_scan(
        scan_dir.path(),
        &pools,
        std::sync::Arc::new(lanrurugi_search::Equivalence::default()),
        thumbs.path(),
        &jobs,
        &job_id,
        None,
        Some(&gate),
    )
    .await;
    assert!(
        summary.aborted,
        "the scan must stop while the gate is closed"
    );
    gate.open();

    restore_full_with(
        &redis_db,
        &library,
        "snapshot-20261124T140000Z-manual.rdb",
        &NoProgress,
    )
    .await
    .expect("the restore must succeed after the scan aborted");
    assert_eq!(get(port, "lrr:probe").await.as_deref(), Some("old"));

    let _ = child.kill();
    let _ = child.wait();
}
