# Tasks: 应用内资源监视：设置页的资源面板与反馈包里的资源快照

**Feature**: `specs/030-resource-monitor/` | **Branch**: `feat/030-resource-monitor`
**Inputs**: [spec.md](./spec.md) · [plan.md](./plan.md) · [research.md](./research.md) · [data-model.md](./data-model.md) · [contracts/](./contracts/) · [quickstart.md](./quickstart.md)

约定：每个任务以「文件 + 验收方式」结尾；Rust 测试一律 `mise run test-one -- <crate> <filter>`，且进程要 `setsid nohup` 脱离代理树；前端验证是 `npx tsc -b` + `npx eslint` + `npx vitest run <file>`。

## Phase 1 — 采集基础（US1/US2 共用）

- [ ] **T001** [P] `crates/lanrurugi-api/src/resources/model.rs`：按 `contracts/resources-snapshot.md` 定义 `ResourceSnapshot`、`ServerSection`、`CgroupSection`、`RuntimeSection`、`PluginSection`/`PluginProcess`、`ProcessSection`/`ProcessEntry`、`ModelsSection`/`ModelUsage`/`InProcessModel`、`QueueSection`、`RedisSection`、`NetworkSection`、`CacheSection`、`LibrarySection`、`StorageSection`、`BuildSection`（`profile` 来自 `cfg!(debug_assertions)`、`binary`）、`Sample`；`#[serde(skip_serializing_if = "Option::is_none")]` 决定哪些键在缺值时省略、哪些必须显式为 `null`（V1 要求配 `unavailable` 说明）。
  验收：`resources::model` 单测断言 `window_ms: 0` 时 `rx_rate_bps`/`cpu_percent_1s` 为 `null`（V2）、序列化后不含 `undefined`-style 的 `0` 占位，以及 `build.profile` 在 `cfg!(debug_assertions)` 下等于 `"debug"`。

- [ ] **T002** [P] `crates/lanrurugi-api/src/resources/procfs.rs`：Linux 采集器。逐个纯函数：`read_status()`（VmRSS/VmHWM/Threads/VmSwap/fd limit）、`read_statm()`、`read_stat()`（utime/stime）、`read_io()`、`fd_count()`、`read_cgroup_path()`、`read_cgroup_memory()`（current/peak/stat/events/swap.current）、`read_psi(path)`、`read_net_dev()`、`read_statvfs(path)`、`read_process(pid)`（statm + stat + starttime + **ppid**）、`process_tree(root_pid, max_depth, max_entries)`（扫一遍 `/proc/*/stat` 建父子关系，再按深度展开；**一次遍历**，不是每个 pid 一次 `read_to_string` 循环）。每个函数返回 `Result<_, Unavailable>`，**不 panic**。
  验收：单测用内联 fixture 文本覆盖 `parse_psi`（`some avg10=1.25 avg60=… full …`）、`parse_memory_stat`（含 `anon`/`file`/`slab`/`inactive_file` 混排）、`parse_memory_events`（`low/high/max/oom/oom_kill`）、`read_net_dev`（过滤 `lo`，只要求汇总正确）、`process_tree`（用一棵手工构造的 ppid 图断言深度截断与 64 条上限）。

- [ ] **T003** [P] `crates/lanrurugi-api/src/resources/portable.rs`：非 Linux 降级。用 `sysinfo::System`（构造失败也不 panic，`recommend_precompute.rs:133` 有同款先例）给出本进程 RSS/CPU/线程数；cgroup、PSI、swap、GPU 全部 `unavailable: "not linux"`。
  验收：单测在 Linux 上也能跑（只断言结构，不断言数值）。

- [ ] **T004** `crates/lanrurugi-api/src/resources/mod.rs`：`pub async fn collect(state: &AppState) -> ResourceSnapshot`——在 `spawn_blocking` 里组装 T002/T003 的读数，合并运行时（`cpu_cores` 用 `std::thread::available_parallelism()`、`tracked_tasks` 读 `ResourceWatch` 的计数器、`queue_depth` 读 `JobRegistry`；**不用 `Handle::metrics()`**，见 research R10）、插件进程（T009）、队列与阶段（读 `JobRegistry` + `DownloadQueueRepository`）、进程树（T002 的 `process_tree`，`root_pid` = 本进程）、本地模型（T006）、Redis `INFO`（`memory`/`stats`/`persistence`/`keyspace`，取 `used_memory_rss`、峰值、碎片率、命中率、淘汰数、`process_id`、`uptime_in_seconds`）、缓存计数、库规模、GPU（T005）；CPU% 用上一次采样保存的 `(utime+stime)` 做差，首帧为 `null`；末尾做 64 KB 截断（V7）。
  验收：`check-crate -- lanrurugi-api` 通过；单测断言 ① 首帧 `cpu_percent_1s == null`、第二帧为 `Some`（用两次 `collect_with` 注入假时钟）；② `tracked_tasks` 在采样任务启动后 ≥ 1、结束后回到原值；③ `processes.tree` 的第一项是 `depth: 0` 的本进程且 `root_pid == tree[0].pid`。

- [ ] **T005** `crates/lanrurugi-api/src/resources/gpu.rs`：`GpuSection` 的可选采集。`LANRURUGI_RESOURCE_GPU=1` 才启用（默认关，FR-006）；NVIDIA 走 `nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits`（2 s 超时），AMD 走 `/sys/class/drm/card*/device/{mem_info_vram_used,gpu_busy_percent}`；结果缓存 5 s；任何失败只产出 `unavailable` + 原因。`probe()` 只通过一个 `GpuProber` trait 拿数据，测试可注入（这样"关闭时不 spawn"是可断言的）。
  验收：`mise run test-one -- lanrurugi-api resources::gpu`；断言 ① 关闭时 `probe()` 返回 `None` 且注入的 prober 计数为 0；② 开启后 5 s 内第二次调用不再调用 prober（缓存命中）；③ prober 返回 `Err` 时 `GpuSection.available == false` 且带 `unavailable` 文本。

- [ ] **T006** `crates/lanrurugi-api/src/resources/models.rs`：`ModelsSection`。`usage` 从 Redis 读当日的 `LANRURUGI_LLM_USAGE:<feature>:<date>`（`lanrurugi_storage::keys::llm_usage_daily_key`，UTC 日期），一次 `SCAN`+`HGETALL` 或按已知 feature 列表逐个 `HGETALL`，结果缓存 5 s（1 Hz 采样不该每次打 Redis）；`in_process` 报 `embed`/`ocr`/`translate`/`inpaint` 的加载状态（能廉价读到就报 `true/false`，否则 `unknown`）；`note` 固定说明进程内模型的内存含在 `server.rss_bytes` 里。
  验收：`mise run test-one -- lanrurugi-api resources::models`；断言 ① 注入的 fake Redis 返回两个 feature 的 hash 时 `usage` 两项且数值正确；② 5 s 内第二次调用不再读 Redis（注入一个计数）；③ Redis 不可用时 `usage` 为空且 `note` 带原因，不影响其它分节。

- [ ] **T007** `crates/lanrurugi-api/src/resources/watch.rs`：`ResourceWatch`——`observers: AtomicUsize`、`sampler: tokio::sync::Mutex<Option<JoinHandle<()>>>`、`history: Mutex<VecDeque<Sample>>`、`samples_taken: AtomicU64`、`last: RwLock<Option<ResourceSnapshot>>`、`tracked_tasks: AtomicUsize`；`attach()`（0→1 才 spawn，返回 `ObserverLease`）、`snapshot()`、`last_within(Duration)`、`interval` 字段。
  验收：单测断言 ① 首次 `attach` 后 `sampler_running == true`；② `drop(lease)` 后 ≤ 2 s `sampler_running == false` 且 `samples_taken` 不再增长（V3）；③ 两个 lease 时只 spawn 一次；④ 归零后 `history` 为空；⑤ `tracked_tasks` 随 attach/detach 加减。

- [ ] **T008** [P] `crates/lanrurugi-api/src/resources/alerts.rs`：三类阈值（`oom`/`oom_kill` 增长、内存 PSI `full10 > 10.0`、fd > limit 的 80%）+ 同类 10 分钟去重；命中时通过 `crate::diagnostics::report` 写一条 warning 事件（`kind = "resources.oom_kill"` 等），context 是 `Sample` 摘要。
  验收：单测断言 ① 第一次命中返回 `Some(alert)`，紧接着 1 秒后再调返回 `None`，10 分钟后可再次命中；② 三个 kind 互不影响；③ 未越过阈值时不调用上报（用注入的 recorder 计数）。

- [ ] **T009** `crates/lanrurugi-plugin/src/pool.rs`：`Worker::pid(&self) -> Option<u32>`（从 `_child.id()`）+ `pub async fn worker_processes(&self) -> Vec<WorkerProcess>`（先克隆 `(namespace, Arc<Mutex<WorkerSlot>>)` 列表、释放表锁，再逐个 `try_lock`，拿不到锁返回 `pid: None, busy: true`）；同时暴露 `transient_available()`、`transient_in_flight()`（已占用槽数）与 `transient_spawned_total()`（累计启动数，`AtomicU64`）。
  验收：`mise run test-one -- lanrurugi-plugin pool::tests`；新用例用 `Scratch` 起一个 worker，断言 `worker_processes` 里该 namespace 的 pid 与 `/proc` 里存在的进程一致，且 `pool::tests` 原有 29 个用例不回归。

- [ ] **T010** 采集层整体校验：`cargo check` 之后跑 `mise run test-one -- lanrurugi-api resources::`，确认 T001–T006 的用例全过且不依赖真实 cgroup 内容（fixture 驱动）。
  验收：日志末尾 `test result: ok. N passed`，N ≥ 14；另外在 dev 容器量一次采样耗时——`for i in $(seq 20); do curl -s -o /dev/null -w '%{time_total}\n' -b /tmp/lrr-cookies.txt localhost:3000/api/resources; done` 的 p95 < 0.05（SC-004），把数字记进 T031 的 Implementation Record。

## Phase 2 — 接口（US2 后端）

- [ ] **T011** 把 `ResourceWatch` 挂到 `AppState`：`crates/lanrurugi-api/src/state.rs` 增加 `pub resources: Arc<ResourceWatch>`，在 `AppState` 构造处初始化（**必须早于 T012**——路由要读它）；`crates/lanrurugi-api/src/lib.rs` 注册 `pub mod resources;`。
  验收：`check-crate -- lanrurugi-api` 通过（此时还没有路由）。

- [ ] **T012** `crates/lanrurugi-api/src/resources/api.rs`：`GET /api/resources`（一次 `collect`，不注册观察者）与 `GET /api/resources/stream`（`axum::response::sse::{Event, KeepAlive, Sse}`，首帧 `snapshot`、之后 `sample`；`tokio_stream::wrappers::BroadcastStream` 或 `mpsc` 二选一，参照 `crates/lanrurugi-api/src/subscriptions/api.rs:33` 的既有写法）；`interval_ms` clamp 到 500–5000；把两条路由挂进 `lib.rs` 的 router。
  验收：`check-crate -- lanrurugi-api`；`resources::api::tests` 用 `tower::ServiceExt::oneshot` 断言 ① 未登录 401；② 访客被拒；③ 管理员 200 且 body 有 `snapshot.watch`；④ `interval_ms=1` 被夹到 500；⑤ 采集失败时 503 且 body 是 `resources unavailable: …`（用一个注入 `collect` 失败的 fake state）。

- [ ] **T013** 权限与文档：`crates/lanrurugi-api/policy/route_policy.csv` 加三行 deny（`token_admin`/`token_guest`/`guest_visitor` 对 `/api/resources/*`）；`docs/api/tag-names.json` 加 `resources` 组（中文「资源监视」/ English "Resources"）；新建 `docs/api/sidecar/resources.yaml` 描述两条路由与响应（引用 `_shared.yaml` 的错误壳）。
  验收：`mise run docs-api -- --check` 输出 `gates passed, nothing written.`；`node apps/docs-site/scripts/generate-api-pages.mjs` exit 0。

## Phase 3 — 反馈包（US1）

- [ ] **T014** `crates/lanrurugi-api/src/diagnostics/bundle.rs`：`Plan` 增加 `extra_files: Vec<(String, String)>`；`plan()` 把 `diagnostics.json`、`config.redacted.json`、`resources.json` 的**文本**放进去（现在只算大小），`archive_to()` 从它写盘——**同时修掉 `diagnostics.json` 为 0 字节、`config.redacted.json` 从不写盘的缺陷**。
  验收：`mise run test-one -- lanrurugi-api diagnostics::bundle`；新增用例断言三个文件在 `entries()` 里字节数 > 2。

- [ ] **T015** `crates/lanrurugi-api/src/diagnostics/bundle.rs`：`Categories` 增加 `#[serde(default = "default_true")] pub resources: bool`；`resources.json` 由 `resources::collect` + `scrub` 生成，计入 64 KB 预算与 `truncations`；`resources.json` 顶层带 `build`（与快照同源），`readme()` 增一行说明。
  验收：用例 ① `resources: true` 时 ZIP 含 `resources.json` 且 `manifest.files` 有记录；② `false` 时不含；③ 造一个含 `?key=secret` 的 URL，断言输出里已脱敏；④ 断言生成的 `resources.json` ≤ 64 KB（SC-006）；⑤ 同一实例连续生成两次，`jq -S 'paths | map(tostring) | join(".")'` 的键集合完全一致（SC-006 的"字段集合稳定"）。

- [ ] **T016** 前端反馈包页签：`apps/frontend/src/diagnostics/api.ts` 的 `BundleSelection.categories` 加 `resources: boolean`；`apps/frontend/src/diagnostics/BundleModal.tsx` 加「资源占用」勾选项（默认 true，与「服务端日志」同排）；14 个 locale 加 `diagnostics.bundle.resources`。
  验收：`npx tsc -b`、`npx eslint`、`node scripts/check-syntax.mjs src tests` 干净；locale JSON 全部可解析（`python3 -c "import json;json.load(...)"`）。

## Phase 4 — 面板（US2/US5 前端）

- [ ] **T017** [P] `apps/frontend/src/api/types.ts`：`ResourceSnapshot` 及其分节类型（与 contract 同名同单位），`Sample`、`PluginProcess`、`WatchState`。
  验收：`npx tsc -b` 通过。

- [ ] **T018** [P] `apps/frontend/src/pages/Settings/resources/useResourceStream.ts`：`EventSource` 订阅 `/api/resources/stream`；`visibilitychange` 隐藏时 `close()`、恢复时重开；重连退避 1 s→5 s；卸载时关闭；返回 `{ snapshot, samples, status, error }`。
  验收：`apps/frontend/tests/unit/resources/useResourceStream.test.tsx` 用假 `EventSource` 断言 ① 挂载即创建；② 隐藏时 `close()` 且不再累加样本；③ 恢复时重建；④ 卸载后无残留监听。

- [ ] **T019** [P] `apps/frontend/src/pages/Settings/resources/format.ts`：`formatBytes`、`formatPercent`、`formatDuration`、`windowLabel(ms)`（"过去 1 s"/"自本进程启动"）、`toText(snapshot)`（复制用，含 version/release/platform 三行）、`toCsv(samples)`、`labelWithProfile(name, profile)`（`profile === "debug"` 时返回 `name (debug)`，否则原样返回；`deno` 进程不经过它）。
  验收：`apps/frontend/tests/unit/resources/format.test.ts` 覆盖 `window_ms == 0` → 百分比显示 `—`、CSV 表头与样本数一致、`labelWithProfile("lanrurugi-server", "debug") === "lanrurugi-server (debug)"` 且 release 时原样。

- [ ] **T020** `apps/frontend/src/pages/Settings/ResourcePanel.tsx`：分节表格（服务进程 / cgroup / 运行时 / 插件进程 / **进程树**（`depth` 缩进，`comm` + pid + RSS + CPU）/ **本地模型**（按 feature 的今日调用与失败，附 in-process 说明）/ 队列 / Redis / 网络与缓存 / 库与磁盘），每行带窗口与来源列；`null` 显示 `—` 并在 title 里给出 `unavailable` 原因；`cgroup.memory_current_bytes` 行标注"含页缓存"；`memory.events.oom_kill > 0` 行用 `.stdbtn-danger` 同一色（新增 `.resources-alert` 类，按 CLAUDE.md 的要求写进 5 个主题文件 `g.css`/`ex.css`/`modern.css`/`modern_red.css`/`modern_clear.css`，不内联色值）；「服务进程」那一行的进程名用 `labelWithProfile`（debug 构建显示 `lanrurugi-server (debug)`）；顶部两个按钮「复制为文本」「导出 CSV」。
  验收：`npx tsc -b` + `npx eslint` 干净；`apps/frontend/tests/unit/resources/resourcePanel.test.tsx` 断言「含页缓存」标注存在、`null` 渲染为 `—`、告警行带 `.resources-alert`、debug 档位下进程名带 `(debug)`、进程树里 `depth > 0` 的行有缩进 class；`grep -c "resources-alert" apps/frontend/public/legacy/themes/*.css` 每个文件 ≥ 1。

- [ ] **T021** 挂载入口：设置页左栏增加「资源」按钮（与「诊断与反馈」同处），打开后渲染 `ResourcePanel`（复用既有面板/弹窗外壳，不新造一层）；14 个 locale 加 `settings.resources.*` 一小组键（标题、复制、导出、各分节名、窗口标签）。
  验收：浏览器打开设置页能看到入口并展开面板（quickstart 场景 10）；14 个 locale 都能 `json.load`。

## Phase 5 — 历史曲线（US3）

- [ ] **T022** [P] `apps/frontend/src/pages/Settings/resources/Sparkline.tsx`：内联 SVG 折线（不引入图表依赖），支持 2–4 条曲线、`null` 断点、hover 显示该点数值；曲线颜色同样走主题类（`.resources-line-1/2/3/4`，5 个主题文件各一条），不写死色值。
  验收：`apps/frontend/tests/unit/resources/sparkline.test.tsx` 断言断点不断线、点数与样本数一致；`grep -c "resources-line-" apps/frontend/public/legacy/themes/*.css` 每个文件 ≥ 4。

- [ ] **T023** `useResourceStream` 增加环形缓冲（1800 帧、超限丢最旧）；面板用 4 条曲线：CPU%、内存 anon、PSI memory full10、队列活跃数。
  验收：单测断言缓冲到 1800 后长度不再增长且首帧被丢弃（V6）；关闭再打开从零开始（US3 场景 2）。

## Phase 6 — 追踪与告警联动（US4）

- [ ] **T024** `crates/lanrurugi-api/src/diagnostics/report.rs`：记录 error 事件时，若 `watch.last_within(30s)` 有值，附 `contexts.resources`（rss、anon、pids、tracked_tasks、plugins_alive、psi_memory_full10、at）。
  验收：`mise run test-one -- lanrurugi-api diagnostics::report`；用例断言 ① 有新鲜样本时附加；② 样本过期（>30 s）时不附加；③ 关掉上报时仍走既有分支不额外上报。

- [ ] **T025** `apps/frontend/src/diagnostics/sdk.ts`：面板打开时每条样本压成 breadcrumb（`category: "resources"`，10 s 节流）并 `Sentry.setContext("resources", …)`；面板关闭即停止。
  验收：`apps/frontend/tests/unit/resources/sentryResources.test.ts` 用 mock 断言 10 s 内最多 1 条 breadcrumb、关闭后不再调用。

- [ ] **T026** 告警事件接入 issues 视图：确认 `resources.*` 前缀的 kind 在既有「问题」列表里可筛可读（`crates/lanrurugi-api/src/diagnostics/issue.rs` 的分组不把它们混进普通错误）。
  验收：`mise run test-one -- lanrurugi-api diagnostics::issue`；quickstart 场景 9 实跑一次。

## Phase 7 — 文档与门禁

- [ ] **T027** [P] 文档：`README.md`「能做什么」加一条（设置页里有资源面板、报障包带资源快照）；`README.en.md`、`README.ja.md` 同步；`docs/comparison-with-lanrarugi.md` 加一条（含"面板关闭后零采样"这一点）；`.env.example` 记 `LANRURUGI_RESOURCE_GPU`（默认关，开启才查 GPU）。
- [ ] **T028** [P] `CLAUDE.md` 的 SPECKIT 块新增 `030-resource-monitor` 条目（状态、模块位置、未验证项）。
- [ ] **T029** 门禁：`mise run fmt`、`mise run clippy`（workspace `--all-targets -D warnings`）、`mise run test-one -- lanrurugi-api resources::`、`-- lanrurugi-plugin pool::tests`、`-- lanrurugi-api diagnostics::`、`mise run docs-api -- --check`、前端 `tsc -b`/`eslint`/`vitest`。
- [ ] **T030** `mise run dev-rebuild` + `dev-down`/`dev-up`，按 `quickstart.md` 场景 1–11 实跑并记录数字（尤其场景 3 的"关掉即停"、场景 5 的三个 JSON、场景 11 的速率比）。
- [ ] **T031** `specs/030-resource-monitor/tasks.md` 末尾写 Implementation Record（中文）：交付内容、与 plan 的偏离、实测数字、未验证项。

## 依赖关系

```text
T001 T002 T003 T005 T006 T008 (parallel)
      └─ T004 ─ T007 ─ T010
                  └─ T009 ──┘
T010 ─ T011 ─ T012 ─ T013
                 ├─ T014 ─ T015 ─ T016      (US1)
                 └─ T017 ─ T018 ─ T019 ─ T020 ─ T021  (US2/US5)
                                   └─ T022 ─ T023     (US3)
                 └─ T024 ─ T025 ─ T026                (US4)
                 └─ T027 T028 ─ T029 ─ T030 ─ T031
```

T011 必须在 T012 之前：路由要读 `AppState.resources`，而那个字段是 T011 加的（analyze 查出的顺序倒置）。

## 交付边界

- 第一版交付 US1–US5 的最小闭环，范围是**整个应用**：主进程 + 子孙进程树 + Redis 进程级指标 + 本地模型调用统计（FR-018）。GPU/温度默认关，开启后按 T005 的 5 s 缓存与 2 s 超时工作；曲线只做 4 条。
- 不做：Prometheus/OTel 导出、跨重启的历史、按存档归因的 CPU（无法在进程内精确拆分）、tokio 的任务数（需要 `tokio_unstable`，见 research R10）。

## 与 analyze 的对应

第一轮 `speckit-analyze` 查出的八条，改动落点：

| analyze 编号 | 问题 | 落到哪里 |
|---|---|---|
| A1 | `Handle::metrics()` 需要 `tokio_unstable` | research R10、plan 的依赖说明、data-model 的 `RuntimeSection`、T004 |
| A2 | T012（旧）依赖 T010（旧）的路由，顺序倒置 | 新增 T011（挂 `AppState`），T012 改成纯路由 |
| A3 | FR-006 的 GPU/温度没有任何任务 | 新增 T005（`resources/gpu.rs`，`LANRURUGI_RESOURCE_GPU` 开关、5 s 缓存、可注入替身） |
| A4 | FR-014 的 50 ms 与"不 spawn"没有可测验收 | T010（p95 实测）+ T005（关闭时替身计数为 0） |
| A5 | SC-004 的速率不降没有测法 | quickstart 场景 11、SC-004 改写为"平均速率 ≥ 关闭时的 90%" |
| A6 | SC-006 的 64 KB 与键集合稳定没有断言 | T015 的用例 ④⑤ |
| A7 | 告警色与曲线色不具体、没进主题文件 | T020（`.resources-alert`）、T022（`.resources-line-*`） |
| A8 | 契约定义的 503 没有用例 | T012 验收 ⑤ |
| A9 | debug 构建的进程名没有标注 | FR-017 / SC-008、data-model 的 `BuildSection`、T001/T015/T019/T020、quickstart 场景 1 与 5 |
| A10 | 监控只覆盖主进程与 deno worker，漏了模型/book worker 与 Redis 的进程级指标 | FR-018 / SC-009、data-model 的 `ProcessSection` 与 `ModelsSection`、T002（`process_tree`）/T004/T006/T009（`transient_*`）、quickstart 场景 1 |
