# Feature Specification: 阅读状态感知的推荐排序

**Feature Branch**: `feat/share-bind-interface`（沿用当前分支；026 的 plan-draft 已在该分支）

**Created**: 2026-11-11

**Status**: Draft

**Input**: 用户需求：读完《夏妻3》后，未读或低进度的《夏妻》《夏妻2》应排到最前；已读完（或进度 ≥80%）的同系列档案不应再被推荐。本 spec 由 `specs/026-reading-aware-recommendations/plan-draft.md`（草案）整理而来。

## Context

阅读页的「接下来读什么」目前只按标题/标签 embedding 的余弦相似度排序，完全不看阅读状态。

- `crates/lanrurugi-recommend/src/recommend.rs` 的 `recommend()` 只算相似度，模块文档明确「不解析卷号」。
- `crates/lanrurugi-api/src/recommend.rs` 的 `recommendations()` 先查每档案的 Top-N 缓存（`recommend_cache`），未命中才全库打分；随后把 `PREFILTER_COUNT = 100` 条（`crates/lanrurugi-api/src/recommend_llm.rs:27`）交给 LLM 重排，其中最多 `MAX_PROMPT_CANDIDATES = 40` 条（同文件 `:35`）进 prompt，失败退回 embedding 顺序。
- LLM 的 `ShortlistEntry`（`crates/lanrurugi-api/src/recommend_llm.rs:46`）只带 `title`（拆成作品名/发行注记）、`tags`、`pagecount`，没有阅读状态。
- 缓存键是锚点 archive id，与「谁读到哪」无关；读完一本后排序不会变。
- 可用字段（`crates/lanrurugi-core/src/entities.rs`）：`lastreadpage`、`pagecount`、`lastreadtime`、`isnew`、`date_added`。
- 前端只在阅读页的「接下来读什么」浮层消费：`apps/frontend/src/pages/Reader/Reader.tsx:1245` 调 `GET /api/reader/recommendations/{id}?limit=10`，路由注册在 `crates/lanrurugi-api/src/recommend.rs:432`。

因此现状是：读完一本后，同系列的其他卷仍按相似度排（通常它自己那一卷最像），已读完的卷也照样出现；LLM 拿到候选时不知道哪些读过，无法做「下一本」判断。

## User Scenarios & Testing

### User Story 1 - 读完一本后，同系列未读的排在前面 (Priority: P1)

用户在阅读页读完某系列的某一卷（进度 ≥80%），打开「接下来读什么」，这个系列里还没读、或只读了一部分的其它卷应出现在最前面。

**Why this priority**: 这是用户报告的核心诉求；没有它，本功能没有存在意义。

**Independent Test**: 造一个系列三卷（第 3 卷已完成 100%、第 1/2 卷未读），以第 3 卷为锚点请求推荐，断言第 1/2 卷排在前 3；无需 LLM、无需真实 embedding 数据即可用固定相似度桩测试。

**Acceptance Scenarios**:

1. **Given** 锚点已完成、同系列另有两卷未读，**When** 请求推荐，**Then** 这两卷进入前 3，且排在相似度相近的无关作品之前。
2. **Given** 锚点已完成、同系列另一卷读到 40%，**When** 请求推荐，**Then** 该卷仍在列表里且获得部分进度加成（低于未读卷，高于无加成候选）。
3. **Given** 锚点只读到 30%，**When** 请求推荐，**Then** 「未读优先」同样生效（不要求锚点已读完）。

---

### User Story 2 - 已读完的档案不再被推荐 (Priority: P1)

任何进度 ≥80% 且已读到末页的档案，不应出现在推荐列表中；LLM 若挑中它，必须被丢弃并顺位补足。

**Why this priority**: 与 US1 同属用户明确要求；推荐一本已经读完的书是纯噪声。

**Independent Test**: 三个档案分别处于已完成/在读/未读，断言输出列表不含已完成者；单独测 LLM 返回已读完 id 时被替换。

**Acceptance Scenarios**:

1. **Given** 候选中有一个进度 100% 的档案，**When** 请求推荐，**Then** 结果不含它。
2. **Given** 同样情形，**When** LLM 重排返回了该档案的 id，**Then** 该条被丢弃、由顺位候选补足，最终条数不减少。
3. **Given** 全库所有档案都已读完，**When** 请求推荐，**Then** 退回按相似度取前 3 条，且 UI 不显示「推荐」话术。

---

### User Story 3 - 同系列内按卷序给出「下一本」 (Priority: P2)

同系列有多卷未读时，推荐顺序应尽量按卷序，而不是按入库时间或相似度微差。

**Why this priority**: 没有它，用户仍要自己从三卷里挑顺序；但即使没有卷序信息，US1/US2 也让功能可用。

**Independent Test**: 两卷同系列未读、相似度接近，断言「较早一卷」在前（无 LLM 时按 `date_added` 升序；有 LLM 时由 LLM 按标题里的卷序挑选）。

**Acceptance Scenarios**:

1. **Given** 同系列两卷未读，**When** 无 LLM 请求推荐，**Then** `date_added` 较早的一卷在前。
2. **Given** 同系列两卷未读，**When** LLM 可用，**Then** prompt 中带卷序信息，模型返回的「下一本」排在前面。

---

### User Story 4 - 没有 LLM 时排序仍然正确 (Priority: P2)

未配置 LLM key、或 LLM 调用失败时，US1/US2 的过滤与加成必须仍然生效。

**Why this priority**: LLM 是可选增强，不是正确性前提；现有代码在 LLM 失败时就退回 embedding 顺序，这条路径也必须带阅读状态。

**Independent Test**: 关闭 LLM（空 key），重复 US1/US2 的断言。

**Acceptance Scenarios**:

1. **Given** 未配置 LLM，**When** 请求推荐，**Then** 已读完的档案仍被过滤、未读加成仍生效。
2. **Given** LLM 请求超时/报错，**When** 返回兜底顺序，**Then** 兜底顺序同样满足 US1/US2。

---

### Edge Cases

- `pagecount == 0`（未知页数/文本类）：永远不判「已读完」，只给低权重的在读加成；`lastreadpage > 0` 视为「进度未知」。
- `lastreadpage > 0` 但 `pagecount` 被修正变小：`progress > 1` 时按 1 处理；「已读完」仍要求 `lastreadpage >= pagecount`。
- 过滤后列表为空：按相似度取前 3 条兜底，且不给任何「推荐」措辞。
- 书库只有一个档案：沿用现状，不产生推荐。
- 锚点自身、锚点所属 Tankoubon 的其它成员：仍按现有 `exclude_ids` 排除（`crates/lanrurugi-api/src/recommend.rs:104`）。
- 同系列证据不足（相似度 < 0.72）：不给同系列加成，避免「同作者不同作品」被硬推。
- 推荐缓存里存的是读完之前算出的 Top-N：进度变化后不得直接沿用它排序。

## Requirements

### Functional Requirements

- **FR-001**: 系统 MUST 用现有字段（`lastreadpage`、`pagecount`、`lastreadtime`）推导每个档案的阅读进度，不新增持久化字段。
- **FR-002**: 系统 MUST 把候选分为三档：未读（`lastreadtime == 0` 且 `lastreadpage == 0`）、已读完（`pagecount > 0` 且 `lastreadpage >= pagecount` 且进度 ≥ 80%）、其它（在读/进度未知）。
- **FR-003**: 系统 MUST 从推荐候选中排除「已读完」的档案。
- **FR-004**: 系统 MUST 给未读候选 +0.12、给在读候选 +0.06 × (1 − progress)、给进度未知候选 +0.03 的加成，且任何候选的总加成不超过 0.15。
- **FR-005**: 系统 MUST 在「同系列证据」（共享 artist/circle/parody 标签之一，或归一化标题前缀相同）且相似度 ≥ 0.72 时给 +0.08；证据不足时不得加成。
- **FR-006**: 过滤后候选为空时，系统 MUST 返回相似度前 3 条作为兜底，且不标记为「推荐」。
- **FR-007**: LLM 重排的每条候选 MUST 带上进度描述（未读 / 已读 N% / 读完），锚点 MUST 带「刚读完」标记；prompt MUST 指示同系列未读优先、按卷序给下一本、不得选择已读完候选。
- **FR-008**: 系统 MUST 校验 LLM 返回的 id：落在已读完集合里的丢弃并由顺位候选补足，而不是整批退回。
- **FR-009**: 重排缓存的键 MUST 包含锚点进度档位与候选进度摘要，使进度变化后的下一次请求不命中旧排序。
- **FR-010**: 预计算的 Top-N MUST 不少于 150 条（当前 `PREFILTER_COUNT` 的 1.5 倍），请求路径再做进度过滤/加成。
- **FR-011**: 无 LLM 或 LLM 失败时，FR-002–FR-006 的过滤与加成 MUST 仍然生效。
- **FR-012**: 结果顺序 MUST 稳定：同分时按 id 排序，保证缓存命中与未命中两条路径一致。

### Key Entities

- **阅读进度（ReadingProgress）**：由档案的 `lastreadpage`/`pagecount`/`lastreadtime` 派生，不是新存储；三档：未读、在读、已读完，另有「进度未知」。
- **推荐候选（RecommendationCandidate）**：档案 id、标题、标签、页数、相似度、阅读进度、同系列证据。
- **同系列证据（SeriesEvidence）**：共享的 artist/circle/parody 标签，或归一化标题前缀；只在相似度足够高时参与加成。

## Success Criteria

### Measurable Outcomes

- **SC-001**: 以已完成的一卷为锚点请求推荐时，同系列未读卷出现在前 3 条（10 条结果内必现）。
- **SC-002**: 已读完的档案在推荐结果中出现 0 次；唯一例外是「全库已读完」的兜底，且兜底不超过 3 条、不带推荐措辞。
- **SC-003**: 关闭 LLM 后，SC-001/SC-002 仍然成立。
- **SC-004**: LLM 返回已读完候选时，100% 被丢弃且最终条数与请求的 limit 一致（有足够候选时）。
- **SC-005**: 阅读进度变化后，下一次推荐请求即反映新排序（不出现读到 100% 后仍按旧顺序排列的情况）。

## Assumptions

- 单用户部署，阅读进度是全局的（本项目既有定位），多用户隔离不在本次范围。
- 80% 阈值先写成常量（`FINISHED_PROGRESS`），暂不变成设置项。
- 不引入卷号解析；卷序由 LLM 判断，无 LLM 时用 `date_added` 升序近似，并在文档中写明。
- 推荐入口只有阅读页浮层（`/api/reader/recommendations/{id}`）；其它页面不在本次范围。
- 同系列召回依赖 embedding 相似度，Top-150 足够覆盖同系列（同系列通常落在个位数名次）。
