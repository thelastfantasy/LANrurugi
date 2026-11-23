# Tasks: 删除安全网（删除台账 / 自动快照 / 批量删除守卫 / 库目录诊断）

**Input**: `specs/028-deletion-safety-net/` 下的 spec.md、plan.md、research.md、data-model.md、contracts/、quickstart.md

**Tests**: 每个 User Story 都有 Independent Test，故各故事带测试任务；测试只跑改动到的 crate（`mise run test-crate -- <crate>`），全量 `mise run test` 交给 CI。

**实现约定**：台账与快照都必须在**隔离副本实例**上验证（见 quickstart.md），不得动 `lrr-dev`。

## Phase 1: Setup

- [x] T001 在 `crates/lanrurugi-storage/src/keys.rs` 增加 `DELETION_JOURNAL_KEY`（`LANRURUGI_DELETION_JOURNAL`），并注明它与档案记录同库、上限 20000。
- [x] T002 在 `crates/lanrurugi-api/src/settings.rs` 增加 5 个数字设置项及默认值/范围：`deletion_journal_days`(30)、`deletion_guard_percent`(25)、`deletion_guard_min`(50)、`snapshot_daily_keep`(7)、`snapshot_week_keep`(4)，沿用 `NUMBER_FIELDS` 模式。
- [x] T003 新建 `crates/lanrurugi-api/src/deletions.rs` 与 `crates/lanrurugi-api/src/snapshots.rs` 两个模块骨架，在 `lib.rs` 里 `pub mod` 并挂上空 router。

## Phase 2: Foundational（各故事共用）

- [x] T004 `deletions.rs`：实现 `journal_entry` 结构（serde）、`journal_put(state, archive, source)`、`journal_list`、`journal_take(id)`、`journal_len`，以及「超过 20000 条按 deleted_at 淘汰」的逻辑；写入失败返回明确错误供调用方降级。
- [x] T005 `snapshots.rs`：实现快照目录解析（`<library>/.lanrurugi-backups`）、原子写入（`.tmp` + rename）、文件名解析（时间戳 + reason）、列表（含 `usable` 轻量校验）。
- [x] T006 `snapshots.rs`：实现 `take_snapshot(state, reason)`，内部调用 `lanrurugi_backup::build` 并流式写盘；失败只返回 `Err` 供调用方降级。
- [x] T007 在 `crates/lanrurugi-scanner/src/watcher.rs` 与扫描的目录枚举处把 `.lanrurugi-backups` 加入忽略集合（与 `.lanrurugi-recycle` 同一处）。

## Phase 3: US1（删除台账，P1）

- [x] T008 [US1] `archives.rs::purge_one_archive`：在 `archives.delete(id)` 之前调用 `deletions::journal_put`，把 `PurgeOptions` 映射成 `source`；写失败时继续删除并让结果带 `journal_failed`。
- [x] T009 [US1] `deletions.rs`：`restore_one(state, id)` —— 读条目、`HSET` 原记录、清台账、`stat` 文件给出 `file_missing`；同 id 已存在则 `AlreadyExists`。
- [x] T010 [US1] `deletions.rs`：`restore_all` 逐条恢复、统计 `restored/skipped/missing_files/failed`。
- [x] T011 [US1] 端点：`GET /api/deletions`、`POST /api/deletions/{id}/restore`、`POST /api/deletions/restore-all`、`DELETE /api/deletions/{id}`、`DELETE /api/deletions`（`confirm=1`）。
- [x] T012 [US1] 保留期清理：`purge_expired(state)`，接进 `recycle.rs` 的每小时 sweep（同一次 sweep 里先快照后清理，见 T018）。
- [x] T013 [US1] 审计：恢复与清除写 `record_manual`，`route_policy.csv` 加 `/api/deletions*` 的会话管理员策略。
- [x] T014 [P] [US1] 测试 `crates/lanrurugi-api/src/deletions.rs` 单测：journal 往返、超限淘汰、恢复冲突、无效 JSON 跳过；Redis 缺失时按既有约定跳过。
- [x] T015 [US1] 前端：`apps/frontend/src/pages/Settings/Backup.tsx` 增加「最近删除」列表（标题、删除时间、文件是否存在、恢复按钮）+ 对应 i18n key（14 个 locale）。

## Phase 4: US2（自动快照，P1）

- [x] T016 [US2] `snapshots.rs`：`daily_on_startup(state)` —— 当天没有 daily 就生成一份；失败只 warn 且当天不再重试（进程内标记）。
- [x] T017 [US2] `snapshots.rs`：`retention_sweep(state)` —— 每日按日期去重、每周按 ISO 周分组，保留 `snapshot_daily_keep` 与 `snapshot_week_keep`；`pre-*` 只留最近 5 份；永不删最新一份可用快照。
- [x] T018 [US2] 破坏性操作前快照：`database.rs` 的 clean / restore / rebuild-index 开头各调一次 `take_snapshot`，结果写进任务输出。
- [x] T019 [US2] 端点：`GET /api/backups`、`POST /api/backups`、`GET /api/backups/{name}`、`POST /api/backups/{name}/restore`、`DELETE /api/backups/{name}`（名字校验防目录穿越）。
- [x] T020 [US2] `main.rs`：启动流程里接 `daily_on_startup`（后台 spawn，不阻塞就绪）。
- [x] T021 [P] [US2] 测试 `snapshots.rs`：保留策略（7+4、pre-* 5 份、最新不可删）、损坏文件 `usable=false`、原子写入不留半截文件。
- [x] T022 [US2] 前端：备份区增加「自动快照」列表（名称、原因、大小、可下载/可恢复）+ i18n。

## Phase 5: US3（批量删除守卫，P2）

- [x] T023 [US3] `database.rs::clean_database`：删除前算 `planned`（`file` 为空的记录数），超阈值且无 `force` 时返回 409（`planned/total/percent/guard_*`），不删任何记录。
- [x] T024 [P] [US3] 测试：守卫数学（阈值边界、`force` 绕过、小库不拦）。
- [x] T025 [US3] `clean_database` 成功响应加 `journaled` 与 `diagnostics` 字段；前端设置页的清理按钮在 409 时提示需要确认。

## Phase 6: US4（库目录诊断，P3）

- [x] T026 [US4] 新建 `crates/lanrurugi-api/src/library_diagnostics.rs`：`library_source()`（Linux 读 mountinfo 最长前缀匹配，否则 unknown）与 `overlap_percent(records)`。
- [x] T027 [US4] `full_scan.rs` 的 summary 增加 `scanned_files/records_total/overlap_percent/library_source`，并写一条 `tracing::info!`。
- [x] T028 [US4] `main.rs` 把 summary 的新字段写进扫描 job 的 result；`clean_database` 也带同一份 diagnostics。
- [x] T029 [P] [US4] 测试 `library_diagnostics.rs`：mountinfo 解析（含合成样例）、取不到时的 unknown 分支、overlap 计算。

## Phase 7: Polish & 验收

- [x] T030 文档：`docs/dev/` 增补一页说明台账/快照的位置、保留策略与恢复流程；`docs/comparison-with-lanraragi.md` 记一条「删除可撤销」。
- [x] T031 按 `quickstart.md` 在**数据库副本实例**上跑完 S1–S5，记录实际输出。
- [x] T032 回归：`mise run test-crate -- lanrurugi-api lanrurugi-storage lanrurugi-scanner`、`mise run clippy`、`mise run fmt-check`、前端 `tsc -b --force` + `eslint`。
- [ ] T033 提交（一个 commit）→ push → PR（body 引用 #351 并把归因更正写进去）。
