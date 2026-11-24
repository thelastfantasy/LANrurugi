# Tasks: 下载 → 入库的耗时构成与插件发现开销

**Input**: `specs/029-download-ingest-latency/` 下的 spec.md、plan.md、research.md、data-model.md、contracts/（4 份）、quickstart.md

**Tests**: spec 的每个 User Story 都写了 Independent Test，contracts 里有可直接断言的不变量（P1–P7、C1–C7、V1–V13），所以各故事都带测试任务。测试只跑改动到的 crate（`mise run test-crate -- <crate>`）；全量 `mise run test` 与 `mise run clippy` 按项目约定由用户/CI 跑，本地不跑，且必须用 `setsid nohup` 脱离代理进程树启动。

**生成时对照代码（`feat/028-deletion-safety-net` @ `ed7d231`）确认的计划细节**：

- 阶段计时的载体是 `JobStatus` 的追加字段，**不能放 `job.result`**：`jobs.finish` 才写 `result`（`crates/lanrurugi-core/src/jobs.rs:297-321`），而队列行要在任务**运行中**就显示"入库 6s"；`useJobs()` 在任务 active 时 1 s 轮询（`apps/frontend/src/api/hooks.ts:2207-2211`）。
- 跳过逻辑不能整条链跳过：tag 的落库在 `ensure_metadata_cached` 里已经做过（`plugins.rs:3655-3661`、`:3757-3765`），但 **title 只有插件链会写**（`plugins.rs:1806-1815` vs `scanner/src/pipeline.rs:625-632`）。跳过时必须显式应用缓存 title，否则是可见回归。
- `content_lock` 是在被计时的区域内获取的（`scanner/src/pipeline.rs:318-322`），所以超时分支里可以用 `try_lock` 探测"是不是别人正持有"，无需改动锁语义。
- `plugin_mtime` 已存在（`crates/lanrurugi-plugin/src/pool.rs:104-107`），缓存键直接复用它 + 追加 `size`。
- `discover_namespaces`（`plugins.rs:992-1025`）本身只是 `readdir`，不缓存它（缓存会破坏"编辑/上传即生效"）。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 不同文件、无未完成依赖，可并行
- **[Story]**: 对应 spec.md 的用户故事
- 每条都给出确切的文件路径

## Path Conventions

Rust workspace（`crates/*`）+ 前端（`apps/frontend`）+ 插件（`plugins/`）。无新 crate、无新进程、无新持久化。

---

## Phase 1: Setup

- [x] T001 确认无需新增依赖：`tokio`（`sync::Mutex`/`task_local`）、`blake3`、`sentry` 均已在 workspace 与相关 crate 的 `Cargo.toml` 中可用；若某 crate 缺 `tokio` 的 `sync` 特性，只在 `crates/lanrurugi-plugin/Cargo.toml` 补 `features`，不引入新 crate。
- [x] T002 [P] 记录改动前基线，写进本文件末尾的 Implementation Record 待填区：quickstart 场景 1 的 `plugin network: host relay` 计数、场景 3 的入库 p50/p95、场景 5 的元数据调用次数、场景 6 的 `ingestion timed out` 条数。基线取 NAS beta 那组已实测数据（1.88 s / 23.5 s / 44 s / 34 次发现 / 2 条超时）。

---

## Phase 2: Foundational（阻塞 US2、US4；US1/US3 不依赖）

- [x] T003 在 `crates/lanrurugi-core/src/jobs.rs` 里新增：
  - `pub struct JobPhase { pub name: String, pub started_at: i64, pub ended_at: Option<i64> }`（毫秒，与 `started_at`/`finished_at` 同单位）。
  - `JobStatus.phases: Vec<JobPhase>`，`#[serde(skip_serializing_if = "Vec::is_empty")]`。
  - `pub async fn begin_phase(&self, id: &str, name: &str)`：结束当前未闭合阶段（补 `ended_at = now_ms()`）后 append 新阶段；若已有队列项则忽略（一个 job 一条时间线）。
  - `pub async fn end_phase(&self, id: &str)`：给最后一个未闭合阶段补 `ended_at`。
  - `finish`/`fail` 收尾时把所有未闭合阶段补上 `ended_at`（契约 C4）。
  - 阶段名词表常量 `pub const PHASE_*: &str`（`queued`/`negotiate`/`waiting`/`transfer`/`ingest`/`publish`），供后端打点与追踪 span 共用（契约 T2：同一份词表）。
- [x] T004 [P] 在 `crates/lanrurugi-core/src/jobs.rs` 的 tests 里覆盖契约 C3/C4/C6/V7/C7：顺序单调（后一项 `started_at` ≥ 前一项 `ended_at`）、至多一个未闭合、进入终态后全部闭合、跳过未走到的阶段、`phases` 为空时序列化里没有该键。
- [x] T005 [P] 在 `apps/frontend/src/api/types.ts` 的 `JobRecord` 上加可选 `phases?: JobPhase[]`，并导出 `JobPhase`/`DownloadPhaseName` 联合类型；与 `contracts/jobs-phases.md` 的字段名逐字一致。

**Checkpoint**: 类型与词表就位；US1（插件缓存）与 US3（跳过重复元数据）此刻就可独立开工。

---

## Phase 3: User Story 1 — 批量入库不再被插件发现拖成几十秒（P1）🎯 MVP

**Goal**: 插件声明信息按 `(mtime, size)` 缓存、并发合并、变更即失效，使一次入库的"发现插件"进程数从 34 降到 0（契约 P1–P7）。

**Independent Test**: quickstart 场景 1（一次入库的进程新增数）与场景 2（编辑/上传/删除即时生效）。

- [x] T006 [P] [US1] 在 `crates/lanrurugi-plugin/src/pool.rs` 新增缓存条目与存储：
  - `struct CachedPluginInfo { stamp: PluginStamp, info: PluginInfo, options: Option<PluginOptionsResult>, fetched_at: Instant }`（`PluginStamp` 后由 T039 扩为 `mtime`+`size`+`ctime`+`inode`）。
  - `PluginPool` 加 `info_cache: Mutex<HashMap<String, CachedPluginInfo>>` 与 `inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>`（按 namespace 的 single-flight）。
  - `const INFO_CACHE_TTL`（契约 P6 的保守上限；初始 60 s，T039 后改为 10 分钟纯兜底）。
  - 失效键取自 `plugin_mtime()` + `std::fs::metadata().len()`（数据模型 V1）。
- [x] T007 [US1] 让 `plugin_info`（`pool.rs:388-410`）与 `plugin_options`（`:417-440`）走缓存：
  - 命中且 `(mtime,size)` 未变且未过 TTL → 直接返回克隆，**不 spawn**。
  - 未命中 → 取该 namespace 的 inflight 锁后二次检查缓存，仍未命中才 `Worker::spawn` 真实加载；成功后写缓存；失败**不写**（V3/P5）。
  - spawn 参数（`--allow-read` / `--allow-net` / `--allow-env`）**逐字不变**（契约 P3）；缓存不得改变任何一次真实加载的授权。
- [x] T008 [P] [US1] 在 `pool.rs` 的 tests 里覆盖契约：
  - 两次连续 `plugin_info` 只加载一次（用一个记录加载次数的测试替身或临时插件目录里的 mtime 变化）。
  - 改动插件文件 mtime → 下次调用看到新内容（P1）。
  - 删除文件 → 返回 `NotFound` 且缓存条目被清理（P2 后半）。
  - TTL 过期 → 即使 mtime/size 未变也重新加载（P6）。
  - 加载失败 → 错误原样返回、缓存里没有条目、下次重试（P5）。
  - 两个并发调用 → 真实加载恰好 1 次（P4）。
- [x] T009 [US1] 在 `pool.rs::plugin_info` 成功后比较 `info.namespace` 与请求的 namespace：不一致时 `tracing::warn!`（每个失效周期只打一次，用缓存条目的 `fetched_at` 判定）并走既有 `crate::diagnostics` 捕获路径（契约 P7）。**不改 API 形状**。
- [x] T010 [P] [US1] 修正 `plugins/metadata/chaika.ts:26` 的 `namespace: "trabant"` → `"metadata/chaika"`，并在同文件注释里说明它与路径必须一致（避免下次照模板复制又带回来）。
- [ ] T011 [US1] 跑 quickstart 场景 1、2、3 并记录：一次入库新增的 `plugin network: host relay` 行数（期望：只剩实际执行的插件调用，不含 34 次发现）、重复 `GET /api/plugins/metadata` 是否再启动进程与其 p50、编辑/删除插件后的即时生效、空闲单条与 8 条并发的入库耗时（SC-001/SC-002 的实际测量值，写进 T036）。

**Checkpoint**: US1 单独可交付——批量并发时的入库尾巴消失，插件编辑行为不变。

---

## Phase 4: User Story 2 — 队列行把"排队 / 下载 / 入库"分开（P1）

**Goal**: job 记录六段 `phases`；队列行折成三段并标注"总耗时含排队"；无 `phases` 时完整回退（契约 C1–C7、V6–V9）。

**Independent Test**: quickstart 场景 4（三段读数与 `phases` 对照）与场景 8（重启后的回退）。

- [x] T012 [US2] 在 `crates/lanrurugi-api/src/plugins.rs::run_managed_downloads` 打前四段：
  - job 建立后立即 `begin_phase(queued)`；`acquire_job_deferrable` 返回后 `end_phase` 并 `begin_phase(negotiate)`。
  - `state.plugins.execute(...)` 返回后 → `negotiate` 结束、`begin_phase(waiting)`。
  - 首次拿到 domain permit（`on_permit_acquired` 回调）→ `waiting` 结束、`begin_phase(transfer)`。
  - 所有资源传输结束、`set_queue_item_transferring(false)` 之前 → `transfer` 结束、`begin_phase(ingest)`。
- [x] T013 [US2] 新增 `crates/lanrurugi-api/src/ingest_clock.rs`：一个 `tokio::task_local!` 的阶段时钟（照 `trace_context.rs:27-60` 的写法），暴露 `with_clock(future, &JobRegistry, job_id)` 与 `mark(name)`；无时钟时 `mark` 为 no-op。
- [x] T014 [US2] 在 `crates/lanrurugi-api/src/download_manager/ingest.rs::catalogue_staged_file` 里，于 `run_enabled_metadata_plugins_on_archive` 调用前 `ingest_clock::mark("publish")`；`pending.publish()` 之后不再打点（`publish` 由 `end_phase` 在任务收尾时闭合）。
- [x] T015 [US2] 在 `plugins.rs::start_download` 的任务体里用 `ingest_clock::with_clock(...)` 包住整段下载+入库逻辑（`run_managed_downloads` 的调用），确保跨 crate 的阶段标记能落到同一个 job 上。
- [x] T016 [P] [US2] 补 `docs/api/sidecar/` 里 `/api/jobs` 的 `phases` 字段文档片段（字段、词表、可省略语义、单位），使 `lanrurugi-api-docgen` 生成的 OpenAPI 带说明。
- [x] T017 [P] [US2] 新增 `apps/frontend/src/pages/Upload/queuePhases.ts`：纯函数把 `JobPhase[]` 折成 `{ queuedMs, transferMs, ingestMs, totalMs, unattributedMs, currentPhase }`；`currentPhase` 取未闭合阶段（映射到 `queued|transfer|ingest` 三态，`negotiate`/`waiting` 归入 `queued`，`publish` 归入 `ingest`）。附 vitest（`queuePhases.test.ts`）覆盖：空数组、只有 transfer、已闭合全部、未闭合一段、乱序数组（防御性）。
- [x] T018 [US2] 在 `apps/frontend/src/pages/Upload/QueueItemRow.tsx` 展示三段：
  - 运行中：`排队 {m:ss} · 下载 {m:ss} · 入库 {s}s`，当前段高亮沿用现有文本不透明度的做法（不新增颜色/主题类）。
  - `ingesting` 的判定改为"当前阶段是 `ingest`/`publish`"（`phases` 可用时），否则退回 `state === "downloading" && transferring === false`（`:125` 与 `:397`（RowStats）两处既有判定都保留为回退路径，改一处漏一处会出现行内与统计条不同步）。
  - 总耗时标注"（含排队）"；速率旁加"仅传输"限定（复用 `JobProgress` 的 `speedTooltip` 机制，不新增颜色）。
  - `phases` 缺失时**完全不显示**三段（场景 8）。
- [x] T019 [P] [US2] 在 `apps/frontend/src/i18n/locales/` 下**全部 14 个**语言文件补文案键（排队/下载/入库/含排队/仅传输），与仓库既有约定一致（现状 14/14 都带同一套 key，`fallbackLng: "en"` 不能当作“只加 4 个”的理由）。
- [ ] T020 [US2] 跑 quickstart 场景 4、8 并记录（含那条 916 MB 样本：期望 排队 13:03 · 下载 4:27 · 入库 0:06）。

**Checkpoint**: US2 单独可交付——即使 US1 还没做，用户也不再误读耗时。

---

## Phase 5: User Story 3 — 下载入库不再重跑同一个元数据插件（P2）

**Goal**: 记录 preview 的产出插件，入库只跳过它，其余照跑；跳过时显式应用缓存 title/tags（spec FR-005/FR-005a、契约 Q1–Q4）。

**Independent Test**: quickstart 场景 5（同一插件调用 1 次 + 标题是插件标题 + 关闭自动抓取时仍跑全链）。

- [x] T021 [P] [US3] 在 `crates/lanrurugi-storage/src/download_queue.rs` 给 `DownloadQueueItem` 加 `metadata_preview_namespace: Option<String>`（`#[serde(default, skip_serializing_if = "Option::is_none")]`），并在该结构体的构造函数/默认值处补 `None`（数据模型 E3、V13）。
- [x] T022 [US3] 在 `crates/lanrurugi-api/src/plugins.rs::ensure_metadata_cached` 成功分支（`:3700-3734`）写入 `metadata_preview_namespace = Some(plugin_ns)`，随同现有的"只把元数据字段写回新读到的副本"逻辑一起保存；并在 preview 被清空的重试路径（`crates/lanrurugi-api/src/download_queue.rs:1302-1304` 的 `item.title/metadata_preview/metadata_preview_at = None`）一并清空（V10/V11）。
- [x] T023 [US3] 给 `plugins.rs::run_enabled_metadata_plugins_on_archive` 加 `skip: Option<&str>` 参数：循环内 `if Some(ns.as_str()) == skip { continue; }`；同步更新 5 处调用方（`upload.rs:927`、`subscriptions/runner.rs:1548`、`download_manager/ingest.rs:755`、`server/src/main.rs:971`；`cli/bench.rs:192` 传 `None`）。
- [x] T024 [US3] 在 `crates/lanrurugi-api/src/download_manager/ingest.rs` 里：
  - 从队列项读出 `metadata_preview_namespace` 与 `metadata_preview`（`queue_item_id` 已在手）。
  - 传给 `run_enabled_metadata_plugins_on_archive(state, archive_id, skip)`。
  - 跳过时把缓存的 `title`（若存在且非空）与 `tags` 应用到档案：走 `repos.archives.update_then` + `reindex_title_and_tags`，与插件链 `:1806-1815` 用的是同一条写入路径（不得用整条记录覆写）。
- [x] T025 [P] [US3] 单测（`crates/lanrurugi-api/src/plugins.rs` / `download_manager/ingest.rs` 的 tests）：
  - `metadata_preview_namespace = Some("metadata/ehentai")` → 该插件不被调用，`metadata/chaika` 仍被调用。
  - 该字段为 `None` → 完整链执行（含失败场景：`metadata_preview` 存在但 namespace 为空时不得跳过）。
  - 跳过时档案 title 变成缓存里的插件标题（不是文件名清洗结果），tags 被合并去重。
  - 手动 `POST /api/download_queue/{id}/fetch-metadata` 仍真实调用插件（不受缓存影响，契约 Q 的"不改变的部分"）。
- [ ] T026 [US3] 跑 quickstart 场景 5 并记录（EH 元数据调用次数、档案标题、反例：关闭自动抓取后的行为）。

**Checkpoint**: US3 单独可交付——减少一次外部 API 往返与一次插件执行；US1 未做时它仍省下网络时间。

---

## Phase 6: User Story 4 — 从追踪系统看见下载/入库各阶段（P2）

**Goal**: 一条 `download_url` 事务 + 六个阶段子 span（契约 T1–T5）。

**Independent Test**: quickstart 场景 7（含 DSN 清空对照）。

- [x] T027 [P] [US4] 在 `crates/lanrurugi-api/src/diagnostics/` 新增下载任务的事务封装（照 `report.rs:196-260` 的 `LlmTracer` 写法）：
  - `begin(queue_item_id, plugin, url)`：有 `trace_context::current()` 就 `new_with_details` 挂父 trace，否则 `TransactionContext::new("download_url", "queue.task")`；设 `download.queue_item_id` / `download.plugin` / `download.url`（去 query）。
  - `phase(name)` / `end_phase()`：`transaction.start_child(op, name)`，op 按 `contracts/trace-spans.md` 的表。
  - `fail(reason)` / `finish()`：失败设 `InternalError` + `error.class`；两条路径都必须 `finish()`。
- [x] T028 [US4] 在 `plugins.rs::start_download` 的任务体里接上 T027：任务开始 `begin`，六个阶段的边界处 `end_phase` + `phase(next)`，任务结束（成功/失败/取消）`finish`；与 T012 的打点共用同一份阶段词表常量。
- [ ] T029 [US4] 跑 quickstart 场景 7：在 rustrak 里确认父事务 + 6 个子 span；再清空 DSN 跑一次，确认 `phases` 与耗时不变。

**Checkpoint**: 下一次同类问题可直接在追踪里看，无需登机器翻日志。

---

## Phase 7: User Story 5 — 诊断噪音与声明一致性（P3）

**Goal**: 锁等待型超时不再计失败；插件声明不一致可见（spec FR-011/FR-012、research R5/R6）。

**Independent Test**: quickstart 场景 6。

- [x] T030 [US5] 在 `crates/lanrurugi-scanner/src/pipeline.rs` 的 `timeout` 分支（`:207-216`）里先算 `size_aware_id(&path)`，再对 `content_lock(id)` 做非阻塞探测：锁被他人持有 → `tracing::debug!`（"another ingest owns this content, standing down"）且**不**发 `IngestEvent::Failed`；否则维持现有 `warn` + `Failed`。
- [x] T031 [P] [US5] 单测（`pipeline.rs` tests）：构造"锁被另一任务持有"的场景断言不算失败、不产生 Failed 事件；构造"锁空闲但 ingest 真的卡住"的场景断言仍然 Failed + warn。
- [ ] T032 [US5] 跑 quickstart 场景 6 并记录（扫描失败数 0、日志级别变化）；同时确认 `Plugins` 列表里不再出现 `trabant`（T010 的产出）。

---

## Phase 8: Polish & Cross-Cutting

- [x] T033 [P] 文档：按 `CLAUDE.md` 的 push 前检查过一遍 `README.md` 的"能做什么"、`docs/comparison-with-lanraragi.md`（本次是否值得记为改进项：入库延迟与耗时可见性）、`CONTRIBUTING.md`（门禁命令未变则不动）。
- [x] T034 [P] 跑 `mise run check`（rust-check + frontend-lint），并对改动到的 crate 跑 `mise run clippy -- -p <crate>`（若 `result_large_err` 等既有告警拦截，记录在 Implementation Record，不擅自改无关 crate）。
- [ ] T035 跑 quickstart 场景 9（NAS beta 整批对照，需真实负载与用户配合），记录聚合吞吐与入库段 p95 对比。
- [x] T036 填 Implementation Record：每条 SC 的实测值、与基线的对比、未验证项（视觉确认归用户）。
- [x] T037 收尾加固（用户 2026-11-25 追问“一大批同时下载会不会起一堆 Deno 进程”时补的）：给“读声明/自省”这类临时 Deno 进程加自动上限（`lanrurugi_plugin::pool::worker_limit()`，核数/2 夹在 2–8），常驻 worker 不淘汰；新增两个测试（上限边界、槽位被占满时读取会等待且事后归还）。
- [x] T038 收尾加固（用户 2026-11-25 要求“下载全流程能合并的步骤都优化”时补的，对应 spec 的 FR-015）：
  - ① `crates/lanrurugi-plugin/src/pool.rs` 的 `execute` 原先持整张 worker 表的全局锁直到调用返回，等于“A 插件在等站点响应时，B 插件的所有调用都在排队”；改为 `workers: Mutex<HashMap<String, Arc<tokio::sync::Mutex<WorkerSlot>>>>`，表锁只在取/插槽位时持有，`WorkerSlot { worker: Option<Worker> }` 在**槽锁内**做 mtime 新鲜度检查、冷启与调用失败后的丢弃。同一插件仍严格串行、仍是一个进程；`restart_workers` 改为按槽等待（能拿到槽锁就说明没有在飞调用）后才停进程。
  - ② `crates/lanrurugi-api/src/plugins.rs` 的元数据链去重：每次入库的 `LRR_CONFIG` 由“每插件 2 次 `HGETALL`”降为整轮 1 次（`PluginRunConfig` + `relevant_settings`/`tag_rules_text` 两个纯函数复用），`file_modified_time` 由“每插件一次 `std::fs::metadata`”降为整轮 1 次（它是档案属性，不是插件属性）。
  - 新增两个测试把 FR-015 的两半都钉住：不同插件计时方法并发执行、耗时小于两倍睡眠；同一插件的两次并发调用仍串行（耗时 ≥ 两倍睡眠）且 worker 表只有 1 项。
- [x] T039 收尾加固（用户 2026-11-25 问“拉长 TTL 有没有副作用”后补的）：把描述符缓存的键从 `(mtime, size)` 换成 `PluginStamp { modified, size, ctime, inode }`（`#[cfg(unix)]` 走 `std::os::unix::fs::MetadataExt`，非 unix 回落 `(0,0)`/`0`），`INFO_CACHE_TTL` 60 s → 600 s。理由：`cp -p`/`rsync -a`/解 tar 覆盖会保留 `mtime`+`size`，旧键只能靠 60 秒兜底；`ctime` 用户态改不掉、`inode` 在“写临时文件再 rename”时必变，于是这类替换也能即时生效，TTL 退化为内存卫生。新增测试 `a_swap_that_preserves_the_timestamp_still_changes_the_stamp`（同长度、恢复旧 mtime、经 rename 落地，断言 `size`/`mtime` 相等而整枚 stamp 不等）与 `an_entry_stays_current_across_an_idle_stretch_longer_than_one_minute`（五分钟旧条目仍 current，旧 60 秒 TTL 下会失效）。契约 P1/P6 与可验证观测点同步改写。

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup（Phase 1）**：无依赖。
- **Foundational（Phase 2）**：T003 先于 T004/T005；T012–T020（US2）与 T027–T029（US4）都依赖 T003/T005。
- **US1（Phase 3）**：只依赖 Setup；与其它故事完全独立（改动集中在 `lanrurugi-plugin`）。
- **US2（Phase 4）**：依赖 T003/T005（`phases` 类型）；T012 与 T014 共用阶段边界，T013/T015 必须先于 T014 生效。
- **US3（Phase 5）**：依赖 Setup；T021 → T022 → T024；T023 改动 5 处调用方，与 T024 同一提交内完成（否则编译不过）。
- **US4（Phase 6）**：依赖 T003（词表）与 T012（阶段边界位置）。
- **US5（Phase 7）**：独立（`lanrurugi-scanner` + 插件那一行）。
- **Polish（Phase 8）**：依赖所有想交付的故事。

### User Story Dependencies

- US1（P1）与 US3（P2）共享"入库阶段很贵"这个主题，但**互不依赖**：US1 砍进程，US3 砍重复网络调用。
- US2（P1）与 US4（P2）共享阶段词表，US4 依赖 US2 打点的位置（但不依赖前端部分）。
- US5（P3）完全独立。

### Parallel Opportunities

- Phase 2：T004 与 T005 可并行（不同文件）。
- US1 内：T006/T008/T010 与其它故事并行；T007 必须在 T006 之后。
- US2 内：T016（文档）与 T017/T019（前端）可与 T012–T015（后端）并行，但 T018 依赖 T017。
- US3 内：T021 与 T025（测试骨架）可并行；T024 依赖 T022/T023。
- US5 与任何故事并行。

---

## Parallel Example: US1 + US5（两条完全独立的线）

```bash
# 线 A：插件发现缓存（lanrurugi-plugin）
Task: "T006 缓存条目与 single-flight 存储 in crates/lanrurugi-plugin/src/pool.rs"
Task: "T010 修正 chaika.ts 的 namespace in plugins/metadata/chaika.ts"

# 线 B：watcher 超时分类（lanrurugi-scanner）
Task: "T030 try_lock 探测区分锁等待型超时 in crates/lanrurugi-scanner/src/pipeline.rs"
Task: "T031 单测 in crates/lanrurugi-scanner/src/pipeline.rs"
```

---

## Implementation Strategy

### MVP（只做 US1）

1. Phase 1 + Phase 2（T001–T005）。
2. Phase 3（T006–T011）。
3. 用 quickstart 场景 1/2/3 验证：一次入库的发现进程数归零、批量入库 p95 从 23.5 s 降到 5 s 以内。
4. 这一步就能独立发布——它是用户直接抱怨的那条。

### 增量交付

1. **US1** → 入库变快（可单独上线）。
2. **US2** → 耗时读得懂（纯前端 + 后端字段，可单独上线）。
3. **US3** → 少一次外部元数据调用（依赖 T021 的字段；与 US1 叠加后入库 ≈ 拷贝时间）。
4. **US4** → 可观测（无行为变化）。
5. **US5** → 诊断清理（无用户可见行为变化）。

### 提交策略

按 `CLAUDE.md`：一次提交把本特性所有改动作为一个 changeset 提交（不按故事拆 commit）；UI 部分（T018）在提交前交给用户看，代理不做截图自证。

---

## Notes

- [P] = 不同文件、无未完成依赖。
- 跳过元数据插件是本次唯一的**行为变更**，T024/T025 必须成对完成，且不得用整条记录覆写档案（Principle III 的"读旧写回"禁令）。
- 阶段计时与追踪 span 必须同源（契约 T2）：不允许 span 用另一套打点，否则两条时间线会互相矛盾。
- 前端不新增颜色/主题类：三段读数是文本追加，沿用 `QueueItemRow.tsx` 已有的 `FONT_SIZE_XS` 文本样式（与 `upload.ingestingHint` 同款）。
- 视觉（T018 的排版）由用户在自己浏览器里确认；`quickstart` 的场景 4 只核对数字与回退行为。

---

## Analyze 修订记录（2026-11-24，实现前）

`/speckit-analyze` 在实现前对 spec/plan/tasks 做了一次只读交叉检查，发现并就地修正了八处不一致：

1. **FR-004 写得比方案强**：原文要求“一次询问得到全部插件声明”，而 `research.md` R1 已否决 manifest 方案（合体进程会把 `--allow-net`/`--allow-read` 变成所有插件的并集，违反 Principle IV）。改为“稳态下发现进程数为 0，冷启动允许每插件一次”，SC-003 的口径不变。
2. **FR-007 的阶段名与词表漂移**：原文写“入库写出”“元数据”，与 `contracts/jobs-phases.md`、`data-model.md`、T003 的 `queued/negotiate/waiting/transfer/ingest/publish` 对不上。已统一为词表名。
3. **SC-001/SC-002 没有任务实测**：quickstart 场景 3 是所有耗时指标的唯一测量步骤，却没有任何任务去跑。已并入 T011。
4. **T022 引用了错误的文件**：preview 清空不在 `plugins.rs:1290-1295`（那里是 `get_computed_tagrules`），而在 `crates/lanrurugi-api/src/download_queue.rs:1302-1304`（`item.title/metadata_preview/metadata_preview_at = None`）。已改正，调用方数量也在 T023 一并说清是 5 处。
5. **T018 只改了一处 `ingesting`**：`QueueItemRow.tsx` 的 `:125`（行内）与 `:397`（`RowStats`）各自独立计算，只改一处会让行内与统计条不同步。已写明两处都要走新的阶段判定。
6. **T023 自相矛盾**：写“4 处调用方”却列了 5 个路径。已改为 5 处。
7. **T019 只加 4 个语言**：仓库现状是 14 个 locale 文件都带同一套 key，`fallbackLng: "en"` 不是“只加 4 个”的许可。已改为全部 14 个。
8. **FR-001 的“列表请求”一半无人验证**：缓存同时服务 `GET /api/plugins/{type}`，但 quickstart 场景 1 只测入库。已在场景 1 加第 4 步（连发 5 次取 p50，并检查不再新增进程启动行）。

未做改动的两处判断，理由记录在此：plan 的 A–F 工作流与 tasks 的 Phase 1–8 是两套正交编号（前者是排期，后者是交付顺序），不算冲突，不加映射表；FR-006（手动 Fetch 必须真实调用）只有 T025 的反例测试覆盖，因为手动路径本来就不经过缓存，无需额外实现任务。

---

## Implementation Record（2026-11-24）

代码在 `feat/029-download-ingest-latency` 上一次提交完成；T011/T020/T026/T029/T032/T035 是需要真实部署的验收步骤，本记录标明哪些实测过、哪些没有。

### 交付内容

- **阶段计时（FR-001/FR-007）**：`crates/lanrurugi-core/src/jobs.rs` 的 `JobStatus.phases`（`Vec<JobPhase>`，`started_at`/`ended_at` 毫秒，`ended_at` 缺失序列化为 `null`，`phases` 为空时整键省略）由 `JobRegistry::begin_phase`/`end_phase` 维护；`mark_active` 清空（重试即新时间线），`finish`/`fail` 闭合所有未闭合段。打点在 `crates/lanrurugi-api/src/plugins.rs` 的 queued/negotiate/waiting/transfer/ingest 五个边界，加 `crates/lanrurugi-api/src/download_manager/ingest.rs` 的 `ingest_clock::mark(PHASE_PUBLISH)`；`crates/lanrurugi-api/src/ingest_clock.rs`（`task_local`，带 job id 与队列项 id）让 ingest 内部不必一路透传 job id。
- **前端呈现（FR-002）**：`apps/frontend/src/pages/Upload/queuePhases.ts` 把六段折成排队/下载/入库三段（`negotiate`+`waiting` → 排队，`publish` → 入库），未闭合段按当前时刻收尾；`QueueItemRow.tsx` 两处 `ingesting` 判定都改走 `ingestingNow`（时间线优先，旧 `transferring` 字段回退），队列行写成「排队 13:03 · 下载 4:27 · 入库 0:06」，平均速率 tooltip 说明只统计传输段。14 个 locale 全部补齐 5 个 key。
- **插件声明缓存（FR-003/FR-004）**：`crates/lanrurugi-plugin/src/pool.rs` 的 `CachedPluginInfo`（`PluginStamp`：mtime + size + ctime + inode，外加 10 分钟兜底 TTL）+ 每命名空间 single-flight 的 `inflight` 表；`plugin_info`/`plugin_options` 都先查缓存，零权限 worker 只用来问声明。写插件、删除插件、改文件、以及 `cp -p` 式的原地替换都会让缓存失效（stamp 变化即重读）。`plugins/metadata/chaika.ts` 的 `namespace` 从 `trabant` 改回与路径一致的 `metadata/chaika`。
- **跳过重复元数据（FR-005/FR-006）**：`DownloadQueueItem.metadata_preview_namespace` 记录"这份预览是哪个插件答的"，随 preview 一起清空；`run_enabled_metadata_plugins_on_archive` 新增 `skip: Option<&str>` 参数（5 处调用方），只跳过声明过的那一个，其余插件照常跑；跳过时 `apply_preview_metadata` 把缓存的 title/tags 落到档案上（`update_then` + `reindex_title_and_tags`，不整条覆写）。
- **追踪（FR-008）**：`crates/lanrurugi-api/src/diagnostics/download_trace.rs` 为每个下载任务开一条 `download_url` 事务，六个阶段各一个子 span（`queue.wait`/`plugin.exec`/`http.wait`/`file.transfer`/`file.ingest`/`plugin.metadata`），与 `phases` 共用同一份词表和同一个打点函数。
- **临时进程闸（FR-014）**：`crates/lanrurugi-plugin/src/pool.rs` 的 `worker_limit()`（核数/2，夹在 2–8）给“读声明/自省”这类一次性 worker 设并发上限，`spawn_transient_worker` 返回的 `TransientWorker` 在存活期间持有 `OwnedSemaphorePermit`，`Drop` 时连同 worker 一起归还；冷启动时整库发现最多同时 8 个临时进程，而不是一次一个插件。**常驻 worker 不设上限、也不淘汰**：它本来就是一个插件一个进程、并被该插件的所有并发调用共用，淘汰等于杀掉可能正在被调用的进程。
- **超时分类（FR-010）**：`crates/lanrurugi-scanner/src/pipeline.rs` 的 `content_lock_held` 用 `try_lock` 区分"别人正在入库同一内容"与真超时，前者只记 `debug` 并静默退出，不再发 `IngestEvent::Failed`。

### 与计划的偏离

1. **T009 没有走 `crate::diagnostics` 的捕获路径**：`lanrurugi-plugin` 没有 diagnostics 模块，而 API 侧的 tracing 桥只捕获 error 级。实现为"每次真实加载记一条 `tracing::warn!`（稳态下等于每个失效周期一条）"，并在源头修掉 `chaika.ts` 的命名空间错配——那正是这条告警本来要暴露的问题。
2. **T027/T028 的 trace 用按 job id 的注册表，而不是透传 tracer**：阶段边界分散在 `run_managed_downloads`（没有 trace 参数）与 `move` 闭包 spawn 出去的任务里，透传要给一串与下载无关的签名加参数。注册表在 `diagnostics/download_trace.rs` 内部，`begin` 返回的 guard 在 `Drop` 时收尾，失败路径显式 `fail` 标 `error.class`。
3. **预置元数据在其余插件之后应用**：被跳过的是该站点自己的元数据插件，它的答案对这份档案最权威，因此 title 以它为准；tags 走既有的"先出现者优先"合并，所以其余插件补的标签不会覆盖它，它也不会覆盖已有的标签。
4. **`PluginPool::restart_workers` 不清缓存**：声明与 worker 生命周期无关，清掉只会让下一次调用白付一次进程启动。

### 验证

已实测（`mise run test-one`，各自临时 Redis）：

- `crates/lanrurugi-core` `jobs::tests` → 29 passed（新增 7 个阶段契约测试）。
- `crates/lanrurugi-plugin` `pool::tests` → 29 passed（新增 5 个：缓存命中、改文件失效、删除失效、声明失败不缓存、冷缓存并发只加载一次；T037 的 2 个：进程上限落在 2–8、槽位被占满时声明读取会等待且事后归还；T038 的 2 个并发/串行；T039 的 2 个：保留时间戳的替换仍改 stamp、五分钟旧的条目仍算 current）。
- `crates/lanrurugi-scanner` `pipeline::tests` → 10 passed（新增 2 个：别人持锁时超时不算失败、无人持锁仍算失败）。
- `crates/lanrurugi-api` `download_manager::ingest::tests` → 14 passed（新增 1 个：被跳过的插件仍贡献 title/tags）、`diagnostics::download_trace` → 2 passed、`only_the_recorded_plugin_is_skipped_*` → 1 passed。
- 前端：`tests/unit/queuePhases.test.ts` 9 passed、`tests/unit/queueRowStats.test.tsx` 12 passed（3 个新用例覆盖完整五段、无 `phases` 回退、传输中未闭合 ingest）。`npx tsc -b`、`npx eslint`（改动文件）通过。
- 门禁：`mise run clippy`（workspace，`-D warnings`）通过；`mise run check`（lefthook pre-push：rust-check 32.56 s + frontend-lint 34.69 s）通过；`mise run docs-api -- --check` 通过（`document: 269 path(s), 343 operation(s)`，`--check: gates passed, nothing written.`），确认 `docs/api/sidecar/_shared.yaml` 的 `phases` 增补与路由表一致。

基准测试（`mise run bench -- plugin_descriptor` / `-- job_phase`，`crates/lanrurugi-bench/benches/plugin_discovery.rs`，dev 容器、tmpfs 上的临时插件目录、真实 deno 进程）：

| 项目 | 实测 |
| --- | --- |
| 读一次插件声明（冷，含启动 deno） | 14.30 ms（10 样本，95% CI 14.15–14.40 ms） |
| 同一插件再次读（缓存命中） | 1.848 µs（95% CI 1.846–1.852 µs） |
| 比值 | ×7 740 |
| 一次下载任务的全部阶段打点（mark_active + 5 次 begin + 1 次 end） | 0.44 µs |
| 对照：一次 `set_progress`（既有每块进度写入） | 30 ns |

按基线的一次入库 34 次发现折算：14.30 ms × 34 ≈ 486 ms 的纯进程启动时间 → 1.85 µs × 34 ≈ 63 µs，稳态 0 次启动（SC-003 的"34 → 0"由 `pool::tests` 的两个用例同时证明：同一插件二次读取 `fetched_at` 不变、8 个并发读只加载一次）。62 条批量按 2 100 次启动折算 ≈ 30 s 的纯启动时间。

跨插件并发（T038 的锁粒度改造，`mise run bench -- plugin_call_concurrency`，两个各睡眠 25 ms 的插件）：

| 场景 | 改动前（全局锁） | 改动后（按插件加锁） |
| --- | --- | --- |
| 两个插件先后调用 | 52.97 ms | 52.82 ms |
| 两个插件**同时**调用 | 52.81 ms（第二个在等第一个） | 26.40 ms |
| 同时调用的收益 | — | **2.00×**（52.81 → 26.40 ms） |

改动前的 `同时调用` 与 `先后调用` 都是 52.8 ms —— 这就是"全局锁"的证据：并发写法拿到串行结果。改动后 `同时调用` 降到一条调用的耗时（26.4 ms），说明第二个插件不再等第一个答完；同一插件的串行语义由新测试 `one_plugin_still_answers_its_calls_one_at_a_time` 钉住（耗时 ≥ 两倍睡眠、worker 表仍只有 1 项）。折算到真实场景：一个元数据插件的站点往返按 1 s 计，改动前另一个插件的调用要白等 1 s，改动后只等自己的。

注意这几条是**插件发现 / 插件调用调度**的独立开销，不等于端到端的入库 p50/p95——后者还受网络、解码与索引影响，仍待 NAS 实测。

未实测（需要真实部署 / 真实负载，留给部署验证）：

- **T011（场景 3）**：单条入库 p50/p95 的前后对比，以及场景 1 的发现进程计数——本机没有 NAS 那批真实下载，未跑。基线仍是 1.88 s / 23.5 s / 44 s、34 次发现。
- **T020（场景 4/8）、T026（场景 5）、T032（场景 6）、T029（场景 7）**：分别需要真实队列、真实订阅、真实超时与真实 rustrak 实例。
- **T035（场景 9）**：整批入库的吞吐对照。
- **视觉**：队列行三段读数的排版由用户在自己浏览器里确认（`CLAUDE.md` 的 UI 确认规则）。
