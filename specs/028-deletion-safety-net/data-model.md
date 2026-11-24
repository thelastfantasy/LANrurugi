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

## 2. 备份目录里的两类文件

`<library>/.lanrurugi-backups/` 里有**两种**东西，接口用 `kind` 区分：

| `kind` | 文件名 | 内容 |
|---|---|---|
| `backup`（快照） | `snapshot-<UTC 时间戳>-<reason>.rdb` | Redis 自己的 RDB 字节拷贝，即 `BGSAVE` 之后的**全量**数据：逻辑库 0（档案）、2（配置/文件映射）、3（搜索索引） |
| `export`（导出） | `export-<UTC 时间戳>-<reason>.json` | 既有的备份文档（`lanrurugi_backup::build` 的产物，格式不变）：档案/标签/分类/单行本/stamp/书签等**可选择性数据**；旁边另有一份 `<同文件名>.summary.json` 摘要 |

`reason`：快照为 `scheduled`（cron 命中）/ `manual`；导出为 `manual` / `pre-clean` / `pre-restore` / `pre-reindex` / `pre-journal-purge`。旧版本写的 `daily` / `weekly`（两类都有）仍可解析、仍会列出。
历史文件（旧版只写 `<时间戳>-<reason>.json`，没有 `export-` 前缀）仍然可解析、仍然列出，按 `kind=export` 处理——升级不会让旧备份从列表里消失。

恢复方式不同，这一点必须写清楚：**导出能在 app 内恢复**（`POST /api/backups/{name}/restore`），**快照不能**——恢复快照 = 停 Redis、用该 `.rdb` 替换 Redis 的 RDB、再启动（`docs/dev/deletion-safety-net.md` 有步骤），app 内只提供下载。

命名与派生字段（不写进 JSON，从文件名与 mtime 派生）：

| 字段 | 来源 |
|---|---|
| `name` | 文件名（下载响应的 `Content-Disposition` 用它） |
| `kind` | 文件名前缀：`snapshot-` → `backup`，`export-` / 无前缀 → `export`（**不只看扩展名**，历史文件没有前缀） |
| `created_at` | 文件名里的时间戳（UTC） |
| `reason` | 文件名后缀：`scheduled` / `manual`（快照，兼容旧的 `daily` / `weekly`）；`manual` / `pre-*`（导出，同样兼容旧的 `daily` / `weekly`） |
| `size_bytes` | 文件大小 |
| `usable` | 轻量完整性校验：导出看「可读 + 像一份备份文档」，快照看开头 5 字节是否为 `REDIS` 魔数 |
| `summary` | 兄弟文件 `<name>.summary.json` 的解析结果；**不存在或解析失败为 `null`**，绝不为此打开文件本身；**快照一律为 `null`** |
| `pinned` | 名字是否在 `LANRURUGI_SNAPSHOT_PINS`（见 §2.2） |

摘要文件（`SnapshotSummary`，与快照同时写入、同为 `.tmp` + `rename`）：

```json
{"archives": 384, "archives_with_tags": 360, "categories": 12,
 "tankoubons": 5, "stamps": 3, "bookmarks": 8, "bytes": 412345}
```

`archives_with_tags` 只计「标签串去掉空白后非空」的档案；`bytes` 等于这份**导出**文件自身大小（与 entry 的 `size_bytes` 同值）。快照（RDB）没有摘要。

不变量：

- 写入必须原子：导出先写 `*.json.tmp` 再 `rename`，不允许出现半截 `.json`；摘要文件同理（`*.summary.json.tmp`）。快照（RDB）先写 `*.rdb.tmp`，**rename 之前校验开头是 `REDIS` 魔数**，半截拷贝不会变成一份看起来能恢复的备份。
- 计划快照由 **cron 命中**决定：`snapshot_cron`（6–7 段，按 `timezone` 设置求值，默认每周一 03:00:00）命中且上一次命中已被服务时生成一份 `scheduled` 快照（见 §2.1）。计划只生成**快照（RDB）**；`pre-*` 只生成**导出**（一次破坏性操作的关键路径上不能写几百 MB 的 RDB）。
- 旧的 `daily` / `weekly` 两类文件（RDB 或 JSON）仍然能解析、仍然按原来的规则保留；新版本不再写这两个 reason（快照写 `scheduled`，导出写 `manual` 或 `pre-*`）。
- 导出类的 `weekly` 按 ISO 周分组，每组只保留最新一份，最多 `snapshot_week_keep` 组。
- 快照类不分 `reason`，只按时间保留最近 **3** 份（`FULL_BACKUP_KEEP`，刻意不做成设置项：一个没人要求的旋钮，却可能让一份全量备份都不剩）。
- 任何清理都不得删除**最新一份可用文件**；删除时连同其 `.summary.json`（导出才有）与 pin 一起删除；**被 pin 的文件永不淘汰，也不占该档份数**（见 §2.2）。

## 2.1 计划状态（Redis）

Redis hash `LANRURUGI_SNAPSHOT_SCHEDULE`（**config** 逻辑库，与设置项同库）：

| field | 值 |
|---|---|
| `last_cron_fire` | 最近一次计划快照**生成完成**的瞬时，RFC 3339 UTC 字符串（如 `2026-11-23T03:00:04Z`） |

不变量 / 语义：

- 调度每 **5 秒** tick 一次，判定的是 `snapshot_cron` 这个 **6–7 段 crontab**（`秒 分 时 日 月 星期 [年]`、按 `timezone` 设置求值）有没有命中。
- 命中判定与 `last_cron_fire` 比较：候选时刻不晚于记录 → 不重复生成；记录之后的第一个命中 → 生成一次。因此重启不会重复生成（记录已写），停机错过的命中在下一次 tick 里**只补一次**。
- 记录写的是**这次生成完成的时间**（不是被补的那个槽位）。写槽位时间的话，停机期间错过的每一个命中都会排在被补的那一个之后，于是每个 tick 都再补一份；写完成时间则把「补一次」真正封顶。
- 首次安装（没有记录）时，第一个 tick 立刻生成一份并写下记录，之后按 cron 走；这样 5 秒 tick 与 `*/10` 这类表达式不会因相位错开而永远不触发。
- `snapshot_schedule_enabled` 关闭时不写、不读记录，也不生成任何计划快照；手动 `POST /api/backups` / `POST /api/backups/export` 与所有 `pre-*` 不受影响。
- 时区按 `timezone` 设置（IANA 名，非法/为空回退 UTC），与进程 `TZ` 无关；文件命名的时间戳仍是 UTC。DST 见 contracts（不存在的时刻跳过，重复的时刻取较早的，偏移随季节变化）。
- 生成失败只 `warn!`，并把该命中记在进程内标记里，**同一命中不再重试**（下个命中时间戳不同，自动重新启用）。

## 2.2 pin（Redis）

Redis set `LANRURUGI_SNAPSHOT_PINS`（**config** 逻辑库），成员 = 备份目录里的文件名（`snapshot-*.rdb` 或 `export-*.json`）。

不变量 / 语义：

- 被 pin 的文件**永不**被保留策略淘汰，**且不占用**它那一档的份数（pin 5 份 `pre-*` 之后，仍会另留 5 份未 pin 的）。
- pin 只是「豁免淘汰」，不改变「最新一份可用文件永不删」这条既有保护，也不改变手动 `DELETE` 的 `409 last_usable` 行为。
- 删除文件时一并 `SREM` 掉它的 pin，所以 set 不会指向不存在的文件；pin 一个不存在的名字返回 `404`，不会静默成功。
- 读不到 pin（Redis 不可用）时**跳过这一轮保留清理**并 `warn!`：宁可晚清理一轮，也不能把用户要求永久保留的文件删掉。列表接口则按「没有 pin」展示（它不删任何东西）。

## 2.3 备份任务（进程内，不落盘）

`POST /api/backups`（RDB）与 `POST /api/backups/export`（JSON）立刻返回 `job_id`，实际工作在后台任务里跑。
任务状态**不写 Redis、不写库**——它只是进度书签，不是数据：

| 字段 | 含义 |
|---|---|
| `state` | `running` / `done` / `failed` |
| `kind` | `backup` / `export` / `restore` / `import` |
| `started_at` / `finished_at` | RFC 3339 UTC |
| `percent` | 0–100，**单调不减**，终态正好 100（`failed` 停在失败时的值） |
| `bytes_written` / `bytes_expected` | 已写 / 预期字节；RDB 阶段的分母来自 `INFO memory`（或上一次 RDB ×1.05），导出阶段是随记录数收敛的估计，结束时精确 |
| `error` | `failed` 时的原因 |
| `entry` | `done` 时的条目（与 §2 的列表项同形；`restore` / `import` 为 `null`） |
| `result` | `done` 时的汇总（`restore` / `import` 用；备份/导出为 `null`） |

不变量：

- 完成（或失败）的任务保留 1 小时、最多 32 个（进程内注册表，`snapshots/jobs.rs`）；进程重启即全部丢失，客户端按 `404` 处理。
- 进度事件一律带**绝对**数值，客户端丢事件不会算错；SSE 连上时第一个事件就是当前状态。
- 任务的产出与同步路径完全一致：同一个 `take_full_backup` / `take_export` / `restore_with_progress` /
  `import_from_legacy_with_progress`，只是多了一个进度接收器。
- 记录口径的操作（restore / import）把 `bytes_written` / `bytes_expected` 当作「已处理 / 总数」用（单位是记录），
  `percent` 仍是真实百分比；档案写入占 0–90%，收尾（分类/印章/翻译状态/索引重建）占最后 10%，终态正好 100。

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
| `snapshot_daily_keep` | number | 7 | `daily` **导出**的保留份数 |
| `snapshot_week_keep` | number | 4 | `weekly` **导出**的保留份数 |
| `snapshot_schedule_enabled` | bool | `true` | 自动快照计划总开关；关闭后不再按计划生成（手动端点与 `pre-*` 不受影响） |
| `snapshot_cron` | string | `"0 0 3 * * 1"` | 6–7 段 crontab（`秒 分 时 日 月 星期 [年]`），按 `timezone` 设置的墙钟求值；默认每周一 03:00:00。非法表达式一律 400，错误信息指出是哪一段 |
| `snapshot_cron_preset` | string | `"weekly"` | 仅供 UI 回显：`hourly` / `daily` / `weekly` / `monthly` / `custom`；服务端不据此算时间 |

取值范围：`deletion_journal_days` 0–3650；`deletion_guard_percent` 1–100；`deletion_guard_min` 1–1,000,000；`snapshot_*_keep` 1–365；`snapshot_cron` 的字段范围见 contracts（秒 0–59、分 0–59、时 0–23、日 1–31、月 1–12、星期 0–7、年 1970–2100）。

老实例（任何新字段都没写过）读取时全部取默认值：总开关开启、`cron = "0 0 3 * * 1"`、`preset = weekly`——即「不改设置也有**每周一 03:00 一份全量备份**」。
升级后不会追溯补发（没有 `last_cron_fire` 记录，只有正好命中时才生成）；计划生成需要本进程能读到 Redis 的 RDB（远程/托管 Redis 会显式报错，见 contracts）。
快照类份数固定 3，**不可通过设置调整**；导出类份数由 `snapshot_daily_keep` / `snapshot_week_keep` 决定，`pre-*` 与 `manual` 固定 5。任何一份都可以 pin（§2.2）来绕过上限。
旧版写下的 `JSON` 快照没有 `<name>.summary.json` 的，在列表里的 `summary` 就是 `null`，其它字段不变；它们没有 `export-` 前缀，按 `kind=export` 列出。


## 5. 关系

- 一条删除台账条目 ↔ 一条曾经的 Archive 记录（1:1，按 id）。
- 一份**导出**包含全部档案记录 / 分类 / 单行本 / stamps / 书签 / 翻译状态（既有备份文档），与台账**不重叠**：台账是记录级、导出是库级。
- 一份**快照**是 Redis 的全量 RDB，包含台账、导出之外的一切（配置、文件映射、搜索索引），是唯一能整机还原的产物。
- 三者都不改变任何既有实体（Archive / Category / Tankoubon / Stamp / Bookmark）的形状。
