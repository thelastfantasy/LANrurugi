# Quickstart — 阅读状态感知的推荐排序

**Feature**: 026-reading-aware-recommendations

## 前置

- 工作区在 `feat/share-bind-interface`，依赖已装好（`pnpm install`、`cargo` 工具链）。
- 单测/检查一律走 `mise run`（资源与并发保护）：

```sh
mise run test-crate -- lanrurugi-recommend
mise run test-crate -- lanrurugi-api
mise run clippy
mise run fmt-check
```

## 场景 1：已读完的卷不再被推荐（无 LLM）

1. 准备一个至少 6 卷的同系列档案（或 dev 库里现有的同系列），把卷 1 读到最后一页
   （`lastreadpage == pagecount`，或 ≥80%）。
2. `curl 'http://localhost:3000/api/reader/recommendations/<卷2 id>?limit=10'`。
3. 断言：响应里不含卷 1；卷 3/4/5 排在相似但无关的档案之前；`score` 在 0–1 之间。

## 场景 2：读完一卷后同系列下一卷提前

1. 把锚点卷读到中间（例如 30%），记录当前顺序。
2. 再读到 100%，重新请求同一 URL（绕过浏览器缓存）。
3. 断言：同系列未读卷的位置上升；已读完的那卷消失；接口形状不变。

## 场景 3：全部候选已读完

1. 选一个所有候选都已读完的锚点（或临时把候选的 `lastreadpage` 设为 `pagecount`）。
2. 请求推荐。
3. 断言：返回相似度最高的 3 条（不是空数组），HTTP 200。

## 场景 4：进度变化使重排缓存失效

1. 配置了 LLM 的实例，先请求一次（写入 `LANRURUGI_RECOMMEND_RERANK:*`）。
2. 改动锚点或候选的 `lastreadpage`，再请求。
3. 断言：Redis 里出现带新 digest 段的键；旧键不再命中；结果顺序按新进度。

## 场景 5：LLM 不可用

1. 不配置 `LANRURUGI_LLM_*`（或让模型调用失败）。
2. 请求推荐。
3. 断言：仍返回带加成的顺序（不是纯相似度），日志没有 panic。

## 失败排查

- `mise run test-crate -- lanrurugi-recommend` 失败：先看 `reading.rs` 的单测断言，分档边界
  （0.80、`lastreadpage == pagecount`、`pagecount == 0`）最容易写反。
- 缓存命中/未命中顺序不一致：检查两条路径是否都把 `ReadingState` 传进同一个 `rank()`，
  以及候选集是否都排除了 `exclude_ids`。
- 已读完仍出现：确认过滤发生在 LLM 之后（校验）以及缓存命中路径之前（Top-N 读取后）。
