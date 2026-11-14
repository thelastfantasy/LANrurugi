# Quickstart: 重复档案检测与智能对比

手工验收步骤。自动验证在 `tasks.md` 的门禁一节。

## 准备

1. 造两个「同一作品、不同扫本」的档案：同一组页面，B 版本去掉两页、重新压缩成更低质量、
   改一下封面编码（JPEG → WebP）。放进库目录，等扫描收录。
2. 再造一个「相邻卷」：页面完全不同、标题相近（`… 第2巻`），用来验证不会被误判成重复。
3. 再造一个「不同译本」：同页面、气泡内的文字不同（可直接改两张页面的局部像素足够）。

## 扫描

```sh
curl -s -X POST 'http://localhost:3000/database/duplicates/scan?threshold=7' -H 'Authorization: Bearer …'
# → {"job":"job_…"}
# 轮询作业直到 done；期间不应出现 CPU 持续打满（30% 上限）
curl -s 'http://localhost:3000/database/duplicates/groups' -H 'Authorization: Bearer …' | jq '.groups | length, .[0].members | length'
```

期望：两个扫本在同一组；相邻卷不在同一组；不同译本的组带 `likely` 提示（由逐页对比给出）。

## 组卡片

打开 `/duplicates`：

- [ ] 卡片是 grid，不是表格；封面并排、组头有成员数与相似度。
- [ ] 「智能对比」是整宽主按钮，其余动作是描边按钮或溢出菜单。
- [ ] 3 个以上成员时首行 3 个 + `+N`，展开后全部可见。
- [ ] 每个成员有勾选框，默认全选；取消一个后「保留勾选的 N 个」随之变化。

## 智能对比

点「智能对比」：

- [ ] 顶部固定显示建议与理由（LLM 可用时），以及「按建议选择」「保留当前勾选」。
- [ ] 两侧并排、同步翻页；放大镜可用；对齐带显示缺页位置。
- [ ] 页数不同的版本：多出的页面对应位置显示为空，不把两张无关页面硬凑成一对。
- [ ] 关闭 modal 后库中数据无变化（`GET /database/duplicates/groups` 前后一致）。

## 决定与删除

1. 在三个成员的组里只勾 A 与 C，点「保留勾选的 2 个」→ B 变成 `discard`。
2. 点「删除未勾选的成员」→ 弹窗列出分类/单行本引用；确认后只删 B。
3. 另一组点「不是重复」→ 该对从列表消失；重新扫描后仍然不出现。
4. 把一个档案换成不同内容（新 id）→ 旧决定不再生效，组重新出现（如果仍像）。

## 缓存与增量

1. 第二次扫描：作业时长应显著短于第一次（哈希命中、复核命中）。
2. `redis-cli -p 16379 -n 2 HGETALL LANRURUGI_DUP_COVER_HASH | wc -l` 在第一次扫描后等于
   档案数 × 2（字段 + 值），第二次扫描不再增长。
3. 关掉 LLM（清空模型配置）后重新扫描：卡片仍出现，`review.fallback === true`，理由来自本地
   规则，UI 标注「未经过语义复核」。

## legacy 兼容

```sh
redis-cli -p 16379 -n 2 HKEYS LRR_DUPLICATE_GROUPS | head
curl -s 'http://localhost:3000/database/duplicates' -H 'Authorization: Bearer …' | jq 'length'
```

- [ ] 扫描后仍写入 `dupgp_*` 字段；旧端点返回非空。
