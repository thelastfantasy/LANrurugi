# API Contract: 备份与导出（`/api/backups`）

同一套响应约定与权限（会话管理员）。文件都在 `<library>/.lanrurugi-backups/`，本组端点只暴露文件名，从不接受调用方传入的路径。

目录里有两类文件，接口用 `kind` 区分：

| `kind` | 文件名 | 内容 | 恢复方式 |
|---|---|---|---|
| `backup`（快照） | `snapshot-YYYYMMDDTHHMMSSZ-<reason>.rdb` | Redis **全量** RDB：逻辑库 0（档案）、2（配置/文件映射）、3（搜索索引）全部数据 | **只能由运维执行**：停 Redis → 用该文件替换 RDB → 启动 Redis。**app 内没有一键还原** |
| `export`（导出） | `export-YYYYMMDDTHHMMSSZ-<reason>.json` | **可选择性数据**：档案、标签、分类、单行本、stamp、书签、翻译状态等（与 `GET /api/database/backup` 同一份文档） | app 内 `POST /api/backups/{name}/restore` |

`<reason>`：快照为 `scheduled`（计划 cron 命中）/ `manual`；导出为 `manual` / `pre-clean` / `pre-restore` / `pre-reindex` / `pre-journal-purge`。旧版本写下的 `daily` / `weekly`（两类都有）仍能识别、仍会列出、仍按原来的保留规则处理，只是新版本不再写这两个 reason。

历史文件（旧版只写 `<stamp>-<reason>.json`，没有前缀）仍然会列出来，按 `kind=export` 处理。

## GET `/api/backups`

```json
{
  "operation": "list_backups",
  "success": 1,
  "daily_keep": 7,
  "week_keep": 4,
  "schedule": {
    "enabled": true,
    "cron": "0 0 3 * * 1",
    "preset": "weekly",
    "timezone": "Asia/Tokyo (+09:00)",
    "last_fire_at": "2026-11-23T03:00:00+09:00",
    "next_runs": [
      "2026-11-30T03:00:00+09:00",
      "2026-12-07T03:00:00+09:00",
      "2026-12-14T03:00:00+09:00",
      "2026-12-21T03:00:00+09:00",
      "2026-12-28T03:00:00+09:00",
      "2027-01-04T03:00:00+09:00",
      "2027-01-11T03:00:00+09:00",
      "2027-01-18T03:00:00+09:00",
      "2027-01-25T03:00:00+09:00",
      "2027-02-01T03:00:00+09:00"
    ]
  },
  "entries": [
    {
      "name": "snapshot-20261123T030000Z-scheduled.rdb",
      "kind": "backup",
      "created_at": "2026-11-23T03:00:00Z",
      "reason": "scheduled",
      "size_bytes": 52428800,
      "usable": true,
      "summary": null,
      "pinned": false
    },
    {
      "name": "export-20261123T050000Z-manual.json",
      "kind": "export",
      "created_at": "2026-11-23T05:00:00Z",
      "reason": "manual",
      "size_bytes": 412345,
      "usable": true,
      "summary": {
        "archives": 384,
        "archives_with_tags": 360,
        "categories": 12,
        "tankoubons": 5,
        "stamps": 3,
        "bookmarks": 8,
        "bytes": 412345
      },
      "pinned": true
    }
  ]
}
```

- 两类在**同一个 `entries` 数组**里，按 `created_at` 从新到旧。
- `kind` 是客户端唯一必须分支的字段：只有 `export` 能在 app 内恢复。
- `usable=false` 表示文件存在但不像完整文件（导出：无法解析成备份文档；快照：没有 `REDIS` 魔数，即被截断）。前端应禁用「恢复」按钮。
- `summary` 只有导出有：它是兄弟文件 `<name>.summary.json` 的内容，缺失或解析失败时为 `null`（字段始终存在）。**快照一律为 `null`**（绝不为了列表去解析 RDB）。
- `pinned=true` 表示被 pin：不会被保留策略淘汰。
- `daily_keep` / `week_keep` 是**导出类**的保留份数设置；快照类固定只留最近 3 份。既然计划只写 `scheduled` 快照，这两个设置现在只作用于旧版本留下的 `daily` / `weekly` 导出文件（新版本只写 `manual` 与 `pre-*`）。

## POST `/api/backups`（开始生成快照 = 全量 RDB）

**异步**：立刻返回任务号，备份在自己的任务里跑（`BGSAVE` → 等 `LASTSAVE` 前进 → 拷 RDB）。

```json
{
  "operation": "create_backup",
  "success": 1,
  "job_id": "backup-3f2c…",
  "started_at": "2026-11-23T06:12:00Z",
  "events": "/api/backups/jobs/backup-3f2c…/events"
}
```

- 进度见下面的 `GET /api/backups/jobs/{job_id}/events`（SSE，主路径）与 `GET /api/backups/jobs/{job_id}`（轮询兜底）。
- 写入是 `<name>.tmp` + `rename`，并且 rename 之前校验文件开头是 `REDIS` 魔数，所以半截拷贝不会变成一份「看起来能恢复」的备份。
- **远程 / 托管 Redis 无法这样备份**：`CONFIG` 被禁用、或 Redis 报告的 RDB 路径本进程读不到时，任务以 `state=failed` 结束、`error` 明确写出原因（不会产生空文件）。

## POST `/api/backups/export`（开始生成导出）

同样的任务形状，`kind=export`：

```json
{ "operation": "create_export", "success": 1, "job_id": "backup-9a41…", "started_at": "…", "events": "/api/backups/jobs/backup-9a41…/events" }
```

## GET `/api/backups/jobs/{job_id}`（轮询兜底）

```json
{
  "state": "running",
  "kind": "backup",
  "started_at": "2026-11-23T06:12:00Z",
  "finished_at": null,
  "percent": 62.4,
  "bytes_written": 4300000,
  "bytes_expected": 6900000,
  "error": null,
  "entry": null
}
```

- `state`：`running` / `done` / `failed`。
- 结束后 `finished_at` 有值；`done` 带 `entry`（与列表里的条目同形），`failed` 带 `error`。
- `percent` **单调不减**，结束时正好 `100`；`failed` 停在失败时的值（不会假装 100）。
- 任务只保存在进程内存里：完成（或失败）后保留 1 小时、最多 32 个，不落库。

## GET `/api/backups/jobs/{job_id}/events`（SSE，主路径）

`Content-Type: text/event-stream`；响应头带 `Cache-Control: no-cache` 与 `X-Accel-Buffering: no`（防止代理缓冲把进度攒到最后才发）。
一条连接只跟一个任务；终态事件发出后服务端关闭流。

```
event: progress
data: {"kind":"progress","percent":62.4,"bytes_written":4300000,"bytes_expected":6900000,"elapsed_ms":8100}

event: done
data: {"kind":"done","percent":100.0,"bytes_written":6900000,"bytes_expected":6900000,"elapsed_ms":9200,"entry":{…}}

event: error
data: {"kind":"error","error":"could not ask Redis where its RDB lives (…)", "percent":12.3, "elapsed_ms":900}
```

- 连上时的**第一个事件**就是当前状态的绝对快照（`progress`，或已经是 `done` / `error`），所以晚订阅/断线重连都不会丢状态。
- **同一套任务通道也承载其它长耗时数据操作**（同一注册表、同样的 URL 形状）：

| 操作 | 起点端点 | `kind` | 进度口径 | `done` 的 `result` |
|---|---|---|---|---|
| 全量快照 | `POST /api/backups` | `backup` | RDB 字节（见下） | —（`entry` 里是快照条目）|
| 导出 | `POST /api/backups/export` | `export` | 已序列化记录数 / 总数 | —（`entry` 里是导出条目）|
| 导出恢复 | `POST /api/backups/{name}/restore` | `restore` | 已应用档案数 / 总档案数 | `restored` / `skipped` / … |
| JSON 恢复（上传） | `POST /api/database/restore` | `restore` | 已应用档案数 / 总档案数 | 同上 |
| LANraragi 导入 | `POST /api/database/import-legacy` | `import` | 已写入档案数 / 待写入数 | 导入/跳过/失败计数 + `rebuild` |

记录口径的操作把 `bytes_written` / `bytes_expected` 用作「已处理 / 总数」（单位是记录），`percent` 仍是真实百分比；
档案写入占 0–90%，其后的分类/印章/重建等收尾占最后 10%，终态事件正好 100。
导入回滚快照的创建发生在 `import-legacy` 任务内部，没有独立端点；「应用」一份回滚快照就是
`POST /api/database/restore`（同样是任务）。
- 每个事件都带**绝对**数值（不是增量），客户端落后或丢事件也不会算错。
- 采样间隔 ≤200 ms（RDB 保存阶段按 `temp-*.rdb` 的大小采样，拷贝阶段按已拷贝字节采样，导出阶段按已序列化记录数采样）。
- 空闲时服务端每 12 s 发一行 `: keep-alive` 注释，避免代理/浏览器把连接判超时。
- 百分比来源：
  - 快照：`BGSAVE` 阶段按 `temp-*.rdb` 当前大小 ÷ 分母（`INFO memory` 的 `used_memory`，取不到则用上一次 RDB 大小 ×1.05）算到 90%，拷贝阶段按已拷贝字节/总字节把剩下 10% 走完；
  - 导出：按「已序列化的记录数 / 总记录数」算，`bytes_expected` 在写的过程中是个随进度收敛的估计，结束时两者相等。

`404` 当任务号不存在（超过保留时间或进程重启后）。

## GET `/api/backups/{name}`

下载该文件的原始字节：

- `kind=export` → `Content-Type: application/json`；
- `kind=backup` → `Content-Type: application/octet-stream`；
- 两者都带 `Content-Disposition: attachment; filename="<name>"`，`<name>` 就是列表里的真实文件名。

**导出**额外支持 `?format=zip`：返回只含该 JSON 的 zip，`Content-Type: application/zip`，`Content-Disposition: attachment; filename="<name>.zip"`。
**快照（`.rdb`）始终按单文件下载**，即使带了 `?format=zip` —— 运维要替换的就是这个文件本身。

`400` 当 `name` 含路径分隔符或不是本模块写出的名称（防止目录穿越）；`404` 当文件不存在。

## DELETE `/api/backups/{name}`

```json
{ "operation": "delete_backup", "success": 1, "deleted": "export-20261123T050000Z-daily.json" }
```

- 最新一份**可用**文件（两类合在一起算）不可删除：`409`，`"reason": "last_usable"`。
- 删除时连同它的 `<name>.summary.json`（导出才有）和 pin 一起清掉。

## POST / DELETE `/api/backups/{name}/pin`

把某个文件标记为「永久保留」/取消。**幂等**：pin 已 pin 的、unpin 未 pin 的都返回成功。

```json
{ "operation": "pin_backup", "success": 1, "name": "snapshot-20261123T030000Z-scheduled.rdb", "pinned": true }
```

- 两类都能 pin；被 pin 的文件**永不被保留策略淘汰**，也**不占用**它那一档的份数。
- `400` 名称非法；`404` 文件不存在（pin 一个不存在的名字不会静默成功）。
- pin 不是删除/恢复一类的破坏性操作，不写活动日志。

## POST `/api/backups/{name}/restore`

只接受 `kind=export`：用该导出恢复整库（复用既有恢复流程，并在恢复前先落一份 `pre-restore` 导出）。
**异步**：立刻返回进度任务号，客户端按 `GET /api/backups/jobs/{job_id}/events` 看「已应用记录数 / 总记录数」。

```json
{
  "operation": "restore_backup",
  "success": 1,
  "restored_from": "export-20261123T050000Z-manual.json",
  "safety": "export-20261123T061300Z-pre-restore.json",
  "job": "…Jobs 控制台的任务号（保留兼容）…",
  "job_id": "backup-7c2e…",
  "started_at": "2026-11-23T06:13:00Z",
  "events": "/api/backups/jobs/backup-7c2e…/events"
}
```

`done` 事件的 `result` 带汇总：`restored` / `skipped`（档案在磁盘上不存在）/ `archives_updated` /
`archives_skipped_missing` / `categories_restored` / `tankoubons_restored` / `stamps_restored` /
`bookmarks_restored` / `glossaries_restored` / `font_patterns_restored` / `text_region_sets_restored` /
`snapshot`（本次的 `pre-restore` 结果）。

对 `kind=backup`（`.rdb`）返回 `409`，`"reason": "full_backup_not_restorable"`，`error` 写明运维步骤：

1. 下载 `.rdb`；
2. 停掉 Redis；
3. 用它替换 Redis 的 RDB 文件（`CONFIG GET dir` / `dbfilename` 指向的位置，通常是 `dump.rdb`）；
4. 启动 Redis。

`400` 当导出不可解析；`404` 当不存在。

## 自动生成时机（非端点）

| 时机 | kind | reason | 阻塞性 |
|---|---|---|---|
| 计划开启（`snapshot_schedule_enabled`）且 `snapshot_cron` 命中时（5 秒一次的 tick 判定） | `backup`（RDB） | `scheduled` | 后台，不阻塞 HTTP 服务就绪 |
| `POST /api/database/clean` 开始前 | `export`（JSON） | `pre-clean` | 同步（job 线程内） |
| 恢复（上传或从导出）开始前 | `export` | `pre-restore` | 同步 |
| `rebuild-index` 开始前 | `export` | `pre-reindex` | 同步 |
| 台账到期批量清除前（若本次将清除 > 0 条） | `export` | `pre-journal-purge` | 同步 |
| `POST /api/backups` / `POST /api/backups/export` | `backup` / `export` | `manual` | 同步 |

**计划生成的是快照（RDB），`pre-*` 生成的是导出（JSON）**：前者是完整备份，后者要快、要能在一次破坏性操作的关键路径上同步完成，绝不能在这里写几百 MB 的 RDB。

生成失败：只 `warn!` 并写进任务结果 `snapshot: {ok:false, error}`，不阻塞操作；失败的那个槽位在进程内记下，**同一槽位不再重试**，下一个槽位自动恢复。（远程 Redis 上计划快照会一直走到这条路径：显式报错、不写文件。）

## 计划语义（`schedule` 字段的行为）

- 设置项是一个 **6–7 段 crontab**：`秒 分 时 日 月 星期 [年]`，按服务器本地时区求值。默认 `0 0 3 * * 1`（每周一 03:00:00），即开箱只有每周一次全量备份。
- **标准 cron 语义**（与本项目其它地方一致，也方便照搬 TrueNAS/crontab 的经验）：
  - 数字星期 `0` 和 `7` 都表示周日、`1` 是周一；也接受 `SUN`…`SAT`、`JAN`…`DEC` 名字；
  - 支持 `*` / `?`、`a`、`a-b`、`*/n`、`a/n`、`a-b/n` 和逗号列表；
  - **日与星期两个字段都受限时取「或」**（Vixie 规则：每月 1 号**或**每个周一），任一字段是 `*` 时取「且」；
  - 字段范围：秒 0–59、分 0–59、时 0–23、日 1–31、月 1–12、星期 0–7、年 1970–2100。
- **一个 5 秒 tick 的后台任务**驱动：命中秒级表达式，所以 `*/10 * * * * *` 这类写法也可用。每小时 sweep 只做保留期清理。
- Redis hash `LANRURUGI_SNAPSHOT_SCHEDULE`（config 库）的 `last_cron_fire` 记录**最近一次生成完成的时间**：
  - **重启不重复**：候选时刻不晚于该记录就不再生成；
  - **停机补一次**：有记录时取记录之后的第一个命中，无论错过多少次都只补**一份**；记录写的是这次生成完成的时间（不是被补的那个槽位），否则每个错过的槽位都会再触发一次；
  - **首次安装立即生成一份**：没有 `last_cron_fire` 记录时（新装、或第一次升级到 cron 计划），第一个 tick 立刻生成一份，之后交给 cron；这也是子分钟表达式能稳定工作的前提——若要求「当前这一秒正好命中」，5 秒的 tick 与 `*/10` 这类表达式可能永远错开；
  - **改设置立即生效**：每次 tick 都重新读设置，保存后下一个 tick 就按新表达式执行，无需重启。
- **时区**：一律按服务器本地时区（`chrono::Local`，即进程的 `TZ`）计算；`schedule.timezone` 只是把它展示出来（`Asia/Tokyo (+09:00)`，读不到 IANA 名时只有偏移），不参与计算，也不是 `timezone` 设置项（那项只影响日期显示与搜索）。
- **DST**：不存在的本地时刻（春季跳过的那一小时）跳过不执行；重复的本地时刻（秋季那一小时）取**较早**的那个瞬时，所以同一命中不会被算两次。
- `snapshot_schedule_enabled=false`：不生成任何计划快照，也不写 `last_cron_fire`；手动端点与所有 `pre-*` 照旧。
- 存进 Redis 的表达式若损坏（只能手改造成）：服务端 `warn!` 并按默认表达式调度，`schedule.cron` 仍如实回显存着的字符串。
- `snapshot_cron_preset`（`hourly|daily|weekly|monthly|custom`）只是 UI 的回显状态，服务端从不据它推算时间。

## POST `/api/backups/schedule/preview`

保存前预览：给表达式，回校验结果与接下来 10 个命中时刻。恒返回 `200`（这是表单校验辅助，不是失败的请求；保存时非法表达式才会被 `PUT /settings` 以 `400` 拒绝）。

```bash
curl -fsS -b $J -X POST -H 'Content-Type: application/json' \
  -d '{"cron":"*/10 * * * * *"}' $BASE/api/backups/schedule/preview
```

```json
{ "valid": true, "timezone": "Asia/Tokyo (+09:00)", "next_runs": ["2026-11-23T12:00:10+09:00", "…"] }
```

```json
{ "valid": false, "timezone": "Asia/Tokyo (+09:00)", "error": "seconds (segment 1): \"61\" is outside 0-59", "next_runs": [] }
```

`error` 一定是「哪一段 + 为什么」，例如 `seconds (segment 1): "61" is outside 0-59`、`expected 6 or 7 fields (…), got 5: "0 3 * * 1"`。

## 保留与 pin

| 类别 | 份数 | 分组 |
|---|---|---|
| 导出 `daily` | `snapshot_daily_keep`（默认 7） | 按**本地日期**去重，同一天只留最新一份 |
| 导出 `weekly` | `snapshot_week_keep`（默认 4） | 按 ISO 周分组，每组只留最新一份 |
| 导出 `pre-*` | 固定 5 | — |
| 导出 `manual` | 固定 5 | — |
| 快照（RDB） | 固定 **3** | 不分 reason，只按时间取最新 3 份 |

所有上限都可以被 pin 打破：**被 pin 的文件不淘汰、也不占名额**。
无论哪一档，**最新一份可用文件**都永不自动删除（`DELETE` 也拒绝，见上）。
