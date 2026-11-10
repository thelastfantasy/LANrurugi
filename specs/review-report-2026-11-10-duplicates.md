# 024 重复档案检测与智能对比 — 实现报告（供 cc review）

日期：2026-11-10 · 分支：`feat/share-bind-interface` · PR #258

## 1. 范围与结论

按 `specs/024-duplicate-review/spec.md` 实现：感知哈希 + 元数据证据的重复检测、每组成员级的
保留/丢弃决定、逐页对比、可选的模型语义复核。**没有**删除任何既有数据格式：legacy 的
`LRR_DUPLICATE_GROUPS` 与 `GET /database/duplicates` 完全保留。

## 2. 交付内容

### 后端（`crates/lanrurugi-api/src/duplicates/`）

| 文件 | 内容 |
|---|---|
| `mod.rs` | 六个端点、扫描作业、组视图、legacy 读端点、清空 |
| `cover.rs` | 封面感知哈希 + `LANRURUGI_DUP_COVER_HASH` 缓存（按内容 id 失效） |
| `detect.rs` | 8 段鸽笼候选、元数据门禁、并查集成组、链式检测、按存活边重组 |
| `model.rs` | `PairEvidence`/`GroupRecord`/`ReviewVerdict`/`DecisionRecord` |
| `review.rs` | 模型输入构造、校验、缓存、本地降级、`compare` |
| `decisions.rs` | `LANRURUGI_DUP_DECISIONS` 读写 |

新端点：`POST /database/duplicates/scan`、`GET /database/duplicates/groups`、
`GET /database/duplicates/groups/{key}`、`GET /database/duplicates/compare`、
`GET|POST /database/duplicates/decisions`。扫描是作业（`find_duplicates`，与 legacy 同名），
有并发保护与进度。

`FeatureId::DUPLICATE_REVIEW` 加在 `lanrurugi-llm`，提示词在 `llm_prompts.rs`；模型只收文本与
数字（标题、作者/社团、语言、页数、大小、可选清晰度），绝不发图像。

### 前端

- 删除确认框会先查每个待删档案的分类与单行本引用（`GET /api/archives/{id}/categories`、
  `GET /api/archives/{id}/tankoubons`），按标题列出「分类名（+单行本数）」，查询失败标 `?` 而不阻塞
  删除（spec FR-011）。
- `apps/frontend/src/pages/Duplicates.tsx` 重写为卡片页：成员多选（默认全保留）、推荐星标、
  `+N` 展开、链式标记、建议块、整宽「智能对比」主按钮、按建议选择、不是重复、删除未勾选。
- 对比 modal 复用同一文件内组件：并排页、逐页清晰度、对齐页数、单侧独有页、建议与语言版本提示、
  翻页。
- `api/types.ts` / `api/hooks.ts` 新增类型与四个 hook；四份 locale（en/zh/ja/zh_Hant）补齐 33 个键。

### 文档

- `docs/src/content/docs/guide/duplicates.md` 与 `en/guide/duplicates.md`：检测原理、阈值含义、
  模型角色与降级、删除不可逆、legacy 兼容。

## 3. 关键设计决定（与 spec 对照）

1. **哈希选型**：64 位平均哈希（`imgcompare::alignment::hash_page`），不是 SHA-1。理由：SHA-1
   对重新编码/缩放必然不同，正是重复的常见来源。分组阈值默认 7（0–16 可调）。
2. **候选生成**：8 段 × 8 bit 鸽笼。任意汉明距离 ≤7 的两张图必有一段完全相同，因此不漏召回；
   阈值 ≥8 时退化为全对比较。
3. **元数据门禁**：作者/社团集合完全不相交即否决；否则要求标题（去发布标记后）三元组 Jaccard
   ≥0.55 或作者相同；页数比 ≥0.8、大小比 ≥0.2。
4. **链式组**：组内存在超过阈值的成员对时标记 `chained`，并给出最接近两端的 `middle`。
5. **决定语义**：成员三态（keep/discard/undecided，默认 keep）；`not_duplicate` 按对记录。
6. **模型降级**：无 key、调用失败、输出不合法（枚举/下标覆盖/长度/置信度任一不满足）→ 本地规则
   （保留页数最接近最多的副本），`fallback: true`，且**不写缓存**。
7. **缓存键**：`v1:<sha256(group_key|prompt_version|model|members 的 id:pages:size 签名)>`；组内
   任一文件被替换（内容 id 变化）即自动失效。

## 4. 偏离与未做

- **对比 modal 是重建的，不是恢复**：被删的 `OverlayPage`/`MagnifierOverlay`/
  `AlignmentBandOverlay` 没有恢复，因此没有放大镜、没有点对齐格跳页。页面图走逐页缩略图端点，
  不是原尺寸页字节。对「判断哪个扫本更好」够用，对「逐像素检查」不够。
- **`quickstart.md` 未跑**：dev 实例需要密码，本会话拿不到（与 020 T027 同一阻塞）；也没有造
  三份夹具档案。这是本轮最大的验证缺口。
- **`not_duplicate` 的落法**：不是过滤「生成候选的边」，而是在写入前按存活边重组连通分量
  （`detect::regroup_by_pairs`）。效果相同，且不会让只剩被否决边的成员以单例留在组里。
- **扫描每组调用一次模型**：没有做「只复核可见组」的懒加载。组多时会产生一次扫描内 N 次调用；
  已有缓存键，重复扫描不重复计费。

## 5. 验证

| 命令 | 结果 |
|---|---|
| `mise run check-crate -- lanrurugi-api` | 通过 |
| `mise run clippy` | 通过（无新增 warning） |
| `mise run fmt-check` | 通过 |
| `mise run test-crate -- lanrurugi-api` | **659 + 1 + 1 + 5 通过，0 失败**（新增 13 条：哈希距离/十六进制往返、候选召回、同作品成组、不同作者否决、链式与中间成员、标题去标记、重组、端点 key、校验矩阵、降级、缓存键） |
| `npx tsc -b --force` | 通过 |
| `npx eslint src` | 0 error / 4 既有 warning |
| `mise run docs-build` | 通过，121 页 |

未验证：真实档案上的召回率与误报率（没有夹具，见 §4）；真机性能（1 万档案的哈希耗时）。

## 6. 建议 review 重点

1. `detect.rs` 的元数据门禁是否过严/过松——尤其「作者/社团完全不相交就否决」在合志与多人合辑
   上的表现。
2. `run_scan` 在每组调用一次模型的成本控制是否可接受；是否需要改成页面可见时才复核。
3. `regroup_by_pairs` 之后 `built_at` 仍是本次扫描时间，但组的成员可能已变——决定记录以
   group key 为键，key 变了就等于新组，这是有意的，请确认没有遗漏的引用。
4. 删除前的引用清单是**逐档案两次请求**（分类、单行本），组大时是一次十几请求的突发；若要更省，
   可以再加一个批量端点。FR-011 的语义已满足。
