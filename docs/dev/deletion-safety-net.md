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

## 备份与导出：`<library>/.lanrurugi-backups/`

目录里有**两种**东西，`GET /api/backups` 的每条 entry 用 `kind` 区分。**恢复方式完全不同**，这是排查时最容易
搞混的一点：

| `kind` | 文件名 | 内容 | 恢复 |
|---|---|---|---|
| `backup`（快照） | `snapshot-<UTC yyyymmddThhmmssZ>-<reason>.rdb` | **全量**：Redis 自己的 RDB 字节拷贝（逻辑库 0 档案 / 2 配置与文件映射 / 3 搜索索引） | **只能运维**：停 Redis → 用该文件替换 RDB → 启动 Redis。app 内没有一键还原 |
| `export`（导出） | `export-<UTC yyyymmddThhmmssZ>-<reason>.json` | **可选择性数据**：档案/标签/分类/单行本/stamp/书签/翻译状态，与 `GET /api/database/backup` 同一份文档 | app 内 `POST /api/backups/{name}/restore`（或上传到 `/api/database/restore`） |

`<reason>`：快照 = `scheduled-<计划 id>` / `manual`；导出另加 `pre-clean` / `pre-restore` / `pre-reindex` /
`pre-journal-purge`。旧版只写过 `<时间戳>-<reason>.json`（没有 `export-` 前缀），这些文件仍然会列出，按
`kind=export` 处理；更早的 `<时间戳>-scheduled.rdb`（没有计划 id）也照旧列出，归到「无归属」组。

### 什么时候生成什么

- **计划命中时生成快照（RDB）**：设置项 `snapshot_schedules` 是一个 **JSON 数组**，每条计划
  `{id, label, enabled, preset, cron}`，`cron` 是 6–7 段 crontab（`秒 分 时 日 月 星期 [年]`，按应用设置
  `timezone` 求值——**不是**进程 `TZ`）。出厂带一条启用的「每周日 03:00」(`0 0 3 * * 0 *`)——一份全量快照
  不是免费的，生产规模的库默认每周一次。最多 10 条；每条各自有水位线、失败槽记忆与保留名额（各 3 份），
  文件名带自己的 id，所以删掉一条计划不会带走另一条的历史，删掉的计划留下的文件归「无归属」组。
- **`pre-*` 生成导出（JSON）**：每个破坏性操作前一份（`pre-clean` / `pre-restore` / `pre-reindex` /
  `pre-journal-purge`）。这些在操作的关键路径上同步执行，绝不能在那里写几百 MB 的 RDB。
- **手动**：`POST /api/backups` = 立刻生成快照（RDB，`reason=manual`）；`POST /api/backups/export` = 立刻
  生成导出（JSON，`reason=manual`）。

计划调度是一个 **5 秒 tick** 的后台任务（秒级表达式需要它）：用 cron 表达式判定「自上次生成以来有没有命中」，
Redis hash `LANRURUGI_SNAPSHOT_SCHEDULE`（config 库）里的 `last_cron_fire:<计划 id>`（RFC 3339 UTC）记录**每一条计划上一次生成完成的时间**；
读不到这个字段（Redis 故障）与「从没跑过」是两件事——前者这一 tick 直接跳过，不会按猜测生成（否则 Redis 一坏就每 5 秒刷一条告警）。
所以重启不会重复生成；停机错过的命中在启动后只补**一份**（记录写完成时间，而不是被补的槽位，否则错过的每个槽位都会再触发一次）；
改设置后下一个 tick 即生效（无需重启）。首次安装没有 `last_cron_fire` 记录时，**第一个 tick 立刻生成一份**（等价于旧版的启动快照），之后按 cron 走——
否则 5 秒的 tick 与 `*/10` 这类表达式可能永远错开相位，一份也不生成。
时间按应用设置 `timezone`（IANA 名，非法/为空回退 UTC）算墙钟；`schedule.timezone` 回传 `IANA 名 (+HH:MM)`，
`next_runs`/`last_fire_at` 带该时区偏移；文件命名的时间戳始终是 UTC。DST 不存在的时刻跳过，重复的时刻取较早的一次，
偏移随季节变化（`America/New_York` 冬 `-05:00` / 夏 `-04:00`）。

crontab 的语义（与标准 cron 一致）：数字星期 `0` 和 `7` 都是周日、`1` 是周一；名字接受 `JAN`…`DEC` / `SUN`…`SAT`；
支持 `*`、`a`、`a-b`、`*/n`、`a/n`、`a-b/n` 与逗号列表；**日与星期两个字段都受限时取「或」**，任一为 `*` 时取「且」。
非法表达式在 `PUT /settings` 是 400，错误信息指出是哪一段（`seconds (segment 1): "61" is outside 0-59`）；
保存前可用 `POST /api/backups/schedule/preview` 预览接下来 10 个命中时刻。

### 生成是异步任务，进度走 SSE

**所有长耗时数据操作**都在这一套任务通道里（同一注册表、同一对端点）：

| 操作 | 起点 | `kind` |
|---|---|---|
| 全量快照 | `POST /api/backups` | `backup` |
| 导出 | `POST /api/backups/export` | `export` |
| 导出恢复 | `POST /api/backups/{name}/restore` | `restore` |
| 上传 JSON 恢复 | `POST /api/database/restore` | `restore` |
| LANraragi 导入 | `POST /api/database/import-legacy` | `import` |

后三者按「已处理记录数 / 总数」算进度（`bytes_written`/`bytes_expected` 就是记录数），档案写入占 0–90%，
其后的分类/印章/翻译状态/索引重建占最后 10%。`done` 事件的 `result` 带汇总（`restored`/`skipped`/导入计数/`rebuild`）；
备份与导出则把结果放在 `entry` 里，`result` 为 `null`。
导入回滚快照在 `import-legacy` 任务内部创建（没有独立端点），应用它就是 `POST /api/database/restore`。

进度查询有两条路：

- 主路径 `GET /api/backups/jobs/{job_id}/events`：SSE，`text/event-stream`，事件 `progress` / `done` / `error`。
  复用 `tankoubon_grouping/run.rs` / `archive_split.rs` 已有的那套连接管理（axum `Sse` + `broadcast` 通道 +
  `KeepAlive`），只新增了「绝对进度」载荷。每个事件自带 `percent / bytes_written / bytes_expected / elapsed_ms`；
  连上时第一个事件就是当前状态的绝对快照，晚订阅或断线重连都不会丢状态。
- 兜底 `GET /api/backups/jobs/{job_id}`：同一份状态，适合不能挂 `EventSource` 的客户端。

百分比来源：

- 快照：`BGSAVE` 阶段按 Redis 的 `temp-*.rdb` 当前大小 ÷ 分母（`INFO memory` 的 `used_memory`，取不到则上一次 RDB 大小
  ×1.05）算到 90%，拷贝阶段（手写分块拷贝，不用 `fs::copy`）按字节把剩下 10% 走完，结束正好 100。
- 导出：按「已序列化记录数 / 总记录数」算；文档是逐条记录写出的（`snapshots/progress.rs::write_document`，
  输出与 `serde_json::to_writer` 字节一致），所以 `bytes_expected` 在过程中是随进度收敛的估计、结束时精确。
- 百分比保证**单调不减**、终态 100；失败停在失败时的值并带 `error`，不会假装完成。
- 任务只在进程内存里（`snapshots/jobs.rs` 的进程级注册表），完成后保留 1 小时、最多 32 个；进程重启即丢失。
- 空闲时每 12 s 一行 `: keep-alive`；响应带 `Cache-Control: no-cache` 与 `X-Accel-Buffering: no`，避免 nginx 缓冲。

### 快照（RDB）是怎么取的

`POST /api/backups` 与计划任务走同一条路径：

1. `CONFIG GET dir` + `CONFIG GET dbfilename` 问出 Redis 的 RDB 路径；
2. 读一次 `LASTSAVE`，发 `BGSAVE`，然后轮询 `LASTSAVE` 直到它前进（最多 120 秒）；
3. 把 RDB 拷成 `<name>.tmp`，校验开头 5 字节是 `REDIS` 魔数，`fsync` 后 `rename` 成 `.rdb`。

**远程 / 托管 Redis 不能这样备份**：`CONFIG` 被禁用、或者它报告的路径本进程读不到时，端点返回 `500` 并在
`error` 里写明原因，计划任务则只 `warn!`（该槽位记进进程内标记，不重试）。不会产生空文件，也不会写半截文件。

### 运维还原一份快照

1. `GET /api/backups/<name>` 下载 `.rdb`；
2. 停掉 Redis；
3. 用它替换 Redis 的 RDB（路径见 `redis-cli CONFIG GET dir` / `CONFIG GET dbfilename`，通常 `dump.rdb`；
   若开了 AOF，注意 AOF 会在启动时优先恢复——必要时同时处理 `appendonlydir`）；
4. 启动 Redis，再启动本服务。

app 内的 `POST /api/backups/{name}/restore` 对 `.rdb` 返回 `409`（`reason=full_backup_not_restorable`），
只接受导出。

### pin（永久保留）

- Redis set `LANRURUGI_SNAPSHOT_PINS`（config 库），成员 = 文件名；`POST|DELETE /api/backups/{name}/pin` 幂等。
- 被 pin 的文件**永不**被保留策略淘汰，**且不占**它那一档的份数。
- 删除文件时连带 `SREM` 掉它的 pin；pin 一个不存在的文件返回 404。
- 读不到 pin（Redis 不可用）时**跳过这一轮清理**并 `warn!`——宁可晚清理，也不能删掉要求永留的文件。

### 保留

| 类别 | 份数 | 说明 |
|---|---|---|
| 导出 `daily` | `snapshot_daily_keep`（默认 7） | 按本地日期去重，同一天只留最新一份 |
| 导出 `weekly` | `snapshot_week_keep`（默认 4） | 按 ISO 周分组 |
| 导出 `pre-*` | 固定 5 | — |
| 导出 `manual` | 固定 5 | — |
| 快照（RDB） | 固定 **3** | 不分 reason，按时间取最新 3 份；不做成设置项 |

- 既然计划只写 `scheduled` 快照，`snapshot_daily_keep` / `snapshot_week_keep` 现在只作用于**旧版本留下的**
  `daily` / `weekly` 导出文件（新版本只写 `manual` 与 `pre-*` 导出）。旧文件照旧按本地日期 / ISO 周分组淘汰。
- **保留只在写入新备份时执行**：`take_full_backup` / `take_export` 写完立刻跑一次 `retention_sweep`，
  每小时的维护（`recycle::sweep`）只做到期台账清理，不再顺手删快照。这样把 `snapshot_full_keep` 从 3 改小
  不会当场删文件——多余的那几份等到下一次手动或自动备份时才被淘汰（历史上 `snapshots_removed` 字段因
  此恒为 0，保留它是为了不改变响应形状）。
- 任何一档的上限都可以被 pin 打破（pin 的不淘汰、不占名额）。
- **最新一份可用文件永不自动删除**（`DELETE /api/backups/{name}` 对它返回 409 `last_usable`）。
- 写入是 `.tmp` + `rename`（导出同 `.summary.json.tmp`）；崩溃最多留下被忽略的 `.tmp`。
- 导出旁边有同名 `<文件名>.summary.json`（`archives / archives_with_tags / categories / tankoubons /
  stamps / bookmarks / bytes`），列表的 `summary` 用它；缺失或损坏为 `null`，绝不为此解析文件本身。
  **快照的 `summary` 恒为 `null`。**
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
| `snapshot_daily_keep` | 7 | `daily` **导出**的保留份数 |
| `snapshot_week_keep` | 4 | `weekly` **导出**的保留份数 |
| `snapshot_schedules` | 一条启用的 `{"id":"plan-1","cron":"0 0 3 * * 0 *","preset":"weekly"}` | 计划列表（JSON 数组，≤10 条）；每条 `{id,label,enabled,preset,cron}`，`cron` 为 6–7 段 crontab，按 `timezone` 求值 |

快照（RDB）的保留份数是**每条计划各 3 份**，不是设置项；要长期留着某一份请用 pin（见上文）。旧版的
`snapshot_cron` / `snapshot_schedule_enabled` / `snapshot_cron_preset` 三个键仍会被读（升级上来的实例用它
们造出第一条计划），但已不接受写入（`PUT /settings` 对它们返回 Unknown settings field）。

备注不是设置项：`PATCH /api/backups/{name}` `{"note":"…"}` 写进 Redis hash `LANRURUGI_BACKUP_NOTES`（200 字符上限、
控制字符拒绝），只给人看，不影响保留、pin 与 `usable`；文件被删除或被保留策略清掉时备注一起删。
