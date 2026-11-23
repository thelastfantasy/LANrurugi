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

文件 `<library>/.lanrurugi-backups/<UTC 时间戳>-<reason>.json`，内容是既有的备份文档（`lanrurugi_backup::build` 的产物，格式不变）。

命名与派生字段（不写进 JSON，从文件名与 mtime 派生）：

| 字段 | 来源 |
|---|---|
| `name` | 文件名 |
| `created_at` | 文件名前缀（UTC） |
| `reason` | 文件名后缀：`daily` / `weekly` / `pre-clean` / `pre-restore` / `pre-reindex` / `pre-journal-purge` / `manual` |
| `size_bytes` | 文件大小 |
| `usable` | 能被解析为合法备份文档（进度列表时轻量校验：可读 + 顶层含 `version`） |

不变量：

- 写入必须原子：先写 `*.json.tmp` 再 `rename`；不允许出现半截 `.json`。
- `daily` 每天最多一份（以本地日期判断「今天」）。
- `weekly` 按 ISO 周分组，每组只保留最新一份，最多 `snapshot_week_keep` 组。
- 任何清理都不得删除**最新一份可用快照**。

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

取值范围：`deletion_journal_days` 0–3650；`deletion_guard_percent` 1–100；`deletion_guard_min` 1–1,000,000；`snapshot_*_keep` 1–365。

## 5. 关系

- 一条删除台账条目 ↔ 一条曾经的 Archive 记录（1:1，按 id）。
- 一份快照包含全部档案记录 / 分类 / 单行本 / stamps / 书签 / 翻译状态（既有备份文档），与台账**不重叠**：台账是记录级、快照是库级。
- 台账与快照都不改变任何既有实体（Archive / Category / Tankoubon / Stamp / Bookmark）的形状。
