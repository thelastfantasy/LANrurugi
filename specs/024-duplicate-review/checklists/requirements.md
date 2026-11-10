# Specification Quality Checklist: 重复档案检测与智能对比

- [x] 每个用户故事都有验收场景（spec.md「Acceptance」小节）
- [x] 每个 FR 可测（阈值、计数、端点、缓存键都有具体值）
- [x] 成功标准可度量（SC-001–SC-006）
- [x] 未解决的 `[NEEDS CLARIFICATION]`：无（组大小上限取 8、LLM 每组一次已写进 Assumptions）
- [x] 与既有实现一致：`LRR_DUPLICATE_GROUPS` 继续写、`imgcompare` 的能力被复用而不是重写
- [x] 不引入新的档案字段（派生数据全部落 Redis）
- [x] 删除路径必须显式确认（FR-011）
- [x] LLM 只收文本与数字，且失败必降级（FR-013）
