# Implementation Plan: 下载 → 入库的耗时构成与插件发现开销

**Branch**: `feat/028-deletion-safety-net`（沿用当前分支） | **Date**: 2026-11-24 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/029-download-ingest-latency/spec.md`

## Summary

三条并行的改造，目标都是让"入库中…"这一段既**真的快**、也**读得懂**：

1. **插件声明信息缓存**（`lanrurugi-plugin`）：把"这个插件是什么类型、声明了什么参数/权限"从"每次入库为 34 个插件各起一个 Deno 进程"变成"按文件 mtime+size 缓存、并发合并、变更即失效"。这是宪法 Principle IV 已明确要求的 persistent-worker 语义在发现路径上的补齐。
2. **下载入库不再重跑同一个元数据插件**（`lanrurugi-api` / `lanrurugi-storage`）：记录"这份 preview 是哪个插件产出的"，入库阶段只跳过它，其余启用插件照跑，并把缓存的标题/标签显式落到档案上（今天这一步由被跳过的插件顺带完成）。
3. **让耗时看得见**（`lanrurugi-core` / `lanrurugi-api` / 前端）：job 增加六段 `phases`，队列行把它折成"排队 / 下载 / 入库"三段并标注"总耗时含排队"，同时给下载任务补一条带子 span 的追踪事务。

顺带两个小清理：watcher 因"另一条 ingest 正持有同一内容锁"而超时不再计入扫描失败；`plugins/metadata/chaika.ts` 的 `namespace: "trabant"` 残留修正，并让声明与路径不一致这件事可见。

实测基线（NAS beta）：跨盘拷贝 **315 MB/s**（不是瓶颈）；`hidden → published` p50 1.88 s / p90 23.5 s / max 44 s，19/138 条 >20 s 且集中在批量并发时；一次入库为"发现插件"起 34 个进程，一个 44 s 窗口内有 696 次进程启动。

## Technical Context

**Language/Version**: Rust 2021（Cargo workspace，`crates/*`）；React 19 + TypeScript（Vite + Tailwind + Zustand + TanStack Query，`apps/frontend`）；Deno 2.9.8（插件运行时）

**Primary Dependencies**: tokio / axum / deadpool-redis / `zip` / `sentry` 0.49（已有 `sentry-tracing`、`trace_context` 手工事务先例）/ 无新依赖

**Storage**: Redis（档案、下载队列、配置、搜索索引）；job registry 为进程内 `RwLock<HashMap>`，不落盘

**Testing**: `mise run check-crate -- <crate>`（快速单 crate）、`mise run test-crate -- <crate>`、`mise run fmt/clippy`；前端 `pnpm vitest`；UI 视觉由用户确认；全量 `mise run test` 由用户/CI 跑（CLAUDE.md 规定 build/test 走 `mise run` 且需脱离代理进程树）

**Target Platform**: Linux x86_64/aarch64（Docker 镜像与原生包）；插件运行时为独立 Deno 子进程

**Project Type**: 单体服务（一个 `lanrurugi-server` 二进制）+ 内嵌 React 前端

**Performance Goals**: 空闲单条入库 p50 ≤ 1.0 s；8 条并发入库 p95 ≤ 5 s 且无 >20 s；一次入库的插件发现进程数 34 → 0；同一元数据插件每次下载 ≤ 1 次调用

**Constraints**: 不新增持久化结构、不改插件 ABI、API 只增可选字段、插件权限边界不放大、埋点不得影响任务耗时、UI 变更需用户视觉确认

**Scale/Scope**: 单实例；典型 7502 档案、34 个插件；批量下载并发 8–12 条

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 结论 | 说明 |
|---|---|---|
| I. Legacy 数据与用户信任兼容 | PASS | 不改变任何档案/队列/配置的键或语义；`DownloadQueueItem` 只追加可选字段（serde 缺省即兼容），无需迁移工具 |
| II. API 契约保真 | PASS | `/api/jobs` 与 `/api/download_queue` 只追加可选字段；`phases` 为空时整键省略；文档站 sidecar 同步补 |
| III. 资源自觉的并发单进程架构 | PASS（正向） | 本特性**减少**进程启动与重复网络调用；缓存必须 single-flight（并发合并）以免把 34 次并发放大成 N×34；阶段计时走内存、不新增线程 |
| IV. 沙箱化的插件扩展 | PASS | 决策明确否决"一个进程装全部插件"（会要求把 `--allow-read` 扩到整个插件目录）；缓存只存声明、不存执行环境 |
| V. 密钥与网络信任边界 | PASS | 不触碰凭据与出站策略；跳过重复元数据调用反而减少了出站请求 |
| VI. 分阶段范围纪律 | PASS | Phase 1 加固项；不改动 025/027 的发布与完整性语义 |
| VII. 前端工程纪律与 legacy UI 保真 | PASS（有约定） | 三段计时是纯文本追加，不涉及断点布局 → 无需新 CSS 类/主题色；派生函数放在独立模块（不在 `index.tsx` 内联）；视觉最终由用户确认，代理不做截图自证 |
| 质量标准：Verify against source | PASS | research.md 的每条决定都引用源码位置；FR-005 的收窄正是"读源码后发现 title 只有插件链会写"的结果 |
| 质量标准：兼容性变更显式声明 | PASS | 两处追加字段在 spec Assumptions 与 contracts 里都显式声明 |

**Complexity Tracking**: 无宪章偏离。唯一值得记录的取舍是**用 `tokio::task_local!` 的阶段时钟代替给 `catalogue_staged_file` 加参数**——该函数已有 9 个参数并被 5 处调用（上传、共享导入、冲突解决、启动修复），加参数会传染；`task_local` 在无时钟的调用方自动 no-op，本仓已有先例（`trace_context.rs:27`）。

## Project Structure

### Documentation (this feature)

```text
specs/029-download-ingest-latency/
├── plan.md              # 本文件
├── spec.md
├── research.md          # Phase 0：R1–R8 决定与被否方案
├── data-model.md        # Phase 1：E1 缓存条目 / E2 阶段计时 / E3 preview 产出者
├── quickstart.md        # Phase 1：9 个验证场景
├── contracts/           # Phase 1
│   ├── jobs-phases.md
│   ├── plugin-descriptor-cache.md
│   ├── queue-item-metadata-namespace.md
│   └── trace-spans.md
├── checklists/requirements.md
└── tasks.md             # Phase 2（/speckit-tasks 产出）
```

### Source Code (repository root)

```text
crates/lanrurugi-plugin/src/pool.rs            # 插件声明缓存 + single-flight + namespace 一致性告警
crates/lanrurugi-core/src/jobs.rs              # JobStatus.phases / begin_phase / end_phase
crates/lanrurugi-storage/src/download_queue.rs # metadata_preview_namespace（可选字段）
crates/lanrurugi-api/src/plugins.rs            # 六段打点、跳过已执行插件、缓存 title 应用、追踪事务
crates/lanrurugi-api/src/download_manager/ingest.rs # ingest/publish 边界标记；跳过参数透传
crates/lanrurugi-api/src/diagnostics/          # 下载任务的事务/span 封装（复用 report.rs 模式）
crates/lanrurugi-scanner/src/pipeline.rs       # 锁等待型超时与真超时的区分
apps/frontend/src/api/types.ts                 # JobRecord.phases / QueueItem 新字段
apps/frontend/src/pages/Upload/QueueItemRow.tsx        # 三段读数展示与回退
apps/frontend/src/pages/Upload/queuePhases.ts          # 阶段→三段的纯函数（独立文件）
plugins/metadata/chaika.ts                     # namespace 残留修正
docs/api/sidecar/                              # /api/jobs 的 phases 文档片段
```

**Structure Decision**: 沿用单 workspace 结构；后端改 `lanrurugi-plugin` / `lanrurugi-core` / `lanrurugi-storage` / `lanrurugi-api` / `lanrurugi-scanner` 五个 crate，前端只改 Upload 页面与其数据类型。没有新 crate、没有新进程、没有新持久化。

## Phase 0: Outline & Research

已完成，见 [research.md](./research.md)。八个决定：

| 编号 | 决定 |
|---|---|
| R1 | 缓存放 `PluginPool`，键 `(mtime, size)` + 60 s 保守上限 + single-flight |
| R2 | 入库只跳过"启动阶段已成功执行的那一个插件"，其余照跑；跳过时必须显式落 title/tags |
| R3 | `JobStatus.phases`（内存态，1 s 轮询可见）；跨 crate 边界用 `tokio::task_local!` 时钟 |
| R4 | 复用 `diagnostics/report.rs` 的手工事务模式，六个子 span |
| R5 | 超时分支用 `try_lock` 探测内容锁，区分"另一条 ingest 持有"与真超时 |
| R6 | 修 `chaika.ts` 的 namespace；不一致时 warn + 诊断，不动 API |
| R7 | 前端三段读数 + "含排队"标注 + 无 `phases` 时的完整回退 |
| R8 | 截图那一行作为固定验收样本（排队 13:03 / 下载 4:27 / 入库 0:06） |

## Phase 1: Design & Contracts

已完成：

- [data-model.md](./data-model.md)：E1 插件声明缓存条目（含 V1–V5）、E2 阶段计时（V6–V9）、E3 preview 产出者（V10–V13）；明确不新增任何持久化。
- [contracts/](./contracts/)：`/api/jobs` 追加字段（C1–C7）、插件声明缓存的可观察承诺（P1–P7）、队列项追加字段（Q1–Q4）、追踪 span 与阶段词表（T1–T5）。
- [quickstart.md](./quickstart.md)：9 个验证场景，逐条对上 SC-001..007 与契约/回退检查。

### 实现顺序（供 tasks 展开）

| 阶段 | 内容 | 依赖 |
|---|---|---|
| A | 插件声明缓存 + single-flight + namespace 一致性告警 | — |
| B | 队列项 `metadata_preview_namespace` + 入库跳过已执行插件 + 显式应用缓存 title/tags | A（跳过逻辑依赖发现变便宜；字段独立） |
| C | `JobStatus.phases` + 六段打点 + watcher 超时分类 | — |
| D | 下载任务追踪事务与子 span | C（共用阶段边界） |
| E | 前端三段读数、速率标注、回退 | C（字段） |
| F | 清理与文档（chaika namespace、sidecar 片段、README/comparison 评估） | — |

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 跳过插件导致档案标题退回文件名（真实的回归面） | research.md R2 已定位"title 只有插件链会写"；B 阶段把"显式应用缓存 title"列为独立任务并加测试；quickstart 场景 5 直接断言标题是插件标题 |
| 缓存让插件编辑在 dev 里"不生效"（`plugins/` 是 bind mount） | P1 契约 + quickstart 场景 2 做显式验证；失效键用 mtime+size，编辑必然改 mtime |
| 并发发现退化成 N×34 | single-flight 是验收项（契约 P4），不是优化项 |
| `phases` 让 job 序列化变大 / 影响 SSE | 只有 6 条小对象、只放进 `/jobs` 响应、不进 SSE progress delta；`skip_serializing_if` 空则省略 |
| 埋点开销或失败影响下载 | 复用既有事务模式，未配置 DSN 时 sentry 侧 no-op；契约 T4 + quickstart 场景 7 对照 |
| 调度/计时的改动引入数据竞态 | 阶段计时只写自己的内存 job，不碰 Redis 记录；`metadata_preview_namespace` 的写入沿用既有 `metadata_preview` 的同一 `repo.update` 路径（不新增"读旧写回"） |

## 验证

- 单元/集成：`mise run check-crate -- lanrurugi-plugin lanrurugi-core lanrurugi-api lanrurugi-scanner`，再 `mise run test-crate -- ...`；前端 `pnpm vitest` + `mise run frontend-lint`。
- 端到端：quickstart 的 9 个场景；场景 9（NAS beta 整批对照）在实现完成后由用户或代理在真实容器上跑一次。
- 视觉：三段计时的排版由用户确认（不自己截图自证）。
