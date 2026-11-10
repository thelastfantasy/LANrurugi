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

## 6. spec 一致性自查（analyze 结果）

| FR | 状态 | 说明 |
|---|---|---|
| FR-001 封面哈希 + 缓存 | ✅ | 未命中才解码，键按内容 id |
| FR-002 距离 + 元数据证据 | ✅ | 作者/社团否决、标题 Jaccard、页数/大小比 |
| FR-003 不设组上限 + 最佳成员 | ⚠ | 不截断组；「最佳副本」按胜场 → 页数 → 大小已实现并在卡片标出；但卡片没有直接把「像到什么程度」写成文字（只有对列表里的 similarity 与对比窗里的逐页清晰度） |
| FR-004 按需逐页比对 | ✅ | 只有 compare 端点解码 |
| FR-005 两个只读接口 | ✅ | 实际给了 groups / group / compare / decisions 四个只读面 |
| FR-006 整组一次复核 | ⚠ | 输入缺「平均清晰度」——那要对每个成员解码采样页，成本高；已在 §4 记为偏离 |
| FR-007 复核缓存 + 非法输出降级 | ✅ | 缓存键含 content 签名；降级结果不写缓存 |
| FR-008 成员三态 + 按对 not_duplicate | ✅ | 默认 keep；按对记录并在下一次扫描重组 |
| FR-009 grid 卡片 | ✅ | 默认全选、`+N` 折叠、整宽智能对比 |
| FR-010 恢复智能对比 modal | ⚠ | 重建版，无放大镜、无对齐带点击跳页（§4） |
| FR-011 删除前列出引用 | ✅ | 逐档案查 categories/tankoubons 并列在确认框里 |
| FR-012 继续写 legacy 键 | ✅ | 格式与键名不变 |
| FR-013 后台作业 | ⚠ | 有进度、防并发；**取消未接线**（作业的 cancel token 未检查） |
| FR-014 增量 | ✅ | 哈希缓存 + 复核缓存 |
| FR-015 不发图像 | ✅ | `build_input` 只有文本与数字 |
| FR-016 30% 线程 | ✅ | 专用 Rayon 池，`(cores*3/10).clamp(1,4)` |
| FR-017 四语言 + 主题色 | ✅ | en/zh/ja/zh_Hant；无新硬编码颜色 |

## 7. 建议 review 重点

1. `detect.rs` 的元数据门禁是否过严/过松——尤其「作者/社团完全不相交就否决」在合志与多人合辑
   上的表现。
2. `run_scan` 在每组调用一次模型的成本控制是否可接受；是否需要改成页面可见时才复核。
3. `regroup_by_pairs` 之后 `built_at` 仍是本次扫描时间，但组的成员可能已变——决定记录以
   group key 为键，key 变了就等于新组，这是有意的，请确认没有遗漏的引用。
4. 删除前的引用清单是**逐档案两次请求**（分类、单行本），组大时是一次十几请求的突发；若要更省，
   可以再加一个批量端点。FR-011 的语义已满足。


## 9. 无用控件清理（2026-11-10 追加，用户指出）

020 把档案改成按内容寻址后，设置页与订阅表单里仍留着一些**已经不可能改变结果**的控件。这一轮
把它们删掉：

| 控件 / 字段 | 处理 | 理由 |
|---|---|---|
| 设置 → 档案文件 → **「替换重复的档案」开关**（`replacedupe`） | 从前端移除（状态、保存负载、脏检查、i18n 键） | 磁盘名现在由内容 id 决定，同名档案不可能出现；字节完全相同的重复一律拒绝，`DuplicatePolicy::Overwrite` 对 `ContentHash` 从不生效。这个开关开着也没有任何可覆盖的对象 |
| 上传页解析 `replacedupe` 与插件 `overwrite_on_duplicate` 再塞进请求体 | 移除 | 同上；后端 `AddQueueItem.overwrite_on_duplicate` 改为 `#[serde(default)]`，旧客户端发送仍被接受但不再影响结果 |
| 订阅表单 → **「文件名冲突时」单选组**（先等我确认/自动改名/自动覆盖/丢弃 + 说明） | 从表单移除 | 它描述的冲突暂存流程（24 小时、`pending_filename_conflict`、覆盖/改名端点）在 020 已整体删除 |
| 阅读缓存两段说明里的「不会影响上传页面中仍在等待处理的冲突文件」 | 改写 | 那段流程已不存在 |
| 9 个死 i18n 键 | 从全部 locale 删除 | 对应控件已删 |

顺带修掉 020 遗留的两个前端单测：`tests/unit/alignmentBands.test.ts` 引用了已删除的
`AlignmentBandOverlay`，`tests/unit/queueGroupTabs.test.ts` 仍在断言已删除的
`pending_filename_conflict` 字段。**CI 的 Frontend 作业只跑 `eslint`，不跑 `test:unit`**，所以这两个
失败一直没被发现；本轮 `vitest run tests/unit` 为 379 passed / 1 skipped。

有意保留的（不是漏删）：

- `replacedupe` 仍在 `/api/settings` 的响应与允许字段里——它是 legacy 配置键，第三方脚本可能读写；
  现在只作为兼容字段保留，前端不再展示。
- 下载队列项的存储字段 `overwrite_on_duplicate` 仍在（旧记录需要反序列化），但写入方一律传
  `false`，读取方已不依赖它。
- 插件协议里的 `overwrite_on_duplicate` 声明仍会出现在插件向导的试用结果页（纯信息展示），
  以及活动记录的变更详情里；彻底删除需要改插件契约，留作后续。
