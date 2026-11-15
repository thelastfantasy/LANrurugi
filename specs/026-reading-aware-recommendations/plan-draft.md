# 026 — 把阅读状态纳入推荐排序（计划草案）

状态：草案，尚未实现（2026-11-11）。需求来自用户：读完《夏妻3》后，未读或低进度的《夏妻》《夏妻2》
应排到最前；已读完（或进度 ≥80%）的同系列档案不应再被推荐。

## 现状

- `crates/lanrurugi-recommend/src/recommend.rs`：`recommend()` 只做纯余弦相似度（标题 + 标签的
  embedding），不读任何阅读状态；模块文档明确「不解析卷号」。
- `crates/lanrurugi-api/src/recommend.rs`：先查每档案的 Top-N 缓存（`recommend_cache`），未命中才
  全库打分；随后把 `PREFILTER_COUNT = 100` 条交给 LLM 重排（`MAX_PROMPT_CANDIDATES = 40` 条进
  prompt），失败退回 embedding 顺序。
- LLM 的 `ShortlistEntry` 只带 `title`（拆成作品名/发行注记）、`tags`、`pagecount`，没有阅读状态。
- 缓存键是 anchor id（`LANRURUGI_RECOMMEND_TOPN_{id}`），与观看者的进度无关。
- 可用的进度字段（`crates/lanrurugi-core/src/entities.rs` 的 `Archive`）：`lastreadpage`、
  `pagecount`、`lastreadtime`、`isnew`、`date_added`。

## 进度模型

对每个档案计算：

```
progress = if pagecount > 0 { lastreadpage / pagecount } else { None }
state    = Unread   if lastreadtime == 0 && lastreadpage == 0
           Finished if progress >= 0.80
           Reading  otherwise (含 progress == None 的未知情形)
```

- 阈值 80% 先写成常量（`FINISHED_PROGRESS`），需要一个设置项时再加，避免膨胀设置面。
- `pagecount == 0`（未知/文本类）永远不判 Finished，只给低权重的 Reading 加成。
- 「读完」判定还要求 `lastreadpage >= pagecount`，防止 pagecount 被修正后误判。

## 排序（不依赖 LLM 也必须正确）

1. **过滤**：丢掉 `Finished` 的候选。若过滤后为空，退回按相似度取前 3 条（UI 不出现空面板），
   但不给它们任何「推荐」话术。
2. **打分**：`final = similarity + read_boost + series_boost`，两者都设上限，保证不会把不相关作品
   顶上来（实测同系列 ~0.97、最相关的无关作品 ~0.88，加成总和上限 0.15）。

   | 信号 | 加成 |
   |---|---|
   | 从未打开（`lastreadtime == 0`，`lastreadpage == 0`） | +0.12 |
   | 读过一部分（0 < progress < 0.8） | +0.06 × (1 − progress) |
   | 进度未知（pagecount == 0 且 lastreadpage > 0） | +0.03 |
   | 同系列证据（同 artist/circle/parody 标签，或归一化标题前缀相同）且 similarity ≥ 0.72 | +0.08 |
   | Finished | 排除 |

   同系列加成只在相似度已经像同一作品时生效，避免「同作者但不同作品」被硬推。
3. **同系列内的顺序**：不引入卷号解析（模块既有决定）。交给 LLM 按卷序挑；无 LLM 时按
   `date_added` 升序作为「先读早的」的近似，并在文档里写明这是近似。

## LLM 重排扩展

- 每条候选多给一个字段：`进度: 未读 / 已读 12% / 读完`；锚点额外给 `刚读完` 标记。
- 指令补三条：同系列未读优先；按卷序给出「下一本」；**不得**选择进度 ≥80% 的候选。
- 返回值校验：模型给出的 id 若落在 Finished 集合里，丢弃并顺位补足，而不是整批退回。
- 重排缓存键加入「锚点进度档位 + 候选进度摘要」（如 `id:0/1/2` 的短哈希）；否则读完一本后
  仍会命中读完之前的排序。

## 缓存层

- `recommend_precompute` 的 Top-N 从「只存 limit 条」改为存 **150 条**（`PREFILTER_COUNT` 的
  1.5 倍），请求路径再做进度过滤/加成——这样进度变化不需要重新 embedding。
- 过滤后不足时，可以从同系列（标签重叠）里补，但不在本 spec 首版做。

## 边界

- 锚点自身、同一单行本的其他成员仍按现有 `exclude_ids` 排除。
- 锚点未读完时同样适用（「未读优先」不等于「只推未读」）。
- 书库只有一个档案时不推荐（现状）。
- 多用户部署下进度是全局的（本项目定位单用户），本次不改。

## 验证

- 单元测试（`lanrurugi-recommend`）：给定相似度与进度组合，断言排序与过滤；同分时按 id 稳定。
- API 测试：seed 三个档案（Finished/Reading/Unread），断言 cache-hit 与 miss 两条路径给出相同顺序。
- LLM：prompt 快照测试更新；校验测试（模型挑 Finished → 被丢弃）。
- dev 手测：把《夏妻3》读到最后一页（≥80%），刷新阅读页推荐：夏妻/夏妻2 居前；把夏妻读到 81% 后
  它从列表消失；把夏妻3 重置进度后，推荐恢复为纯相似度顺序。

## 风险

- 缓存里只有 Top-150：若某同系列卷相似度落在 150 名之外（几乎不可能，同系列通常在个位数），
  需要「按标签并集补充」的第二步。
- 书页数未知的档案无法判定「读完」，只能保守给低加成。
- LLM 重排缓存键变更会让旧条目失效（一次性的冷启动成本）。
