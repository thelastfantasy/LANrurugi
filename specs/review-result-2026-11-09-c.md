# 复查结果（2026-11-10）：对 review-request-2026-11-09-c 的回答

分支 `feat/share-bind-interface`，HEAD `501d8cb`。只报告，未改代码、未提交、未开 issue。

## 门禁

| 门禁 | 结果 |
|---|---|
| `fmt-check` / `clippy` | 通过 |
| `check-crate -- lanrurugi-share lanrurugi-api lanrurugi-scanner` | 通过 |
| `test-crate`（REDIS=16380）lanrurugi-api | 645 + 1 + 1 + 5 全过 |
| lanrurugi-share / share-proto | 25 / 28 等全过 |
| lanrurugi-scanner | 127 过，1 败：`animation::tests::converts_a_real_animated_page`（`animation.rs:1243`，环境变量指向的文件不在容器里，与本 PR 无关） |
| `tsc` | 通过 |
| `eslint` | 0 error，4 个旧 warning |

门禁全绿，但这次的阻断 1 正好是测试本身被改成了断言错误的行为，所以门禁过了不代表没有问题。日志：`/tmp/claude-1000/-home-jade/705ae16c-b227-403c-a142-33611c939ef8/scratchpad/gates3.log`

## 上次五条阻断的复核

| 阻断 | 结论 |
|---|---|
| 绑接口没接线 | **Linux 上已接**（`crates/lanrurugi-share/src/net.rs:173-174`，在 `builder.bind()` 之前，取消固定时传 `None`）。macOS 见下面应修 1。 |
| 来宾拿整本签名链接 | **已封**（`route_policy.csv:183`）。缺回归测试，见可延后 1。 |
| 搬家非原子 | **只修了一半**，见阻断 2。 |
| 快照覆盖 | **已修**（`relocate.rs:199-206` 重读后只改 `file`）。剩一个记录被删时的边角，见可延后 3。 |
| 入库删唯一字节 | **主路径已修**。并发和收编两处还有残留，见应修 2、应修 3。 |

---

## 阻断级

### 1. `crates/lanrurugi-api/src/share_net/import.rs:1017`：87db3af 把「传输中断」也判成了终态

- **现象**：`terminal_import_error` 现在把所有 `ShareNetError::Protocol(_)` 都当终态。但 `Protocol` 不只表示「对端说了读不懂的东西」，它也装着纯传输错误：
  - `share_net/mod.rs:596/599/605/609`：`read_frame` 的 `read_exact` 失败（连接断开、流被 reset）经 `protocol_err` 映射成 `Protocol`。
  - `mod.rs:619-622`：`write_frame` 的 `write_all` 失败，同样映射成 `Protocol`。
  - `mod.rs:636`：`send.finish()` 失败，同样映射成 `Protocol`。
  - `mod.rs:670-676`：`read_body` 的 `read_to_end` 失败，以及中途断开产生的 `"truncated body"`，都是 `Protocol`。

  因此在 `fetch_piece`（`import.rs:536-558`）里，只要 Range 的响应头已经收到、body 还在传，此时对端掉线、中继切换或 VPN 重新换钥，这个分块就直接返回 `Protocol`。随后走到 `import.rs:955`，再到 `(Err((true, e)), Source::Sharer)`，整个档案判为 `Failed`。另外，`reopen_sharer` 之后的第二次 `call(...)?` 如果在 `read_frame` 阶段断开，结果也一样。
- **为什么**：这正是 022 US2 要消除的行为，也就是「网络错误是等待，不是失败」。代码内部也有自相矛盾的地方：`import.rs:412-417` 的注释明确说「`Protocol` 放在这里是故意的：回答的多半是半重建的端点」，并把它当作可重试。同一个错误类型，两个地方给了相反的判定。修改后的测试（`import.rs:1904`）只验证了映射表本身，没有覆盖「`Protocol` 里有哪些其实是传输错误」。
- **最小验证**：
  - 单元级：对 `read_body` 喂一个先宣布 `len=N`、只写 N/2 字节就 `finish` 的流，确认得到 `Protocol("truncated body")`，而 `terminal_import_error` 对它返回 `true`。
  - 真机：导入一个大档案，传输过程中在分享方执行 `ip link set <tun> down` 并保持 5 秒以上，导入应当进入等待，现在会变成失败。
  - 修法供参考：要么把 I/O 类错误（`read_exact`、`write_all`、`finish`、`read_to_end`、truncated）改映射为 `Unreachable`，只让解码失败和意外的响应类型留在 `Protocol`；要么在 `terminal_import_error` 里不要整体匹配 `Protocol`。

### 2. `crates/lanrurugi-scanner/src/relocate.rs:141` + `:218`：保存失败时的回滚可能失败，留下「记录指向空气」

- **现象**：`relocate_file` 在返回 `Moved` 之前就执行了 `prune_empty_parents(library, path)`（`:141`），把原目录删空了。之后如果 `archives.save` 失败，回滚用的是裸 `tokio::fs::rename(&dest, &path)`（`:218`），会遇到三个问题：
  - 原父目录已经被 prune，rename 返回 ENOENT，回滚失败。
  - 跨文件系统（EXDEV）时，裸 `rename` 必然失败，因为这里没有走 `move_file`。
  - 档案自己的 `.patch.zip` 已经跟着搬进 `_byname/`，回滚时没有搬回来。

  回滚失败时只打一条 warn，`report.failed += 1`。这时记录仍然指向旧路径，而文件在 `_byname/`。这正是上次阻断 3 描述的状态：注释（`:211-215`）自己也写了「nothing later repairs」，两次 Clean Database 之后元数据就丢了。
- **为什么**：最常见的形态是平铺书库里一个子目录只放了一本，这时首次搬家就会触发 prune。回滚恰恰是为保存失败准备的，而它在这种形态下不起作用。另外，进程在「move 完成」和「save 完成」之间被 kill 的窗口依然存在，启动时也没有对应的修复。
- **最小验证**：
  - 构造 `lib/a/x.zip`，`a/` 下只有这一个文件，并让 `archives.save` 返回错误（可以注入一个失败的 repository，或在 move 之后断开 Redis）。
  - 跑一遍 `relocate_library`，断言 `lib/a/x.zip` 存在。现在会失败：`lib/a/` 已经不存在，文件留在 `lib/_byname/a/x.zip`。
  - 修法供参考：把 prune 推迟到保存成功之后；回滚用 `create_dir_all(parent)` 加 `move_file`，并把 patch 一起搬回。

---

## 应修

### 1. `crates/lanrurugi-core/src/http_client.rs:91` + `vendor/iroh-relay/src/client/tls.rs:70` + `vendor/netwatch/src/udp.rs:43`：macOS 上会接受固定接口，但分享 socket 实际不会被钉住，中继 TCP 还会必然失败

- **现象**：
  - `interface_binding_supported()` 对 macOS、iOS 等平台返回 true，因为 reqwest 在这些平台上用 `IP_BOUND_IF`，插件出口确实能绑接口。`share_settings.rs:199` 复用了同一个判定，所以 macOS 上 `share_bind_interface` 会被接受。
  - 但两个 fork 的钉接口逻辑只在 Linux 上生效：netwatch 的 `bind_to_device` 在非 Linux 上是空操作（`udp.rs:43-46`）；iroh-relay 的 `connect_bound` 在非 Linux 上跳过 setsockopt，但仍然走非阻塞 connect。
  - 更糟的是，`tls.rs:70` 仍然是字面量 `Some(115)`。上次的 `expected_in_progress()` 在 10ca5af 加入时从未被调用，又在 2952181 被当作死代码删掉了。于是 macOS 和 BSD 上的 connect 拿到 EINPROGRESS=36，被当成错误，中继每次都连不上。
  - `vendor/iroh-relay/FORK.md` 仍然写着「`EINPROGRESS` comes from `expected_in_progress()`」，与代码不符。
- **为什么**：015 把 macOS arm64 的原生包列为发行目标。这里的后果是「设置被接受、显示已固定，但 UDP 没被钉住、中继全断」，正是 `interface_binding_supported` 注释里说的「quietly does nothing is worse than a refusal」。
- **最小验证**：
  - 在 macOS CI 或本机上设置 `share_bind_interface=utun3`，观察中继连接日志里出现 `os error 36`。
  - 代码层面：`grep -n "115" vendor/iroh-relay/src/client/tls.rs`，再对照 FORK.md。
  - 修法供参考：为分享单独设一个 `share_binding_supported()`，目前只返回 Linux；或者在 macOS 上实现 `IP_BOUND_IF`，同时把 EINPROGRESS 改成平台常量。

### 2. `crates/lanrurugi-api/src/download_manager/ingest.rs:350` + `:389`：并发入库时，「dest 已存在 → 当作已入库」会和另一方的失败清理叠加，导致字节和记录都没有

- **现象**：
  - 入库 A 已经把字节 rename 到 `dest`，但还没写记录。入库 B 看到 `dest` 存在，删掉自己的 `incoming`，返回 `Ok(is_new: false)`（`:350-356`）。
  - 接着 A 的 `ingest_file_with_policy` 失败（解析失败或 Redis 抖动），在 `:389` 查到没有记录，于是 `remove_file(&dest)`。
  - 结果是 B 的队列项显示完成，指向一个不存在的 archive id，两份字节都没了。
- **为什么**：这仍然是「删唯一字节」的同一类问题，只是从单路径挪到了并发路径。触发需要「同一份字节同时入库两次，并且先到的那次失败」。重复订阅、或者用户把同一个 URL 加了两次，都能做到这一点。
- **最小验证**：用两个 task 同时对同一份 staging 内容调用 `catalogue_staged_file`，并让第一个在 `ingest_file_with_policy` 中返回 Err（例如传一个解析会失败的 zip），断言最终至少有一份字节或一条记录。现在两者都不存在。修法供参考：B 在 `dest` 已存在时不要删 `incoming`，而是等待或返回可重试错误；或者 A 的清理条件改成「无记录且不是我独占的副本」。

### 3. `crates/lanrurugi-api/src/download_manager/ingest.rs:118-142` + `:389`：启动收编失败时，pre-020 暂存的唯一一份下载会被删除，指针也被清空

- **现象**：
  - `adopt_pending_conflict` 调用 `catalogue_staged_file`。后者先把 `temp_path` 搬成 `.ingest-*`，再 rename 到 `dest`。
  - 如果 `ingest_file_with_policy` 失败，并且还没有写下记录，`:389` 会删除 `dest`。
  - `main.rs:678` 无论成败都执行 `item.pending_filename_conflict = None`，失败时把状态置为 `Error`。

  这份字节原本是 pre-020 版本特意保留下来、等用户决定的下载。现在它被删掉，队列项上也不再记录它在哪里。
- **为什么**：普通下载失败时删除是合理的，因为还可以重下。但收编的对象是用户已经下载完、正在等待处理的数据。启动阶段 Redis 刚就绪就抖动一次，就会永久丢失。
- **最小验证**：在 `a_legacy_pending_conflict_is_adopted_by_content_once` 的基础上，staged 一个会让 `ingest_file_with_policy` 返回 Err 的文件（例如损坏的 zip），断言调用后 `temp_path` 或 `dest` 至少存在一个。现在两个都不存在。修法供参考：收编走一个「失败时把字节放回 `temp_path`」的变体，或者失败时保留 `pending_filename_conflict`。

### 4. `crates/lanrurugi-api/src/archives.rs:2016`：分享缩略图的格式实际变成了「永远 JPEG 并按需重生成」，与复查请求里的描述相反

- **现象**：
  - 旧路径（17a8d6e 之前）用空 `HeaderMap` 调用 `get_archive_thumbnail`，会走 `ClientImageSupport::from_request`。在无 `Accept`、无 cookie 时，它的兜底是 `Self::webp()`（`archives.rs:3009`），所以会返回已存的 WebP。
  - 新路径用的是 `ClientImageSupport::source()`，其中 jxl、webp、avif 全为 false。配合默认配置 `jxlthumbpages=1`（格式为 Jxl），`desired_thumbnail_format` 落到 `_ => Jpeg`。
  - `read_thumbnail_from_disk` 也只探测 Jpeg（`thumbnail_formats_for`，`:2139`）。

  结果是：分享出去的每个档案封面第一次被请求时，都会在分享方重新生成一份 JPEG，写进 `thumb_dir`，然后传出 JPEG。
- **为什么**：
  1. 023 声称「行为不变」，但这里确实变了。
  2. 复查请求第 3 节写的是「只支持 WebP 的客户端会拿到原格式」。真实的变化是：以前对端拿到的是 WebP，现在拿到的是新生成的 JPEG。旧路径同样没有 `Accept`，所以「不再协商 Accept」并不是这次的变化点。
  3. 远端浏览一次分享，就会触发分享方为整份清单重算一遍 JPEG，并占用额外磁盘。另外 `share_thumbnail` 的 doc 写着「the same path a headerless browser request would」，与实际不符。
- **最小验证**：用默认缩略图设置建一个分享，从另一个节点浏览，检查 `thumb_dir/<ab>/` 下是否新增了 `.jpg`，并查看返回的 `content_type`。修法供参考：把 `source()` 换成 `ClientImageSupport::webp()`，即可恢复旧行为。

### 5. `crates/lanrurugi-scanner/src/relocate.rs:138` + `full_scan.rs:390-391`：搬家汇总的口径（回答复查请求第 4 问）

- **现象**：
  - 「目标被占」没有被漏算成正常，它在 `skipped_occupied` 里。但 `relocation_skipped` 是 occupied、settling、outside 三者之和，而字段注释（`full_scan.rs:120-121`）仍然写着「destination already held a different file」。
  - API 消费者无法区分三种情况：需要人处理的冲突、下一轮会自己消失的 settling、以及永久存在的 outside。outside 每次扫描都会计入，所以导入了外部挂载的书库，`relocation_skipped` 永远不会归零。
  - patch 搬不动时（`relocate.rs:138`），也返回 `Occupied`，被计入 `skipped_occupied`，日志打的是「`_byname` already holds a file」，实际原因是 patch I/O 失败，应该算 `failed`。
  - 汇总日志（`full_scan.rs:393-398`）不输出 settling 和 outside。
- **为什么**：五态拆开的目的是让人看懂报告，但汇总这一层又把它们合了回去。
- **最小验证**：在 `database.rs:754` 的响应里分别暴露三个计数（可以保留 `relocation_skipped` 作为兼容字段）。另外给 patch 失败加一个独立的 outcome，或者改为返回 `Err`，并补一个单测：patch 的目标目录不可写时，计数应落在 `failed`。

---

## 可延后

1. **`crates/lanrurugi-api/src/authz.rs:520`**：`tankoubon_download_is_admin_only` 只测了 `/download`，没测 `/download-link`。阻断项 2 的修复目前只有 CSV 一行，没有回归保护。在同一个测试里加上 `"/api/tankoubons/TANK_1/download-link"` 的五个角色断言即可。
2. **`relocate.rs:110-120`**：`_byname` 里已经有同名 patch 时，档案照样搬过去，于是档案会认领一个不属于它的 patch，自己原来的 patch 留在原地，pair 拆散。
   - 这和 `:121-124` 注释里「pair 拆散是永久的」那条逻辑相互矛盾。
   - 建议把这种情况也按 `Occupied` 处理，不搬档案。
3. **`relocate.rs:205`**：`Ok(None) => archives.save(&archive)`。搬家过程中记录被并发删除时，会用旧快照把它复活。更稳妥的做法是把文件搬回原处，或者就地留下，交给扫描处理。
4. **`ingest.rs:409-416`**：注释说「`dest` is that archive's file」。只有当 Rejected 来自竞态时，`existing.file` 才可能不是 `dest`，例如同 id 的旧档案在 `_byname/` 里。这时 `dest` 会成为一份写进 FILEMAP 的冗余副本，留在哈希布局里，永远不会被清理。另外 id 由「前 512000 字节加文件大小」算出，所以「exact bytes」这个说法本身也不严格。两点都属于罕见情况，改注释或加一个比对都可以。
5. **`share_net/import.rs:46`**：复查请求说已经「明确成 `blobs::CHUNK_BYTES`」，但 import.rs 仍然有自己的 `const CHUNK_BYTES: u64 = 1024`，两处各自定义。建议改为引用 `lanrurugi_share::blobs::CHUNK_BYTES`。
6. **doc 注释错位**：新插入的项目放在了别人的 doc 注释和它所属的 fn 之间，rustdoc 会把两段拼在一起。共四处：
   - `http_client.rs:85-91`：`bind_outbound` 的 doc 跑到了 `interface_binding_supported` 上。
   - `blobs.rs:253-256`：`MAX_RANGE_BYTES` 的 doc 跑到了 `CHUNK_BYTES` 上。
   - `archives.rs:1996-2008`：`thumbnail_disk_path` 的 doc 并进了 `share_thumbnail`。
   - `relocate.rs:68-76`：`relocate_file` 的 doc 跑到了 `SETTLE` 上，而且里面「`Ok(None)` means…」已经不符合现在的元组返回值。
7. **`vendor/iroh-relay/src/client.rs:63-65`**：上游是 `#[cfg(not(wasm_browser))] mod tls;` 和 `#[cfg(not(wasm_browser))] mod util;` 两行都有 cfg。fork 现在只有 `tls` 带 cfg，`util` 的 cfg 丢了。FORK.md 说「the same attribute stays on `mod tls;` where upstream had it」，与事实不符。这只影响 wasm，但和「除列出的改动外逐字拷贝」的承诺不一致。
8. **`lanrurugi-share-proto/src/record.rs:172`**：钉字节的测试保护的是 knock 记录，但 023 并没有动 `share-proto`。023 实际搬动的是计算这些值的逻辑：manifest 指纹、via 码地址筛选、中继 URL 计算，而不是编码层。所以「这是 023 搬迁不改线格式唯一的自动化保证」这个说法偏乐观。
   - 真正的回归风险在语义，不在编码。
   - 如果确实要钉编码，`Request`、`Response`、`ShareManifest` 比 knock 更该钉。
   - 要防语义回归，可以给 `browse_manifest` 和 `relay_urls` 各加一个固定输入、固定输出的快照。
9. **`blobs.rs:263-283`**：`MAX_RANGE_BYTES` 按请求的 1 KiB 块计数，但 bao 编码按 16 KiB 的 chunk group 外扩。64 个零散的小 span，实际发出的字节可能比上限多出约 2 MiB。这只是上限不精确，不是错误。

---

## 对复查请求第 3 节五问的直接回答

1. **Range 单位**：一致，已经对着源码验证过。`bao-tree 0.17.0` 中，`ChunkNum::to_bytes` 为 `self.0 * 1024`（`tree.rs:233`），`ChunkNum::chunks(size)` 为 `ceil(size / 1024)`（`tree.rs:216`）。`BlockSize::from_chunk_log(4)` 只决定哈希分组。所以协议 span 和 `ChunkNum` 都以 1 KiB 计。剩下的问题见可延后 5 和 9。
2. **`admit_upload`**：没有应当持有的锁。设置每次重新读取，本来就是快照语义。「检查加插入」在 `limits::admit` 的同一把 `ACTIVE` mutex 内完成，是原子的。默认方法与之前 api 侧的实现等价，同样是 `share_settings::load` 加 `timezone`。
3. **thumbnail**：确实有行为变化，但方向和请求里描述的不同，见应修 4。
4. **搬家五态**：「目标被占」没有被漏算，但口径混在一起，patch 失败也被算成了 occupied，见应修 5。
5. **CF 最近一跳**：在以下部署下推演，规则都成立：
   - 多层 CF：XFF 的最右一跳是 edge，或者 peer 本身就是 edge。
   - 「CF → 自建反代 → 源站」：前提是中间反代在 `share_trusted_proxies` 里。不在的话，会把反代 IP 当成所有访客的地址。这属于配置错误，但值得在设置说明里写一句。
   - cloudflared 隧道：XFF 里没有 edge，会落到 XFF 的最右一项，结果仍然正确。
   - 参考 nginx 配置：用 `real_ip` 改写之后覆盖 XFF，结果仍然正确。

   建议把「CF → 受信反代 → 本机」和「中间反代未受信」这两种情况补成测试，两者现在都没有覆盖。

## 第 1 节「仍未做」两项是否阻塞

- **半截下载收编**：不阻塞，前提是半截文件仍然留在原位，不会被启动清理删掉。需要确认 `sweep_stale_ingest_copies` 只匹配书库根目录下的 `.ingest-*`（`ingest.rs` 文末的实现确实如此），不会碰 temp_dir 里的半截下载。
- **`original_path` 填充**：不阻塞。重建时，标题恢复依赖的是 `original_filename`，这个字段已经填了，`pipeline.rs:633` 也会优先沿用旁车里的值。`original_path` 目前只是信息字段。
