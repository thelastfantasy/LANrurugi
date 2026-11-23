# API Contract: 自动快照（`/api/backups`）

同一套响应约定与权限（会话管理员，写审计日志）。快照文件位于 `<library>/.lanrurugi-backups/`，本组端点只暴露文件名，从不接受调用方传入的路径。

## GET `/api/backups`

```json
{
  "operation": "list_backups",
  "success": 1,
  "daily_keep": 7,
  "week_keep": 4,
  "entries": [
    {
      "name": "20261123T050000Z-daily.json",
      "created_at": "2026-11-23T05:00:00Z",
      "reason": "daily",
      "size_bytes": 412345,
      "usable": true
    }
  ]
}
```

- 按 `created_at` 从新到旧。
- `usable=false` 表示文件存在但无法解析为备份文档（损坏 / 被截断），前端禁用「恢复」按钮。

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
| 每天首次启动且当天没有 daily | `daily` | 后台，不阻塞 HTTP 服务就绪 |
| `POST /api/database/clean` 开始前 | `pre-clean` | 同步（job 线程内） |
| 恢复（上传或从快照）开始前 | `pre-restore` | 同步 |
| `rebuild-index` 开始前 | `pre-reindex` | 同步 |
| 台账到期批量清除前（若本次将清除 > 0 条） | `pre-journal-purge` | 同步 |

生成失败：只 `warn!` 并写进任务结果 `snapshot: {ok:false, error}`，不阻塞操作；同一自然日内不重复尝试 `daily`。
