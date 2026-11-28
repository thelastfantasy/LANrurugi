//! `GET /api/resources` and the settings panel's data (spec `030-resource-monitor`).
//!
//! Design rules this module follows, all of them from the spec:
//!
//! - Nothing here runs unless somebody is looking. The sampler is owned by [`watch::ResourceWatch`]
//!   and only exists while an [`watch::ObserverLease`] is held.
//! - A number that could not be read is `None` plus an explanation — never `0` (FR-003).
//! - Sampling is at most one `/proc` walk per tick and never starts a subprocess, except the GPU
//!   probe that is off by default (FR-014).

pub mod alerts;
pub mod api;
pub mod gpu;
pub mod model;
pub mod models;
pub mod portable;
pub mod procfs;
pub mod watch;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::resources::model::*;
use crate::AppState;

/// Metadata-preview cache counters (`plugins::ensure_metadata_cached`). Global because that function
/// is called from request handlers that have no pool handle of their own for this purpose.
static METADATA_HITS: AtomicU64 = AtomicU64::new(0);
static METADATA_MISSES: AtomicU64 = AtomicU64::new(0);

pub fn note_metadata_cache(hit: bool) {
    if hit {
        METADATA_HITS.fetch_add(1, Ordering::Relaxed);
    } else {
        METADATA_MISSES.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn metadata_cache_counters() -> (u64, u64) {
    (
        METADATA_HITS.load(Ordering::Relaxed),
        METADATA_MISSES.load(Ordering::Relaxed),
    )
}

/// Library totals are recomputed at most this often: `list_all()` walks every archive record.
const LIBRARY_CACHE_TTL: Duration = Duration::from_secs(60);
static LIBRARY_CACHE: std::sync::Mutex<Option<(Instant, LibrarySection)>> =
    std::sync::Mutex::new(None);

/// The hard cap on a serialised snapshot (contract V7). The truncation order below is what makes the
/// cap reachable without losing the numbers a bug report always needs.
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;

fn now_ms() -> i64 {
    crate::diagnostics::now_ms()
}

fn monotonic_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// One full observation. Async because Redis, the job registry and the plugin pool are async; the
/// `/proc` walk itself is moved to a blocking thread so a slow filesystem cannot stall the runtime.
pub async fn collect(state: &AppState) -> ResourceSnapshot {
    let started = Instant::now();
    let at = now_ms();

    let blocking = {
        let library = state.library.clone();
        let pid = std::process::id();
        tokio::task::spawn_blocking(move || {
            read_blocking(pid, &library.archive_dir, &library.temp_dir, at)
        })
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "resource snapshot: blocking reader panicked");
            Blocking::empty(pid)
        })
    };

    let jobs = state.jobs.list_all().await;
    let queue = state.download_queue.list_all().await.unwrap_or_default();

    let mut snapshot = ResourceSnapshot {
        at,
        monotonic_ms: 0,
        window_ms: blocking.window_ms,
        version: crate::version::current_version().to_string(),
        release: crate::version::deployment().release.unwrap_or_default(),
        platform: portable::platform_label(),
        build: BuildSection::default(),
        server: blocking.server,
        cgroup: blocking.cgroup,
        cgroup_unavailable: blocking.cgroup_unavailable,
        runtime: RuntimeSection {
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(0),
            tracked_tasks: watch::ResourceWatch::global().tracked_tasks(),
            queue_depth: jobs
                .iter()
                .filter(|job| job.state == lanrurugi_core::jobs::JobState::Queued)
                .count() as u64,
        },
        plugins: plugin_section(state).await,
        processes: blocking.processes,
        models: ModelsSection {
            usage: models::usage(state).await,
            in_process: models::in_process(),
            note: models::NOTE.to_string(),
        },
        queues: queue_section(&jobs, &queue),
        redis: None,
        redis_unavailable: None,
        network: blocking.network,
        caches: CacheSection {
            descriptor_hits: state.plugins.cache_counters().0,
            descriptor_misses: state.plugins.cache_counters().1,
            metadata_hits: metadata_cache_counters().0,
            metadata_misses: metadata_cache_counters().1,
        },
        library: library_section(state).await,
        storage: blocking.storage,
        gpu: None,
        gpu_unavailable: None,
        watch: watch::ResourceWatch::global().section(),
        truncations: Vec::new(),
    };

    let (redis, redis_unavailable) = redis_section(state).await;
    snapshot.redis = redis;
    snapshot.redis_unavailable = redis_unavailable;

    let (gpu, gpu_unavailable) = gpu::GpuReader::global().read();
    snapshot.gpu = gpu;
    snapshot.gpu_unavailable = gpu_unavailable;

    snapshot.monotonic_ms = monotonic_ms(started);
    truncate(&mut snapshot);
    snapshot
}

struct Blocking {
    window_ms: u64,
    server: ServerSection,
    cgroup: Option<CgroupSection>,
    cgroup_unavailable: Option<String>,
    processes: ProcessSection,
    network: NetworkSection,
    storage: StorageSection,
}

impl Blocking {
    fn empty(pid: u32) -> Self {
        Self {
            window_ms: 0,
            server: ServerSection {
                pid,
                ..Default::default()
            },
            cgroup: None,
            cgroup_unavailable: Some("reader_failed".to_string()),
            processes: ProcessSection {
                root_pid: pid,
                ..Default::default()
            },
            network: NetworkSection::default(),
            storage: StorageSection::default(),
        }
    }
}

fn read_blocking(
    pid: u32,
    archive_dir: &std::path::Path,
    temp_dir: &std::path::Path,
    at: i64,
) -> Blocking {
    let sampler = procfs::sampler();
    let hz = procfs::clock_ticks_per_second();
    let boot = procfs::boot_time_secs();
    let stat = procfs::read_process_stat(pid);

    let mut server = ServerSection {
        pid,
        rss_bytes: stat.as_ref().map(|s| s.rss_bytes),
        peak_rss_bytes: portable::peak_rss_bytes(),
        swap_bytes: None,
        threads: procfs::thread_count(pid),
        fd_count: procfs::fd_count(pid),
        fd_limit: procfs::fd_limit(pid),
        cpu_percent_1s: stat
            .as_ref()
            .and_then(|s| sampler.cpu_percent(pid, s.total_ticks, at)),
        cpu_seconds_total: stat
            .as_ref()
            .map(|s| s.total_ticks as f64 / hz)
            .unwrap_or(0.0),
        read_bytes: procfs::process_io_bytes(pid).map(|(read, _)| read),
        write_bytes: procfs::process_io_bytes(pid).map(|(_, write)| write),
    };
    // `swap` per process needs `/proc/<pid>/status`; report it only when there is something to say.
    if let Some(swap) = read_status_kb(pid, "VmSwap:") {
        server.swap_bytes = Some(swap);
    }

    let (cgroup, cgroup_unavailable) = match procfs::self_cgroup_path() {
        Some(relative) => match procfs::cgroup(&relative) {
            Some(section) => (Some(section), None),
            None => (None, Some(format!("unreadable: /sys/fs/cgroup{relative}"))),
        },
        None => (None, Some("no_cgroup".to_string())),
    };

    let (tree, truncated) = procfs::process_tree(pid, 3, 64);
    let mut live = std::collections::HashSet::new();
    let entries: Vec<ProcessEntry> = tree
        .into_iter()
        .map(|(stat, depth)| {
            live.insert(stat.pid);
            ProcessEntry {
                pid: stat.pid,
                ppid: stat.ppid,
                depth,
                comm: stat.comm,
                rss_bytes: Some(stat.rss_bytes),
                cpu_percent: sampler.cpu_percent(stat.pid, stat.total_ticks, at),
                started_at: boot.map(|boot| {
                    let seconds = boot as f64 + stat.start_ticks as f64 / hz;
                    (seconds * 1000.0) as i64
                }),
            }
        })
        .collect();
    sampler.retain_pids(&live);

    let (rx_bytes, tx_bytes) = procfs::network_totals();
    let (rx_rate_bps, tx_rate_bps) = sampler.network_rates(rx_bytes, tx_bytes, at);

    Blocking {
        window_ms: 1_000,
        server,
        cgroup,
        cgroup_unavailable,
        processes: ProcessSection {
            root_pid: pid,
            tree: entries,
            truncated,
        },
        network: NetworkSection {
            rx_bytes,
            tx_bytes,
            rx_rate_bps,
            tx_rate_bps,
        },
        storage: StorageSection {
            library: procfs::disk(archive_dir).unwrap_or_default(),
            temp: procfs::disk(temp_dir).unwrap_or_default(),
        },
    }
}

/// One `VmXxx:` line from `/proc/<pid>/status`, in bytes.
fn read_status_kb(pid: u32, field: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = text.lines().find(|line| line.starts_with(field))?;
    let kb: u64 = line[field.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kb.saturating_mul(1024))
}

async fn plugin_section(state: &AppState) -> PluginSection {
    let (available, in_flight, spawned) = state.plugins.transient_counters();
    let processes = state
        .plugins
        .worker_processes()
        .await
        .into_iter()
        .map(|process| {
            let stat = process.pid.and_then(procfs::read_process_stat);
            PluginProcess {
                namespace: process.namespace,
                pid: process.pid,
                busy: process.busy,
                rss_bytes: stat.as_ref().map(|s| s.rss_bytes),
                cpu_percent_since: None,
                started_at: None,
                last_call_at: process.last_call_at,
                calls: process.calls,
                failures: process.failures,
                persistent: true,
            }
        })
        .collect();
    PluginSection {
        limit: lanrurugi_plugin::pool::worker_limit(),
        transient_available: available,
        transient_in_flight: in_flight,
        transient_spawned_total: spawned,
        processes,
    }
}

fn queue_section(
    jobs: &[lanrurugi_core::jobs::JobStatus],
    queue: &[lanrurugi_storage::download_queue::DownloadQueueItem],
) -> QueueSection {
    use lanrurugi_core::jobs::JobState;
    use lanrurugi_storage::download_queue::DownloadQueueState;

    let mut phases: HashMap<&str, Vec<u64>> = HashMap::new();
    for job in jobs {
        for phase in &job.phases {
            let Some(ended) = phase.ended_at else {
                continue;
            };
            let millis = (ended - phase.started_at).max(0) as u64;
            phases.entry(phase.name.as_str()).or_default().push(millis);
        }
    }
    let mut p50 = BTreeMap::new();
    let mut p95 = BTreeMap::new();
    for (name, mut values) in phases {
        values.sort_unstable();
        p50.insert(name.to_string(), percentile(&values, 50));
        p95.insert(name.to_string(), percentile(&values, 95));
    }
    let permit_wait_p50_ms = p50.get(lanrurugi_core::jobs::PHASE_WAITING).copied();

    QueueSection {
        download_active: queue
            .iter()
            .filter(|item| {
                matches!(
                    item.state,
                    DownloadQueueState::Starting
                        | DownloadQueueState::Waiting
                        | DownloadQueueState::Downloading
                )
            })
            .count() as u64,
        download_queued: queue
            .iter()
            .filter(|item| item.state == DownloadQueueState::Queued)
            .count() as u64,
        download_error: queue
            .iter()
            .filter(|item| item.state == DownloadQueueState::Error)
            .count() as u64,
        jobs_active: jobs
            .iter()
            .filter(|job| job.state == JobState::Active)
            .count() as u64,
        jobs_failed_total: jobs
            .iter()
            .filter(|job| job.state == JobState::Failed)
            .count() as u64,
        phase_p50_ms: p50,
        phase_p95_ms: p95,
        permit_wait_p50_ms,
    }
}

/// Nearest-rank percentile over an already-sorted slice; `0` for an empty one.
fn percentile(sorted: &[u64], percent: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (percent as usize * sorted.len()).div_ceil(100);
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

async fn library_section(state: &AppState) -> LibrarySection {
    {
        let cache = LIBRARY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, section)) = cache.as_ref()
            && at.elapsed() < LIBRARY_CACHE_TTL
        {
            return section.clone();
        }
    }
    let archives = state
        .repos
        .archives
        .list_ids()
        .await
        .map(|ids| ids.len() as u64)
        .unwrap_or(0);
    let total_bytes = state
        .repos
        .archives
        .list_all()
        .await
        .map(|all| all.iter().map(|archive| archive.arcsize).sum())
        .unwrap_or(0);
    let section = LibrarySection {
        archives,
        total_bytes,
    };
    let mut cache = LIBRARY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *cache = Some((Instant::now(), section.clone()));
    section
}

async fn redis_section(state: &AppState) -> (Option<RedisSection>, Option<String>) {
    let fetched: Result<String, String> = async {
        let mut conn = state
            .redis
            .config
            .get()
            .await
            .map_err(|error| error.to_string())?;
        deadpool_redis::redis::cmd("INFO")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|error| error.to_string())
    }
    .await;

    match fetched {
        Ok(text) => (Some(parse_redis_info(&text)), None),
        Err(reason) => (None, Some(format!("info_failed: {reason}"))),
    }
}

/// `INFO` is `key:value` lines grouped by `# Section`. Only the keys the panel shows are read; a
/// missing key stays absent rather than becoming zero.
pub fn parse_redis_info(text: &str) -> RedisSection {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut keys: BTreeMap<String, u64> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.starts_with("db") && value.contains("keys=") {
            if let Some(count) = value
                .split(',')
                .find_map(|part| part.strip_prefix("keys="))
                .and_then(|count| count.parse::<u64>().ok())
            {
                keys.insert(key.to_string(), count);
            }
            continue;
        }
        fields.insert(key, value);
    }
    let number = |key: &str| -> Option<u64> { fields.get(key).and_then(|v| v.parse().ok()) };
    let float = |key: &str| -> Option<f64> { fields.get(key).and_then(|v| v.parse().ok()) };

    RedisSection {
        used_memory_bytes: number("used_memory"),
        used_memory_rss_bytes: number("used_memory_rss"),
        used_memory_peak_bytes: number("used_memory_peak"),
        mem_fragmentation_ratio: float("mem_fragmentation_ratio"),
        connected_clients: number("connected_clients"),
        ops_per_sec: number("instantaneous_ops_per_sec"),
        keys,
        expired_keys: number("expired_keys"),
        evicted_keys: number("evicted_keys"),
        keyspace_hits: number("keyspace_hits"),
        keyspace_misses: number("keyspace_misses"),
        process_id: number("process_id"),
        uptime_seconds: number("uptime_in_seconds"),
        persistence_ok: fields
            .get("rdb_last_bgsave_status")
            .map(|status| *status == "ok"),
        rdb_last_save_at: number("rdb_last_save_time").map(|secs| secs as i64 * 1000),
    }
}

/// Enforces the 64 KB cap (contract V7) by dropping the least load-bearing detail first: per-plugin
/// process rows, then cgroup per-device I/O. Everything a bug report needs — totals, cgroup memory,
/// the process tree's own summary, queue and Redis numbers — survives.
fn truncate(snapshot: &mut ResourceSnapshot) {
    if serialized_len(snapshot) <= MAX_SNAPSHOT_BYTES {
        return;
    }
    snapshot.plugins.processes.clear();
    snapshot.truncations.push("plugins.processes".to_string());
    if serialized_len(snapshot) <= MAX_SNAPSHOT_BYTES {
        return;
    }
    if let Some(cgroup) = snapshot.cgroup.as_mut() {
        cgroup.io_stat.clear();
        snapshot.truncations.push("cgroup.io_stat".to_string());
    }
    if serialized_len(snapshot) <= MAX_SNAPSHOT_BYTES {
        return;
    }
    // Last resort: the process tree's individual rows, keeping the count via `truncated`.
    let entries = snapshot.processes.tree.len();
    snapshot.processes.tree.truncate(8);
    snapshot.processes.truncated = true;
    snapshot
        .truncations
        .push(format!("processes.tree ({entries} rows)"));
}

fn serialized_len(snapshot: &ResourceSnapshot) -> usize {
    serde_json::to_vec(snapshot)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_the_nearest_rank_and_never_panic_on_empty_input() {
        assert_eq!(percentile(&[], 50), 0);
        assert_eq!(percentile(&[10], 95), 10);
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&values, 50), 50);
        assert_eq!(percentile(&values, 95), 95);
    }

    #[test]
    fn redis_info_keeps_only_the_keys_the_panel_shows() {
        let text = "# Server\r\nredis_version:8.0.1\r\nprocess_id:42\r\nuptime_in_seconds:900\r\n\
                    # Stats\r\nused_memory:1024\r\nused_memory_rss:2048\r\nused_memory_peak:4096\r\n\
                    mem_fragmentation_ratio:1.15\r\nconnected_clients:3\r\ninstantaneous_ops_per_sec:17\r\n\
                    keyspace_hits:90\r\nkeyspace_misses:10\r\nevicted_keys:0\r\nexpired_keys:5\r\n\
                    # Persistence\r\nrdb_last_bgsave_status:ok\r\nrdb_last_save_time:1700000000\r\n\
                    # Keyspace\r\ndb0:keys=12763,expires=0,avg_ttl=0\r\ndb2:keys=14,expires=0,avg_ttl=0\r\n";
        let section = parse_redis_info(text);
        assert_eq!(section.process_id, Some(42));
        assert_eq!(section.used_memory_rss_bytes, Some(2048));
        assert_eq!(section.mem_fragmentation_ratio, Some(1.15));
        assert_eq!(section.persistence_ok, Some(true));
        assert_eq!(section.keys.get("db0"), Some(&12763));
        assert_eq!(section.keys.get("db2"), Some(&14));
        assert_eq!(section.rdb_last_save_at, Some(1_700_000_000_000));
    }

    #[test]
    fn an_info_reply_without_a_key_leaves_it_absent_rather_than_zero() {
        let section = parse_redis_info("redis_version:8.0.1\r\n");
        assert!(section.used_memory_bytes.is_none());
        assert!(section.keys.is_empty());
        assert!(section.persistence_ok.is_none());
    }

    #[test]
    fn the_metadata_cache_counter_is_monotonic() {
        let (hits, misses) = metadata_cache_counters();
        note_metadata_cache(true);
        note_metadata_cache(false);
        let (after_hits, after_misses) = metadata_cache_counters();
        assert_eq!(after_hits, hits + 1);
        assert_eq!(after_misses, misses + 1);
    }
}
