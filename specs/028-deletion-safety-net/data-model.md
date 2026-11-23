# Phase 1 Data Model: 删除安全网

## 1. 删除台账条目（Deletion journal entry）

存于 Redis hash `LANRURUGI_DELETION_JOURNAL`（**archive** 逻辑库，与档案记录同库），field = `ArchiveId`，value = 紧凑 JSON：

```json
{
  "record": { /* Archive 的完整序列化，字段与原记录一致 */ },
  "deleted_at_ms": 1795412345678,
  "source": "clean | single | superseded | split | other",
  "path": "/home/koyomi/lanrurugi/content/a1/a105….zip",
  "journal_version": 1
}
```

不变量：

- `record.id` 必须与 field 一致；不一致的条目视为损坏，读取时跳过并计数。
- 同一 id 只保留**最近一次**删除（后删覆盖前删）——恢复语义按「最后状态」。
- 条目数上限 20,000；超限时按 `deleted_at_ms` 升序淘汰。
- 保留期 `deletion_journal_days`（默认 30），`now - deleted_at_ms > 保留期` 即到期。

状态：`journaled`（可恢复）→ `restored`（已恢复并移除）或 `purged`（到期/人工清除）。没有「部分恢复」状态。

## 2. 快照（Snapshot）

文件 `<library>/.lanrurugi-backups/<UTC 时间戳>-<reason>.json`，内容是既有的备份文档（`lanrurugi_backup::build` 的产物，格式不变）；同一目录下另有一份 `<同文件名>.summary.json` 内容摘要。

命名与派生字段（不写进 JSON，从文件名与 mtime 派生）：

| 字段 | 来源 |
|---|---|
| `name` | 文件名 |
| `created_at` | 文件名前缀（UTC） |
| `reason` | 文件名后缀：`daily` / `weekly` / `pre-clean` / `pre-restore` / `pre-reindex` / `pre-journal-purge` / `manual` |
| `size_bytes` | 文件大小 |
| `usable` | 能被解析为合法备份文档（进度列表时轻量校验：可读 + 顶层含 `version`） |
| `summary` | 兄弟文件 `<name>.summary.json` 的解析结果；**不存在或解析失败为 `null`**，绝不为此打开快照本身 |

摘要文件（`SnapshotSummary`，与快照同时写入、同为 `.tmp` + `rename`）：

```json
{"archives": 384, "archives_with_tags": 360, "categories": 12,
 "tankoubons": 5, "stamps": 3, "bookmarks": 8, "bytes": 412345}
```

`archives_with_tags` 只计「标签串去掉空白后非空」的档案；`bytes` 等于快照文件自身大小（与 entry 的 `size_bytes` 同值）。

不变量：

- 写入必须原子：先写 `*.json.tmp` 再 `rename`；不允许出现半截 `.json`。摘要文件同理（`*.summary.json.tmp`）。
- `daily` / `weekly` 由**计划槽位**决定：本地时间每天 `snapshot_daily_at` 生成一份 `daily`，每周 `snapshot_weekly_day` 那天的同一时刻生成一份 `weekly`（见 §2.1）；同一槽位最多一份。
- `weekly` 按 ISO 周分组，每组只保留最新一份，最多 `snapshot_week_keep` 组。
- 任何清理都不得删除**最新一份可用快照**；删除快照时连同其 `.summary.json` 一起删除。

## 2.1 计划状态（Redis）

Redis hash `LANRURUGI_SNAPSHOT_SCHEDULE`（**config** 逻辑库，与设置项同库）：

| field | 值 |
|---|---|
| `last_daily_at` | 最近一次真正生成 `daily` 的瞬时，RFC 3339 UTC 字符串（如 `2026-11-23T03:00:00Z`） |
| `last_weekly_at` | 同上，`weekly` |

不变量 / 语义：

- 调度每**一分钟** tick 一次，比较「最近一个已到点的计划槽位」与 `last_*`：前者更新则生成一次。因此重启不会重复生成（槽位已记录），停机错过的槽位在下次启动后**只补一次**（不管错过几天）。
- `schedule` 关闭时不写、不读槽位，也不生成任何计划快照；手动 `POST /api/backups` 与所有 `pre-*` 不受影响。
- 槽位计算用**服务器本地时区**（`chrono::Local`，与保留策略按本地日期去重同一套）；DST 见 contracts 的说明。
- 生成失败只 `warn!`，并把该槽位记在进程内标记里，**同一槽位不再重试**（下一槽位时间戳不同，自动重新启用）。

## 3. 库目录诊断（Library diagnostics）

不落盘，只出现在扫描/清理的日志与 job 结果里：

```json
{
  "scanned_files": 1246,
  "records_total": 384,
  "overlap_percent": 12,
  "library_source": "/home/jade/LANrurugi/data/manga",
  "library_source_known": true
}
```

`library_source` 的取值规则（Linux）：`/proc/self/mountinfo` 里与库路径**最长前缀匹配**的挂载条目，取其 root 字段；root 为 `/` 表示「就是根文件系统的一部分」，此时 `library_source_known=false` 且值记 `"root-filesystem"`。非 Linux 或读不到文件时 `library_source_known=false`、值 `"unknown"`。

计算成本：`overlap_percent` 需要逐条 `stat`（与 clean_database 现有循环同源），10 万条约几十毫秒到数百毫秒，放在既有扫描的收尾阶段，不额外线程。

## 4. 设置项（新增，`settings.rs`）

| 键 | 类型 | 默认 | 含义 |
|---|---|---|---|
| `deletion_journal_days` | number | 30 | 台账保留天数；0 表示不保留（关掉台账） |
| `deletion_guard_percent` | number | 25 | 单次删除占比阈值（%） |
| `deletion_guard_min` | number | 50 | 触发守卫的绝对量下限（条） |
| `snapshot_daily_keep` | number | 7 | 每日快照保留份数 |
| `snapshot_week_keep` | number | 4 | 每周快照保留份数 |
| `snapshot_schedule_enabled` | bool | `true` | 自动快照计划总开关；关闭后不再按计划生成（手动与 `pre-*` 不受影响） |
| `snapshot_daily_at` | string `"HH:MM"` | `"03:00"` | 每天生成 `daily` 的本地时间（也决定 `weekly` 的时刻） |
| `snapshot_weekly_day` | number | 1 | 生成 `weekly` 的星期；**0 = 周日**，与前端 `Date.getDay()` 一致 |

取值范围：`deletion_journal_days` 0–3650；`deletion_guard_percent` 1–100；`deletion_guard_min` 1–1,000,000；`snapshot_*_keep` 1–365；`snapshot_daily_at` 必须是 `HH:MM` 且落在 00:00–23:59（其它写法一律 400）；`snapshot_weekly_day` 0–6。

老实例（任何新字段都没写过）读取时全部取默认值：计划开启、每天 03:00、周一——即「不改设置也有每日备份」。
升级后第一次 tick 因为 `last_daily_at` 还是空，会立刻补一份 `daily`（与旧版「启动时生成」同效果）；此后按槽位走。
旧版写下的快照没有 `<name>.summary.json`，它们在列表里的 `summary` 就是 `null`，其它字段不变。


## 5. 关系

- 一条删除台账条目 ↔ 一条曾经的 Archive 记录（1:1，按 id）。
- 一份快照包含全部档案记录 / 分类 / 单行本 / stamps / 书签 / 翻译状态（既有备份文档），与台账**不重叠**：台账是记录级、快照是库级。
- 台账与快照都不改变任何既有实体（Archive / Category / Tankoubon / Stamp / Bookmark）的形状。
