# Tasks: 应用内资源监视：设置页的资源面板与反馈包里的资源快照

**Feature**: `specs/030-resource-monitor/` | **Branch**: `feat/030-resource-monitor`
**Inputs**: [spec.md](./spec.md) · [plan.md](./plan.md) · [research.md](./research.md) · [data-model.md](./data-model.md) · [contracts/](./contracts/) · [quickstart.md](./quickstart.md)

约定：每个任务以「文件 + 验收方式」结尾；Rust 测试一律 `mise run test-one -- <crate> <filter>`，且进程要 `setsid nohup` 脱离代理树；前端验证是 `npx tsc -b` + `npx eslint` + `npx vitest run <file>`。

## Phase 1 — 采集基础（US1/US2 共用）

- [ ] **T001** [P] `crates/lanrurugi-api/src/resources/model.rs`：按 `contracts/resources-snapshot.md` 定义 `ResourceSnapshot`、`ServerSection`、`CgroupSection`、`RuntimeSection`、`PluginSection`/`PluginProcess`、`QueueSection`、`RedisSection`、`NetworkSection`、`CacheSection`、`LibrarySection`、`StorageSection`、`Sample`；`#[serde(skip_serializing_if = "Option::is_none")]` 决定哪些键在缺值时省略、哪些必须显式为 `null`（V1 要求配 `unavailable` 说明）。
  验收：`resources::model` 单测断言 `window_ms: 0` 时 `rx_rate_bps`/`cpu_percent_1s` 为 `null`（V2），以及序列化后不含 `undefined`-style 的 `0` 占位。

- [ ] **T002** [P] `crates/lanrurugi-api/src/resources/procfs.rs`：Linux 采集器。逐个纯函数：`read_status()`（VmRSS/VmHWM/Threads/VmSwap/fd limit）、`read_statm()`、`read_stat()`（utime/stime）、`read_io()`、`fd_count()`、`read_cgroup_path()`、`read_cgroup_memory()`（current/peak/stat/events/swap.current）、`read_psi(path)`、`read_net_dev()`、`read_statvfs(path)`、`read_process(pid)`（statm + stat + starttime）。每个函数返回 `Result<_, Unavailable>`，**不 panic**。
  验收：单测用内联 fixture 文本覆盖 `parse_psi`（`some avg10=1.25 avg60=… full …`）、`parse_memory_stat`（含 `anon`/`file`/`slab`/`inactive_file` 混排）、`parse_memory_events`（`low/high/max/oom/oom_kill`）、`read_net_dev`（跳过 `lo` 之外的干扰行不准，只要求汇总正确）。

- [ ] **T003** [P] `crates/lanrurugi-api/src/resources/portable.rs`：非 Linux 降级。用 `sysinfo::System`（构造失败也不 panic，`LoadThrottle` 已有同款先例）给出本进程 RSS/CPU/线程数；cgroup、PSI、swap、GPU 全部 `unavailable: "not linux"`。
  验收：单测在 Linux 上也能跑（只断言结构，不断言数值）。

- [ ] **T004** `crates/lanrurugi-api/src/resources/mod.rs`：`pub async fn collect(state: &AppState) -> ResourceSnapshot`——在 `spawn_blocking` 里组装 T002/T003 的读数，合并运行时（`Handle::metrics()`）、插件进程（T007）、队列与阶段（读 `JobRegistry` + `DownloadQueueRepository`）、Redis `INFO`、缓存计数、库规模；CPU% 用上一次采样保存的 `(utime+stime)` 做差，首帧为 `null`；末尾做 64 KB 截断（V7）。
  验收：`check-crate -- lanrurugi-api` 通过；单测断言"首帧 `cpu_percent_1s == null`，第二帧为 `Some`"（用两次 `collect_with` 注入假时钟）。

- [ ] **T005** `crates/lanrurugi-api/src/resources/watch.rs`：`ResourceWatch`——`observers: AtomicUsize`、`sampler: tokio::sync::Mutex<Option<JoinHandle<()>>>`、`history: Mutex<VecDeque<Sample>>`、`samples_taken: AtomicU64`、`last: RwLock<Option<ResourceSnapshot>>`；`attach()`（0→1 才 spawn，返回 `ObserverLease`）、`snapshot()`、`last_within(Duration)`、`interval` 字段。
  验收：单测断言 ① 首次 `attach` 后 `sampler_running == true`；② `drop(lease)` 后 ≤ 2 s `sampler_running == false` 且 `samples_taken` 不再增长（V3）；③ 两个 lease 时只 spawn 一次；④ 归零后 `history` 为空。

- [ ] **T006** [P] `crates/lanrurugi-api/src/resources/alerts.rs`：三类阈值（`oom`/`oom_kill` 增长、内存 PSI `full10 > 10.0`、fd > limit 的 80%）+ 同类 10 分钟去重；命中时通过 `crate::diagnostics::report` 写一条 warning 事件（`kind = "resources.oom_kill"` 等），context 是 `Sample` 摘要。
  验收：单测断言 ① 第一次命中返回 `Some(alert)`，紧接着 1 秒后再调返回 `None`，10 分钟后可再次命中；② 三个 kind 互不影响；③ 未越过阈值时不调用上报（用注入的 recorder 计数）。

- [ ] **T007** `crates/lanrurugi-plugin/src/pool.rs`：`Worker::pid(&self) -> Option<u32>`（从 `_child.id()`）+ `pub async fn worker_processes(&self) -> Vec<WorkerProcess>`（先克隆 `(namespace, Arc<Mutex<WorkerSlot>>)` 列表、释放表锁，再逐个 `try_lock`，拿不到锁返回 `pid: None, busy: true`）；同时暴露 `transient_available()`（信号量空闲数）。
  验收：`mise run test-one -- lanrurugi-plugin pool::tests`；新用例用 `Scratch` 起一个 worker，断言 `worker_processes` 里该 namespace 的 pid 与 `/proc` 里存在的进程一致，且 `pool::tests` 原有 29 个用例不回归。

- [ ] **T008** 采集层整体校验：`cargo check` 之后跑 `mise run test-one -- lanrurugi-api resources::`，确认 T001–T006 的用例全过且不依赖真实 cgroup 内容（fixture 驱动）。
  验收：日志末尾 `test result: ok. N passed`，N ≥ 12。

## Phase 2 — 接口（US2 后端）

- [ ] **T009** `crates/lanrurugi-api/src/resources/api.rs`：`GET /api/resources`（一次 `collect`，不注册观察者）与 `GET /api/resources/stream`（`axum::response::sse::{Event, KeepAlive, Sse}`，首帧 `snapshot`、之后 `sample`；`tokio_stream::wrappers::BroadcastStream` 或 `mpsc` 二选一，参照 `crates/lanrurugi-api/src/subscriptions/api.rs:33` 的既有写法）；`interval_ms` clamp 到 500–5000。
  验收：`check-crate -- lanrurugi-api`；`resources::api::tests` 用 `tower::ServiceExt::oneshot` 断言 ① 未登录 401；② 访客被拒；③ 管理员 200 且 body 有 `snapshot.watch`；④ `interval_ms=1` 被夹到 500。

- [ ] **T010** 权限与文档：`crates/lanrurugi-api/policy/route_policy.csv` 加三行 deny（`token_admin`/`token_guest`/`guest_visitor` 对 `/api/resources/*`）；`docs/api/tag-names.json` 加 `resources` 组（中文「资源监视」/ English "Resources"）；新建 `docs/api/sidecar/resources.yaml` 描述两条路由与响应（引用 `_shared.yaml` 的错误壳）。
  验收：`mise run docs-api -- --check` 输出 `gates passed, nothing written.`；`node apps/docs-site/scripts/generate-api-pages.mjs` exit 0。

- [ ] **T011** 把 `ResourceWatch` 挂到 `AppState`（`crates/lanrurugi-api/src/state.rs`，`Arc<ResourceWatch>`），并在 `crates/lanrurugi-api/src/lib.rs` 注册 `pub mod resources;`。
  验收：`check-crate -- lanrurugi-api`；`GET /api/resources` 在 dev 容器返回真实数字（quickstart 场景 1）。

## Phase 3 — 反馈包（US1）

- [ ] **T012** `crates/lanrurugi-api/src/diagnostics/bundle.rs`：`Plan` 增加 `extra_files: Vec<(String, String)>`；`plan()` 把 `diagnostics.json`、`config.redacted.json`、`resources.json` 的**文本**放进去（现在只算大小），`archive_to()` 从它写盘——**同时修掉 `diagnostics.json` 为 0 字节、`config.redacted.json` 从不写盘的缺陷**。
  验收：`mise run test-one -- lanrurugi-api diagnostics::bundle`；新增用例断言三个文件在 `entries()` 里字节数 > 2。

- [ ] **T013** `crates/lanrurugi-api/src/diagnostics/bundle.rs`：`Categories` 增加 `#[serde(default = "default_true")] pub resources: bool`；`resources.json` 由 `resources::collect` + `scrub` 生成，计入 64 KB 预算与 `truncations`；`readme()` 增一行说明。
  验收：用例 ① `resources: true` 时 ZIP 含 `resources.json` 且 `manifest.files` 有记录；② `false` 时不含；③ 造一个含 `?key=secret` 的 URL，断言输出里已脱敏。

- [ ] **T014** 前端反馈包页签：`apps/frontend/src/diagnostics/api.ts` 的 `BundleSelection.categories` 加 `resources: boolean`；`apps/frontend/src/diagnostics/BundleModal.tsx` 加「资源占用」勾选项（默认 true，与「服务端日志」同排）；14 个 locale 加 `diagnostics.bundle.resources`。
  验收：`npx tsc -b`、`npx eslint`、`node scripts/check-syntax.mjs src tests` 干净；locale JSON 全部可解析（`python3 -c "import json;json.load(...)"`）。

## Phase 4 — 面板（US2/US5 前端）

- [ ] **T015** [P] `apps/frontend/src/api/types.ts`：`ResourceSnapshot` 及其分节类型（与 contract 同名同单位），`Sample`、`PluginProcess`、`WatchState`。
  验收：`npx tsc -b` 通过。

- [ ] **T016** [P] `apps/frontend/src/pages/Settings/resources/useResourceStream.ts`：`EventSource` 订阅 `/api/resources/stream`；`visibilitychange` 隐藏时 `close()`、恢复时重开；重连退避 1 s→5 s；卸载时关闭；返回 `{ snapshot, samples, status, error }`。
  验收：`apps/frontend/tests/unit/resources/useResourceStream.test.tsx` 用假 `EventSource` 断言 ① 挂载即创建；② 隐藏时 `close()` 且不再累加样本；③ 恢复时重建；④ 卸载后无残留监听。

- [ ] **T017** [P] `apps/frontend/src/pages/Settings/resources/format.ts`：`formatBytes`、`formatPercent`、`formatDuration`、`windowLabel(ms)`（"过去 1 s"/"自本进程启动"）、`toText(snapshot)`（复制用，含 version/release/platform 三行）、`toCsv(samples)`。
  验收：`apps/frontend/tests/unit/resources/format.test.ts` 覆盖 `window_ms == 0` → 百分比显示 `—`、CSV 表头与样本数一致。

- [ ] **T018** `apps/frontend/src/pages/Settings/ResourcePanel.tsx`：分节表格（服务进程 / cgroup / 运行时 / 插件进程 / 队列 / Redis / 网络与缓存 / 库与磁盘），每行带窗口与来源列；`null` 显示 `—` 并在 title 里给出 `unavailable` 原因；`cgroup.memory_current_bytes` 行标注"含页缓存"；`memory.events.oom_kill > 0` 行红色（沿用主题里的告警色）；顶部两个按钮「复制为文本」「导出 CSV」。
  验收：`npx tsc -b` + `npx eslint` 干净；`apps/frontend/tests/unit/resources/resourcePanel.test.tsx` 断言「含页缓存」标注存在、`null` 渲染为 `—`、告警行带对应 class。

- [ ] **T019** 挂载入口：设置页左栏增加「资源」按钮（与「诊断与反馈」同处），打开后渲染 `ResourcePanel`（复用既有面板/弹窗外壳，不新造一层）；14 个 locale 加 `settings.resources.*` 一小组键（标题、复制、导出、各分节名、窗口标签）。
  验收：浏览器打开设置页能看到入口并展开面板（quickstart 场景 10）；14 个 locale 都能 `json.load`。

## Phase 5 — 历史曲线（US3）

- [ ] **T020** [P] `apps/frontend/src/pages/Settings/resources/Sparkline.tsx`：内联 SVG 折线（不引入图表依赖），支持 2–4 条曲线、`null` 断点、hover 显示该点数值。
  验收：`apps/frontend/tests/unit/resources/sparkline.test.tsx` 断言断点不断线、点数与样本数一致。

- [ ] **T021** `useResourceStream` 增加环形缓冲（1800 帧、超限丢最旧）；面板用 4 条曲线：CPU%、内存 anon、PSI memory full10、队列活跃数。
  验收：单测断言缓冲到 1800 后长度不再增长且首帧被丢弃（V6）；关闭再打开从零开始（US3 场景 2）。

## Phase 6 — 追踪与告警联动（US4）

- [ ] **T022** `crates/lanrurugi-api/src/diagnostics/report.rs`：记录 error 事件时，若 `watch.last_within(30s)` 有值，附 `contexts.resources`（rss、anon、pids、alive_tasks、plugins_alive、psi_memory_full10、at）。
  验收：`mise run test-one -- lanrurugi-api diagnostics::report`；用例断言 ① 有新鲜样本时附加；② 样本过期（>30 s）时不附加；③ 关掉上报时仍走既有分支不额外上报。

- [ ] **T023** `apps/frontend/src/diagnostics/sdk.ts`：面板打开时每条样本压成 breadcrumb（`category: "resources"`，10 s 节流）并 `Sentry.setContext("resources", …)`；面板关闭即停止。
  验收：`apps/frontend/tests/unit/resources/sentryResources.test.ts` 用 mock 断言 10 s 内最多 1 条 breadcrumb、关闭后不再调用。

- [ ] **T024** 告警事件接入 issues 视图：确认 `resources.*` 前缀的 kind 在既有「问题」列表里可筛可读（`crates/lanrurugi-api/src/diagnostics/issue.rs` 的分组不把它们混进普通错误）。
  验收：`mise run test-one -- lanrurugi-api diagnostics::issue`；quickstart 场景 9 实跑一次。

## Phase 7 — 文档与门禁

- [ ] **T025** [P] 文档：`README.md`「能做什么」加一条（设置页里有资源面板、报障包带资源快照）；`README.en.md`、`README.ja.md` 同步；`docs/comparison-with-lanraragi.md` 加一条（含"面板关闭后零采样"这一点）。
- [ ] **T026** [P] `CLAUDE.md` 的 SPECKIT 块新增 `030-resource-monitor` 条目（状态、模块位置、未验证项）。
- [ ] **T027** 门禁：`mise run fmt`、`mise run clippy`（workspace `--all-targets -D warnings`）、`mise run test-one -- lanrurugi-api resources::`、`-- lanrurugi-plugin pool::tests`、`-- lanrurugi-api diagnostics::`、`mise run docs-api -- --check`、前端 `tsc -b`/`eslint`/`vitest`。
- [ ] **T028** `mise run dev-rebuild` + `dev-down`/`dev-up`，按 `quickstart.md` 场景 1–10 实跑并记录数字（尤其场景 3 的"关掉即停"与场景 5 的三个 JSON）。
- [ ] **T029** `specs/030-resource-monitor/tasks.md` 末尾写 Implementation Record（中文）：交付内容、与 plan 的偏离、实测数字、未验证项。

## 依赖关系

```text
T001 T002 T003 T006 T007 (parallel)
      └─ T004 ─ T005 ─ T008
                  └─ T009 ─ T010 ─ T011
                        ├─ T012 ─ T013 ─ T014      (US1)
                        └─ T015 ─ T016 ─ T017 ─ T018 ─ T019  (US2/US5)
                                          └─ T020 ─ T021     (US3)
                        └─ T022 ─ T023 ─ T024                (US4)
                        └─ T025 T026 ─ T027 ─ T028 ─ T029
```

## 交付边界

- 第一版交付 US1–US5 的最小闭环；GPU/温度为可选项（默认关），曲线只做 4 条。
- 不做：Prometheus/OTel 导出、跨重启的历史、按存档归因的 CPU（无法在进程内精确拆分）。
