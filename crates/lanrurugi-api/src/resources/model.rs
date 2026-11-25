//! The wire shape of one observation (`specs/030-resource-monitor/contracts/resources-snapshot.md`).
//!
//! Two rules drive every `Option` here. A number that could not be read is `None`, never `0` — a
//! zero is a real reading and the panel would happily show it. And a section that could not be read
//! at all is omitted together with a sibling `*_unavailable` string explaining why, so the UI can
//! say "no cgroup on this platform" instead of drawing an empty table.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `-1`-free timestamp helper: `cfg!(debug_assertions)` decides the profile once, at compile time,
/// so a release build can never be relabelled by an environment variable.
fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub at: i64,
    pub monotonic_ms: u64,
    /// The window every rate/percentage in this snapshot was measured over. `0` means "first
    /// frame" — those fields are `None` rather than divided by an unknown window.
    pub window_ms: u64,
    pub version: String,
    pub release: String,
    pub platform: String,
    pub build: BuildSection,
    pub server: ServerSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup: Option<CgroupSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup_unavailable: Option<String>,
    pub runtime: RuntimeSection,
    pub plugins: PluginSection,
    pub processes: ProcessSection,
    pub models: ModelsSection,
    pub queues: QueueSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis: Option<RedisSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis_unavailable: Option<String>,
    pub network: NetworkSection,
    pub caches: CacheSection,
    pub library: LibrarySection,
    pub storage: StorageSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_unavailable: Option<String>,
    pub watch: WatchSection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub truncations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSection {
    /// `debug` or `release`, from the compiler — never from the environment.
    pub profile: String,
    pub binary: String,
}

impl Default for BuildSection {
    fn default() -> Self {
        Self {
            profile: build_profile().to_string(),
            binary: "lanrurugi-server".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerSection {
    pub pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fd_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fd_limit: Option<u64>,
    /// `None` on the first frame of a process (no previous counter to subtract).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent_1s: Option<f64>,
    pub cpu_seconds_total: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CgroupSection {
    pub path: String,
    /// `memory.current`, which counts page cache. The flag is not decoration: reading it as RSS is
    /// the mistake this field exists to prevent, so it is always present and always `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_current_bytes: Option<u64>,
    pub includes_page_cache: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anon_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slab_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_current_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids_current: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    pub events: MemoryEvents,
    pub psi_memory: Psi,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub io_stat: Vec<IoStat>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryEvents {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oom: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oom_kill: Option<u64>,
}

/// One `/proc/pressure/*` file: `some` is "at least one task stalled", `full` is "everything
/// stalled", each over a 10 s and a 60 s window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Psi {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub some10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub some60: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full10: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full60: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoStat {
    pub device: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeSection {
    pub cpu_cores: usize,
    /// Tasks this process knows about itself, counted by `resources::tracked`. Deliberately not
    /// tokio's own metrics: those need `--cfg tokio_unstable`, which is a workspace-wide build flag
    /// this feature does not get to impose (research R10).
    pub tracked_tasks: u64,
    pub queue_depth: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSection {
    /// The automatic cap on throwaway declaration processes (`pool::worker_limit`).
    pub limit: usize,
    pub transient_available: u64,
    pub transient_in_flight: u64,
    pub transient_spawned_total: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<PluginProcess>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginProcess {
    pub namespace: String,
    /// `None` exactly when `busy` — the slot's lock was held by an in-flight call, so nothing about
    /// the process could be read without waiting for that call to finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub busy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent_since: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_call_at: Option<i64>,
    pub calls: u64,
    pub failures: u64,
    pub persistent: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessSection {
    pub root_pid: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<ProcessEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub depth: u32,
    /// `/proc/<pid>/stat`'s second field, truncated to 15 characters by the kernel.
    pub comm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelsSection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub usage: Vec<ModelUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_process: Vec<InProcessModel>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub feature: String,
    pub requests: u64,
    pub failures: u64,
    pub elapsed_ms_total: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InProcessModel {
    pub name: String,
    /// `true`/`false` when that is cheap and honest to read, `unknown` otherwise — never a guess.
    pub loaded: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueSection {
    pub download_active: u64,
    pub download_queued: u64,
    pub download_error: u64,
    pub jobs_active: u64,
    pub jobs_failed_total: u64,
    /// Per phase name (queued/negotiate/waiting/transfer/ingest/publish), over the jobs the
    /// registry still holds. `p50 > p95` never happens; a phase with no closed sample is absent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phase_p50_ms: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phase_p95_ms: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permit_wait_p50_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedisSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_memory_bytes: Option<u64>,
    /// The Redis *process's* resident memory — the number a container's RSS view shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_memory_rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_memory_peak_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem_fragmentation_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_clients: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_per_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired_keys: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evicted_keys: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyspace_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyspace_misses: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistence_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rdb_last_save_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkSection {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx_rate_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_rate_bps: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheSection {
    /// Plugin descriptor cache (`pool::PluginPool::info_cache`), the one 029 built.
    pub descriptor_hits: u64,
    pub descriptor_misses: u64,
    /// The queue row's `metadata_preview` cache (`plugins::ensure_metadata_cached`).
    pub metadata_hits: u64,
    pub metadata_misses: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibrarySection {
    pub archives: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageSection {
    pub library: DiskStat,
    pub temp: DiskStat,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskStat {
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub free_inodes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSection {
    pub vendor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utilization_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_used_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct WatchSection {
    pub observers: usize,
    pub samples_taken: u64,
    pub sampler_running: bool,
}

/// One history point — small on purpose: 30 minutes at 1 Hz is ~1800 of these in memory, and the
/// panel draws them as sparklines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sample {
    pub at: i64,
    pub monotonic_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent_1s: Option<f64>,
    pub rss_bytes: u64,
    pub anon_bytes: u64,
    pub file_bytes: u64,
    pub pids: u64,
    pub fd_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psi_memory_full10: Option<f64>,
    pub tracked_tasks: u64,
    pub queue_active: u64,
    pub queue_queued: u64,
    pub plugins_alive: u64,
}

impl ResourceSnapshot {
    /// The compact history point this snapshot also represents. Absent numbers become `0` here
    /// rather than `None` because a sparkline is a shape, not a reading — the panel's numeric rows
    /// read the snapshot itself.
    pub fn sample(&self) -> Sample {
        Sample {
            at: self.at,
            monotonic_ms: self.monotonic_ms,
            cpu_percent_1s: self.server.cpu_percent_1s,
            rss_bytes: self.server.rss_bytes.unwrap_or(0),
            anon_bytes: self.cgroup.as_ref().and_then(|c| c.anon_bytes).unwrap_or(0),
            file_bytes: self.cgroup.as_ref().and_then(|c| c.file_bytes).unwrap_or(0),
            pids: self
                .cgroup
                .as_ref()
                .and_then(|c| c.pids_current)
                .unwrap_or(0),
            fd_count: self.server.fd_count.unwrap_or(0),
            psi_memory_full10: self.cgroup.as_ref().and_then(|c| c.psi_memory.full10),
            tracked_tasks: self.runtime.tracked_tasks,
            queue_active: self.queues.download_active,
            queue_queued: self.queues.download_queued,
            plugins_alive: self.plugins.processes.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            at: 1,
            monotonic_ms: 1,
            window_ms: 0,
            version: "0.1.0".into(),
            release: "abc".into(),
            platform: "linux/x86_64".into(),
            build: BuildSection::default(),
            server: ServerSection {
                pid: 42,
                ..Default::default()
            },
            cgroup: None,
            cgroup_unavailable: Some("not linux".into()),
            runtime: RuntimeSection::default(),
            plugins: PluginSection::default(),
            processes: ProcessSection::default(),
            models: ModelsSection::default(),
            queues: QueueSection::default(),
            redis: None,
            redis_unavailable: Some("no connection".into()),
            network: NetworkSection::default(),
            caches: CacheSection::default(),
            library: LibrarySection::default(),
            storage: StorageSection::default(),
            gpu: None,
            gpu_unavailable: Some("disabled".into()),
            watch: WatchSection::default(),
            truncations: Vec::new(),
        }
    }

    /// V2: a first frame has no window, so its rates are absent rather than `0.0`.
    #[test]
    fn a_zero_window_leaves_the_rate_fields_absent() {
        let value = serde_json::to_value(snapshot()).expect("serialises");
        assert!(value["server"].get("cpu_percent_1s").is_none());
        assert_eq!(value["window_ms"], 0);
    }

    /// V1: an unreadable section carries its reason next to it instead of silently vanishing.
    #[test]
    fn an_unavailable_section_keeps_its_reason() {
        let value = serde_json::to_value(snapshot()).expect("serialises");
        assert!(value.get("cgroup").is_none());
        assert_eq!(value["cgroup_unavailable"], "not linux");
        assert_eq!(value["gpu_unavailable"], "disabled");
    }

    #[test]
    fn the_build_profile_follows_the_compiler_not_the_environment() {
        let build = BuildSection::default();
        let expected = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        assert_eq!(build.profile, expected);
        assert_eq!(build.binary, "lanrurugi-server");
    }

    /// A sample is the compact form: absent numbers collapse to `0`, and every key stays present so
    /// the sparklines have a fixed shape to index into.
    #[test]
    fn a_sample_flattens_absent_numbers_instead_of_dropping_keys() {
        let sample = snapshot().sample();
        assert_eq!(sample.rss_bytes, 0);
        assert_eq!(sample.anon_bytes, 0);
        assert!(sample.cpu_percent_1s.is_none());
    }
}
