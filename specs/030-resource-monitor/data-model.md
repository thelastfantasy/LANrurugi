# Data Model: 资源监视

## E1 ResourceSnapshot

一次完整的观测。所有可选数值用 `null` + `unavailable` 表达，不用 0（0 会被当作真实读数）。

```text
ResourceSnapshot
  at                 采样时刻（毫秒，墙钟）
  monotonic_ms       采样时刻（单调时钟，用于窗口计算）
  window_ms          本快照中速率类字段的窗口（首帧为 0）
  version/release/platform   与反馈包 diagnostics.json 同源
  server             ServerSection
  cgroup             CgroupSection | null
  runtime            RuntimeSection
  plugins            PluginSection
  queues             QueueSection
  redis              RedisSection | null
  network            NetworkSection
  caches             CacheSection
  library            LibrarySection
  storage            StorageSection
  gpu                GpuSection | null
  watch              WatchSection          # observers / samples_taken / sampler_running
```

`BuildSection`：`profile`（`"debug"` / `"release"`，来自编译期 `cfg!(debug_assertions)`）、`binary`（`lanrurugi-server`）。

`ServerSection`：`pid`、`rss_bytes`、`peak_rss_bytes`、`swap_bytes`、`threads`、`fd_count`、`fd_limit`、`cpu_percent_1s`（首帧 `null`）、`cpu_seconds_total`、`read_bytes`、`write_bytes`。

`CgroupSection`：`path`、`memory_current_bytes`（附 `includes_page_cache: true`）、`anon_bytes`、`file_bytes`、`slab_bytes`、`peak_bytes`、`swap_current_bytes`、`pids_current`、`cpu_usage_usec`、`events: {low, high, max, oom, oom_kill}`、`psi_memory: {some10, some60, full10, full60}`、`io_stat: [{device, read_bytes, write_bytes}]`。

`RuntimeSection`：`cpu_cores`（`std::thread::available_parallelism()`）、`tracked_tasks`（本进程自己维护的任务计数：采样任务、后台任务、在飞插件调用各加一，结束减一）、`queue_depth`（`JobRegistry` 里排队中的任务数）。tokio 的 `RuntimeMetrics` 在 `cfg(tokio_unstable)` 后面，第一版不启用，因此没有 `alive_tasks`/`global_queue_depth` 这两个字段。

`PluginSection`：`limit`（`worker_limit()`）、`transient_available`、`transient_in_flight`、`transient_spawned_total`、`processes: [PluginProcess]`。

`ProcessSection`：`root_pid`、`tree: [ProcessEntry]`、`truncated`（超过 64 个进程或深度 3 时置 true）。

`ProcessEntry`：`pid`、`ppid`、`depth`、`comm`（`/proc/<pid>/stat` 的第 2 字段，含 `deno`、`lanrurugi-book-worker` 这类名字）、`rss_bytes`、`cpu_percent`（相邻两次采样之差，首帧 `null`）、`started_at`。

`ModelsSection`：`usage: [ModelUsage]`（当日 UTC，来自 `LANRURUGI_LLM_USAGE:<feature>:<date>` 这个 hash）、`in_process: [InProcessModel]`、`note`（进程内模型与主进程共用地址空间的说明）。

`ModelUsage`：`feature`、`requests`、`failures`、`elapsed_ms_total`、`tokens_in`、`tokens_out`。

`InProcessModel`：`name`（`embed`/`ocr`/`translate`/`inpaint`）、`loaded`（`true`/`false`/`unknown`）。

## E2 PluginProcess

```text
PluginProcess
  namespace          "download/ehentai"
  pid                number | null        # null = 槽锁被占用，无法读取
  busy               bool
  rss_bytes          number | null
  cpu_percent_since  number | null        # 相邻两次采样之差
  started_at         number | null
  last_call_at       number | null        # 由插件池已有的调用记录提供
  calls / failures   number
  persistent         bool                 # 常驻 true，声明/自省用的一次性 worker 不在此表
```

## E3 Sample（历史与 SSE 帧）

```text
Sample
  at, monotonic_ms
  cpu_percent_1s, rss_bytes, anon_bytes, file_bytes, pids, fd_count
  psi_memory_full10, psi_io_some10, psi_cpu_some10
  tracked_tasks, queue_active, queue_queued, plugins_alive
```

一帧约 160–220 B；30 分钟 × 1 Hz ≈ 1800 帧 ≈ 400 KB，只在采样任务内存里。

## E4 ThresholdAlert

```text
ThresholdAlert
  kind          "oom_kill" | "memory_psi" | "fd_pressure"
  value         触发值
  at            触发时刻
  snapshot      Sample 的摘要（不是完整快照，避免诊断库膨胀）
```

去重键 = `kind`；同一 `kind` 10 分钟内只产生一条诊断事件。

## E5 ObserverLease

```text
ObserverLease
  observers          当前观察者数（原子计数）
  samples_taken      累计样本数（单调，测试与面板都用它验证"关掉即停"）
  sampler_running    采样任务是否存在
```

`ObserverLease::drop` → 计数减一；归零时 `abort` 任务、清空历史、`samples_taken` 保留（用于验证停止）。

## V 不变量

- V1 任一可选字段为 `null` 时必须同时给 `unavailable`（字符串，说明原因），不得只给 `null`。
- V2 `window_ms == 0` 时所有速率/百分比字段为 `null`。
- V3 `watch.observers == 0 ⇒ watch.sampler_running == false`，且 `samples_taken` 在两个时刻之间不增长。
- V4 `CgroupSection.memory_current_bytes` 存在时 `includes_page_cache` 必须为 `true`（防止 UI 把它当 RSS）。
- V5 `PluginProcess.pid == null ⇒ busy == true`。
- V6 历史长度 ≤ 1800 帧；超限丢最旧。
- V7 快照序列化后 ≤ 64 KB；超出时先丢 `plugins.processes` 明细，再丢 `cgroup.io_stat`，并在 `truncations` 里记录。
