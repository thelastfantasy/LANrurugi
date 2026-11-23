# API Contract: 自动快照（`/api/backups`）

同一套响应约定与权限（会话管理员，写审计日志）。快照文件位于 `<library>/.lanrurugi-backups/`，本组端点只暴露文件名，从不接受调用方传入的路径。

## GET `/api/backups`

```json
{
  "operation": "list_backups",
  "success": 1,
  "daily_keep": 7,
  "week_keep": 4,
  "schedule": {
    "enabled": true,
    "daily_at": "03:00",
    "weekly_day": 1,
    "last_daily_at": "2026-11-23T03:00:00Z",
    "last_weekly_at": null,
    "next_daily_at": "2026-11-24T03:00:00Z",
    "next_weekly_at": "2026-11-24T03:00:00Z"
  },
  "entries": [
    {
      "name": "20261123T030000Z-daily.json",
      "created_at": "2026-11-23T03:00:00Z",
      "reason": "daily",
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
      }
    }
  ]
}
```

- 按 `created_at` 从新到旧。
- `usable=false` 表示文件存在但无法解析为备份文档（损坏 / 被截断），前端禁用「恢复」按钮。
- `summary` 是兄弟文件 `<name>.summary.json` 的内容；文件不存在或解析失败时**为 `null`（字段始终存在，不会省略）**。该字段只读那份小文件，绝不为了列表去解析快照本身。`summary.bytes` 与 `size_bytes` 同值。
- `schedule` 是**当前设置**下的计划视图（见下节）：
  - `enabled` / `daily_at` / `weekly_day` 直接来自设置项；
  - `last_daily_at` / `last_weekly_at` 是 Redis 里记录的最近一次计划生成瞬时（RFC 3339 UTC），没有记录为 `null`；
  - `next_daily_at` / `next_weekly_at` 按当前设置与服务器本地时间计算出的下一次到期瞬时（RFC 3339 UTC）；**`enabled=false` 时两者都为 `null`**。

## POST `/api/backups`

立刻生成一份快照（`reason=manual`）。同步完成（大库可能需要数秒），返回新条目：

```json
{ "operation": "create_backup", "success": 1, "entry": { "name": "20261123T061200Z-manual.json", … } }
```

## GET `/api/backups/{name}`

下载该快照文件原始字节。
`Content-Type: application/json`、`Content-Disposition: attachment; filename="<name>"`。
`400` 当 `name` 含路径分隔符或不是已知条目（防止目录穿越）；`404` 当文件不存在。

## POST `/api/backups/{name}/restore`

用该快照恢复整库。复用既有的恢复流程（与 `POST /api/database/restore` 上传同一份文档的语义一致），并在恢复前先按 D4 生成一份 `pre-restore` 快照。

```json
{ "operation": "restore_backup", "success": 1, "restored_from": "20261123T050000Z-daily.json", "safety": "20261123T061300Z-pre-restore.json" }
```

`400` 当条目不可解析；`404` 当不存在。

## DELETE `/api/backups/{name}`

```json
{ "operation": "delete_backup", "success": 1, "deleted": "20261123T050000Z-daily.json" }
```

最新一份可用快照不可删除（`409`，`"reason": "last_usable"`），与 D5 的「不删除最新一份」一致。

## 自动生成时机（非端点）

| 时机 | reason | 阻塞性 |
|---|---|---|
| 计划开启时，每天本地 `snapshot_daily_at` 之后的第一分钟内 | `daily` | 后台，不阻塞 HTTP 服务就绪 |
| 计划开启时，`snapshot_weekly_day` 那天的同一时刻之后的第一分钟内 | `weekly` | 后台 |
| `POST /api/database/clean` 开始前 | `pre-clean` | 同步（job 线程内） |
| 恢复（上传或从快照）开始前 | `pre-restore` | 同步 |
| `rebuild-index` 开始前 | `pre-reindex` | 同步 |
| 台账到期批量清除前（若本次将清除 > 0 条） | `pre-journal-purge` | 同步 |

生成失败：只 `warn!` 并写进任务结果 `snapshot: {ok:false, error}`，不阻塞操作；失败的那个槽位在进程内记下，**同一槽位不再重试**，下一个槽位自动恢复。

## 计划语义（`schedule` 字段的行为）

- **一个每分钟 tick 的后台任务**驱动（取代早先「启动时 + 每小时」的隐式检查）；每小时 sweep 只保留保留期清理。
- 每分钟比较「最近一个已到点的槽位」与 Redis 里的 `last_daily_at` / `last_weekly_at`：前者更新则生成一次。
  - **重启不重复**：槽位已记录，进程重启后同一槽位不再生成。
  - **停机补一次**：无论错过多少天，启动后只补最近那个槽位一次（catch-up）。
  - **改设置立即生效**：每次 tick 都重新读设置，保存后的下一分钟内按新时间/新区日/新开关执行，无需重启。把 `daily_at` 改成一个「今天已经过去」的时刻，会在下一分钟内立刻补一份（该时刻今天还没服务过），这是预期行为。
- **时区**：按服务器本地时区（`chrono::Local`，即进程的 `TZ`）计算，与保留策略按本地日期去重同一套；不是 `timezone` 设置项（那项只影响日期显示与搜索）。
- **DST**：
  - 春季「跳过」的小时里不存在的时刻，顺延到缺口结束的那一刻执行（个别 2 小时跳变的时区最多顺延 2 小时）；
  - 秋季「重复」的小时一律取**较早**的那个瞬时，这样「最近槽位」的计算在当天剩余时间里仍然等于已记录的瞬时，不会重复生成第二次。
- `snapshot_schedule_enabled=false`：不生成任何 `daily` / `weekly`，也不写槽位；`POST /api/backups` 与所有 `pre-*` 照旧。
