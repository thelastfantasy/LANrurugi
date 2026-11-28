# Research: 下载 → 入库的耗时构成与插件发现开销

**Feature**: `specs/029-download-ingest-latency`
**Date**: 2026-11-24
**Baseline**: `@ed7d231`（`feat/028-deletion-safety-net`）

本文件把 spec 里每一条“为什么慢/该怎么改”变成可执行的决定，并记录被否掉的方案。所有涉及既有行为的地方都按宪法「Verify against source」的要求读过实际源码，引用到文件:行。

---

## R1. 插件声明信息缓存在哪、怎么失效

**Decision**: 缓存放在 `PluginPool`（`crates/lanrurugi-plugin/src/pool.rs`）内部，按 namespace 键，条目值为
`(mtime, size, PluginInfo)` 与该插件 `pluginOptions()` 的 `Option<PluginOptionsResult>`；
查找键每次由 `std::fs::metadata(plugin_module)` 现场取；`(mtime, size)` 任一变化即视为失效并重取。
同一 namespace 的并发发现用 single-flight（一个 `tokio::sync::Mutex` + “谁先到谁填”，其余等待后读缓存）合并。
另加一个保守上限（默认 60 s），覆盖“mtime+size 恰好不变但内容被替换”的边界（`cp -p`、部分网络文件系统）。

**Rationale**:
- 权限边界不动：缓存保存的是“声明了什么”，不保存可复用的执行环境；每次拿到的 `PluginInfo` 仍来自一次带最小授权的真实加载（`pool.rs:122-196` 的 `--allow-read=<该插件文件>`），只是不再每次都付这个进程的钱。
- single-flight 是必需的：spec 的边界条件要求“两个入库同时要求发现同一批插件时只能发现一次”。8 条并发入库正是这个场景。
- 60 s 上限只影响“声明被原地替换且时间戳未变”的病态情况，不会掩盖正常编辑（`write` 会改 mtime）。

**Alternatives considered**:
- **一次 `deno run` 装全部插件输出 manifest**：进程数从 34 降到 1，但要把 `--allow-read` 扩到整个 `plugins/` 目录、且 `pluginInfo()` 是任意插件代码（它可以发请求、读文件）。这与 Principle IV 的最小授权相冲突。**不作为第一步**；缓存已经把“每次入库 34 次”降到 0，manifest 只在“冷启动发现一次”上再省 1 个进程，收益不值得扩权。记录在案，若将来插件数倍增再议。
- **Rust 侧静态解析 `pluginInfo()`**：`pluginInfo` 是任意 JS，静态解析不可靠（条件、导入、helper）。否。
- **持久化到 Redis**：插件目录是镜像内文件，进程重启后一次发现即可；持久化引入失效与多实例一致性问题，收益为零。否。

**Verified against source**: `pool.rs:388-410`（`plugin_info` 每次 `Worker::spawn`）、`:417-440`（`plugin_options` 同样）、`:569-601`（`execute` 的常驻 worker + mtime 失效）、`:100-110`（`plugin_mtime` 已存在，可直接复用）。

---

## R2. 下载入库阶段到底该跳过哪个插件（对 spec FR-005 的收窄）

**Decision**: 跳过的是**“开始下载时已经成功执行过的那一个插件 namespace”**，不是整条插件链。
要点：
1. 队列项新增可选字段 `metadata_preview_namespace`（写于 `ensure_metadata_cached` 成功时）；它记录“这份 preview 是哪个插件产出的”。
2. 入库阶段调用链时传 `skip: Option<&str>`，只跳过该 namespace；其余启用插件照常执行（legacy `exec_enabled_plugins_on_file` 的语义对其他插件保持不变）。
3. 跳过时必须**显式把缓存的 title 与 tags 落到档案上**（今天这是被跳过的那个插件顺带做的，见下）。
4. 只有“开始阶段真的成功过”才跳过：`metadata_preview_namespace` 为空（自动抓取关闭 / 抓取失败 / 无匹配插件）时照常跑完整链。

**Rationale / 必要性**:
- 今天 tag 的落库**不是**由插件链独家完成的：`plugins.rs:3757-3765` 与 `:3655-3661` 在 `ensure_metadata_cached` 里、拿到 `archive_ids` 之后就会把 preview 的 tags 合并进档案（下载成功分支在 `:2938-2952` 先把新档案 id 填进 `item.archive_ids` 再调它）。所以 tags 不会丢。
- 但 **title 会丢**：`ensure_metadata_cached` 只把插件返回的 title 写进 `item.title`，从不写档案；档案 title 是 `catalogue_new_archive` 用 `release_markers::clean_title(文件名)` 得来的（`scanner/src/pipeline.rs:625-632`）。真正把插件 title 写到档案上的是插件链（`plugins.rs:1806-1815` 的 `update_then` + `archive.title = title`）。因此“跳过”必须补上这一步，否则入库后的档案会退回文件名标题——这是本次最容易引入的回归。
- 不跳过其余插件，是因为它们与“已经抓过的那个插件”不是重复劳动：`regexparse`/`comicinfo`/`dateadded` 等是本地规则，legacy 在每条新档案上都会跑。
- 起始调用与入库调用的 `execMetadata` 参数形状不同（起始：`{url, arg, customargs, existing_tags:"", archive_title:"", thumbnail_hash:""}`，见 `plugins.rs:3689-3700`；入库：带 `archive_id`/`file_path`/`existing_tags`/`archive_title`，见 `plugins.rs:1653-1669`）。因此被跳过的插件理论上可能给出与起始调用不同的结果。接受这个差异：起始调用拿到的 title/tags 已被写入 preview 并被落库，语义上等价于“这个插件已经描述过这本档案”。

**Alternatives considered**:
- **整条链都跳过**：会连带丢掉 regexparse/comicinfo 等本地规则的产出，属于行为变更，超出本特性范围。否。
- **不跳过任何插件，只靠缓存把进程成本去掉**：那样 SC-005（同插件 ≤1 次）无法达成，且仍在重复请求外部 API。否。
- **改成“先发布、元数据后台补”**：会改变 025 的发布语义（档案可能在无标题/无标签状态下被看到），属于产品决策，不在本特性内。写入 Assumptions。

**Verified against source**: `download_manager/ingest.rs:751-758`；`plugins.rs:2938-2952`、`:3635-3668`、`:3700-3770`、`:1806-1815`；`scanner/src/pipeline.rs:615-640`。

---

## R3. 阶段耗时放在哪里、如何跨 crate 打点

**Decision**:
- `JobStatus`（`crates/lanrurugi-core/src/jobs.rs`）新增可选字段 `phases: Vec<JobPhase>`，`JobPhase { name, started_at, ended_at: Option<i64> }`（毫秒，与既有 `started_at`/`finished_at` 同单位）；`#[serde(skip_serializing_if = "Vec::is_empty")]`，既有客户端不受影响。
- 只存在内存（job registry 本来就不落盘，`jobs.rs` 模块注释已说明重启即丢），不新增 Redis/磁盘结构。
- 阶段名固定为六个：`queued` / `negotiate` / `waiting` / `transfer` / `ingest` / `publish`。
- 打点接口：`jobs.begin_phase(id, name)` / `jobs.end_phase(id)`。结束当前阶段时把 `ended_at` 补上并推进。
- `ingest` 与 `publish` 的边界在 `download_manager/ingest.rs` 内部（`run_enabled_metadata_plugins_on_archive` 调用前后），跨 crate 传递用 `tokio::task_local!` 的阶段时钟（本仓已有先例：`crates/lanrurugi-api/src/trace_context.rs:27`）。没有安装时钟的调用方（上传、共享导入、扫描）自动 no-op。

**Rationale**:
- **不能放 `job.result`**：`jobs.finish` 才写 `result`（`jobs.rs:297-321`），而 UI 需要在任务**运行中**就看到“入库 6s”；`useJobs()` 在任务 active 时按 1 s 轮询（`hooks.ts:2207-2211`），所以只要字段在内存里更新，1 s 内就能读到，不需要新增 SSE 通道。
- 六个阶段里前四个（queued/negotiate/waiting/transfer）都在 `plugins.rs::run_managed_downloads` 的同一函数体内，直接打点即可；后两个跨 crate，`task_local` 是零签名改动的办法。
- 单位与类型与既有 job 字段一致，前端不需要新的时间解析逻辑。

**Alternatives considered**:
- **把阶段塞进 `result`**：运行中不可见，且 `MAX_JOB_RESULT_BYTES` 截断逻辑会波及。否。
- **给 `catalogue_staged_file` 加参数**：它有 9 个参数（`#[allow(clippy::too_many_arguments)]`），再传一个计时器会传染所有调用方（上传、共享导入、冲突解决、启动修复）。`task_local` 更合适。
- **新的 `/api/jobs/{id}/phases` 端点**：多一次请求，且 1 s 轮询已经在读 `/jobs`。否。

**Verified against source**: `jobs.rs:30-120`（JobStatus 字段与 `skip_serializing_if` 惯例）、`:297-321`（finish 写 result）、`trace_context.rs:27-60`（task_local 先例）、`hooks.ts:2207-2221`。

---

## R4. 追踪 span 怎么接

**Decision**: 复用仓内已有的手工事务模式（`crates/lanrurugi-api/src/diagnostics/report.rs:196-260` 的 `LlmTracer`）：
- 下载任务开始时，若有 HTTP 父上下文（`trace_context::current()`）就 `new_with_details` 挂到那条 trace，否则 `TransactionContext::new("download_url", "queue.task")` 起一条独立 trace。
- 六个阶段各一个 `transaction.start_child(op, name)`，结束时 `finish()`；父 span 在任务结束（成功或失败）时 `finish()`，失败时 `set_status(InternalError)`。
- 埋点关闭时 `sentry::start_transaction` 返回的事务不发送任何东西（无 `Hub`/client 时是 no-op），不改行为——与 `LlmTracer` 的既有前提一致。

**Rationale**: 同一套 API、同一套门控（DSN 未配置 → `diagnostics::CAPTURE_GUARD` 为空 → 不初始化 client），无需新依赖；rustrak 的事务详情页已经能展示子 span（`get_transaction_spans`）。

**Alternatives considered**:
- 用 `tracing::info_span!` + `sentry-tracing` 自动事务：现有 HTTP 事务那套只覆盖请求生命周期，后台任务没有现成的 root span 机制；手工事务更可控，也和 LLM 那条路径一致。

---

## R5. watcher 重复 ingest 的超时如何与真超时区分

**Decision**: 保留 30 s 的 `INGEST_TIMEOUT`，但在超时分支里做一次判定：对 `size_aware_id(&path)` 得到的 id 试 `content_lock` 的非阻塞探测（`try_lock`）。若锁被他人持有，则本条记为“**deferred（另一条 ingest 正在处理同一内容）**”：不计入扫描失败数、不产生 `IngestEvent::Failed`、日志降到 `debug`；否则维持现有的 `warn` + `Failed`。

**Rationale**:
- `content_lock` 是在被计时的区域内获取的（`scanner/src/pipeline.rs:318-322`），而下载路径在 `catalogue_staged_file` 里会持锁很久（写书库 + 插件链 + 发布）。实测两条 44 s/36 s 的记录正是这种情况。
- 用 `try_lock` 探测不需要改动锁语义，也不需要把锁移到超时区域之外（那会让 watcher 变成无上限等待）。id 由 `size_aware_id` 现算，一次 stat，成本可忽略。
- 计入扫描失败会污染诊断数据（`full_scan.rs:393` 把 `Failed` 计入 `summary.failed`），而这次“失败”实际什么都没坏。

**Alternatives considered**:
- 把 `content_lock` 的等待移出超时区域：watcher 会无限等，违背 30 s 保护的初衷（`pipeline.rs:36` 的注释说明这是 Phase 1 的安全网）。
- 直接调大超时：治标且放大了真卡死的窗口。否。

---

## R6. 插件 namespace 与安装路径不一致

**Decision**: 两件事。
1. 修 `plugins/metadata/chaika.ts:26` 的 `namespace: "trabant"` → `"metadata/chaika"`（其余插件都是路径形式，实测只有它不对）。
2. 在 `PluginPool::plugin_info` 成功后比较 `info.namespace` 与请求的 namespace，不一致时打一条 `warn`（每次失效周期只打一次）并走既有 `diagnostics` 捕获；**不改 API 形状**（插件列表一直用路径 namespace，本来就显示正确）。

**Rationale**: 不一致本身不致命，但它会让“按 info.namespace 走”的逻辑（以及第三方插件作者的直觉）与运行时实际使用的路径 namespace 分裂。暴露 + 修正比静默更安全；不动 API 是最保守做法（Principle II）。

**Alternatives considered**:
- 在 `/api/plugins` 里加 `declared_namespace` 字段：可行但会诱导前端展示两个名字，收益不足。先用日志 + 诊断。

---

## R7. UI 三段计时的呈现与回退

**Decision**:
- 由 `job.phases` 派生三段读数：**排队**（`queued`+`negotiate`+`waiting`）、**下载**（`transfer`）、**入库**（`ingest`+`publish`）。
- 运行中把当前所在阶段标出来（与现有 `ingesting` 判定的含义对齐：`ingest`/`publish` 阶段显示“入库中…”）。
- 总耗时仍显示，但明确标注“含排队”（文案层面区分），避免与三段混淆。
- **回退**：`job` 不存在、或 `phases` 为空（旧服务、服务重启后 job 丢失）时，完全退回今天的展示（单一 `耗时` + `ingesting` 布尔），不显示任何伪造的分段。
- 传输速率旁边补一句“仅传输”的限定（复用 `speedTooltip` 机制，不新增颜色）。

**Rationale**: `ingesting` 现有判定（`QueueItemRow.tsx:125`）在 `phases` 可用时可以由阶段名取代，语义更准；但必须保留旧路径，因为 job 是内存态、重启即丢（`jobs.rs` 注释）。纯文案/数字追加，不涉及布局断点，因此不需要新的 CSS 类或主题色（`CLAUDE.md` 的主题规则针对“新颜色”）。

**Verified against source**: `QueueItemRow.tsx:118-131`（durationMs/ingesting）、`:470-530`（三种进度条分支）、`JobProgress.tsx:359-400`（`readingLabel`/`pendingLabel`）。

---

## R8. e-hentai 那条行的口径（用于验收样本）

用截图对应的队列项（`9ba8f386…`，916 MB，2026-11-24 10:12:21 开始）作为 US2 的固定验收样本：

| 段 | 实测 |
|---|---|
| 排队（job 槽 + archiver 协商 + domain permit） | ≈ 13:03 |
| 下载（传输） | 4:27（916 072 365 B ÷ 3 430 804 B/s） |
| 入库（拷贝 + 落库 + 元数据 + 发布） | ≈ 6 s |
| 总 | 17:36 |

改动后该行应显示为“排队 13:03 · 下载 4:27 · 入库 0:06（总 17:36）”，而不是今天的“耗时 17:30 + 入库中…”。

---

## 未决 / 不在本特性范围

- `*.hath.network` 的 `max_concurrent`（当前用户配置 8，插件默认 10）：运行时配置项，不在 spec 范围内；建议在 quickstart 里记录“调到 10–12 可提高吞吐”。
- e-hentai archiver 协商 40–144 s：来源侧行为，无法优化。
- `hidden → published` 的发布语义本身（025）：本特性只缩短窗口。
