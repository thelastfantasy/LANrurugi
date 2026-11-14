# Data Model: 重复档案检测与智能对比

## 存储总览

全部落在 Redis 配置库（`state.redis.config`）。不新增 `Archive` 字段：这些是派生数据，
清库重建可以丢，档案记录保持原样。

| 键 | 类型 | 内容 | 失效方式 |
|---|---|---|---|
| `LANRURUGI_DUP_COVER_HASH` | hash | archive id → `v1:<16 hex>` | id 是内容哈希，内容变则 id 变，天然不会给出旧值；算法升级换前缀 `v2:` |
| `LANRURUGI_DUP_GROUPS` | hash | group key → `GroupRecord` JSON | 每次扫描重写；扫描开始时先 `DEL` |
| `LANRURUGI_DUP_REVIEW` | hash | review key → `ReviewVerdict` JSON | 键含成员内容与提示词版本，永不需要主动失效 |
| `LANRURUGI_DUP_DECISIONS` | hash | group key → `DecisionRecord` JSON | 用户改决定时覆盖；组成员内容变了 group key 就变 |
| `LRR_DUPLICATE_GROUPS` | hash | legacy 形状（`dupgp_*`） | 与旧行为一致，继续写 |

**group key** = 成员 id 升序、逗号连接。id 是内容哈希，所以「组内任何一个档案被替换」都会
产生新的 key，旧决定自然失效，不需要额外的版本字段。

## GroupRecord

```json
{
  "algorithm": "cover-phash-v1",
  "threshold": 7,
  "built_at": 1762742400,
  "members": ["<id>", "<id>"],
  "pairs": [
    {
      "a": "<id>", "b": "<id>",
      "distance": 5,
      "similarity": 0.921,
      "title_similarity": 0.78,
      "pages_ratio": 0.991,
      "size_ratio": 0.208,
      "shared_tags": ["artist:…", "circle:…"],
      "chained": false
    }
  ],
  "chained": false,
  "middle": null
}
```

- `distance`：封面感知哈希汉明距离（0–64）。
- `similarity`：`1 - distance/64`，用于排序与 UI 徽标。
- `chained` / `middle`：组内存在一对低于阈值的成员时为真，`middle` 是连接两端的中间成员 id。

## ReviewVerdict

```json
{
  "key": "<review key>",
  "verdict": "same_work",
  "keep": ["<id>", "<id>"],
  "discard": ["<id>"],
  "reason": "B 缺 2 页，A 与 C 是不同译本",
  "confidence": 0.82,
  "model": "deepseek-v4-pro",
  "prompt_version": 1,
  "fallback": false,
  "created_at": 1762742500
}
```

`fallback: true` 表示 LLM 不可用或输出不合法，`keep` 由本地规则给出（页数完整优先，其次清晰度、
再次大小），`reason` 是本地生成的一句话。

## DecisionRecord

```json
{
  "decisions": { "<id>": "keep", "<id>": "discard" },
  "not_duplicate": [["<id>", "<id>"]],
  "updated_at": 1762742600,
  "updated_by": "jade"
}
```

- 没有记录的成员按 `keep` 处理（「都保留」是零操作）。
- `not_duplicate` 是**按对**的；一对被标记后，只要两个 id 都没变，下次扫描不再把这一对作为
  候选（即使它们仍在同一个连通分量里——这种情况下组会被拆成不包含该边的分量）。

## 决定状态机

```
        ┌────────── 勾选/取消 ──────────┐
        ▼                               │
   undecided ──勾选──► keep ──取消勾选──► discard
        ▲                               │
        └────────「按建议选择」─────────┘
```

- 删除只发生在用户点「删除未勾选的成员」并且确认之后；删除调用现有档案删除接口。
- 「不是重复」不改成员状态，只写 `not_duplicate`。

## 扫描作业状态

```
queued → hashing (x/y) → comparing (x/y) → reviewing (x/y) → done
                 │              │               │
                 └──────────────┴───────────────┴──► cancelled / failed
```

- 进度写进现有 `jobs` 注册表（`JobStatus` 的 message/processed/total 字段）。
- 取消检查点：每批哈希之后、每 20 对比较之后、每组复核之前。

## 对比结果（API 形状，裁剪自 imgcompare::ComparisonResult）

```json
{
  "a": { "id": "<id>", "title": "…", "filename": "A.cbz", "size": 287000000, "pages": 214 },
  "b": { "id": "<id>", "title": "…", "filename": "B.zip", "size": 59600000, "pages": 212 },
  "aligned_pairs": 210,
  "likely_different_language": false,
  "recommendation": "a",
  "samples": [
    {
      "a_page": 12, "b_page": 12,
      "a_sharpness": 141.2, "b_sharpness": 233.9,
      "a_size": 812345, "b_size": 203456,
      "a_width": 1600, "a_height": 2300,
      "b_width": 1600, "b_height": 2300,
      "crop": { "scale": 1.0, "dx": 0.0, "dy": 0.0, "confidence": 1.0 },
      "favor": "b"
    }
  ],
  "a_only_pages": [209, 210],
  "b_only_pages": [],
  "a_entries": [ { "path": "001.jpg", "is_page": true, "size": 812345 } ],
  "b_entries": [ … ]
}
```

`samples` 最多 12 对（与 imgcompare 的 `SAMPLE_SIZE` 一致）；`favor` 是逐对的清晰度判定，
`null` 表示差距在噪声内。
