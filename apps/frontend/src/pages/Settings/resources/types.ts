/** The subset of `GET /api/resources` the panel renders (`specs/030-resource-monitor/`).
 *
 * Every number that the server could not read is `null` and carries a sibling `*_unavailable`
 * reason — a `0` here always means "measured, and it was zero", which is why these are `| null`
 * rather than optional. Keep the section names in step with `crates/lanrurugi-api/src/resources/model.rs`. */

export interface ResourceBuild {
  profile: "debug" | "release" | string
  binary: string
}

export interface ResourceServer {
  pid: number
  rss_bytes: number | null
  peak_rss_bytes: number | null
  swap_bytes: number | null
  threads: number | null
  fd_count: number | null
  fd_limit: number | null
  cpu_percent_1s: number | null
  cpu_seconds_total: number
  read_bytes: number | null
  write_bytes: number | null
}

export interface ResourcePsi {
  some10: number | null
  some60: number | null
  full10: number | null
  full60: number | null
}

export interface ResourceMemoryEvents {
  low: number | null
  high: number | null
  max: number | null
  oom: number | null
  oom_kill: number | null
}

export interface ResourceIoStat {
  device: string
  read_bytes: number
  write_bytes: number
}

export interface ResourceCgroup {
  path: string
  memory_current_bytes: number | null
  /** True by definition: `memory.current` counts page cache. The panel must say so. */
  includes_page_cache: boolean
  anon_bytes: number | null
  file_bytes: number | null
  slab_bytes: number | null
  peak_bytes: number | null
  swap_current_bytes: number | null
  pids_current: number | null
  cpu_usage_usec: number | null
  events: ResourceMemoryEvents
  psi_memory: ResourcePsi
  io_stat: ResourceIoStat[]
}

export interface ResourceRuntime {
  cpu_cores: number
  tracked_tasks: number
  queue_depth: number
}

export interface ResourcePluginProcess {
  namespace: string
  pid: number | null
  /** A call is in flight (or the slot was locked), so `pid`/`rss_bytes` may be stale or missing. */
  busy: boolean
  rss_bytes: number | null
  cpu_percent_since: number | null
  started_at: number | null
  last_call_at: number | null
  calls: number
  failures: number
  persistent: boolean
}

export interface ResourcePlugins {
  limit: number
  transient_available: number
  transient_in_flight: number
  transient_spawned_total: number
  processes: ResourcePluginProcess[]
}

export interface ResourceProcessEntry {
  pid: number
  ppid: number
  depth: number
  comm: string
  rss_bytes: number | null
  cpu_percent: number | null
  started_at: number | null
}

export interface ResourceProcesses {
  root_pid: number
  tree: ResourceProcessEntry[]
  truncated: boolean
}

export interface ResourceModelUsage {
  feature: string
  requests: number
  failures: number
  elapsed_ms_total: number
  tokens_in: number
  tokens_out: number
}

export interface ResourceModels {
  usage: ResourceModelUsage[]
  in_process: { name: string; loaded: string }[]
  note: string
}

export interface ResourceQueues {
  download_active: number
  download_queued: number
  download_error: number
  jobs_active: number
  jobs_failed_total: number
  phase_p50_ms: Record<string, number>
  phase_p95_ms: Record<string, number>
  permit_wait_p50_ms: number | null
}

export interface ResourceRedis {
  used_memory_bytes: number | null
  used_memory_rss_bytes: number | null
  used_memory_peak_bytes: number | null
  mem_fragmentation_ratio: number | null
  connected_clients: number | null
  ops_per_sec: number | null
  keys: Record<string, number>
  expired_keys: number | null
  evicted_keys: number | null
  keyspace_hits: number | null
  keyspace_misses: number | null
  process_id: number | null
  uptime_seconds: number | null
  persistence_ok: boolean | null
  rdb_last_save_at: number | null
}

export interface ResourceNetwork {
  rx_bytes: number
  tx_bytes: number
  rx_rate_bps: number | null
  tx_rate_bps: number | null
}

export interface ResourceCaches {
  descriptor_hits: number
  descriptor_misses: number
  metadata_hits: number
  metadata_misses: number
}

export interface ResourceDisk {
  free_bytes: number
  total_bytes: number
  free_inodes: number
}

export interface ResourceGpu {
  vendor: string
  utilization_percent: number | null
  memory_used_bytes: number | null
  memory_total_bytes: number | null
  temperature_c: number | null
  unavailable: string | null
}

export interface ResourceWatch {
  observers: number
  samples_taken: number
  sampler_running: boolean
}

export interface ResourceSnapshot {
  at: number
  monotonic_ms: number
  window_ms: number
  version: string
  release: string
  platform: string
  build: ResourceBuild
  server: ResourceServer
  cgroup: ResourceCgroup | null
  cgroup_unavailable: string | null
  runtime: ResourceRuntime
  plugins: ResourcePlugins
  processes: ResourceProcesses
  models: ResourceModels
  queues: ResourceQueues
  redis: ResourceRedis | null
  redis_unavailable: string | null
  network: ResourceNetwork
  caches: ResourceCaches
  library: { archives: number; total_bytes: number }
  storage: { library: ResourceDisk; temp: ResourceDisk }
  gpu: ResourceGpu | null
  gpu_unavailable: string | null
  watch: ResourceWatch
  truncations: string[]
}

/** One frame of the stream: small enough to keep 1800 of them in memory. */
export interface ResourceSample {
  at: number
  monotonic_ms: number
  cpu_percent_1s: number | null
  rss_bytes: number
  anon_bytes: number
  file_bytes: number
  pids: number
  fd_count: number
  psi_memory_full10: number | null
  tracked_tasks: number
  queue_active: number
  queue_queued: number
  plugins_alive: number
}
