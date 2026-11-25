# Research: 资源监视的取数方式与成本边界

本文件只记录"为什么这样取数"，不重复 spec 里的需求。每条结论都对应一个可验证的实现约束。

## R1 采样必须由观察者持有，而不是进程生命周期

**备选**：① 常驻 1 Hz 采样（最简单，图表随取随有）；② 每次请求现采（无历史）；③ 租约式（首个观察者 spawn、最后一个 abort）。

**决定：③**。①的直接代价是 24 h 内 86 400 次采样 + 一份常驻环形缓冲，且违反 FR-001（"没有观察者时不得有采样任务"）与用户原话"不在设置页打开的时候就是0占用"。②无法回答"刚才那波慢是不是内存压力"，而这个仓库里三次事故全部是"间歇性的"。③的代价是每次打开面板有一段空白历史，这与 spec US3 的验收一致（关闭再打开从零开始）。

**实现约束**：`observers: AtomicUsize` + `sampler: Mutex<Option<JoinHandle>>` + `Mutex<RingBuffer>`；`attach()` 用一次 CAS 判断"是否由我从 0 变 1"；`Drop` 归零时 `abort()` 并 `clear()`。

## R2 用 SSE 而不是 1 Hz 轮询

仓库已有 4 处 SSE 先例（`subscriptions/api.rs` 的 `/subscriptions/stream`、`snapshots/jobs.rs`、`plugin_wizard/generate.rs`、`duplicates/mod.rs`），都配 `KeepAlive`。1 Hz 轮询每次要传一份完整快照（约 20–30 KB），等于 30 KB/s 的持续流量，且每次都要重新序列化；SSE 首帧发完整快照、后续只发紧凑样本（时间 + 十几个数字）。

**决定**：`GET /api/resources/stream` 首帧 `event: snapshot`，之后 `event: sample`，`KeepAlive::default()`；一次性导出/反馈包走 `GET /api/resources`，它不注册观察者。

## R3 取数口径（Linux 优先，其它平台降级）

| 指标 | 文件/接口 | 备注 |
|---|---|---|
| 进程 RSS / 峰值 / 线程 / swap | `/proc/self/status`（`VmRSS`/`VmHWM`/`Threads`/`VmSwap`） | 与 `statm` 互为校验 |
| fd 数 | `/proc/self/fd` 条目数 | 泄漏的第一信号 |
| CPU | `/proc/self/stat` 的 `utime+stime`（两次采样做差） | 首帧必须显示 `—` |
| IO | `/proc/self/io`（`read_bytes`/`write_bytes`）+ cgroup `io.stat` | 累计值 + 速率窗口 |
| cgroup 内存 | `memory.current`/`memory.peak`/`memory.stat`/`memory.events`/`memory.swap.current` | 必须拆 anon/file/slab |
| cgroup 压力 | `memory.pressure`、`cpu.stat`、`pids.current` | v2 才有 |
| 全局压力 | `/proc/pressure/{cpu,memory,io}` 的 `some`/`full` | 本机实测可读（`some avg10=0.00`） |
| 插件进程 | `PluginPool` 的 worker 表 + `/proc/<pid>/statm`、`/proc/<pid>/stat` | 只读自己起的子进程 |
| Tokio | `Handle::metrics()`（`num_workers`/`num_alive_tasks`/`global_queue_depth`） | 免费 |
| Redis | `INFO`（`used_memory`、`db*`、`rdb_last_bgsave_status`、`aof_last_write_status`） | 一次 RTT |
| 磁盘 | `statvfs`（库盘、temp 盘分开） | 含 inode |
| 网络 | `/proc/net/dev` 累计收发 | 差值给速率 |
| 库规模 | 搜索索引计数 / Redis 键数 | 归一化分母 |

**非 Linux**：`/proc`、cgroup、PSI 都没有；用 `sysinfo` 的进程级 API 给 CPU/RSS，其余字段 `unavailable: "not linux"`。

## R4 `memory.current` 会撒谎，必须拆开

实测同一次批量下载：容器 `memory.current` 2675 → 4293 MB，而服务进程 `VmRSS` 只从 1653 → 1808 MB——差额全是文件页缓存。2026-10-11 那次 `systemd-oomd` 杀容器的事故也是同一类误读（`memory.high` 贴顶是因为缓存撑满）。

**决定**：面板与 `resources.json` 都以 `memory.stat` 的 `anon`/`file`/`slab` 为主，`current` 只作为附注并标注"含页缓存"；同时给 `memory.peak` 与 `memory.events`（`high`/`max`/`oom`/`oom_kill`）。

## R5 插件进程归属：只读快照，不抢锁

`PluginPool.workers` 是 `HashMap<String, Arc<tokio::sync::Mutex<WorkerSlot>>>`，`execute` 会在槽锁内跑整次调用（可能几秒）。采样若 `lock().await` 就会排在一次下载后面。

**决定**：新增 `pub async fn worker_processes(&self) -> Vec<WorkerProcess>`：先短暂持表锁复制 `Vec<(namespace, Arc<Mutex<WorkerSlot>>)>`，再对每个槽 `try_lock()`——拿到就取 `worker.pid()`，拿不到就返回 `pid: None, busy: true`。表锁持有时间 = 一次 HashMap 克隆。

## R6 阈值与去重

三类：`memory.events` 的 `oom`/`oom_kill` 增长；内存 PSI `full` 的 10 s 均值 > 10%；fd 使用 > `ulimit -n` 的 80%。判据都用"窗口内的值"，避免单帧抖动。

去重：`HashMap<ThresholdKind, i64>` 记上次触发时刻，同类 10 分钟内只写一条 warning 诊断事件（走既有 `state.repos.diagnostics` + `crate::diagnostics::report`，因此也自动进 issues 视图与 rustrak 转发）。

## R7 Sentry / rustrak 联动的三条通道

1. **前端**：`@sentry/react` 已接（`apps/frontend/src/diagnostics/sdk.ts`）。面板打开时每条样本压成 breadcrumb（`category: "resources"`，10 s 节流）；`Sentry.setContext("resources", …)` 让每个错误事件自带现场；关闭面板即停止，不注册任何全局钩子。
2. **后端**：错误事件附 `contexts.resources`，只在最近一次采样 ≤ 30 s 时附，避免陈旧数据误导。走 `crate::diagnostics::report` 既有的 `capture` 路径，不新增上报通道。
3. **反馈包**：`resources.json` 是附件而不是事件，正好满足"用户点上报才产生、平时零成本"。

**脱敏**：三条通道共用 `diagnostics::scrub`（路径、URL 中的 key、cookie、token）。`resources.json` 里出现的下载 URL 与环境变量名必须先过它。

## R8 GPU：第一版单帧、默认关闭

- NVIDIA：`nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits`，一次 spawn 约 20–40 ms（本机无 NVIDIA，未实测；按"可能更慢"对待，给 2 s 超时）。
- AMD：`/sys/class/drm/card*/device/mem_info_vram_used` 与 `gpu_busy_percent`，读文件，免费。
- 决策：默认 **关**（FR-014 要求采样不得 spawn 子进程，除非显式开启）；打开后结果缓存 5 s，且失败只标 `unavailable`。`crates/lanrurugi-gpu-worker/src/vram_budget.rs` 已有显存预算概念，字段命名与之对齐（`vram_used_bytes`/`vram_total_bytes`）。

## R9 采样放在 `spawn_blocking` 里

采集是十几次小文件读（每个插件进程 2 个 `open`），总计个位数毫秒；放在 reactor 上 1 Hz 也能接受，但一次 `/proc` 扫描在极端情况（几百个进程）会挡住同一 worker 上的其它任务。**决定**：整个 `collect()` 走 `tokio::task::spawn_blocking`，配合 `Handle::metrics()` 的读取在主线程完成后再合并。
