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

## Phase 8: 可配置备份计划 + 快照摘要（追加，2026-11-23）

- [x] T034 `settings.rs`：新增 `snapshot_schedule_enabled`（bool，默认 true，进 `BOOL_FIELDS`）、
  `snapshot_daily_at`（string `"HH:MM"`，默认 `"03:00"`，进 `STRING_FIELDS`）、`snapshot_weekly_day`
  （number，默认 1，进 `NUMBER_FIELDS`）；`validate_setting_field` 加 `parse_hhmm` 校验（严格 `HH:MM`、
  00:00–23:59）与 `snapshot_weekly_day` 0–6（**0 = 周日**）范围；三个读取函数供调度与列表用。
- [x] T035 `snapshots.rs`：`SnapshotSchedule` / `ScheduleState` + 纯函数 `next_daily_at`、
  `most_recent_daily_at`、`next_weekly_at`、`most_recent_weekly_at`、`slot_is_due`、`resolve_local`
  （DST：缺口顺延、重复取较早瞬时），槽位记录在 Redis hash `LANRURUGI_SNAPSHOT_SCHEDULE`
  （config 库，`last_daily_at` / `last_weekly_at`，RFC 3339 UTC）。
- [x] T036 `snapshots.rs::schedule_tick`：每分钟 tick 的调度实现（取代 `daily_on_startup`），比较最近槽位与
  `last_*`——重启不重复、停机只补一次、失败槽位进程内不重试、生成后立即跑一次保留期清理；关闭时不生成。
- [x] T037 `main.rs`：把「启动时 daily + 每小时补跑」换成单个每分钟 tick（首 tick 立即 = 启动补跑），
  每小时任务只保留 `recycle::sweep` 的保留期清理。
- [x] T038 `snapshots.rs`：生成快照时同目录写 `<name>.summary.json`（原子写），`list_in` 读它并放进
  `SnapshotEntry.summary`（缺失/损坏为 `null`，不读快照本体）；删除/保留清理时连摘要一起删。
- [x] T039 `GET /api/backups` 增加 `schedule` 字段（`enabled/daily_at/weekly_day/last_*/next_*`，关闭时两个
  `next_*` 为 null）与每条 entry 的 `summary`。
- [x] T040 [P] 测试：跨天/跨周的下一次到期、关闭时两个 next 为 null、catch-up 只欠一次、摘要往返
  （含 `.summary.json` 不被当成快照、缺失/损坏为 null、删除时一并删掉）、`parse_hhmm`/`snapshot_weekly_day`
  的校验矩阵。
- [x] T041 文档：`data-model.md` 设置项表与快照摘要/槽位状态、`contracts/backups-api.md` 的
  `schedule` + `summary` + 计划语义（时区/DST）、`quickstart.md` 的 S5「改计划后一分钟内生效」、
  `docs/dev/deletion-safety-net.md` 的生成时机与设置项表。

## Phase 9: 分档开关（追加，2026-11-24）

背景：整库快照在生产规模的库上不是免费的，默认每天一份偏重。改成两档各自独立开关，**默认只每周一次**，
`daily` 由用户主动开启。

- [x] T042 `settings.rs`：`BOOL_FIELDS` 新增 `snapshot_daily_enabled`（默认 **false**）、
  `snapshot_weekly_enabled`（默认 **true**），配默认值常量与 `read_snapshot_daily_enabled` /
  `read_snapshot_weekly_enabled` 两个读取函数（与 `read_snapshot_schedule_enabled` 同形，抽出
  `read_bool(state, key, default)` 共用）；写侧放行沿用 `validate_setting_field` 的 `BOOL_FIELDS` 检查。
- [x] T043 `snapshots.rs`：`SnapshotSchedule` 增加 `daily_enabled` / `weekly_enabled`，`load()` 读取、
  `next()` 对关闭的一档返回 `None`、`schedule_tick()` 跳过关闭的一档（不比较槽位、不写 `last_*`）；
  `schedule_json` 增加 `"daily_enabled"` / `"weekly_enabled"` 两个字段。
- [x] T044 [P] 测试：`next()` 在仅 weekly（默认）/ 两档都开 / 都关 / 仅 daily 四组下的返回值；
  `schedule_json` 对每档开关的 `next_*` 为 null 的断言；`settings.rs` 的两个新布尔键默认值与写侧校验。
- [x] T045 文档：`data-model.md`（§1 不变量、§2.1 语义、§4 表与默认语义）、
  `contracts/backups-api.md`（`schedule` 字段与生成时机、分档语义）、`quickstart.md`（S3 默认只有
  weekly、S5 先开 daily 再改时间、只关一档的验证）、`docs/dev/deletion-safety-net.md`（生成时机与设置项表）。

## Phase 10: 快照（RDB）与导出（JSON）分家 + pin + zip 导出（追加，2026-11-24）

背景：用户拍板方案 A。此前目录里只有一种 JSON「快照」，既当自动备份又当可恢复导出，语义含混：
JSON 恢复不了配置/文件映射/搜索索引，而一次真正的全量备份在 JSON 里根本表达不出来。现在拆成两类：

- **快照 `kind=backup`**：`snapshot-<时间戳>-<reason>.rdb`，Redis 全量 RDB（`BGSAVE` + 拷文件），
  app 内可异步还原（`DEBUG RELOAD NOSAVE` + `BGREWRITEAOF`），也可用 CLI / 纯 shell 还原。
- **导出 `kind=export`**：`export-<时间戳>-<reason>.json`，可选择性数据，app 内可还原；历史无前缀的
  `<时间戳>-<reason>.json` 仍按导出列出。

- [x] T046 `snapshots.rs`：`SnapshotKind` + 文件名解析按**前缀**判 kind（`snapshot-`/`export-`/历史无前缀），
  `parse_name` 返回 `{created, kind, reason}`；`SnapshotEntry` 增加 `kind`，`entry_json` 输出它；
  `looks_like_rdb` 只读前 5 字节校验 `REDIS` 魔数。
- [x] T047 `snapshots.rs`：`take_export`（原 `take_snapshot` 改名）与新增 `take_full_backup`
  （`CONFIG GET dir/dbfilename` → `BGSAVE` → 轮询 `LASTSAVE` → 拷到 `.tmp` → 校验魔数 → `rename`），
  远程/托管 Redis 与禁 `CONFIG` 时返回明确错误；`copy_rdb` 全部在 `spawn_blocking` 内。
- [x] T048 调度与操作路径：`schedule_tick`/`run_slot` 生成**快照**；`take_for_operation` 与
  `deletions.rs` 的两处 `pre-journal-purge` 仍是**导出**。
- [x] T049 端点：`POST /api/backups` = 立即生成快照（RDB），新增 `POST /api/backups/export` = 立即生成导出；
  `GET /api/backups` 一个 `entries` 数组带 `kind`；共用 `GET|DELETE /api/backups/{name}`、
  `POST|DELETE /api/backups/{name}/pin`；`POST /api/backups/{name}/restore` 两类都接受，快照走异步 job（`start_full_restore`，
  `GET /api/backups/jobs/{id}/events` 复用导出还原的 SSE）；`route_policy.csv` 补 `POST /api/backups/export`
  与 pin 的 deny 行。
- [x] T050 pin：Redis set `LANRURUGI_SNAPSHOT_PINS`（`keys.rs`）、`read_pins`/`set_pin`、
  `retention_plan` 跳过 pinned 且不占名额、删除文件时清 pin、列表每条带 `pinned`；读不到 pin 时跳过清理。
- [x] T051 保留：快照类固定最近 3 份（`FULL_BACKUP_KEEP`，不做成设置项），导出类沿用 7/4/5/5；
  两类都不删「最新一份可用文件」。
- [x] T052 下载：导出支持 `?format=zip`（单成员、文件名 `<name>.zip`、`Content-Disposition` 用它）；
  快照始终是单文件 `.rdb`（`application/octet-stream`），导出是 `application/json`。
- [x] T053 [P] 测试：两类命名解析（含历史无前缀、前缀与扩展名不匹配）、快照 3 份上限、pin 永不入选且
  取消后可淘汰、pin 不占名额、zip 可读回且只含该文件、pinning 往返 + 删除清 pin、
  全量备份落盘；非 RDB 文件被 `preflight` 拒绝（400 `not_a_redis_backup`，远程 Redis 走显式错误分支）。
- [x] T054 文档：`data-model.md`（§2 两类文件与字段、§2.2 pin、§4 设置项）、
  `contracts/backups-api.md`（两类契约 + 运维还原 + 保留/pin 表）、`quickstart.md`（S3 两类、S5 计划生成 RDB）、
  `docs/dev/deletion-safety-net.md`（含运维还原步骤与远程 Redis 限制）。

## Phase 11: cron 驱动的快照计划（追加，2026-11-24）

背景：用户要的是 TrueNAS 那种「任意粒度」的计划，而不是「每天几点 + 每周星期几」两个旋钮。Phase 8 的
`snapshot_daily_at` / `snapshot_weekly_day`（以及 Phase 9 的两档开关）全部移除，换成一个 6–7 段 crontab：

- [x] T055 新建 `crates/lanrurugi-api/src/snapshots/cron.rs`：自研的 6–7 段解析器（`秒 分 时 日 月 星期 [年]`），
  标准 cron 语义——数字星期 `0`/`7`=周日、`1`=周一，`SUN`…`SAT`/`JAN`…`DEC` 名字，`*`/`?`/`a`/`a-b`/`*/n`/`a/n`/`a-b/n`/逗号列表，
  **日与星期都受限时取「或」**（Vixie 规则）；`CronError` 带字段名，`Display` 输出 `seconds (segment 1): "61" is outside 0-59`；
  `next_after`（按年/月/日/时/分/秒边界推进，不逐秒扫描）与 `next_runs`（10 个）；DST 缺口跳过、重复取较早。
- [x] T056 `settings.rs`：移除 `snapshot_daily_at` / `snapshot_weekly_day` / `snapshot_daily_enabled` /
  `snapshot_weekly_enabled` 及 `parse_hhmm`；新增 `snapshot_cron`（string，默认 `"0 0 3 * * 1"`）与
  `snapshot_cron_preset`（string，默认 `"weekly"`，闭集 `hourly|daily|weekly|monthly|custom`）；读取函数
  `read_snapshot_cron` / `read_snapshot_cron_preset`（共用新的 `read_string`）；校验非法表达式 400 且指出字段。
- [x] T057 `snapshots.rs`：`SnapshotSchedule` 改为 `{enabled, cron, cron_text, preset}`；`schedule_tick` 每 5 秒
  （`main.rs` 的 interval 同步改成 5 秒）；纯函数 `due_firing`（重启不重复 / 停机只补一次 / 首次安装不追溯）；
  Redis `LANRURUGI_SNAPSHOT_SCHEDULE` 的 `last_cron_fire` 记**生成完成**的时间；计划快照 reason 改为 `scheduled`。
- [x] T058 `GET /api/backups` 的 `schedule` 改为 `{enabled, cron, preset, timezone, last_fire_at, next_runs}`：
  `next_runs` 是 10 个秒级 ISO 时刻（带本地偏移），停用时为 `[]`；`timezone` 用 `iana-time-zone`（已在
  `Cargo.lock` 里，经 chrono 引入，不新增下载）+ `chrono::Local` 的偏移，形如 `Asia/Tokyo (+09:00)`。
- [x] T059 新增 `POST /api/backups/schedule/preview`（`{"cron": "..."}` → `{valid, error?, timezone, next_runs}`，
  恒 200），`route_policy.csv` 补两个 token 角色的 deny 行。
- [x] T060 [P] 测试：`cron.rs` 覆盖秒级 `*/10`、每小时/每周一/每月 1 号、跨日/跨年、年字段、闰日、名字、
  dom/dow 的「或」语义、非法段（数量/范围/步长）与 `next_runs` 严格递增；`snapshots.rs` 覆盖 `due_firing`
  的首次安装不追溯、已服务的不重复、错过的 tick 仍服务、停机只补一次，以及 `schedule_json` 的形状与停用时的空 `next_runs`。
- [x] T061 文档：`data-model.md`（§2.1 状态字段、§2 不变量、§4 设置项）、`contracts/backups-api.md`
  （`schedule` 形状 + cron 语义 + preview 端点 + 生成时机）、`quickstart.md`（S3/S5 重写）、
  `docs/dev/deletion-safety-net.md`（tick 频率、cron 语义、时限）；`spec.md` FR-007 与 `research.md` D11 同步。

## Phase 12: 快照/导出的异步任务与 SSE 进度（追加，2026-11-24）

背景：用户要「精确到 0.1% 的进度」，前端每 250 ms 显示一次。同步端点无法给进度（RDB 的 `BGSAVE` 可能几十秒），
所以两个创建端点改为异步任务 + SSE，复用仓库已有的 SSE 连接管理。

- [x] T062 新建 `snapshots/progress.rs`：`BackupProgress` trait（`update(written, expected, percent)`）+ `NoProgress`；
  导出用 `write_document`（逐条记录序列化，输出与 `serde_json::to_writer` **字节一致**，有测试），按记录数报进度，
  `bytes_expected` 随进度收敛；采样 150 ms。
- [x] T063 `snapshots.rs`：`take_full_backup` 增加进度参数——`BGSAVE` 阶段每 200 ms 采样 Redis 的 `temp-*.rdb`
  大小 ÷ 分母（`INFO memory` 的 `used_memory`，取不到则上一次 RDB ×1.05）走 0–90%，`copy_rdb` 改为手写分块拷贝
  按字节走 90–100%；`take_export` / `write_atomic` 接进度；`run_firing` 与 `take_for_operation` 用 `NoProgress`。
- [x] T064 新建 `snapshots/jobs.rs`：进程内任务注册表（完成/失败保留 1 小时、最多 32 个）、`BackupJob`
  （`percent` 单调不减、终态 100、失败带 error）、`start_full_backup` / `start_export`；SSE 复用
  `tankoubon_grouping/run.rs` / `archive_split.rs` 的 axum `Sse` + `broadcast` + `KeepAlive` 组合，
  首事件为绝对状态快照，终态后关闭流；响应带 `Cache-Control: no-cache` 与 `X-Accel-Buffering: no`，
  空闲每 12 s 一行 `: keep-alive`。
- [x] T065 端点：`POST /api/backups` / `POST /api/backups/export` 返回
  `{job_id, started_at, events}`；新增 `GET /api/backups/jobs/{job_id}`（轮询兜底）与
  `GET /api/backups/jobs/{job_id}/events`（SSE）；`route_policy.csv` 补两条 GET 的 deny 行。
- [x] T066 [P] 测试：进度单调不减 + 终态 100 + 失败带 error；broadcast 上先 `progress` 后 `done`、失败为 `error`；
  逐条序列化的文档与 `serde_json::to_writer` 字节一致（含 `share_node_secret` 与非空集合）；两个 create 端点
  经 `wait_for_job` 断言 `state=done` / `percent=100` / `entry.kind`。
- [x] T067 文档：`contracts/backups-api.md`（任务端点 + SSE 事件与百分比来源 + 头）、`data-model.md` §2.3
  （任务状态字段与保留规则）、`quickstart.md`（S3 第 2 步改成 job + SSE）、
  `docs/dev/deletion-safety-net.md`（异步任务与进度来源）。

## Phase 13: 其余长耗时数据操作接入同一任务通道（追加，2026-11-24）

背景：用户要求「所有长耗时数据操作都要有准确进度」，统一走 Phase 12 的任务 + SSE 基建，不另造一套。

- [x] T068 `lanrurugi-backup`：`restore::restore_with_progress`（按 `doc.archives` 逐条报 `(done,total)`，
  旧 `restore` 保留为无进度包装）；`import_legacy::import_from_legacy_with_progress`（按实际要写入的档案数报进度，
  旧函数保留）；`RecordProgress` 类型。
- [x] T069 `snapshots/jobs.rs` 泛化：`kind` 由 `SnapshotKind` 改为自由字符串（`backup`/`export`/`restore`/`import`），
  新增 `result` 字段与 `finish_with_result`、`start_progress_job(kind)`、`record_percent(done,total)`；
  终态事件同时带 `entry` 与 `result`（不适用的那个为 `null`）。
- [x] T070 `database.rs`：`start_restore_job` 同时创建进度任务并返回 `(jobs 控制台 id, 进度 job)`；
  `restore_with_progress` 驱动它；`done.result` 带 `restored`/`skipped`/各分类计数/`snapshot`。
  `POST /api/database/restore` 与 `POST /api/backups/{name}/restore` 的响应都加
  `job_id`/`started_at`/`events`（`job` 字段保留兼容）。
- [x] T071 `database.rs`：`queue_import_legacy` 同样创建进度任务、用
  `import_from_legacy_with_progress` 驱动；`done.result` 是导入汇总 + `rebuild`；部分失败进 `error`
  并附已完成的计数。响应加 `job_id`/`started_at`/`events`。
- [x] T072 记录口径的进度统一：档案写入占 0–90%，收尾占最后 10%，终态 100；`bytes_written`/`bytes_expected`
  在 restore/import 里表示「已处理/总记录数」。
- [x] T073 [P] 测试：`jobs.rs` 新增记录口径任务的单调进度 + 终态 100 + `result` 汇总 + 失败带 error；
  `record_percent` 的 0/0 与 100 边界。
- [x] T074 文档：`contracts/backups-api.md`（restore 的异步响应 + 任务通道承载的操作表）、
  `data-model.md` §2.3（`kind` 取值、`result` 字段、记录口径）、`quickstart.md`（restore 走 SSE）、
  `docs/dev/deletion-safety-net.md`（操作表与百分比分配）。

## Phase 14: 修「计划从不触发」+ 快照等待信号（追加，2026-11-24）

现场复现：隔离实例里 `snapshot_cron=*/10 * * * * *`，`GET /api/backups` 的 `schedule` 正常、
`next_runs` 每 10 秒，但等 3 个槽位也没有任何快照、`last_fire_at` 恒为 `null`。

- [x] T075 根因：首次判定要求「当前这一秒正好命中」，而 tick 是 5 秒一次——`*/10` 这类周期与 5 秒不对齐时，
  tick 永远落不到命中秒上，第一份永远不生成，也就永远没有 `last_cron_fire` 让后续的补发规则生效。
  改为**没有记录时第一个 tick 立即生成一份**（等价于旧版启动快照），之后交给 cron；文档同步。
- [x] T076 tick 可见性：每次判定命中写一条 `tracing::info!`（含 cron、槽位、是否首份），未命中写 `debug!`，
  现场能直接看出调度是否在跑。
- [x] T077 等待信号：`BGSAVE` 完成判定由只看 `LASTSAVE`（整秒）改为 `LASTSAVE` 前进 / RDB mtime 变化 /
  `INFO persistence` 显示无进行中的保存（fork 起跑后 500 ms 才采信）三者任一，避免「同一秒内的保存 +
  远程 RDB 路径」导致 120 秒空转。
- [x] T078 可测性：`schedule_tick` 拆出 `tick_with(state, now, 注入的写入闭包)`，生产路径注入真实
  `take_full_backup`；测试注入写文件的闭包，于是「`*/5` + 连续 tick ⇒ ≥1 份 `snapshot-*.rdb` 且
  `last_fire_at` 非空」可以在不依赖 Redis RDB 可读的环境里跑（真 RDB 路径在测试容器里不可读，只会 skip）。
  同时补「同一水位不重复触发」。

## Phase 15: 计划按应用 `timezone` 求值（追加，2026-11-24）

现场：dev 容器 `TZ=UTC`，应用设置 `timezone=Asia/Tokyo`，`schedule.timezone` 却回 `+00:00`；于是
`0 0 3 * * 1` 实际在 UTC 03:00（= JST 12:00）触发，UI 却写「下次 03:00:00」——差 9 小时。

- [x] T079 `settings.rs`：新增 `read_timezone`（读 `timezone` 字段，`chrono_tz::Tz` 解析，非法/空回退 `UTC`）。
- [x] T080 `snapshots/cron.rs`：`next_after`/`next_runs` 泛化为 `Z: TimeZone`；候选时刻由目标时区自己的墙钟构造
  （DST 缺口 → 无该瞬时并跳过；重复小时取较早）。
- [x] T081 `snapshots.rs`：`SnapshotSchedule.tz`、`due_firing(cron, tz, now, last)`、`tick_with`/`schedule_tick`
  用 `Utc::now().with_timezone(&tz)`；`schedule_json`/`preview` 用同一时区，`timezone` 回传 `IANA 名 (+HH:MM)`，
  `next_runs`/`last_fire_at` 为带偏移 RFC3339；文件命名仍是 UTC（`now_utc()`，未改）。
- [x] T082 去掉不再需要的 `iana-time-zone` 直接依赖（时区名现在来自设置，不是探测宿主）。
- [x] T083 [P] 测试：`Asia/Tokyo` + `0 0 3 * * 1` ⇒ 下一次是 JST 周一 03:00（= UTC 周日 18:00），
  `next_runs[0] == "2026-11-23T03:00:00+09:00"`、`schedule.timezone == "Asia/Tokyo (+09:00)"`；
  `UTC` 对照用例；`America/New_York` 春季缺口跳过 02:30、冬夏偏移分别为 `-05:00`/`-04:00`、墙钟仍 03:00。
- [x] T084 文档：contracts（计划语义的时区段 + DST 段）、data-model（§2/§2.1/§4）、
  `docs/dev/deletion-safety-net.md`（生成时机与设置项表）。

## Phase 16: 快照全量还原（追加，2026-11-24）

背景：「用快照还原整个数据库」原先直接 409，只能停机手工拷 RDB，Web UI 不可用时更没法做。现在三条路径
（Web UI、CLI、纯 shell）都可用，且还原期间暂停写入。

- [x] T085 `snapshots/full_restore.rs`：`restore_full(state, name, progress)` / `restore_full_with(redis, archive_dir, name, progress)`；
  `preflight`（名字/kind/存在/非空/`REDIS` 魔数）；`pre-restore` 全量快照作为安全副本，失败即中止；
  同目录 `<dir>/.lanrurugi-restore-backup-<ts>.rdb` 回滚副本 + `.lanrurugi-restore-<ts>.tmp` + rename；
  `DEBUG RELOAD NOSAVE`（**不是** `DEBUG RELOAD`——后者会先把内存写回文件，等于丢弃还原）；
  `appendonly=yes` 时 `BGREWRITEAOF` 并轮询 `aof_rewrite_in_progress`（上限 5 分钟）；成功清理临时文件并回 `{restored_from, previous_state, entries}`。
- [x] T086 错误类型：`FullRestoreErrorKind` → `reason` → HTTP 状态（`invalid_backup_name` 400、`not_found` 404、
  `not_a_full_backup` 400、`not_a_redis_backup` 400、`safety_backup_failed` 500、`redis_unavailable` 503、
  `debug_command_disabled` 409（附 `enable-debug-command local` 提示）、`reload_failed` 500、`aof_rewrite_timeout` 500、`io_error` 500）。
- [x] T087 HTTP：`POST /api/backups/{name}/restore` 对 `kind=backup` 返回 `{operation, success, job, job_id, started_at, events}`，
  `jobs::start_full_restore` 写 `BACKUP_RESTORE` 活动（成功 Success / 失败 Failure）；删除 `full_backup_not_restorable` 与 409 分支。
- [x] T088 并发写入闸门：`lanrurugi_core::restore_gate::RestoreGate`（`closed/close/open` + 在途计数 + `Notify`）；
  `procedure::restore_write_gate` 中间件挂在 `/api`——关闭后 `POST/PUT/PATCH/DELETE` 503 `restore_in_progress`，
  `GET`、登录/登出、以及「还原 job 自己的 `GET /api/backups/jobs/*`」放行，重复还原请求 409；关闭后最多等 30 秒在途写入，超时 `warn!` 并继续。
- [x] T089 后台任务检查点：全量/增量扫描（`ScanSummary.aborted` + `abort_all`）、缩略图批量、重复档案检测、
  单行本归组、下载入库（`IngestDownloadError::RestoreInProgress` → `QueueError::IngestFailed`）每 200 条/每批检查一次闸门，
  关闭即提前正常结束并把 job 标为 `failed`（error 为「数据库正在还原，任务已中止；还原完成后请重新运行」）；磁盘文件不动。
- [x] T090 进度：`BackupProgress::note` 默认空实现；`BackupJobStatus.note` 与 SSE 的 `note` 字段（人话阶段说明）；
  里程碑 5/20/35/60/90/100，`bytes_expected` 为快照文件大小。
- [x] T091 CLI：`lanrurugi-server snapshots list`（不需要 Redis）与 `snapshots restore <name> [--yes]`
  （`StdoutProgress` 打到 stdout，未 `--yes` 时打印将覆盖整库并等回车）；位于 `crates/lanrurugi-server/src/cli/snapshots.rs`。
- [x] T092 `redis.conf`：新增 `enable-debug-command local`（`local` = 只允许回环连接）；`Dockerfile`/`Dockerfile.dev`
  的 Redis 8.2.1 支持该指令，无需改动。
- [x] T093 [P] 测试：`full_restore.rs` 单测（非 REDIS 头、空文件、合法头、缺失/异类名字、reason↔状态表、DEBUG 禁用识别）；
  `crates/lanrurugi-api/tests/full_restore_redis.rs` 起临时 `redis-server`（随机端口 + 临时目录 + `appendonly yes`）
  验证还原后值回退、`pre-restore` 副本存在、AOF 重写完成、SIGKILL 重启后仍是旧值；另一台**未开** `--enable-debug-command`
  的临时 Redis 断言返回 `debug_command_disabled` 且原 RDB 字节被放回。找不到 `redis-server` 时 `eprintln!` 跳过。
- [x] T094 文档：`docs/dev/deletion-safety-net.md`（UI / CLI / 纯 shell 三条路径 + 闸门行为表 + 「还原期间跑扫描」说明）、
  `contracts/backups-api.md`、`quickstart.md`、`docs/dev` 的生成时机表；前端契约（`note`）在此记录，UI 由用户另行实现。
