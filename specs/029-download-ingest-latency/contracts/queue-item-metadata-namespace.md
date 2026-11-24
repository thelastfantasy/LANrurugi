# Contract: 队列项追加字段 `metadata_preview_namespace`

**Status**: 本特性新增 | **Type**: `/api/download_queue` 响应与 SSE delta 的可选追加字段

## 形状

`GET /api/download_queue`（及 SSE `full` 快照里的 `items[]`）中每个队列项可带：

```json
{
  "id": "9ba8f386-b2f0-4ad7-b157-5d7f3049045e",
  "plugin_namespace": "download/ehentai",
  "title": "(Pixiv) [AI Generated] NewVegas Part 11",
  "metadata_preview": { "title": "…", "tags": "…" },
  "metadata_preview_at": 1795482739920,
  "metadata_preview_namespace": "metadata/ehentai"
}
```

## 语义

| 规则 | 内容 |
|---|---|
| Q1 | 只有在“开始下载时的元数据抓取真的成功了”时才写入；抓取失败、用户关闭自动抓取、无匹配插件时保持缺省（键省略）。 |
| Q2 | 与 `metadata_preview` / `metadata_preview_at` 同生共死：preview 被清空（重试/重新开始）时本字段一并清空。 |
| Q3 | `metadata_preview_namespace` 缺省 = “入库阶段必须执行完整插件链”。消费方**不得**用“`metadata_preview` 存在”来代替它判断。 |
| Q4 | 该字段是入库阶段的内部判据；前端无行为依赖，可忽略。旧客户端/旧服务互不影响（可选项，缺省即键省略）。 |

## SSE delta

- 与 `metadata_preview` 一起变化时，允许在同一个 `{"kind":"update", …}` delta 里带上该字段；不新增 delta 类型。
- 未携带该键的 delta 表示“不变”，与既有 delta 的合并语义一致。

## 不改变的部分

- 队列项的其他字段、状态机、停止/暂停/重试语义一律不变。
- 手动 `POST /api/download_queue/{id}/fetch-metadata` 仍必须真实调用插件（不受任何缓存影响），其返回值形状不变。
