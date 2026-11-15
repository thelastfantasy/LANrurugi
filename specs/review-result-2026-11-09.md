# PR #258 Review 结果（`feat/share-bind-interface`，对照 `origin/dev`，2026-11-09）

依据：`specs/review-prompt-2026-11-09.md`。只报告，未改代码、未开 issue。

结论：有 **5 条阻断级**问题，建议先修再合并。最严重的是 022 的接口绑定在代码里根本没接上，以及 021 的只读 token 能绕过整本下载限制。

## 门禁结果

| 项目 | 结果 |
|---|---|
| fmt-check / clippy / check-crate | ✅ 通过 |
| test-crate：lanrurugi-api | ✅ 650 通过 |
| test-crate：lanrurugi-storage 等 | ✅ 全部通过 |
| test-crate：lanrurugi-scanner | ❌ 127 通过、1 失败 |
| tsc | ✅ 通过 |
| eslint | ✅ 0 错误，4 个旧警告 |

- **scanner 那条失败与本 PR 无关**：失败的是 `animation::tests::converts_a_real_animated_page`。环境变量 `LANRURUGI_TEST_ANIMATED_ARCHIVE_PATH` 设了，但它指向的文件没挂进容器（`NotFound`）。本 PR 没改 `animation.rs`。
- **门禁没覆盖到这次的问题**：下面所有阻断项，现有测试都测不出来。

---

## 阻断级（会丢数据 / 越权 / 泄露）

**1. 022：`SO_BINDTODEVICE` 从来没生效**
- **位置**：`crates/lanrurugi-api/src/share_net/mod.rs:647`（`bind()`）；`specs/022-share-network-hardening/tasks.md:60`
- **现象**：两个 fork 都加了 `set_bind_device`，但整个 `crates/` 里没有任何地方调用它。`git log -S set_bind_device -- crates` 也是空的。
- **为什么是问题**：T008 打了勾，两份 FORK.md 都写着"`share_net::bind` 每次（重）绑时调用"，这是"写了但代码没做到"。结果是绑定状态永远是 `None`：中继的 TCP 和 iroh 的 UDP 都没有钉在接口上，物理地址照样会泄露。US1 实际上没有实现。
- **最小验证**：`grep -rn set_bind_device crates`，结果为空。
- **另外**："两个端点交替 bind 会不会串味"，因为根本没调用，目前不会发生。但一旦接上，`start()` 里旧端点 `close()` 之后还会被借用，旧端点的 rebind 就会读到新的全局值。接上时要一起处理。

**2. 021：只读 token 能绕过 FR-040，下载整本合集**
- **位置**：`crates/lanrurugi-api/policy/route_policy.csv:179`；`crates/lanrurugi-api/src/tankoubons.rs:186`
- **现象**：策略只拒绝了 `token_guest` 访问 `/api/tankoubons/:id/download`。但 `/api/tankoubons/:id/download-link` 是 GET 请求，会被 `/api/*, GET, allow` 放行，返回一个签名的 `/api/dl/tankoubons/{id}` 链接。而 `/api/dl/*` 对 `token_guest` 和匿名用户都放行。
- **为什么是问题**：只要配置了 `direct_download_url`，只读 token 就能下载整本合集。
- **最小验证**：用只读 token 调 `GET /api/tankoubons/<id>/download-link`，再 curl 返回的 URL，能拿到 200。
- **修法**：在策略里给 `token_guest` 加 `/api/tankoubons/:id/download-link, GET, deny`。

**3. 020：搬家时，文件和记录之间不是原子的**
- **位置**：`crates/lanrurugi-scanner/src/relocate.rs:78`、`:127-134`
- **现象**：顺序是先 `move_file`，再 `archives.save`。如果 `save` 失败，代码只计数、不回滚；如果进程在两步之间被杀，同样会落在这个状态。
- **之后会发生什么**：
  - 下一次扫描在 `_byname/` 里看到这个文件，计算出的 id 已经有记录，于是走 `Rejected(ContentHash)`（`pipeline.rs:323-340`）。这一步只补了 FILEMAP，没有修正 `archive.file`。
  - 搬家这一遍会把它当成"记录没有文件"跳过（`relocate.rs:116`）。
  - 结果是记录永久指向一个不存在的路径，阅读器打不开这本档案。
  - 用户之后连点两次 Clean Database（`database.rs:545-574`），第一次把 `file` 清空，第二次用 `PERMANENT` 清除记录，标签、分类、阅读进度全部丢失。
- **FILEMAP 旧键残留**：在这个状态下，旧键在旧路径上还会留着。如果之后有一个不同的文件落到这个旧路径，`rename_id` 会把这条档案的元数据嫁接到新文件上。
- **最小验证**：在测试里让 `archives.save` 失败（或在 move 之后直接 return），然后跑 `full_scan` 两次，检查 `archive.file` 是否存在。

**4. 020：搬家用的是一开始的快照，会整条覆盖并发修改**
- **位置**：`crates/lanrurugi-scanner/src/relocate.rs:103`、`:127`
- **现象**：开头一次性 `list_all()` 拿到所有记录，之后对每条做全字段 `HSET`（`repository.rs:231`）。
- **为什么是问题**：首次升级会搬整个库，跨设备时要整份拷贝，这个过程可能持续几个小时。期间用户改的标签、标题，以及阅读进度，都会被这份旧快照静默覆盖。
- **修法**：只写 `file` 一个字段，或者在 save 前重新 `get` 一次。

**5. 020：下载入库的失败路径会删掉唯一一份字节**
- **位置**：`crates/lanrurugi-api/src/download_manager/ingest.rs:292`、`:296-299`、`:346-348`、`:373`、`:393`
- **起因**：`rename_or_copy` 会删掉源文件（`download_manager/mod.rs:328`），所以 `.ingest-*` 和 `dest` 就是唯一的副本。下面几条路径都会删掉它：
  - (a) `ingest_file_with_policy` 返回 `Err` 时会 `remove_file(dest)`。但这时记录可能已经写入了：`catalogue_new_archive` 先 save，后面的缩略图 save（`pipeline.rs:680`）或 FILEMAP `hset`（`:390`）用 `?` 失败，就会留下一条记录指向已删除的文件。
  - (b) 并发竞态：同一份内容同时入库两次（重复入队、上传和下载同时发生、分享导入和下载同时发生），两次都算出同一个 `dest`。后到的那次被 `Rejected` 后会 `remove_file(dest)`，删掉的正是先到那次刚编目好的档案。
  - (c) 启动时收编旧"等待改名"队列项的那一块（`crates/lanrurugi-server/src/main.rs:666`、`:679`）：先清掉 `pending_filename_conflict`，失败时再标成 Error。启动瞬间如果 Redis 抖一下，暂存字节就被删了，用户也没法重试。
- **最小验证**：
  - 对 (b)：用同一份 zip 并发调用两次 `catalogue_staged_file`，最后检查 `dest` 是否还在。
  - 对 (a)：在缩略图生成之后注入一个 Redis 错误。

## 应修（行为与 spec 不符 / 边界没处理）

**020**
- **旁车里的原文件名，重建一次就被覆盖了**
  - 位置：`crates/lanrurugi-scanner/src/pipeline.rs:629-636`
  - 重建时 `title_filename` 是 `None`，重写旁车时 `original_filename` 会变成 `path.file_name()`，也就是 `<id>.zip`。第一次重建正常，第二次就丢了原名。这直接违反"它是唯一一份原名"的设计。
  - 现有测试（`full_scan.rs:1017`）没有在扫描后重新读旁车。最小验证：扫描后加一句 `assert_eq!(sidecar::read(..).original_filename, "[Group] The Book [DL版].zip")`，它会失败。
  - 修法：读到旁车时，沿用旁车里的 `original_filename`。
- **旁车写失败只 `warn!`**：会永久分叉。`sidecar_audit.rs` 只报告、不重写，而 spec 第 134 行要求"reported and regenerated"。另外，用户在界面上改了标题，旁车也不会跟着更新。
- **FR-009 / FR-014 没做到**
  - `original_path` 永远是空字符串（`sidecar.rs:63`）。
  - 搬家成功时不记录 源→目标 的日志（`relocate.rs:140` 只计数）。
  - `_byname` 没有旁车，所以"从哪搬来的"这条信息根本没地方存。
  - Implementation Record 却写着"relative path live in the sidecar"。
- **EXDEV 回退不是"安全可中断"（FR-013）**
  - 位置：`relocate.rs:167-173`
  - 它直接 copy 到最终名字。copy 中途被杀，`_byname/` 里就会留下半截文件，源文件还在原处。之后每次都会被判成"目标已占"而跳过，而且这个半截文件会被扫描当成一个新档案编目。
  - 修法：先拷到临时名，fsync 之后再 rename。
- **补丁搬失败以后没人再管**
  - 位置：`relocate.rs:84-92`
  - "档案在 `_byname/`、补丁在原地"这种状态确实会出现。档案已经在 `_byname` 里了，以后不会再被处理；补丁因为 `is_patch_path` 被扫描器跳过。`has_patch` 一直是 true，但补丁永远找不到。
  - 另外，目标位置如果已经有补丁，`rename` 会直接覆盖掉它。
- **`Ok(None)` 把三种情况混在了一起**
  - 位置：`relocate.rs:58-74`、`:142-147`
  - "还没静默满 60 秒"、"目标已占"、"路径不在库目录下"（例如从 legacy 导入的路径，`strip_prefix` 失败）都返回 `Ok(None)`，都计入 `skipped_occupied`，日志都写"`_byname` 已有同名文件"。legacy 路径会每次扫描都报一遍。
- **`.ingest-<uuid>` 孤儿文件没人清理**
  - 位置：`ingest.rs:290`
  - 第二次 rename 之前崩溃，这个点开头、没有扩展名的文件就会永久留在库根目录，扫描器不会去碰它。
- **"覆盖重复"选项已经没有作用**
  - 位置：`ingest.rs:206-213`、`:634`
  - `intended_filename: None` 之后，按文件名冲突的分支永远走不到；内容相同的重复本来就从不覆盖。所以 `overwrite_on_duplicate` / `replacedupe` 和 `resolve_overwrite` 的文档描述都失效了，但前端（`UploadPage.tsx:177-188`）还在把这个选项传给后端。
- **半截下载被收编时会被当成好档案**
  - 位置：`main.rs:652` 附近
  - 收编时不做完整性校验。`list_pages` 失败时按 0 页编目（`pipeline.rs:557`），这条队列项还会被标成 Done。

**021**
- **下载域的限制是"失败即放行"**
  - 位置：`crates/lanrurugi-api/src/download_host.rs:42-49`
  - 只有 `Host` 头和配置的下载域完全一致时才拦截。下面几种情况下，这个限制形同虚设，整个应用都能经由下载域访问：
    - 反代改写了 Host（nginx 默认是 `$proxy_host`）；
    - HTTP/2 请求只带 `:authority`、不带 `Host` 头；
    - 中间有 `X-Forwarded-Host`。
  - `..`、大小写、百分号编码、双斜杠：核对过，这几种都绕不过去，测试也覆盖了。
- **FR-008：应用层对下载域没有做任何不信任处理**
  - 位置：`crates/lanrurugi-api/src/shares.rs:289`
  - 判断"Cloudflare 在链路上"用的是 `hops.iter().any(is_cloudflare_edge)`，而 `X-Forwarded-For` 可以由客户端伪造。只要下载域的反代用的是追加式 `$proxy_add_x_forwarded_for`（`reverse-proxy.md:60` 推荐的就是这种），客户端就能伪造 `CF-Connecting-IP`，绕过分享的 IP 白名单、前 N 名限制和失败限流。现在只靠 nginx 把这个头剥掉。
- **参考配置的下载域没设 `X-Forwarded-For`**
  - 位置：`docs/public/nginx/lanrurugi-dual-domain.conf:116-150`
  - 按这份配置部署，下载域上所有 `/api/share/r/` 请求在应用看来都来自 127.0.0.1，IP 白名单和失败限流会把所有访客当成同一个人。
- **scheme 校验**
  - 位置：`crates/lanrurugi-api/src/settings.rs:1058-1062`
  - 从没设置过 `share_public_url` 时，`page_origin` 是 `None`，`http://` 下载地址一律放行，所以确实形同虚设。
  - 建议：没有公开地址时，回退用这次 PUT 请求自己的 `Origin` / `X-Forwarded-Proto` 判断；两个都没有就给 warning，而不是静默接受。
- **HMAC 直接复用了 `session_secret`**
  - 位置：`direct_download.rs:73`
  - 有 `"archives:"` 前缀做域分隔，目前不构成漏洞。但轮换会话密钥会让所有下载链接一起失效，最好派生一个子密钥。

**022**
- **中继复用的判断键漏了字段**
  - 位置：`crates/lanrurugi-api/src/share_net/mod.rs:316`
  - `relay_settings_key` 里没有 `share_node_relay_rate`（`nodes.rs:600`），也没有 ACME 用到的 `share_public_url`。改了限速或公开域名，中继会被复用，新设置不生效，违反 FR-010。
  - "身份变了还沿用旧中继"：不会。`:405` 有 `previous_id == net.id()` 判断，身份变了会重建中继。
- **退避 sleep 卡住了整个取块循环**
  - 位置：`share_net/import.rs:991-999`
  - `tokio::time::sleep(backoff)` 直接在 loop 里 await，最长 60 秒。这期间 helper 的在途请求不被 poll，取消也检查不到（停止导入最多要等 60 秒）。N 个块同时失败时，等待会串行累加。
- **`bool` 的用法核对**：每一处都对（`true` = 终态）。哈希校验失败当作终态是合理的，因为 QUIC 自己保证传输完整性。
- **已删除的分享会不会永远占着**：分享方在线时会回 `NotFound`，导入正常结束，不会占着。
- **不会结束的情况**：
  - 分享方永远离线：导入会无限等待，没有上限，界面上也不提示。
  - `Protocol(_)`（协议不匹配这类确定性错误）也被当成网络错误（`:1009`）无限重试。
- **plugins 的重试类型不全**
  - 位置：`plugins.rs:2728`
  - 只重试 `HttpRequestFailed`。连接中断导致的 `TruncatedDownload`，以及 `HttpStatus` 429/502/503/504 都不重试，其中包括 FR-009 自己定义的 repack 503 + `Retry-After`。这与 FR-018 不一致。
  - 另外，`Stalled` 被映射成 `url: ""`（`stream.rs:161`），错误信息里看不到是哪个 URL。
- **180 秒空闲超时**：它是对每个 chunk 单独计时，只要服务器一直在出字节，再慢也不会误杀。
- **真正的超时缺口在响应头阶段**：`outbound.rs` 只设了 `connect_timeout`。等响应头这一段没有任何超时，服务器接了连接却一直不回，就会永远挂住（FR-018 要求有 read timeout）。
- **FreeBSD 上静默不绑定**
  - 位置：`crates/lanrurugi-core/src/http_client.rs:89-100`
  - `bind_outbound` 在不支持的平台上直接返回未绑定的 builder。FreeBSD 是我们发布的平台，这违反 FR-015（"MUST be an error, never a silent unbound fallback"）。
- **非 Linux 上钉了接口，中继就连不上**
  - 位置：`vendor/iroh-relay/src/client/tls.rs:70`
  - `EINPROGRESS` 写死成 115，只在 Linux 上对；macOS / FreeBSD 上是 36。一旦钉了接口（在调用接上之后），这些平台上中继拨号会直接失败。

## 可延后

- **iroh-relay fork 不止改了一处**：插入新代码时把 `#[cfg(not(wasm_browser))]` 从 `mod tls;` 头上挤到了 `static BIND_DEVICE` 上（`vendor/iroh-relay/src/client.rs:40-61`）。现在 `mod tls;` 没有 cfg，而 `set_bind_device` 引用了被 cfg 掉的 static，wasm 构建会坏。这和 FORK.md 说的"No other line changes"不符。
- **注释里的环境变量不存在**：`vendor/netwatch/src/udp.rs:20` 提到 `LANRURUGI_BIND_DEVICE`，这个变量根本不存在。
- **FR 引用错了**：`import.rs:1007` 引用的是 FR-012，但 FR-012 讲的是 helper 离场，和这段代码无关。
- **020 Implementation Record 过时**：里面写"no discrepancy report yet"，但 T021 的 `sidecar_audit` 已经实现了。
- **搬家留下空目录**：搬家之后不清理空目录（recycle 有 `prune_empty_parents`），库根目录会残留一堆空的目录骨架。

## 023：基本确认只是搬家，没改行为

- 对照 `origin/dev`，`limits` 只是把加载设置 / 时区的步骤挪到了 API 侧的薄包装里，计算逻辑完全一致。限流器全局只有一份，因为 API 侧只是重新导出，没有另建 static。
- `blobs` 改成走 `&dyn ShareHost` 之后，`archive()` 吞错误的方式和原来 `.ok().flatten()` 一样。没发现吞掉新的错误，也没发现并发语义有变化。
- `EmptyHost` 会 panic，但它只在测试里用，生产代码不会碰到。
