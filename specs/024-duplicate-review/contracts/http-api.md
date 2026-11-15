# Contracts: 重复档案检测与智能对比

## HTTP

所有新端点都是 additive；`GET /database/duplicates` 的旧形状（按 `LRR_DUPLICATE_GROUPS` 读
legacy 分组）保持不变。

### `POST /database/duplicates/scan?threshold=7`

启动一次扫描作业。`threshold` 为封面汉明距离（0–16，默认 7）。

```json
{ "job": "job_01H…", "operation": "find_duplicates", "success": 1 }
```

- 作业类型仍是 `find_duplicates`（legacy 页面与作业控制台都认这个名字）。
- 409：已有一次扫描在跑。400：`threshold` 超出范围。

### `GET /database/duplicates/groups`

```json
{
  "scanned_at": 1762742400,
  "algorithm": "cover-phash-v1",
  "chained_groups": 1,
  "groups": [
    {
      "key": "<id>,<id>",
      "chained": false,
      "middle": null,
      "members": [
        { "id": "<id>", "title": "…", "size": 287000000, "pages": 214,
          "cover": "/api/archives/<id>/thumbnail", "decision": "keep",
          "recommended": true },
        { "id": "<id>", "title": "…", "size": 59600000, "pages": 212,
          "cover": "/api/archives/<id>/thumbnail", "decision": "keep",
          "recommended": false }
      ],
      "pairs": [ { "a": "<id>", "b": "<id>", "similarity": 0.921 } ],
      "review": {
        "verdict": "same_work", "keep": ["<id>"], "discard": ["<id>"],
        "reason": "B 缺 2 页", "confidence": 0.82, "fallback": false
      }
    }
  ]
}
```

- 未扫描过时 `scanned_at: 0`、`groups: []`，前端显示空状态。
- 已标 `not_duplicate` 的对不参与分组；因此一个组可能因为某条边被否决而拆成两个。

### `GET /database/duplicates/groups/{key}`

组详情：在列表形状之上加 `matrix`（成员两两相似度，`[[1.0,0.92],[0.92,1.0]]`）与
`qualities`（每成员 `pages` / `size` / `avg_sharpness`，后者只在此前的对比结果里出现过才有，
否则为 `null`）。404：组不存在或已被下一次扫描淘汰。

### `GET /database/duplicates/compare?a=<id>&b=<id>`

只读逐页对比，见 `data-model.md` 的对比结果形状。

- 200：正常结果（即使 `recommendation` 为 `null`）。
- 404：任一档案不存在（含刚被删除）。
- 409：任一档案仍在入库中（`file` 为空）。
- 503：比对线程池已满（带 `Retry-After: 2`）。

### `POST /database/duplicates/decisions`

```json
{
  "group_key": "<id>,<id>",
  "decisions": { "<id>": "keep", "<id>": "discard" },
  "not_duplicate_pairs": [["<id>", "<id>"]]
}
```

响应 `{ "operation": "duplicate_decisions", "success": 1 }`。

- 400：`decisions` 的键不在 `group_key` 里，或状态不是 `keep|discard|undecided`。
- 404：`group_key` 不存在。
- 幂等：同样的请求重复提交结果一致。

### `GET /database/duplicates/decisions?group_key=…`

回读该组当前决定（形状同上，外加 `updated_at`）。没有记录时返回空 `decisions` 与空
`not_duplicate_pairs`。

## LLM（`FeatureId::DUPLICATE_REVIEW`）

### 输入（只含文本与数字，绝不发图像）

```json
{
  "members": [
    { "index": 0, "title": "…", "artist": "…", "circle": "…",
      "language": "zh", "pages": 214, "size": 287000000,
      "avg_sharpness": 182.4 },
    { "index": 1, "title": "…", "artist": "…", "circle": "…",
      "language": "ja", "pages": 212, "size": 59600000,
      "avg_sharpness": 264.1 }
  ],
  "pairs": [ { "a": 0, "b": 1, "similarity": 0.92 } ]
}
```

- `avg_sharpness` 未知时为 `null`；`artist`/`circle`/`language` 缺失时为空串。
- 成员上限 8 个；超出时按相似度与页数完整度截断。

### 输出（必须通过校验）

```json
{
  "verdict": "same_work",
  "members": [
    { "index": 0, "keep": true, "why": "页数完整" },
    { "index": 1, "keep": false, "why": "缺 2 页" }
  ],
  "reason": "同一作品的两个中文扫本，A 完整、B 缺页",
  "confidence": 0.82
}
```

校验规则（任一不满足即整条丢弃并退回本地结论）：

1. `verdict ∈ {same_work, different_versions, adjacent_volumes, unrelated}`；
2. `members` 的 `index` 恰好覆盖输入全部下标、不重不漏；
3. `keep` 为布尔；`why` 非空且 ≤ 200 字符；
4. `reason` 非空且 ≤ 300 字符；
5. `confidence` 在 `[0, 1]`。

### 缓存键

```
review_key = "v1:" + sha256_hex(group_key + "|" + prompt_version + "|" + model + "|" + members)
```

`members` 是每个成员的 `id:pages:size` 连接。命中即直接复用，不再调用模型。
