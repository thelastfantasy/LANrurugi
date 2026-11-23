# 删除安全网（删除台账 / 自动快照 / 批量删除守卫）

`specs/028-deletion-safety-net` 的实现说明。面向运维与排查：数据被误删之后，先看这里。

## 删除台账：`LANRURUGI_DELETION_JOURNAL`

- 位置：Redis **archive** 逻辑库（与档案记录同一个库），一个 hash，field = 档案 id。
- 写入点：`crates/lanrurugi-api/src/archives.rs::purge_one_archive`，在 `archives.delete` **之前**。
  Clean Database、阅读页单条删除、拆分的"删除原档"分支都覆盖到了。
- 每条内容：`{record, deleted_at_ms, source, path, journal_version}`，`record` 是删除前的完整
  `Archive`（标签、简介、阅读进度、时间戳都在里面）。
- 保留：`deletion_journal_days`（默认 30 天，0 = 关闭台账）。到期由每小时的 `recycle::sweep` 清除，
  清除前会先落一份 `pre-journal-purge` 快照。
- 上限：20,000 条，超过按 `deleted_at_ms` 从旧到新淘汰。

恢复用 `GET /api/deletions` + `POST /api/deletions/{id}/restore`（或 `restore-all`）。同 id 记录已存在时
返回 409 `already_exists`，绝不覆盖。台账条目本身不记录分类/单行本归属、书签与 stamp——那些是独立的键，
`purge_one_archive` 删除时一并清掉，需要它们时请用整库快照回退。

台账写入失败不会中止删除，但删除结果与日志里会标 `journal_failed`/「本次删除无法撤销」。

## 自动快照：`<library>/.lanrurugi-backups/`

- 文件名 `<UTC yyyymmddThhmmssZ>-<reason>.json`，内容与 `GET /api/database/backup` 完全一致，可直接用
  现有的恢复入口导入。
- 生成时机：每天首次启动 / 每小时检查一次（当天没有 `daily` 就补一份，每个 ISO 周补一份 `weekly`）；
  每个破坏性操作前（`pre-clean` / `pre-restore` / `pre-reindex` / `pre-journal-purge`）；以及
  `POST /api/backups` 手动触发。
- 写入是 `.json.tmp` + `rename`，崩溃最多留下被忽略的 `.tmp`。
- 保留：`snapshot_daily_keep`（默认 7，按本地日期去重）+ `snapshot_week_keep`（默认 4，按 ISO 周分组）
  + 最近 5 份 `pre-*`；**最新一份可用快照永不删除**（`DELETE /api/backups/{name}` 对它返回 409）。
- 快照失败只写 warning，并在操作的返回/任务结果里带 `snapshot: {ok:false, error}`，不阻塞操作；同一天
  不会重试 `daily`（进程内标记）。
- 备份目录与回收站一样位于库内点目录，扫描与 watcher 都会忽略（`watcher::is_watched_archive_path`）。

## 批量删除守卫

`POST /api/database/clean` 在真正删除那一遍（`file` 为空的记录）之前先算计划删除量：
`planned > deletion_guard_min`（默认 50）**且** `planned*100 >= total*deletion_guard_percent`（默认 25%）
时返回 409，并在响应里给出 `planned / total / percent / guard_percent / guard_min`，**不删除任何记录**。
确认后带 `?force=1` 重试即可执行。`file` 清空那一遍（可逆的软删除）不受守卫限制。

## 库目录诊断

扫描与清理都会输出 `scanned_files / records_total / overlap_percent / library_source /
library_source_known`：

- `library_source` 在 Linux 上取 `/proc/self/mountinfo` 中与库路径最长前缀匹配的条目的 root 字段；
  root 为 `/` 时记 `root-filesystem` 且 `known=false`；非 Linux 或读不到时记 `unknown`。
- `overlap_percent` 是"记录里 file 仍存在的比例"。

这些字段**只用于诊断**，任何情况下都不会据此拒绝操作。

## 设置项

| 键 | 默认 | 说明 |
|---|---|---|
| `deletion_journal_days` | 30 | 台账保留天数，0 = 关闭 |
| `deletion_guard_percent` | 25 | 守卫的占比阈值（%） |
| `deletion_guard_min` | 50 | 守卫的绝对量下限（条） |
| `snapshot_daily_keep` | 7 | 每日快照保留份数 |
| `snapshot_week_keep` | 4 | 每周快照保留份数 |
