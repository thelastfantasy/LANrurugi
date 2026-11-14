# 复查结果（2026-11-10 b）：11-10 修复、024 重复检测，以及报告里没提到的提交

分支 `feat/share-bind-interface`，本地 HEAD `5ad74b0`（比 origin 多一个未推送的 UI 提交）。范围是 `7e3addb..5ad74b0` 的 17 个提交，对照 `specs/review-report-2026-11-10.md` 和 `specs/review-report-2026-11-10-duplicates.md`。只报告，没有改代码。

## 门禁（本地，HEAD `5ad74b0`）

| 项 | 结果 |
|---|---|
| fmt-check / clippy / check-crate（share、api、scanner） | 通过 |
| test-crate lanrurugi-api | 660 + 1 + 1 + 5 通过 |
| test-crate lanrurugi-scanner | 129 通过，1 失败：老的 `animation::tests::converts_a_real_animated_page`（夹具没挂进容器） |
| test-crate lanrurugi-share / share-proto | 28 / 28 通过 |
| tsc | 通过 |
| eslint | 0 error，4 个旧 warning |
| vitest tests/unit | 379 通过，1 跳过 |
| test-crate storage / search / backup / core | 全部通过（31、71、57、131、3），0 失败 |

## 报告覆盖面

两份报告覆盖的提交：`3ba0ae7` `edfaad0` `6a3eba9` `b866224` `2179afb` `04568b0` `264fd05` `f551e39`，以及 `aadafef` 的一部分。

两份报告都没提到的提交：

- `9e39b22`：source 等效域名、Chaika 补 e-hentai source、快捷条件、统计卡片
- `2f7e4ba`：导入剥掉 date_added，外加启动迁移
- `5784187`：迁移改为手动「整理标签」按钮
- `ae749aa`：整理规则补上空值与大小写
- `657ebf4`：Dockerfile 补 COPY vendor、弯引号、scheme 大小写
- `0796c46`：阅读器返回链接
- `fcd1e29`、`5ad74b0`：i18n 与 UI

## 上一轮（review-result-2026-11-10）各项的状态

| 条目 | 结论 |
|---|---|
| 应修 1：portmap 被一起藏掉 | **已修**。只把选择器换成说明文字，`PORTMAP_FIELD` 无条件渲染。 |
| 应修 2：InFlight 留下垃圾 | **已修**。`content_lock` 按 id 串行，后到的一方重新查到记录后返回 `Unchanged`。新测试覆盖了并发，且断言没有遗留 `.ingest-*`。锁只在进程内有效，对单进程部署够用。 |
| 应修 3：收编失败时回搬 dest | **已修**。回搬已删除；`!recorded` 时才删本次放进去的那份。 |
| 可延后 1：运行时不看平台 | **已修**。`effective_bind_interface` 在 `bind()` 和 watcher 两处都用上了。 |
| 可延后 2：死函数与丢失的 doc | **已修**。 |
| 可延后 3：kill 窗口 | **已修**，启动扫描时会就地改正记录。注释位置有个小问题，见可延后 1。 |
| 可延后 4：回归测试 | **已补**。但 `FAIL_SAVES` 引入了测试间竞态，见应修 7。 |
| 可延后 5：`/api/info` 暴露 OS | **已修**。 |

---

## 阻断

### 1. `crates/lanrurugi-api/src/duplicates/detect.rs:17-24`：阈值 ≥ 8 时退化成全对枚举，大库会把服务器撑爆内存

- **现象**：
  - 前端 `Duplicates.tsx:60` 允许输入 0–16。后端只在大于 16 时拒绝（`mod.rs:57`）。
  - 阈值 ≥ 8 时，`candidates` 会把 n(n−1)/2 个 `(String, String)` 全部放进一个 `HashSet`。每个 id 是 40 个字符。
  - 1 万个档案约 5000 万对，每对约 150 B，合计 7 GB 以上，后面还要对每一对做一次 trigram 计算。
  - 计算在 `spawn_blocking` 里跑，没有上限，也不能取消（报告 §6 FR-013 已承认取消没有接线）。
- **为什么**：用户在页面上把数字改成 8 再点扫描，就可能被 OOM kill。这个仓库因为 OOM 出过好几次事故，规则见 CLAUDE.md。阈值 ≤ 7 时也有类似风险，见应修 1：所有没有封面的档案哈希相同，会在每个 band 里堆出一个巨大的桶。
- **最小验证**：先在单测里构造 3000 个随机哈希的 `ArchiveMeta`，以 `threshold=8` 调用 `build_groups`，观察 RSS（约 4.5M 对，已到 GB 级）。或者在 dev 库上把阈值设为 16 扫一次。
- **修法供参考**：
  - 把阈值上限改成 7，前后端一致。
  - 要支持更大的阈值，就改用更多更窄的 band：16 段 × 4 bit 可以覆盖到 15。
  - 不要把全部候选对先收进集合，改成边生成边过滤。
  - 单个桶超过某个规模时截断，并给出告警。

---

## 应修

### 1. `crates/lanrurugi-api/src/duplicates/cover.rs:73-82`：没有封面的档案统一用占位图算哈希，彼此距离为 0，而且会被永久缓存

- **现象**：
  - `hash_one` 走的是 `archives::share_thumbnail`。按 `archives.rs:2034`，它在读不到、也生成不出缩略图时返回 `Some(PLACEHOLDER_THUMBNAIL)`，从不返回 `None`。
  - 于是 `CoverHashes::missing` 永远是 0。缩略图生成失败的档案（加密、损坏、渲染器缺失等）全都得到同一个哈希。
  - 这个值按档案 id 写进 `LANRURUGI_DUP_COVER_HASH`。注释说「id 不变缓存就不会过期」，但缩略图之后补出来时 id 并没有变。
- **为什么**：
  - 同一作者、页数相近、都没有封面的两本不同作品，元数据门禁会放行，于是被判成「封面 100% 相同」的一组。
  - 推荐逻辑和「删除未勾选」会把用户引向误删。
  - 这些档案还会在每个 band 里堆成一个大桶，放大阻断 1 的问题。
- **最小验证**：取两个同作者、页数相近的档案，删掉各自的缩略图，并让生成失败（例如把文件换成损坏的 zip），然后扫描。它们会成组，`distance=0`。
- **修法供参考**：给 `cover::collect` 一个「拿不到真实缩略图就返回 None」的读取路径（不回退占位图），计入 `missing`，并且不缓存。

### 2. `apps/frontend/src/pages/Duplicates.tsx:291-294`：删除前的引用查询多拼了一层 `/api`，FR-011 实际不起作用

- **现象**：`fetchJson` 自己会加 `/api` 前缀（`api/client.ts:188`），这里传入的却是 `/api/archives/${id}/categories`，实际请求的是 `/api/api/archives/...`，返回 404。`catch` 会吞掉错误，所以确认框里每一行都显示 `标题: ?`。
- **为什么**：spec FR-011 是为了防止误删而设的。现在它在任何情况下都不显示信息，而报告把它标成了 ✅。
- **最小验证**：在重复页点「删除未勾选」，打开网络面板，可以看到两个 404 请求，确认框里只有 `?`。
- **修法**：去掉路径里的 `/api`。

### 3. `apps/frontend/src/pages/Duplicates.tsx:164-172` 与 `review.rs:21,47`：「按建议选择」会把模型没看过的成员标成丢弃

- **现象**：复核只把前 `MAX_MEMBERS = 8` 个成员发给模型，`review.keep` 里只可能出现这 8 个。`onlyRecommended` 把所有不在 `keep` 里的成员都设为 `discard` 并取消勾选，所以第 9 个及以后的成员会被直接标成丢弃，点「删除未勾选」就会被删除。
- **为什么**：模型从没对这些成员下过结论，UI 却把它们表现成「建议删除」。
- **最小验证**：构造一个 10 个成员的组，配置模型后扫描，再点「按建议选择」。第 9、10 个成员会被取消勾选。
- **修法供参考**：只对 `keep ∪ discard` 里出现的 id 应用建议，其余保持原状。

### 4. `crates/lanrurugi-api/src/duplicates/mod.rs:134-141`：「不是重复」按组 key 存储，组成员一变就失效；对 3 人以上的组也没有可见效果

- **现象**：
  - 过滤边时用的是新扫描出来的 `group_key(&group.members)`。只要组里多了一个成员（又下载了一份同作品），key 就变了，原来标记的 A–B「不是重复」就查不到，A 和 B 会重新连在一起。spec FR-008 说的是「按对记录」，现在的实现实际上是按组记录。
  - 前端 `markNotDuplicate` 只提交最相似的那一对（`bestPair`）。在 {A,B,C} 这种三边都成立的组里去掉 A–B，A 和 B 仍然通过 C 连通，下次扫描还是同一个组。
  - 在下次扫描之前，卡片也不会有任何变化。
- **为什么**：用户点了「不是重复」，结果基本看不到效果，或者过一阵子又回来了。
- **最小验证**：标记 {A,B} 不是重复并重新扫描，组消失。再加入一个与 A 相似的 C，重新扫描，{A,B,C} 又出现了，A–B 的边也回来了。
- **修法供参考**：把 not_duplicate 改成全局的按对集合，用排序后的 `(a,b)` 作为 key，与组无关；在 `build_groups` 之前过滤边。「不是重复」按钮的语义要么改成整组互不重复，要么让用户选择是哪一对。

### 5. `crates/lanrurugi-api/src/duplicates/review.rs:36-90` 与 `mod.rs:166-177`：复核缓存的失效逻辑不起作用，本地降级结果在 UI 上也看不到

- **现象**：
  1. `review_cache_key`（带 prompt 版本、模型、成员签名）算出来之后只用来查找，写入时用的始终是普通的 group key，所以这个键永远查不到。缓存实际只按 group key 命中：`DUPLICATE_REVIEW_PROMPT_VERSION` 或 `MODEL` 改了，旧结论也不会失效。报告 §3.7 写的是 sha256 内容键，这与代码不符（代码用的是 blake3，而且这个键没有被使用）。
  2. 本地降级结论在 `run_scan` 里会被跳过、不入库，而 `list_groups` 只读库里的结论。所以没有配置模型的实例看不到任何建议：「按建议选择」按钮不出现，`localOnly` 标记也不出现。报告说的「降级结果返回给 UI」并不成立。
  3. 每组调用两次 `load_all`，都是整个 hash 的 `HGETALL`，组越多越慢（O(N²)）。
- **最小验证**：
  - 第 2 点：不配 key 扫描一次，`GET /database/duplicates/groups` 里每组的 `review` 都是 null。
  - 第 1 点：把 `PROMPT_VERSION` 加 1 后重扫，返回的仍是旧结论，日志里没有模型调用。
- **修法供参考**：写入和查找统一用 `review_cache_key`。降级结论要么在读取时现算，要么带上 `fallback` 标记存入（重扫时覆盖）。`load_all` 在循环外读一次即可。

### 6. `crates/lanrurugi-storage/src/tidy_tags.rs:47-61`：整理标签时用快照整条回写，会覆盖期间别处的修改

- **现象**：先 `list_all()` 取全部记录，然后逐条 `archives.save(&archive)`。`save` 会 `hset_multiple` 全部字段（`repository.rs:258`），包括 `file`、`lastreadpage`、`lastreadtime`、`isnew` 等。大库逐条写要花一段时间，这期间的阅读进度、搬家改写的 `file`、入库补的 `source:` 都会被快照旧值覆盖。
- **为什么**：这正是 c 轮阻断「快照覆盖」那一类问题，`relocate.rs` 当时已改成保存前重读，这里又出现了。
- **最小验证**：在 `tidy_tags` 的循环里，保存前人为加一段 sleep；同时用另一个连接修改同一档案的 `lastreadpage`。tidy 完成后，进度回到了旧值。
- **修法供参考**：保存前重读这一条记录，只替换 `tags`；或者只 `HSET` `tags` 这一个字段。

### 7. `crates/lanrurugi-scanner/src/relocate.rs:346-357`：`FAIL_SAVES` 是进程级 static，会让并行跑的其它测试随机失败

- **现象**：`a_failed_save_puts_the_archive_and_its_patch_back` 在调用 `relocate_library` 前后翻转一个全局 `AtomicBool`。`cargo test` 默认在多线程里并行跑。同模块的 `an_interrupted_relocation_is_repaired_by_the_next_pass` 也会走 `save_record`，如果它刚好落在这个窗口里，修复就会失败，`repaired` 为 0，断言失败。`full_scan` 那几个会触发搬家的测试也一样。
- **最小验证**：用 `cargo test -p lanrurugi-scanner relocate:: -- --test-threads=8` 循环跑几十次，能偶发复现。
- **修法供参考**：把 seam 改成按调用注入（传一个 save 闭包，或用 `ArchiveRepository` 的 test double）；或者让两个测试共用一个 `tokio::sync::Mutex` 串行执行。

### 8. `crates/lanrurugi-search/src/engine.rs`（`tag_index_variants` → `collect_tag_index`，`9e39b22`/`657ebf4`）：每个 `source:` 词要做 6–12 次全库 SCAN

- **现象**：每个变体（3 种 scheme 前缀 × 是否去掉尾斜杠 × 等价 host 数）都单独调用一次 `collect_index_keys`，而它是对整个搜索库 `SCAN MATCH` 一遍。默认的 e-hentai 组合加上尾斜杠，最多会扫 12 遍；`legacy_fallback` 打开时还要再翻一倍。另外，去掉尾斜杠后的 pattern 是原 pattern 的前缀超集，原来那遍其实是多余的。
- **为什么**：`source:` 是订阅去重和用户粘贴 URL 时最常用的查询，标签多的库上延迟会成倍增加。这个改动的提交信息里也没有提到性能。
- **最小验证**：在有大量标签 key 的库上，对比改动前后 `source:e-hentai.org/g/1/abc/` 的查询耗时，并用 `redis-cli MONITOR` 数 SCAN 的轮数。
- **修法供参考**：用一次 `SCAN MATCH INDEX_source:*` 拿到全部 source key，在客户端按变体前缀过滤；或者对完全相等的变体先 `EXISTS` 再 `SMEMBERS`，只在必要时才扫描。

---

## 需要人来决定

1. **`date_added` 保留哪一个**（`2f7e4ba`、`5784187`）：
   - 现在的规则：导入时一律丢弃 LANraragi 的 `date_added`；「整理标签」遇到多个值时保留最新的，也就是本实例扫描时写入的那个。
   - 对从 LANraragi 迁移过来的用户来说，旧值才是「我当年加入这本的时间」，新值只是迁移那天，或文件 mtime（取决于 `use_last_modified`）。按这个规则，整个库的「加入日期」排序和「新档案」都会塌缩到迁移日，而且整理一旦执行就无法恢复。
   - 请确认这个方向是想要的。如果不是，应改成保留最早的值，或者在导入时提供选项。
2. **`token_guest` 可以 GET 新的重复端点**：`route_policy.csv:23` 是 `token_guest, /api/*, GET, allow`，所以只读 token 能拿到全库的分组标题，也能调用 `compare`（会解码两本档案的全部页，CPU 开销重，而且没有并发上限）。这是否符合只读 token 的预期，由你判断。

---

## 可延后

1. **`relocate.rs:341-345`**：插入 `FAIL_SAVES` 时，`move_back` 原来的 doc 注释（「Puts a moved archive …」）留在了 static 上方，现在挂在 `FAIL_SAVES` 上；`move_back` 和 `save_record` 都没有自己的注释。
2. **`relocate.rs:198-236`**：如果在「档案已经搬走、补丁还没搬」之间被 kill，修复只会改正档案记录，补丁会孤零零留在旧目录。概率极低。
3. **`detect.rs:285-296`**：`best_member` 的注释说先比较封面谁更近，但条件 `similarity > 0.0 && distance < 64` 恒为真，实际上只比较页数和大小。
4. **`review.rs:188-210`**：本地规则的 verdict 固定为 `"same_work"`，只依据页数。对「同一作者的上下卷」这种情况，它会建议删掉页数少的那卷。如果按应修 5 让它在 UI 上显示出来，建议改成 `different_versions`，或者不给出删除建议。
5. **`mod.rs:116-180`**：扫描进度在 0.6 之后一直不动，直到所有组都复核完。组多时看起来像卡住了。
6. **`engine.rs` `search_exists`**：固定使用默认的等价组，不读配置。动态分类的成员判定和搜索结果可能不一致。
7. **`settings.rs` 的 `source_equivalences` 注释**：写的是「Blank-line-separated groups」，实际解析是一行一组。
8. **`plugins/metadata/chaika.ts`**：e-hentai 的 source 不受 `addsource` 选项控制，总会被加上。确认这是有意为之。
9. **`pipeline.rs:817/1245`**：`test_pools(24)` 和 `test_pools(26)` 都占用 db 26。这是旧问题，与本轮无关。
10. **`database.rs:527`**：tidy-tags 是同步 HTTP 请求，会遍历全库并逐条写回。大库上可能超过反代超时（CF 是 100 s），而且没有活动记录。
11. **报告与代码不符的地方**：
    - FR-016 说的「专用 Rayon 池」在 duplicates 模块里并不存在。compare 走的是 imgcompare 自带的 `with_thread_pool`，扫描是单线程的 `spawn_blocking`。结论没错，但描述不对。
    - `missing` 计数恒为 0，见应修 1。
    - 缓存键的描述与代码不符，见应修 5。

## 对报告 §7 四个问题的回答

1. **元数据门禁**：
   - 「作者/社团完全不相交就否决」对合志和多人合辑会漏判。一份标了全部作者、另一份只标了社团，就会被否决。016 已经有 anthology 豁免规则，可以复用。
   - 反方向也太松：两边都没有作者标签时，只要标题 Jaccard ≥ 0.55 就放行，再叠加应修 1 的占位图，误报会很明显。
2. **每组一次模型调用**：等应修 5 修好之后，缓存才真正有效，成本才可控。在那之前，每次 prompt 升级后的重扫都等于全量计费。建议照 016 的做法，只复核可见的卡片。
3. **`regroup_by_pairs` 与 group key**：组成员变了 key 就变，这个设计对 keep/discard 可以接受。但对 not_duplicate 不行，见应修 4。我没找到其它按 group key 的引用遗漏：`REVIEW_KEY` 和 `DECISIONS_KEY` 都按 key，旧 key 会成为孤儿，但没有危害。
4. **逐档案两次请求**：现在这两次请求都打到了 404（见应修 2），先修路径再考虑批量端点。语义上逐档案查询是够的。
