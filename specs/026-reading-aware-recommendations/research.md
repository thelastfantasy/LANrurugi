# Research — 阅读状态感知的推荐排序

**Feature**: 026-reading-aware-recommendations
**Date**: 2026-11-11

## R1. 阅读状态放在哪一层

- **Decision**: 新增 `crates/lanrurugi-recommend/src/reading.rs`，纯函数：`ReadBucket` 推导、加成、同系列证据、排序。
- **Rationale**: 该 crate 已经是「推荐排序」的归属地，且不依赖 Redis/Axum；纯函数可覆盖全部分支的单测，API 层只负责取数据。符合 Constitution 的「共享逻辑抽成 helper」。
- **Alternatives**: 放在 `lanrurugi-api/src/recommend.rs` 内联——两条路径（缓存命中/未命中）会复制同一段过滤逻辑，且要用 `AppState` 才能测；否决。

## R2. 两条路径如何保证同一顺序

- **Decision**: 两条路径都产出 `Vec<(ArchiveMeta, ReadingState, f32)>`（相似度），交给同一个 `rank()`；缓存命中路径只读 Top-N id 并 `archives.get` 补阅读字段，未命中路径本来就有全量 `Archive`。
- **Rationale**: 排序只依赖 (candidate, similarity)，与数据来源无关；这是「缓存命中与 miss 顺序一致」测试能成立的前提。
- **Alternatives**: 缓存里直接存排序结果——进度变化即失效，违背 R4。

## R3. 同系列证据与相似度门槛

- **Decision**: `artist`/`circle`/`parody` 标签交集非空，或 `normalize_title` 后前 24 字符相同；且 `similarity >= 0.72` 才给 `+0.08`。
- **Rationale**: 实测同系列 ~0.97、最相关的无关作品 ~0.88（`recommend_llm.rs` 的注释），0.72 是安全下界；只靠标签会把「同作者不同作品」硬推，只靠标题前缀对 `【完全版】` 这类差异不敏感（`normalize_title` 已剥掉发布注记）。
- **Alternatives**: 只用 embedding——无法稳定区分「同系列未读」与「无关高分」；否决。

## R4. 缓存策略

- **Decision**: Top-N 从 100 提到 150（只存 id）；重排缓存键加 `progress_digest`。
- **Rationale**: 进度过滤发生在请求路径，Top-N 不需要因进度变化重建；150 给过滤留出余量（读完几卷后仍能凑够 10 条）。重排结果与锚点/候选进度强相关，必须进键，否则读到 100% 后仍按旧顺序返回（SC-005）。
- **Alternatives**: 每次请求重新 embedding——回到 O(n)，issue #70 已证明不可接受；否决。

## R5. LLM 失败/未配置

- **Decision**: `llm_rerank` 只拿过滤后的候选；失败时返回 `rank()` 的结果截断到 limit。
- **Rationale**: 现有可能路径就是「LLM 失败退回 embedding 顺序」，把退回目标换成带加成的顺序即可，不新增分支。
- **Alternatives**: 无 LLM 时也调用模型做卷序——没有 key 时不可能；否决。

## R6. 兜底语义

- **Decision**: 全部候选都 `Finished` 时返回相似度前 3，`fallback_used = true` 仅用于日志/测试，不进响应。
- **Rationale**: 空面板比「没有推荐」更糟；响应形状保持不变避免前端改动。
- **Alternatives**: 返回空数组——UI 会出现空区域；否决。

## R7. 进度摘要的格式

- **Decision**: `progress_digest = hex(hash(anchor_bucket || "|" || join(candidate_id,bucket)))`，取 64 位 hash 的 16 位十六进制；候选顺序按最终排序。
- **Rationale**: 短、稳定、无额外存储；候选集合变化本来就有 `candidate_fingerprint` 兜底，进度摘要只负责「同一候选集、进度变了」。
- **Alternatives**: 把每个候选的进度写进缓存值再比较——键不变时旧条目仍会命中并返回旧排序；否决（不符合 FR-009 的「键必须包含」）。

## R8. 前端是否需要改动

- **Decision**: 不改。
- **Rationale**: 响应仍是 `{ recommendations: [{id,title,score}] }`；标题「接下来读什么」本身中性，兜底时无需换文案。
- **Alternatives**: 加「已读完」角标——没有已读完条目可展示；否决。
