# 订阅：插件修正 + 存储最小化 — 完整方案（cc）

- 日期：2026-11-20
- 依据：`temp/subscription-rating-and-history-investigation.md`（dsh 调查报告）及对现有代码的核对
- 目标（用户原话）：**订阅含历史记录在 Redis 空间占用最小化，但订阅效果和性能受到最小影响或不受影响**
- 本文只是方案，代码一行未改

> 本文不含任何真实作品标题、作者或 URL。

---

## 0. 结论

| 部分 | 内容 | 对目标的效果 |
|---|---|---|
| A. 插件 | 保留 dsh 已做的 `gdata` 全量字段修正，补 5 处问题；P0 需要时新增可选的 `refreshSources` 导出 | 判定输入变正确（效果变好） |
| B. 快照 | 加数据版本号，插件数据形状变化时不误报「已变化」 | 消除升级后的通知风暴 |
| C. 重判（P0） | REJECTED 改为带时间戳的 hash、加截止期；按快照里的 `posted_at` 决定是否继续翻页；窗口外作品是否重判由用户决定 | 让 FR-011i 真正生效 |
| D. 快照修剪（P1） | 接上 `delete_snapshot_entries`，按「窗口 + 宽限 + 历史保留」修剪 | 快照有上限 |
| E. 历史存储（P2） | **每作品一份快照 + 判定区间 + 值变化记录；周期只存摘要** | 空间 ≈ 窗口内作品数 × 单条大小，不随轮数增长；历史弹窗的逐轮明细**完整可还原** |

用户这条订阅（约 200 本、按天窗口）：跑满 50 轮时现状约 **5 MB**，本方案约 **190 KB**，且恒定。

与 dsh 报告第 8 节两版方案的差异：

- **v1（摘要 + 事件流水）**：空间相近，但单订阅历史弹窗（`SubscriptionCheckHistoryModal.tsx:105-152`，按轮切换、按判定分组计数和筛选）会失去数据来源。
- **v2（最近 K 轮保留明细）**：改动小，K=3 约 0.35 MB。代价有两个：
  - K 轮以前的轮次没有明细；
  - 最后一次出现在 K 轮以前的作品会从合并历史里消失，被拒的作品没有其他数据兜底（dsh 自己在 8.4 指出）。
  - 按页数窗口的订阅约 2 MB。
- **本方案**：空间约为 v2 的 60%，按页数窗口的订阅约为它的一半。50 轮的明细都能还原，合并历史也不会丢作品。改动面比 v2 大，和 v1 相近。

如果更看重改动小，v2 是合理的折中；按「空间最小、效果不受影响」的目标，本方案更贴近。

---

## A. 插件：`plugins/discovery/ehentai.ts`

### A.1 已由 dsh 完成，保留

- `parseRating`：半星行改为 `y === -21`（8/8 实测对照）。
- `addOriginalTitles` 改名为 `addApiMetadata`。从 `gdata` 补 `tags`（全量，`namespace: 1`）、`rating`（精确均值）、`title`、`pages`（`filecount`）。列表行的值保留作回退。

### A.2 需要补的

1. **`title` → `en` 的映射。** `gdata` 的 `title` 是画廊主标题，通常是罗马字或英文，但不保证是英文。
   - 建议：只在它与 `origin`（列表行显示的那个）不同时才写入。
   - key 用 `en` 还是另起一个（如 `main`），需看宿主 `Titles` 的约定再定。宿主按 `en` 匹配规则的话就保留 `en`，并在注释里写明「主标题，通常为英文/罗马字」。
2. **HTML 实体解码。** `gdata` 的 `title`/`title_jpn` 带转义（如 `&#039;`、`&amp;`），两者都要过一遍 `decodeEntities`。原有的 `title_jpn` 也没过，这次一起修。
3. **0 票画廊的 `rating`（已决定：算「未评定」）。** dsh 实测 0 票时 `gdata` 返回 `"0.00"`，雪碧图为 `-80px -1px`，改前改后都读成 0。
   - API 为 `0` 且雪碧图也是 0 星时，写 `undefined`。宿主按 `UnknownField`（暂不能判定）处理，作品挂起等票，不再按 0 分拒掉。
4. **测试。** 新增 `plugins/discovery/ehentai_test.ts`（`deno test`）：
   - `parseRating` 的 8 组对照；
   - 一份合成的 `gdata` 响应：带转义的标题、空 `rating`、`"0.00"`、非字符串 tag，断言合并后的候选字段。
   - 这样 `export parseRating` 才有用处。`/tmp/check_rating.ts` 不再需要。
5. **`expunged`（已决定：先只记日志）。** 读到 `expunged: true` 时打一条 warn，不改协议。

### A.3 新增：`refreshSources`（可选导出，已决定做，C.3 选 (b)）

用于重判窗口外、或被停止翻页漏掉的待重判作品。只有用户决定「窗口外也要重判」时才需要（见 C.3）。

```ts
/** 按 source 直接取最新元数据，不翻列表。宿主只用它来重判 REJECTED 里的作品。 */
export async function refreshSources(
  sources: string[],
  criteria: DiscoveryCriteriaArgs,
): Promise<{ candidates: DiscoveredCandidateResult[] }>;
```

- e-hentai 的实现：从 source 解析 `gid/token`，复用 `addApiMetadata` 的批量逻辑（25 个一批、并发 4、带登录态）。`category` 和 `posted_at` 取 `gdata` 的值。
- 协议侧（`lanrurugi-plugin::protocol`）：可选能力。插件在 `pluginInfo` 里显式声明 `capabilities: ["refresh_sources"]`，不靠静态扫源码。`data_version` 同样在 `pluginInfo` 里声明。不支持的插件退回 C.2 的翻页方案。
- 宿主侧也要加对应的调用入口（见 C.3）。

---

## B. 快照数据版本：避免升级后的误报

### B.1 问题

`subscriptions/snapshot.rs:106-114` 拿快照里的 tag 集合和标题与本轮比较。插件改动后的第一轮：

- tag 从列表行的 2–3 个变成 API 的约 18 个，所以窗口内每本都判为 `tags_changed`；
- 标题对象新增了 `en`，所以每本都会写入 `title_changed_from`。这个字段目前前端和 API 都不展示，所以只是快照里多了脏数据，不会刷出通知（dsh 指出，已核实）。
- 订阅设为 `RefetchMetadata` 时，会对窗口内每本已入库的档案重跑一遍元数据插件（`runner.rs:1281`），一下子发出大量请求。`diff.changed` 只在这个策略下会被用到，默认的 `NotifyOnly` 不会触发任何动作。这是本节要挡住的主要后果。

### B.2 方案

- 插件在 `pluginInfo` 里声明 `data_version`。本次改动起为 `2`，未声明视为 `1`。
- `SnapshotEntry` 加字段 `data_version: u32`，缺省为 `1`（serde default）。
- `diff`：候选的数据版本与快照条目不同时，**只更新条目，不产生 `ChangedWork`，不写 `title_changed_from`，也不写 E 节的值变化记录**。
- 以后插件再改变字段的来源或形状，只需把版本号加 1。

---

## C. 重判（P0）

### C.1 REJECTED 改为带时间戳的 hash

- 键名：沿用 `…_REJECTED_<id>` 会与旧的 SET 类型冲突，所以新建 `…_REJECTED_AT_<id>`（hash：`source → 首次被拒的毫秒时间`）。
- 迁移：首次读取时，若旧 SET 存在，把所有成员写入新 hash（时间取迁移时刻），再 `DEL` 旧 SET。
- 截止期：`reconsider_days`，默认 7 天，可在订阅设置里调。`rejected_all` 只返回未过期的成员；过期的在每小时的 sweep 里删掉（与 `sweep_skipped` 同一处）。
- 只登记**可能变化**的拒绝，但只能做到单条规则这一级：条件树整组失败时，matcher 报的是 `field_rule:all`/`field_rule:any`，拿不到具体是哪条叶子规则。所以：
  - 只有当拒绝原因全部是不可变字段的单条规则（`FieldRule("category")`、`FieldRule("uploader")`、`ExcludedCategory`）时，才不登记；
  - 组级失败和其他原因照旧登记；
  - `posted_at older_than` 会随时间从假变真，不算不可变字段，照旧登记。
  （dsh 指出组级拿不到叶子规则，我原先的写法过于乐观。）

### C.2 停止翻页：按快照里的发布时间决定是否继续（不改插件）

问题：`runner.rs:763` 在连续 100 条「已见过」后停止翻页，待重判作品如果在后面的页里就取不到。

方案：
- 每轮开始时，从 REJECTED（未过期）× 快照得到窗口内待重判作品里 `posted_at` 的最小值 `oldest_pending`。
- 停止翻页的条件改为：`cache_run >= SCAN_CACHE_RUN` **且** 已读到的最早 `posted_at <= oldest_pending`，或者本轮已取到全部待重判作品。
- 额外翻页有上限：最多翻到 `oldest_pending` 所在那一页为止，实际一般多 1–2 页。
- 没有待重判作品时，行为与现在完全一样。

### C.3 窗口外的待重判作品（需要用户决定）

事实：`runner.rs:166` 的窗口判断在正式检查时不会生效（见该处注释），因为插件自己按 `criteria.posted_after` 停止翻页（`ehentai.ts:482-488`）。所以只改宿主拿不到窗口外的作品。

| 选项 | 行为 | 改动 |
|---|---|---|
| (a) 不重判窗口外作品 | 窗口就是订阅的边界，作品出窗口即失去重判资格，REJECTED 里的对应条目随之清掉 | 只需 C.1 和 C.2 |
| (b) 截止期内仍然重判 | 每轮对「未过期且本轮没取到」的待重判作品调用 A.3 的 `refreshSources`，结果并入候选，走同一个 `decide` | A.3 + 宿主调用入口，判定路径不变 |

(b) 的成本：每轮额外请求数 = ⌈本轮没取到的待重判数 / 25⌉。截止期 7 天时通常只有 1–2 个请求。

(b) 的两个实现要点（dsh 补充）：
1. 刷新结果**不能算作「列表上看到了它」**：快照只更新会变的字段（`rating`/`tags`/`pages`/`title`），不动 `last_seen_at`/`missing_count`/`disappeared_at`，否则下架检测会被污染。E 节的 `runs` 照常记录这一轮的判定，所以历史里能看到重判结果。
2. 只对 REJECTED 里的 source 调用；已处理（queued/approved/discarded/blocked）的作品绝不重新进入 `decide`。

(b) 还能覆盖「老作品被补了 tag，但在列表里不会往前移」的情况，C.2 的翻页方案覆盖不到。

---

## D. 快照修剪（P1）

接上目前没有调用点的 `delete_snapshot_entries`（`storage/subscriptions.rs:1111`）。在 `apply_snapshot_diff` 末尾、并且只在权威轮（`decision.outcome.is_authoritative() && scan_complete`）执行。一个条目**同时满足**以下所有条件才删：

1. 不在窗口内：
   - 按天窗口：`posted_at < window_cutoff`；
   - 按页数窗口：`last_seen_at < now - 宽限期`，宽限期默认等于 3 个检查间隔。
2. 不在 REJECTED 的有效期内（C.1）。
3. 没有被保留的历史轮次引用：其最后一段判定区间的结束轮号 < 仍保留的最早一轮的轮号（E.2）。
4. 不处于「已下架，等待通知」状态：`disappeared_at` 为空，或者已经过了宽限期。

快照中仍在窗口内、但本轮没出现的作品不删，它们是 `missing_count` 下架检测的依据。

---

## E. 历史存储（P2）：判定区间模型

### E.1 原理

现在的空间来自重复：同一本作品在窗口内每轮写一次完整的 `CandidateRecord`。用户实例 18 轮共 2692 条记录，只对应 194 本作品。

改为**每本作品只存一份，历史记成「判定区间」**：

- 每轮有一个递增轮号 `seq`。
- 每本作品记录 `runs: [[verdict, from_seq, to_seq], …]`：
  - 本轮判定与上一段相同，且上一段结束于 `seq - 1`，就把 `to_seq` 改为 `seq`；
  - 否则新开一段（判定变了，或者中间有几轮没取到）。
- 判定区间之外，元数据只存当前值；值变化记在 `changes` 里，用于还原当时的值。

### E.2 数据结构

**`…_SNAPSHOT_<id>`（hash，field = source）**。现有 `SnapshotEntry` 新增：

```rust
/// 新增字段
pub data_version: u32,                 // B 节
pub runs: Vec<VerdictRun>,             // 按 from_seq 升序
pub changes: Vec<ValueChange>,         // 按 seq 升序；只留仍被保留的历史轮次需要的

pub struct VerdictRun {
    pub verdict: CandidateVerdict,     // 原样复用，含 Rejected{reasons} 等载荷
    pub from_seq: u64,
    pub to_seq: u64,
}

pub struct ValueChange {
    pub seq: u64,                      // 从这一轮起为新值
    pub field: ChangedField,           // rating | tags | title | pages | uploader | category
    pub before: serde_json::Value,     // 变化前的值；tags 只存差异 {added, removed}（约 30 B），不存整个数组
}

/// 新增：最近一次判定的时间，合并历史直接用，不依赖可能已被 LTRIM 的摘要（dsh 指出）
pub last_verdict_at: i64,
```

**评分的变化按 0.1 粒度判断**（用户已确认，2026-11-20）：

- 原因：改成 API 精确均值后，新作品投票活跃期间评分几乎每轮都在变，比如 4.37 → 4.41 → 4.38。逐轮记录会让 `changes` 涨到每本每轮一条。
- 规则：快照里额外存一个 `rating_logged`，即最近一次记入 `changes` 时的评分，按 0.1 取整。本轮评分按 0.1 取整后与 `rating_logged` 不同，才追加一条 `ValueChange { field: rating, before: rating_logged }`，并更新 `rating_logged`。
  - 和 `rating_logged` 比，而不是和上一轮比，这样慢慢漂移的累积变化不会被漏掉。
- 快照里的 `rating` 照常存精确的当前值。判定用的始终是本轮候选里的精确值，不受这个粒度影响。
- 还原旧轮次时，显示的评分与当时的真实值最多差 0.05。最新那段直接用精确的当前值。
- 其他字段（tag、标题、页数、上传者、分类）变化很少，照常按值比较。

`CandidateRecord` 里的 `uploader`/`rating`/`posted_at`/`tags`/`title` 在快照里都已有，不再按轮重复存储。

**`…_CYCLES_<id>`（list）**：每轮只存摘要：

```rust
pub struct CheckCycle {
    pub id: String,
    pub seq: u64,                      // 新增
    pub subscription_id: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub outcome: CycleOutcome,
    pub candidates_seen: u32,
    pub verdict_counts: BTreeMap<String, u32>,   // 新增：按判定分组计数，弹窗顶部直接用
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateRecord>,        // 新数据恒为空；仅为读旧数据保留到迁移结束
}
```

保留轮数仍为 `CYCLE_HISTORY_LIMIT = 50`。摘要每条约 150 B，50 轮约 8 KB。

**轮号**：`…_CYCLE_SEQ_<id>`（string）。先单独 `INCR` 拿到本轮轮号，再用一个 `pipe().atomic()` 写快照和摘要。写入失败只浪费一个轮号，无害。MULTI 里拿不到 INCR 的返回值，所以不能把 INCR 放进同一个事务。

**截断**：`runs` 中 `to_seq <` 最早保留轮号的段可以删；`changes` 中 `seq <=` 最早保留轮号的项也可以删，因为没有保留的轮次需要倒推到那么早。

### E.3 读取

| 接口 | 现在 | 本方案 |
|---|---|---|
| `GET /subscriptions/{id}`（`detail`，`api.rs:324`），单订阅弹窗 | `recent_cycles(20)`，每轮带完整候选 | 返回 20 条摘要。另加 `GET /subscriptions/{id}/cycles/{seq}`：`HGETALL` 快照，筛出 `runs` 覆盖该 `seq` 的作品，用 `changes` 倒推元数据到该轮的值，组装成与现在相同的 `CandidateRecord[]` |
| `GET /subscriptions/history`（`combined_history`，`api.rs:480`） | 每个订阅读 20 轮完整记录，在内存里按作品去重 | 每个订阅 `HGETALL` 快照，每本取最后一段区间作为最新判定，`checked_at` 取快照里的 `last_verdict_at`。输出形状不变 |

前端：`types.ts` 的 `CheckCycle` 加 `seq` 和 `verdict_counts`；弹窗选中某一轮时改为请求 `/cycles/{seq}`。界面不变。

### E.4 写入

`build_cycle` 和 `apply_snapshot_diff` 合并为一次快照写入：每个候选的条目更新元数据、追加或延长 `runs`、按需追加 `changes`，然后 `HSET`（现在本来就对所有候选写一次快照）。周期摘要 `LPUSH` + `LTRIM` 不变，只是体积小了。

- 不再序列化整轮候选明细，写入字节数下降一个数量级。
- `runs` 的比较：`CandidateVerdict` 已实现 `PartialEq`，直接比较即可。

### E.5 一致性要点

- **同一轮内的原子性**：快照 `HSET` 与摘要 `LPUSH` 放在同一个 `pipe().atomic()` 里，避免出现「摘要有、区间没有」的轮次。
- **预览不写入**：预览路径（`keep_out_of_window`）不分配轮号，也不写 `runs`。
- **D 节第 3 条**保证仍被保留的轮次引用的作品不会被修剪掉。
- **REJECTED 与 SEEN 不变**，判定路径不读 `runs`/`changes`。

---

## F. 迁移

1. 新代码读 `CYCLES` 时兼容旧形状（`candidates` 非空的旧记录照旧显示，弹窗的单轮接口在 `seq` 缺失时直接返回旧记录里的 `candidates`）。
2. 新轮次只写摘要。旧的完整记录随 `LTRIM` 自然淘汰：50 轮后全部消失，按 6 小时一轮约 12.5 天。
3. 升级时把旧的完整记录截到最近 12 轮（已决定，与第 0 步的过渡上限一致）。
4. REJECTED：见 C.1，首次读取时迁移。
5. 快照：旧条目没有 `runs`/`changes`/`data_version`，靠 serde default 补齐，无需迁移脚本。

---

## G. 空间与性能估算

用户这条订阅：按天窗口，6 小时一轮，窗口内约 150–200 本，全量 tag 后单条约 860 B。

| 组成 | 现状（跑满 50 轮） | 本方案 |
|---|---|---|
| 周期 | ~5.1 MB | ~8 KB（摘要） |
| 快照（含 runs/changes） | 120 KB，无上限 | ~180 KB，有上限 |
| SEEN / REJECTED / 其他 | ~17 KB | ~17 KB |
| **合计** | **~5.2 MB，随轮数增长到上限** | **典型 ~205 KB；最坏约 1–1.5 MB（判定每轮抖动、tag 每轮被改）。都不随轮数增长** |

按页数窗口 10 页的订阅（每轮约 1000 条）：现状约 34 MB，本方案约 1 MB。

| 路径 | 现状 | 本方案 |
|---|---|---|
| 判定（`decide`） | — | 不变（不读历史） |
| 写一轮 | 快照 HSET + 整轮 JSON 序列化 + LPUSH | 快照 HSET（每条多一个小字段）+ 小摘要 |
| 合并历史 | 读约 1 MB / 订阅，解析约 2700 个对象 | 读约 180 KB / 订阅，约 200 个对象 |
| 单订阅弹窗切换轮次 | 一次读 20 轮完整记录 | 摘要 + 按需读一次快照，在内存里筛选 |
| 扫描 | — | 有待重判作品时最多多翻到 `oldest_pending` 那一页，一般 1–2 页；(b) 选项每轮多 1–2 个 `gdata` 请求 |

---

## H. 实施顺序与验收

| 步骤 | 内容 | 验收 |
|---|---|---|
| 0 | 过渡：`CYCLE_HISTORY_LIMIT` 50 → 12（一行改动；插件改动让单条记录变成约 1.7 倍，E 节上线前先压住增长） | 单订阅 `CYCLES` 不超过约 1.2 MB |
| 1 | A.1 + A.2 + B（插件修正与数据版本，一起发） | `deno test` 通过；升级后第一轮在 beta 上 `title_changed_from` 新增数为 0；`RefetchMetadata` 订阅没有批量重新补全元数据 |
| 2 | C.1 + D（REJECTED 改 hash、截止期、快照修剪） | `REJECTED_AT` 长度不再单调增长；快照条目数 ≈ 窗口内作品数 |
| 3 | C.2（按 `posted_at` 决定是否继续翻页） | 构造一本在第 2 页的待重判作品，每轮都被重判；没有待重判作品时 `pages_read` 与改前相同 |
| 3' | C.3 (b)（若选择）：A.3 + 宿主调用 | 窗口外的待重判作品在截止期内每轮被重判；额外请求数符合估算 |
| 4 | E + F（判定区间模型） | 单订阅 Redis 跑满 50 轮后稳定在约 200 KB；弹窗任一轮的明细与改前同一轮逐条一致（**自动化断言**：同一轮旧记录 == 从 runs/changes 还原的结果，含变化恰好落在最老保留轮的边界情况）；`combined_history` 输出与改前相同 |

测试要点（按仓库规则只跑涉及的 crate，用 `mise run check-crate`）：
- `snapshot.rs`：数据版本不同时不报变化；`runs` 的延长、断开、新开；`changes` 倒推。
- `storage/subscriptions.rs`：REJECTED 迁移与截止期；快照修剪的四个条件。
- `runner.rs`：新的停止翻页条件。
- `api.rs`：`/cycles/{seq}` 还原结果与旧形状一致。

---

## I. 用户决定（2026-11-20）

1. C.3：选 **(b)**，截止期内的窗口外待重判作品也重判。
2. 截止期：7 天。
3. `expunged`：先只记日志。
4. 升级时把旧完整周期截到 12 轮。
5. 0 票：算「未评定」。UI 评分条件旁加提示「按站点平均分；0 票＝未评定」。
6. 评分变化记录按 0.1 粒度判断。
7. 实施：cc 来做。
