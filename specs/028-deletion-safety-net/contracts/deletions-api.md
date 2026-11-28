# API Contract: 删除台账（`/api/deletions`）

沿用既有约定：成功 `{"operation": "<name>", "success": 1, ...}`，失败 `{"error": "<message>", "operation": "<name>", "success": 0}` + 相应状态码。全部为**会话管理员**权限；`route_policy.csv` 中与 `/database/*` 同类（token 角色拒绝、写入审计日志）。

## GET `/api/deletions`

列出仍可恢复的台账条目，按 `deleted_at` 从新到旧。

```json
{
  "operation": "list_deletions",
  "success": 1,
  "retention_days": 30,
  "total": 2,
  "entries": [
    {
      "id": "a1055279aab1e1cbbdc6ff33323c506096e4faab",
      "title": "よちリョウタ 妄想ユニせっくす",
      "deleted_at": "2026-11-23T05:12:33Z",
      "source": "clean",
      "path": "/home/koyomi/lanrurugi/content/a1/a105….zip",
      "file_exists": true,
      "record_exists": false
    }
  ]
}
```

- `file_exists` 是读取时 `stat` 的结果，用于前端提示「恢复后是否需要补文件」。
- `record_exists`：同 id 的记录已经回来了（从回收站恢复、重新扫描到同一文件）。这样的条目再恢复只会得到 `already_exists`，回收站页面不显示它们。
- 损坏条目（JSON 解析失败 / id 不一致）不返回，只计入日志。

## POST `/api/deletions/{id}/restore`

```json
{ "operation": "restore_deletion", "success": 1, "id": "a105…", "restored": 1, "file_missing": false }
```

| 状态码 | 情况 |
|---|---|
| 200 | 恢复成功（`file_missing` 表示文件当前不存在，但记录已恢复） |
| 404 | 台账里没有该 id |
| 409 | 同 id 记录已存在（不覆盖），响应含 `"reason": "already_exists"` |

副作用：写回记录后删除台账条目；写审计日志（`action_types::ARCHIVE_RESTORE` 复用或新增 `DELETION_RESTORE`）。

## POST `/api/deletions/restore-all`

```json
{ "operation": "restore_all_deletions", "success": 1, "restored": 120, "skipped": 3, "missing_files": 3, "failed": 0 }
```

逐条恢复，任一失败不中断；`skipped` 为「已存在」的数量。

## DELETE `/api/deletions/{id}`

```json
{ "operation": "purge_deletion", "success": 1, "purged": 1 }
```

`404` 当条目不存在。

## DELETE `/api/deletions`

清空全部台账（需 `?confirm=1`，否则 `400`）。

```json
{ "operation": "clear_deletions", "success": 1, "purged": 120 }
```

## 与既有端点的关系

- 台账写入发生在 `purge_one_archive` 内部，**所有**调用方（clean / 单条删除 / 修订版取代 / 拆分原始档）自动获得台账，无需新参数。
- `POST /api/database/clean` 的成功响应新增两个只读字段（向后兼容，不改变既有字段）：

```json
{ "operation": "database_clean", "success": 1, "deleted": 12, "unlinked": 3, "journaled": 12, "diagnostics": { … } }
```

- `POST /api/database/clean` 在计划删除量超阈值且未带 `force=1` 时返回 **409**：

```json
{
  "operation": "database_clean",
  "success": 0,
  "error": "This clean would delete 213 records (55% of 384), above the 25%/50 guard. Re-run with force=1 to confirm.",
  "planned": 213,
  "total": 384,
  "percent": 55,
  "guard_percent": 25,
  "guard_min": 50
}
```
