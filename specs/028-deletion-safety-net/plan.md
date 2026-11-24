# Implementation Plan: 删除安全网（删除台账 / 自动快照 / 批量删除守卫 / 库目录诊断）

**Branch**: `028-deletion-safety-net` | **Date**: 2026-11-23 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/028-deletion-safety-net/spec.md`

## Summary

给「档案记录被永久删除」这个动作装三层安全网，并把它们串成一次交付：

1. **删除台账**（Redis，按记录）：`purge_one_archive` 是所有删除路径的唯一出口，在它真正删除前把整条记录序列化进 `LANRURUGI_DELETION_JOURNAL`，保留 30 天，提供列表 / 单条恢复 / 全部恢复。这样 Clean Database、单条删除、修订版取代、拆分原始档等路径一次性全覆盖。
2. **自动快照**（文件，按全库）：复用现有的 `lanrurugi_backup::build` 文档，落到 `<library>/.lanrurugi-backups/`（与回收站 `.lanrurugi-recycle/` 同一套「库内点目录、扫描忽略」约定，容器无需新增挂载），**按可配置计划**（默认每天本地 03:00 一份 `daily`、每周一同一时刻一份 `weekly`，可在设置里改时间/星期/关闭）生成 + 每个破坏性操作前一份，保留 7 份每日 + 4 份每周；每份快照旁边写一份 `<name>.summary.json` 内容摘要。
3. **批量删除守卫 + 诊断**：Clean Database 在真正删除前先算计划删除量，超过「>25% 且 ≥50 条」就拒绝并要求 `force`；扫描与清理把「扫描文件数 / 记录数 / 重合度 / 库目录来源」写进日志与任务结果。

## Technical Context

**Language/Version**: Rust 1.99（workspace edition 2021）、TypeScript/React 19（前端）

**Primary Dependencies**: axum、deadpool-redis、tokio、serde_json；现有 `lanrurugi-backup`、`lanrurugi-storage::recycle`、`lanrurugi-core::jobs`

**Storage**: Redis（台账、设置项）、文件系统（快照 JSON 落在 `<library>/.lanrurugi-backups/`）

**Testing**: `cargo test`（单元 + Redis 支持的集成测试，走 `LANRURUGI_TEST_REDIS_URL`）、Vitest/RTL（前端）

**Target Platform**: Linux / macOS / FreeBSD，容器与原生安装都要能用

**Project Type**: web service（Rust 后端 + React 前端，单进程）

**Performance Goals**: 台账写入是删除路径上的常数开销（一次 HSET）；每日快照一次全量序列化，10 万条记录量级在数十秒内完成且不阻塞启动（后台任务）

**Constraints**: 不改变备份文档格式；不改既有端点的响应形状（守卫只在超阈值时返回 4xx，可用 `force` 恢复原行为）；容器部署不新增挂载；扫描必须忽略备份目录

**Scale/Scope**: 目标库规模 10 万档案；台账保留 30 天、单条数百字节到数 KB

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 影响 | 结论 |
|---|---|---|
| I. Legacy 数据与用户信任兼容 | 台账/快照都是**新增**的 Redis 键与新目录，不改变任何既有键的形状；恢复写回的是删除前的原样记录 | 通过 |
| II. API 契约保真 | 新端点全部是新增（`/api/deletions*`、`/api/backups*`）；既有 `POST /api/database/clean` 只在「计划删除量超阈值」时返回 4xx（正常使用永不触发），并以 `?force=1` 保持原行为 | 通过，且按治理要求显式记录：这是对既有端点的**条件性**行为变更，例外理由写进 spec 的 Assumptions 与本表 |
| III. 资源克制、真并发单进程 | 台账写入在删除路径上同步完成（一次 HSET，微秒级）；快照在后台 job 里流式写盘，不整库进内存；不新增线程池/调度器，复用既有周期性 sweep | 通过 |
| V. 秘密与网络边界 | 台账与快照都是本地数据；新端点走既有会话/管理员权限并写审计日志 | 通过 |
| VII. 前端纪律 | 设置页只在既有备份区追加两块列表，复用既有 class，不新增主题色 | 通过 |
| 工程质量门 | 每步跑 `mise run check-crate`、`mise run test-crate`、fmt/clippy；Redis 依赖测试按既有约定在缺失时跳过 | 通过 |

**Post-design re-check**: 见本文件末尾（Phase 1 完成后回填）。

## Project Structure

### Documentation (this feature)

```text
specs/028-deletion-safety-net/
├── plan.md              # 本文件
├── research.md          # Phase 0：设计决策与取舍
├── data-model.md        # Phase 1：台账条目 / 快照 / 设置项
├── quickstart.md        # Phase 1：可执行的验收脚本
├── contracts/           # Phase 1：新增端点契约
│   ├── deletions-api.md
│   └── backups-api.md
└── tasks.md             # Phase 2：/speckit-tasks 产出
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-api/src/
│   ├── deletions.rs            # 新增：台账读写、恢复、保留期清理、端点
│   ├── snapshots.rs            # 新增：快照生成/保留/列表/恢复端点
│   ├── archives.rs             # 修改：purge_one_archive 删除前写台账
│   ├── database.rs             # 修改：clean_database 守卫 + 操作前快照 + 任务结果字段
│   ├── settings.rs            # 修改：4 个新设置项（台账保留、守卫阈值两项、快照保留两项）
│   ├── recycle.rs             # 修改：周期性 sweep 增加台账到期清理与快照保留
│   └── library_diagnostics.rs # 新增：库目录来源/重合度
├── lanrurugi-scanner/src/full_scan.rs  # 修改：诊断字段进 summary 与日志；忽略备份目录
├── lanrurugi-storage/src/keys.rs       # 修改：新键常量
└── lanrurugi-server/src/main.rs        # 修改：启动时每日快照 + 把诊断字段写进任务结果
apps/frontend/src/pages/Settings/Backup.tsx  # 修改：快照列表 + 最近删除列表 + 计划设置
```

## Phase 0: Research

见 [research.md](./research.md)。所有未知项已在其中以「Decision / Rationale / Alternatives」形式定案，无 NEEDS CLARIFICATION 残留。

## Phase 1: Design & Contracts

- 实体与不变量：[data-model.md](./data-model.md)
- 端点契约：[contracts/deletions-api.md](./contracts/deletions-api.md)、[contracts/backups-api.md](./contracts/backups-api.md)
- 验收脚本：[quickstart.md](./quickstart.md)

## Post-design Constitution Check

- 台账键与快照目录均为新增，未触碰 Principle I 的既有数据形状。
- 新端点全部在 `/api` 下新增，未修改既有端点路径或成功响应形状；`clean` 的条件性 4xx 已在 spec Assumptions 显式声明。
- 快照写盘在 `spawn_blocking` + 临时文件 + rename，避免长事务与半截文件。
- 前端只追加列表，不引入新依赖或新主题色。

结论：通过。
