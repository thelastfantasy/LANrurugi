# 复查请求（2026-11-09 晚）：review-result 的修复 + 023 的搬迁

分支 `feat/share-bind-interface`（PR #258，base `dev`）。这份文档请按「这次改了什么、还差什么、
哪里值得怀疑」来读，不需要重读 020/021 的全貌。

## 1. 上次 review 五条阻断项的现状

| 阻断 | 状态 | 代码位置 |
|---|---|---|
| 绑接口没接线 | 已接 | `crates/lanrurugi-api/src/share_net/mod.rs` 的 `bind()` 调 `netwatch::set_bind_device` 与 `iroh_relay::client::set_bind_device`，取消固定时传 `None` |
| 来宾能拿整本签名链接 | 已封 | `crates/lanrurugi-api/policy/route_policy.csv` 拒绝 `token_guest /api/tankoubons/:id/download-link` |
| 搬家非原子 | 已修 | `crates/lanrurugi-scanner/src/relocate.rs`：只改 `file`、保存失败回滚文件、EXDEV 走 `.part` + fsync + rename、补丁失败则档案搬回 |
| 快照覆盖 | 已修 | 同上，保存前重读记录 |
| 入库删唯一字节 | 已修 | `crates/lanrurugi-api/src/download_manager/ingest.rs`：dest 已存在返回 Unchanged、出错时保留、Rejected 不删 |

「应修」项（旁车重写、`.ingest-` 清理、平台不支持即拒绝、EINPROGRESS、Stalled 带 URL、重试类型、
响应头超时、下载域判定、搬家五态、空目录清理、CF 最近一跳、nginx XFF、中继复用键）也都在
`specs/share-followups-2026-11-08.md` 的 L 节逐条记了处置。

**仍未做**：半截下载收编（要新的队列状态）、`original_path` 填充（自动流程不会把 `_byname/` 搬进
哈希布局）。两者都写明了理由，请判断是否阻塞。

## 2. 023 的搬迁进度

已搬进 `crates/lanrurugi-share`（每次一个 commit，行为不变，测试随之搬）：

`interfaces` `link` `status` `proxy` `settings`（视图）`limits`（判定）`blobs`（含 Range 编码）
`host`（`ShareHost`）`net`（`ShareNet` + `bind` + `from_bound`）`nodes`（解析、会合、限速器）
`relay`（三种 TLS 模式、端口回退、允许名单）`publish`（via 码、pkarr、节点记录）`manifest`
（browse/claim）。

`ShareHost` 目前有：`redis` `jobs` `temp_dir` `archive` `tankoubon` `settings` `timezone`
`node_code` `thumbnail` `categories` `add_import_bytes`，以及默认方法 `admit_upload`。

**仍在 api 的**：`share_net/server.rs`（协议处理器，Range 读取与 `helper` 模块耦合）、
`share_net/import.rs`（导入侧，落地到扫描器与下载管理器）、`start()` 与四个后台循环、
`share_net/nodes.rs` 里 `NodeProtocol`/`run_standalone` 这两个 iroh 适配层。

`cargo tree -p lanrurugi-share | grep -c lanrurugi-api` = 0。

## 3. 这次最该怀疑的五处

1. **Range 的单位**：协议 span 按 1 KiB 计，bao-tree 的 `BLOCK_SIZE` 是 16 KiB。我把 1 KiB 明确成
   `blobs::CHUNK_BYTES` 并写了注释；请确认 `ChunkNum` 的编号单位确实与 span 一致（`import.rs` 的
   注释说是一致的，但这是理解而非验证）。
2. **`admit_upload` 默认方法**：默认实现放在 trait 里，避免两个实现各读一套设置。请确认「读设置 →
   判定」之间没有应当持有的锁或快照语义。
3. **`ShareHost::thumbnail`**：分享侧不再协商 `Accept`，固定走「已存格式 → 重生成 → 任意已存 →
   占位图」。如果对端是只支持 WebP 的客户端，现在会拿到原格式——这是行为变化（更保守，但确实是变化）。
4. **搬家五态**：`RelocationOutcome` 把「还在写」与「目标被占」分开后，`relocation_skipped` 仍是三者
   之和（保持旧字段语义）。请确认汇总口径没有把「目标被占」漏成正常。
5. **CF 最近一跳规则**：规则是「最近一跳要么不受信、要么是 CF 边缘」。既有测试（配置里已含 CF 网段
   时要认边缘）与新增测试（伪造 CF 前缀不得解锁 `CF-Connecting-IP`）同时通过，但两者的组合是否
   覆盖了「多层 CF」与「CF → 自建反代 → 源站」，请判断。

## 4. 门禁与 CI

- 本地：`clippy`、`fmt-check`、`check-crate -- lanrurugi-share lanrurugi-api lanrurugi-scanner`、
  `test-crate -- lanrurugi-share`（25）、`test-crate -- lanrurugi-api`（645 + 1 + 1 + 5）均通过。
- `lanrurugi-scanner` 127 通过；唯一失败是既有的 `animation::tests::converts_a_real_animated_page`
  （环境变量指向的文件没挂进容器，与本 PR 无关）。
- CI：每次 push 后用 `actions/runs?head_sha=` 确认 run 真的创建了；当前 HEAD 的 Rust/Docs/Frontend 全绿。

## 5. 怎么快速看

```
git log --oneline 9116e91..HEAD     # 021 之后的全部
cargo tree -p lanrurugi-share       # 依赖面
grep -rn "lanrurugi_api\|AppState" crates/lanrurugi-share/src | grep -v "^.*://" # 应为 0
```

## 6. 追加（2026-11-09 21:55）

两个 commit 落在这份报告之后，内容都很窄：

- `87db3af` **退避等待可取消 + 协议错误判终态**：
  - 分块失败后的 sleep 改成 `tokio::select!` 与 `run.cancel.cancelled()` 同候，停止导入不再等满 60 秒；
  - `terminal_import_error` 现在把 `ShareNetError::Protocol(_)` 也算终态（确定性失败，重试只是空转），
    与 FR-003「只有对端明确的拒绝才是终态」一致；`NotRunning`/`Unreachable` 仍无限等待。
  - 测试同步：原来断言 Protocol 可重试，改成断言终态。
- `efdb9ba` **钉住 knock 记录的 postcard 字节**：
  - 新测试 `the_knock_payload_bytes_are_stable` 用一个固定 payload（固定 endpoint id、三条不同长度的
    中继 URL、空直连地址）断言字节序列；任何字段重排/新增/长度前缀变化都会失败。这是 023「搬迁
    不改线格式」唯一的自动化保证。
  - 顺带把 023 的基线记进 tasks（依赖面、`docs-api --check` 234 path/299 operation、crate 内
    `AppState` 引用为 0）。

仍未做（与第 1 节一致）：半截下载收编、`original_path` 填充；023 的 `server.rs`/`import.rs`/`start()`
三个 I/O 面。
