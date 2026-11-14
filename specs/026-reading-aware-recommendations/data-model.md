# Data Model — 阅读状态感知的推荐排序

**Feature**: 026-reading-aware-recommendations | **Date**: 2026-11-11

本次不新增持久化实体；下面的类型是运行期派生值与既有 Redis 值的形状变化。

## ReadingState（运行期派生，不落库）

```rust
/// 由 Archive 的三个字段派生；不缓存、不持久化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadingState {
    pub last_read_page: u32,
    pub page_count: u32,
    pub last_read_time: u64,
}
```

来源：`lanrurugi_core::entities::Archive { lastreadpage, pagecount, lastreadtime, .. }`。

## ReadBucket

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadBucket {
    Unread,          // lastreadtime == 0 && lastreadpage == 0
    Reading(f32),    // progress = clamp(lastreadpage / pagecount), 0.0..1.0
    Unknown,         // pagecount == 0 && lastreadpage > 0
    Finished,        // pagecount > 0 && lastreadpage >= pagecount && progress >= 0.80
}
```

- `FINISHED_PROGRESS: f32 = 0.80`（同 `spec.md` FR-002）。
- `Reading` 的 `progress` 计算前把 `lastreadpage` 夹到 `0..=pagecount`。
- `page_count == 0 && last_read_page == 0` 归 `Unread`（没有可读页面的空档案不参与加成）。

## 加成

| 档位 | 加成 |
| --- | --- |
| `Unread` | `+0.12` |
| `Reading(p)` | `+0.06 × (1 − p)` |
| `Unknown` | `+0.03` |
| `Finished` | 不参与排序（过滤） |
| 同系列（`similarity ≥ 0.72` 且证据成立） | `+0.08` |

单条候选总加成封顶 `0.15`：`(state_boost + series_boost).min(0.15)`。

## RankedCandidate / RankOutcome

```rust
pub struct RankedCandidate {
    pub meta: ArchiveMeta,
    pub state: ReadingState,
    pub similarity: f32,
}

pub struct RankOutcome {
    pub items: Vec<Recommendation>,
    /// 全部候选都 Finished、退化为相似度前 3 时为 true；不进 API 响应，只给日志/测试。
    pub fallback_used: bool,
}

pub fn rank(
    anchor: &ArchiveMeta,
    anchor_state: ReadingState,
    candidates: &[RankedCandidate],
    limit: usize,
    include_finished: bool, // 仅测试/兜底路径使用
) -> RankOutcome
```

排序键：`final_score = similarity + boost` 降序，`final_score` 相等时按 `id` 升序（字节序），保证缓存命中/未命中一致。

## ProgressDigest

```rust
/// 锚点档位 + 按最终顺序排列的候选 (id, 档位) 序列，DefaultHasher 的 16 位十六进制。
pub fn progress_digest(anchor_state: ReadBucket, ordered: &[(&str, ReadBucket)]) -> String
```

写入重排缓存键：`recommend_rerank_key(archive_id, limit, prompt_version, &digest)`，新键形如
`LANRURUGI_RECOMMEND_RERANK:{archive_id}:{limit}:{prompt_version}:{digest}`。

## RerankCacheEntry（既有结构，语义收紧）

```rust
struct RerankCacheEntry {
    candidates: String,   // 既有：候选集指纹
    picks: Vec<String>,   // 既有：模型返回的顺序
    progress_digest: String, // 新增：与键里的 digest 一致，读取时双重校验
}
```

旧条目（无 `progress_digest`）反序列化失败 → 当作 miss；不迁移、不删除。

## 推荐接口响应（不变）

```json
{ "recommendations": [ { "id": "…", "title": "…", "score": 0.93 } ] }
```

`score` 现在是 `final_score` 夹在 `0.0..=1.0`；元素数量 ≤ `limit`。全读完兜底时同样这个形状。
