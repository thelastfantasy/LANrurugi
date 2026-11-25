# Contract: ResourceSnapshot 形状

字段级契约。所有 `number` 单位为字节、毫秒、秒或百分比（字段名自带单位）。

```jsonc
{
  "at": 1795590000000,
  "monotonic_ms": 812345,
  "window_ms": 1000,
  "version": "1.0.0-beta.8",
  "release": "f9e81ce",
  "platform": "linux/x86_64",
  "build": { "profile": "debug", "binary": "lanrurugi-server" },
  "server": {
    "pid": 42,
    "rss_bytes": 1405091840,
    "peak_rss_bytes": 1551892480,
    "swap_bytes": 0,
    "threads": 38,
    "fd_count": 214,
    "fd_limit": 1048576,
    "cpu_percent_1s": 3.2,
    "cpu_seconds_total": 812.44,
    "read_bytes": 14502338560,
    "write_bytes": 3398834176
  },
  "cgroup": {
    "path": "/user.slice/user-1000.slice/user@1000.service/user.slice/libpod-abc.scope",
    "memory_current_bytes": 2103447552,
    "includes_page_cache": true,
    "anon_bytes": 1400905728,
    "file_bytes": 702103552,
    "slab_bytes": 23429120,
    "peak_bytes": 4293001216,
    "swap_current_bytes": 0,
    "pids_current": 218,
    "cpu_usage_usec": 945612340,
    "events": { "low": 0, "high": 42, "max": 0, "oom": 0, "oom_kill": 0 },
    "psi_memory": { "some10": 0.0, "some60": 0.0, "full10": 0.0, "full60": 0.0 },
    "io_stat": [ { "device": "8:0", "read_bytes": 0, "write_bytes": 0 } ]
  },
  "runtime": { "cpu_cores": 12, "tracked_tasks": 176, "queue_depth": 2 },
  "plugins": {
    "limit": 6,
    "transient_available": 6,
    "processes": [
      { "namespace": "download/ehentai", "pid": 3311, "busy": false, "rss_bytes": 44040192,
        "cpu_percent_since": 0.1, "started_at": 1795589000000, "last_call_at": 1795589990000,
        "calls": 42, "failures": 1, "persistent": true }
    ]
  },
  "queues": {
    "download_active": 1, "download_queued": 4, "download_error": 0,
    "jobs_active": 2, "jobs_failed_total": 3,
    "phase_p50_ms": { "negotiate": 3200, "transfer": 21100, "ingest": 1400 },
    "phase_p95_ms": { "negotiate": 5500, "transfer": 71000, "ingest": 3200 },
    "permit_wait_p50_ms": 900
  },
  "redis": {
    "used_memory_bytes": 104857600, "used_memory_rss_bytes": 138412032,
    "used_memory_peak_bytes": 134217728, "mem_fragmentation_ratio": 1.2,
    "connected_clients": 12, "ops_per_sec": 340,
    "keys": { "0": 408, "2": 12763, "3": 12900 },
    "expired_keys": 132, "evicted_keys": 0,
    "keyspace_hits": 918234, "keyspace_misses": 4012,
    "process_id": 12, "uptime_seconds": 86400,
    "persistence_ok": true, "rdb_last_save_at": 1795589000000
  },
  "processes": {
    "root_pid": 42,
    "truncated": false,
    "tree": [
      { "pid": 42, "ppid": 1, "depth": 0, "comm": "lanrurugi-serv", "rss_bytes": 1405091840, "cpu_percent": 3.2, "started_at": 1795500000000 },
      { "pid": 3311, "ppid": 42, "depth": 1, "comm": "deno", "rss_bytes": 44040192, "cpu_percent": 0.1, "started_at": 1795589000000 },
      { "pid": 3402, "ppid": 42, "depth": 1, "comm": "lanrurugi-book", "rss_bytes": 90177536, "cpu_percent": 0.0, "started_at": 1795589100000 }
    ]
  },
  "models": {
    "usage": [ { "feature": "tankoubon_grouping_review", "requests": 12, "failures": 1, "elapsed_ms_total": 8123, "tokens_in": 42100, "tokens_out": 1900 } ],
    "in_process": [ { "name": "embed", "loaded": true }, { "name": "ocr", "loaded": "unknown" } ],
    "note": "in-process ONNX models share this process's address space; their memory is inside server.rss_bytes"
  },
  "network": { "rx_bytes": 8123456789, "tx_bytes": 1234567890, "rx_rate_bps": 4200000, "tx_rate_bps": 310000 },
  "caches": { "descriptor_hits": 1840, "descriptor_misses": 21, "metadata_hits": 96, "metadata_misses": 8 },
  "library": { "archives": 408, "total_bytes": 812345678901 },
  "storage": {
    "library": { "free_bytes": 812345678901, "total_bytes": 4000000000000, "free_inodes": 812345 },
    "temp": { "free_bytes": 51234567890, "total_bytes": 100000000000, "free_inodes": 51234 }
  },
  "gpu": null,
  "gpu_unavailable": "no NVIDIA tool and no AMD sysfs device",
  "watch": { "observers": 1, "samples_taken": 137, "sampler_running": true },
  "truncations": []
}
```

## 规则

0. `build.profile` 是编译期常量（`cfg!(debug_assertions)`），运行期不可被环境变量改写；它为 `debug` 时，凡是把进程名渲染成文本的地方（面板、`toText`、`resources.json` 的 `notes`）都要写成 `name (debug)`。
1. 任何 `null` 都必须有一个同级或父级的 `*_unavailable` 字符串说明原因（`V1`）。整体不可用的分节直接省略并在 `unavailable` 里写原因（如 `gpu`）。
2. `window_ms == 0` 时 `*_rate_*`、`*_percent_*` 为 `null`（`V2`）。
3. `plugins.processes` 只含常驻 worker；一次性声明进程只体现在 `transient_available` / `transient_in_flight` / `transient_spawned_total`。
3a. `processes.tree` 是 `/proc` 里的真实父子关系（`ppid` 字段），从 `root_pid` 出发深度 ≤ 3、条目 ≤ 64；超限时 `truncated: true` 并优先保留浅层。
3b. `models.usage` 只统计当日 UTC，来自 Redis 的 `LANRURUGI_LLM_USAGE:<feature>:<date>`；Redis 不可用时整个 `redis` 分节省略、`models.usage` 为空数组且 `note` 说明原因。
4. `cgroup.memory_current_bytes` 必须与 `includes_page_cache: true` 同时出现（`V4`）。
5. 序列化 ≤ 64 KB，超限截断顺序与记录见 `data-model.md` V7。
