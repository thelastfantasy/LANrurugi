# Implementation Plan: 应用内资源监视：设置页的资源面板与反馈包里的资源快照

**Branch**: `feat/030-resource-monitor` | **Date**: 2026-11-25 | **Spec**: [spec.md](./spec.md)

**Input**: `specs/030-resource-monitor/spec.md`

## Summary

一个"按需存在"的资源监视：设置页的资源面板通过一条 SSE 连接持有订阅租约，租约存在时才有一个 1 Hz 的采样任务；快照同时是反馈包里的 `resources.json` 与错误事件的 `resources` 上下文。所有数字带窗口与来源，cgroup 内存拆 anon/file/slab，读不到的东西标 `unavailable` 而不是 0。

## Technical Context

**Language/Version**: Rust 1.9x（workspace，edition 2021）+ TypeScript 5 / React 19
**Primary Dependencies**: axum（SSE 已用于 `subscriptions/stream`、`snapshots/jobs`、`duplicates`）、tokio 1.52（`Handle::metrics()`）、`sysinfo 0.40`（workspace 已有，`recommend_precompute.rs:133` 在用）、`deadpool-redis`（`INFO`）、`zip`/`flate2`（反馈包）、`@sentry/react 11`（前端事件）
**Storage**: 无新存储。历史只在采样任务内存里；快照不落 Redis/磁盘
**Testing**: `mise run test-one -- lanrurugi-api resources::` / `-- lanrurugi-api diagnostics::bundle` / `-- lanrurugi-plugin pool::tests`；前端 `npx vitest`；真实部署验收在 `dev` 容器
**Target Platform**: Linux（cgroup v2 + PSI 全量）；macOS / FreeBSD 降级为进程级指标
**Project Type**: web-service + SPA（单二进制 + 前端）
**Performance Goals**: 单次采样 < 50 ms；面板首帧 < 1 s；关闭面板后 0 采样、0 任务；快照 < 64 KB
**Constraints**: 不给既有路由改形状；不引入 Prometheus/OTel；采样不持插件池锁；脱敏复用 `diagnostics::scrub`
**Scale/Scope**: 单实例；≤ 50 个插件进程；30 分钟 × 1 Hz 历史（约 1800 个样本，只在内存）

## Constitution Check

| 原则 | 结论 |
|---|---|
| I 单一事实来源 | 每个数字只有一个采集点（`resources/procfs.rs` 的 `read_*`），面板、反馈包、错误上下文都从同一 `ResourceSnapshot` 取 |
| II LANrarugi 兼容 | 纯新增 `/api/resources*`；反馈包只多一个可选文件；不改任何既有响应 |
| III 数据写入 | 不写库/存档记录；历史不落 Redis |
| IV UI 与测试 | 新面板走既有设置页 shell；新增单测覆盖租约生命周期、阈值去重、快照形状 |
| V 插件运行时 | 只读插件池的 pid 列表，不新增锁持有时间（FR-016） |
| VI 性能 | 本特性的核心不变量：无观察者即无采样（FR-001/FR-002，可测） |

无违反项，无需例外记录。

## Project Structure

### Documentation (this feature)

```text
specs/030-resource-monitor/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/{resources-api.md, resources-snapshot.md, bundle-resources.md}
├── checklists/requirements.md
└── tasks.md
```

### Source Code (repository root)

```text
crates/lanrurugi-api/src/
├── resources/{mod.rs, model.rs, procfs.rs, portable.rs, watch.rs, alerts.rs}
└── diagnostics/{mod.rs (route + report hook), bundle.rs (resources 类目，修空 diagnostics.json), resources_file.rs}

crates/lanrurugi-plugin/src/pool.rs         # worker_processes()：只读快照 pid 集合
docs/api/sidecar/resources.yaml             # 新分组
docs/api/tag-names.json                     # resources 组名
apps/frontend/src/pages/Settings/ResourcePanel.tsx
apps/frontend/src/pages/Settings/resources/{format.ts, useResourceStream.ts, Sparkline.tsx}
apps/frontend/src/api/types.ts              # ResourceSnapshot 等
apps/frontend/src/i18n/locales/*.json       # 14 个语言键
```

## Phase 0 — Research（结论见 research.md）

| # | 问题 | 决定 |
|---|---|---|
| R1 | 怎么做到"关掉即 0 占用" | 订阅租约 + 首个观察者 spawn、最后一个 abort；历史随任务释放 |
| R2 | SSE 还是轮询 | SSE（仓库已有 4 处先例），一条连接同时带首帧与后续帧；一次性导出走 `GET /api/resources` |
| R3 | 进程/内存数据从哪来 | Linux 读 `/proc` + cgroup v2 + PSI；其它平台只给 sysinfo 的进程级数字，其余 `unavailable` |
| R4 | 怎么避免 `memory.current` 误导 | 强制拆 anon/file/slab + 单列 peak，并在 UI 标注"含页缓存" |
| R5 | 插件进程怎么归属 | `PluginPool` 新增只读 `worker_processes()`，用 `try_lock` 读 pid，拿不到锁的行标 `busy` |
| R6 | 阈值告警怎么去重 | 内存里记上次触发时刻，同类 10 分钟一次；写既有 warning 诊断事件 |
| R7 | 怎么挂到 Sentry/rustrak | 前端 breadcrumb + `setContext`；后端错误事件附 `contexts.resources`；反馈包附 `resources.json`；全部过 `scrub` |
| R8 | GPU 要不要进第一版 | 进，但只做单帧 + 可关闭，查不到就 `unavailable`（`nvidia-smi` 可 spawn，AMD 读 sysfs） |

## Phase 1 — Design

### 数据流

```text
GET /api/resources/stream (SSE, 管理员专有)
  └─ ResourceWatch::attach()  → observers += 1
      ├─ observers == 1 → spawn sampler (1 Hz)
      │     loop: collect() → 写入环形缓冲 → broadcast 给所有订阅者
      │            └─ alerts::check(snapshot) → 诊断 warning（去重）
      └─ 断开 → observers -= 1；归零 → abort + 释放缓冲

GET /api/resources (一次性快照)
  └─ collect() 直接返回（不注册观察者，不启动任务）

POST /api/diagnostics/bundles (categories.resources)
  └─ collect() → resources.json（与日志同一套脱敏）

error 事件
  └─ watch.last_within(30s) → contexts.resources
```

### 关键不变量（都进测试）

1. `observer_count == 0` 时 `sampler_running() == false`，且 `samples_taken` 不增长。
2. `collect()` 只读文件与内存，不 spawn 子进程（GPU 可选路径除外，且默认关闭时完全不走）。
3. 采样不持插件池的 `workers` 锁：`worker_processes()` 先 `try_lock` 复制 `(namespace, pid)` 再逐个读 `/proc`。
4. 每个数值都有 `at` / `window` / `source` 三字段；缺失的字段是 `null` + `reason`。

### 契约

- `contracts/resources-api.md`：两个路由、SSE 帧格式、错误码、权限。
- `contracts/resources-snapshot.md`：`ResourceSnapshot` 的 JSON 形状（字段级）。
- `contracts/bundle-resources.md`：`Selection.categories.resources`、`resources.json` 的落点与 manifest 记录。

### 降级矩阵

| 环境 | CPU/内存 | cgroup | PSI | swap | 磁盘 | 插件进程 | GPU |
|---|---|---|---|---|---|---|---|
| Linux 容器（本仓库默认） | 全量 | 全量 | 全量 | 全量 | 全量 | 全量 | 尽力 |
| Linux 裸机（无 cgroup 限制） | 全量 | `unavailable` | 全量 | 全量 | 全量 | 全量 | 尽力 |
| macOS | sysinfo | `unavailable` | `unavailable` | sysinfo | statvfs | `unavailable` | `unavailable` |
| FreeBSD | sysinfo | `unavailable` | `unavailable` | sysinfo | statvfs | `unavailable` | `unavailable` |

## Phase 2 — Tasks

见 `tasks.md`（T001–T0xx，按 Phase 1 基础设施 → US1 反馈包 → US2 面板 → US3 历史 → US4 联动 → US5 导出 → 文档/门禁 排列）。
