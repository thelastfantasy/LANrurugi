# 2026-11-10 复查修复报告

供 cc 复查。基准是 `specs/review-result-2026-11-10.md`（针对 c 轮 `61fd2f9`、`7e3addb` 的复查），
本轮修掉它列的 3 条应修与 5 条可延后，并按用户要求补上 cc 原来说「可以不做」的两条快照测试。

- 分支：`feat/share-bind-interface`，PR #258（未合并）
- 本轮提交：`3ba0ae7`（应修 + 可延后 1、2）、`edfaad0`（可延后 3、4、5）、`6a3eba9`（两条快照测试）
- 三次推送 CI 均 success（Rust / Docs / Frontend）

---

## 应修 1：非 Linux 上端口映射开关被一起藏掉

**问题**：`SharingSection.tsx` 在 `interface_binding.share === "none"` 时替换了整个 network 面板，
`share_portmap` 跟着消失；它和接口绑定无关，所有平台都有效。

**修复**：`apps/frontend/src/pages/Settings/SharingSection.tsx`

- `:148` `const BIND_INTERFACE_FIELD: SettingField`、`:158` `const PORTMAP_FIELD: SettingField`，
  原来的 `NETWORK_FIELDS` 拆成两个。
- `:382` 非 Linux 时只把接口选择器换成说明文字（`bindInterfaceUnsupported`），
  `:384` `<ShareSettingsForm fields={[PORTMAP_FIELD]} />` 无条件渲染。

**验证**：`tsc -b --force`、`eslint src`（0 error / 4 既有 warning）；
没有新增 i18n 键（复用的是上一轮已有的 `share.network.bindInterfaceUnsupported`）。

---

## 应修 2：`InFlight` 路径留垃圾、3 秒窗口太短

**问题**：两个入库撞同一内容 id 时，先到的一方还没写完记录，后到的一方等 3×100 ms 就报
`QueueError::Internal`，字节留在库根的 `.ingest-*` 里等 6 小时清扫；而大文件入库要读页数、检
PDF，稳定超过 3 秒。

**修复**：`crates/lanrurugi-api/src/download_manager/ingest.rs`

- `:1097` 新增 `fn content_lock(id: &str) -> impl Future<Output = OwnedMutexGuard<()>>`：
  `LazyLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>>`，拿锁前 `retain` 掉死项，
  `lock_owned()` 交出所有权，调用方在算完 id 后一直持有到本次 cataloguing 结束。
- 删除 `IngestDownloadError::InFlight`、30×100 ms 轮询与其 `QueueError` 映射。
- 后到的入库等到锁后重新查记录：已存在则删自己的副本、返回 `Unchanged`（同 id、`is_new=false`）。
- 崩溃留下的「有文件、无记录」由 `rename` 原子覆盖（字节按 id 相同，不存在内容歧义）。

**验证**：新增 `two_ingests_of_the_same_bytes_agree_on_one_archive`——两个并发入库最终只有一个
`is_new`，两次返回同一 id，库根没有 `.ingest-*`。

---

## 应修 3：收编失败时把 `dest` 搬回去多余且有害

**问题**：`adopt_pending_conflict` 失败时把 `dest` 搬回 `staging`。若记录已写，会搬走记录指向的
文件；若并发入库，会抢走别人的文件。

**修复**：`crates/lanrurugi-api/src/download_manager/ingest.rs:114`

- 删掉 `dest` 的计算与回搬代码：原件是复制到 `dest` 的，`staging` 一直在队列行指向的位置，
  失败时什么都不用搬。
- `catalogue_staged_file` 的错误分支改成 `if !recorded { remove dest }`：
  只有「本次 rename 进去、且没有记录」的那份才删；`keep_bytes_on_failure` 只负责不提前删原件。

**验证**：c 轮那条 `a_failed_adoption_keeps_the_staged_file_where_the_row_points` 继续通过
（断言 Err 且原字节完好）；`two_ingests_of_the_same_bytes_agree_on_one_archive` 覆盖并发场景。

---

## 可延后 1：运行时没看平台能力

**问题**：非 Linux 上如果数据库里已经有 `share_bind_interface`，运行时仍然照用；UI 已无入口清除它。

**修复**：`crates/lanrurugi-api/src/share_net/mod.rs:463`

```rust
pub fn effective_bind_interface(settings: &ShareSettings) -> String
```

- `share_binding_scope() != BindingScope::Full` 时返回空串，并用 `Once` 只告警一次
  （「stored interface is ignored on this platform」）。
- `:323`（`bind()`）与 `:348`（`spawn_interface_watch`）两处读取都改用它，端点与 watcher
  看到的是同一个值。

**验证**：`check-crate`、`clippy`；逻辑分支由 `http_client` 的两条 scope 单测覆盖
（`only_linux_pins_every_plugin_request`、`only_linux_pins_the_sharing_engine`）。

---

## 可延后 2：无调用方函数与丢失的 doc

**修复**：

- 删掉 `crates/lanrurugi-core/src/http_client.rs` 里无人调用的 `interface_binding_supported()`
  与 `binding_platform()`（上一轮为 `/api/info` 的 `platform` 字段引入，该字段本轮也删了）。
- 补回 `bind_outbound` 的文档注释——说明它只影响本进程 `reqwest` 的请求，插件子进程的
  `fetch` 由 egress shim 单独拦截（Linux）。

**验证**：`clippy` 干净（删掉后没有 dead_code）。

---

## 可延后 3：「move 完成、save 未完成」被 kill 的窗口

**问题**：搬家先 `rename` 再 `archives.save`；两步之间被 kill，磁盘上档案已在新位置，记录仍指
旧路径，而旧路径不存在——下次扫描只会看到「记录的文件不见了」，不会想到去新位置找。

**修复**：`crates/lanrurugi-scanner/src/relocate.rs:222`

- `relocate_library` 在进入「文件不存在」分支之前，先看 `destination_for(library, original)`
  是否存在，且 `size_aware_id(该文件)` 等于记录 id；是则改正 `archive.file`、改写
  `LRR_FILEMAP`（`hdel` 旧键、`hset` 新键），计入新增的 `repaired`。
- 计数贯通：`RelocationReport.repaired`（`relocate.rs:32`）→ `ScanSummary.relocation_repaired`
  → `crates/lanrurugi-api/src/database.rs` 的 rebuild 结果 JSON（`relocation_repaired`）。

**验证**：新增 `an_interrupted_relocation_is_repaired_by_the_next_pass`
（`relocate.rs:570`）——手工把文件搬到 `destination_for(original)`、记录仍指旧路径，
下一次 `relocate_library` 修好记录并计 `repaired == 1`。

---

## 可延后 4：两条阻断缺回归测试

**新增测试**（`edfaad0`）：

1. `crates/lanrurugi-api/src/share_net/mod.rs` 的 `retry_tests::a_truncated_body_is_worth_retrying`：
   `read_body` 截断时返回具名错误 `truncated_body_error()`（`:713`，`pub(crate)`），
   `terminal_import_error` 判为可重试——钉住「传输中断不是终态」。
2. `relocate::tests::a_failed_save_puts_the_archive_and_its_patch_back`：
   测试专用 `#[cfg(test)] static FAIL_SAVES` + `save_record()` 包装让 save 失败可达，
   断言档案与 `.patch.zip` 都回到原处、report 计 `failed`。
3. `relocate::tests::an_interrupted_relocation_is_repaired_by_the_next_pass`（见上）。

**验证**：`lanrurugi-api` 648 通过、`lanrurugi-scanner` 129 通过
（本地另有 1 个既有 `animation::tests::converts_a_real_animated_page` 失败，原因是动画夹具没挂进
容器，与本次无关，CI 上通过）。

---

## 可延后 5：`/api/info` 对匿名暴露 OS 名

**修复**：`crates/lanrurugi-api/src/misc.rs` 的 `interface_binding` 只保留两项能力 scope：

```json
"interface_binding": { "plugin": "full", "share": "full" }
```

`apps/frontend/src/api/types.ts` 同步删掉 `platform`，设置页只按两个 scope 分支。

**验证**：`tsc -b --force`、`eslint src`。

---

## 快照测试（cc 原来说可以不做，用户要求补上）

`6a3eba9`。目的：`share-proto` 的固定字节测试只钉住编码层，钉不住「算出来的值」；023 是把
这些计算从 `lanrurugi-api` 搬进 `lanrurugi-share`，语义漂移不会被功能测试发现。

### 1. `browse_manifest`（`crates/lanrurugi-share/src/manifest.rs:239`）

`the_browse_manifest_is_stable`：`StubHost`（包一层 `EmptyHost`，只覆写 `archive`/`tankoubon`）
喂三个条目——普通档案、单行本（成员顺序 B、A，其中一个成员标题是空白、靠 `name` 回退）、
以及已被删除的档案。断言 `serde_json::to_value(manifest)` 与写死的 JSON 全等，覆盖：

- 标题回退规则（`"  "` → `"Beta"`）；
- `filename` 取自磁盘路径的 basename（哈希命名后是 `A.cbz`，不是 id）；
- 单行本成员按 `archives` 顺序，缺失成员被跳过；
- 删除项序列化成 `{"Unavailable": {"title": null}}`；
- browse 形态下 `blake3` 恒为 `null`、`categories` 恒为空。

### 2. `relay_urls`（`crates/lanrurugi-share/src/net.rs:418`、`:447`）

- `the_relay_list_is_stable`：三条配置（其中一条重复）+ 自建中继，断言顺序保持、重复被去掉、
  自建中继排尾、每个 URL 是同一个拼写（带尾斜杠）。
- `public_relays_are_appended_only_when_asked_for`：开关打开时公共中继追加在用户配置之后
  （只断言前缀与「确实追加了」，不钉死上游列表，避免 iroh 升级时误报）。
- 写测试时发现的一个真实默认值：`share_public_services` 的默认是 `"1"`，因此第一条测试必须
  显式写 `"0"` 才能隔离出我们自己的列表——这一点写进了测试注释。

**验证**：`lanrurugi-share` 28 通过。

---

## 门禁

| 项 | 结果 |
|---|---|
| `mise run check-crate -- lanrurugi-api lanrurugi-scanner lanrurugi-share` | 通过 |
| `mise run clippy` | 通过 |
| `mise run fmt-check` | 通过 |
| `mise run test-crate -- lanrurugi-api` | 648 + 1 + 1 + 5 通过 |
| `mise run test-crate -- lanrurugi-scanner` | 129 通过（+1 既有 animation 夹具问题，CI 通过） |
| `mise run test-crate -- lanrurugi-share` | 28 通过 |
| `cd apps/frontend && npx --no-install tsc -b --force` | 通过 |
| `cd apps/frontend && npx --no-install eslint src` | 0 error / 4 既有 warning |
| CI（Rust / Docs / Frontend） | `3ba0ae7`、`edfaad0`、`6a3eba9` 均 success |

---

## 本轮没有做的事（如实说明）

- **真机验收**：020/021/022 的 quickstart 步骤仍未在真实部署上跑过（断网恢复、5 GB 下载、
  跨子域无 cookie 下载等）。步骤清单在 `specs/share-followups-2026-11-08.md` 的 I 节。
- **023 仍是部分实现**：端点、中继、发布、会合、manifest、blob 读取、限速判定都已搬走；
  `server.rs` 的请求分发、`import.rs`、`start()` 与四个后台循环仍留在 `lanrurugi-api`
  （它们分别经下载管理器、扫描器、协议挂载，需要先把那几处变成 host 方法）。
- 没有新开 issue、没有合并 PR（按约定合并由人来做）。
