# 代码审查报告

**审查范围**：`12be9c1` / `c446c96` / `a08887d` 三个提交，共 158 文件、约 10,100 行新增。
**审查日期**：2026-10-10
**审查者**：Claude Opus 5（受委托审查 DeepSeek harness 完成的实现）

---

## 一、基线验证

| 检查项 | 命令 | 结果 |
|---|---|---|
| 前端类型检查 | `npx tsc -b --force` | ✅ 干净 |
| Rust lint | `mise run clippy`（含 `--all-targets`） | ✅ 零警告 |
| 前端单元测试 | `mise run test-frontend-unit` | ✅ **172/172 通过**（24 个测试文件） |
| Rust 测试 | `mise run test` | ⚠️ 880+ 通过，**1 个失败**（见 §四，判定为既有问题） |

工作树干净，无未提交改动。

---

## 二、🔴 必须修复：重定向绕过插件网络权限

### 问题

**位置**：`crates/lanrurugi-plugin/src/http.rs`

插件的网络权限（`declared_permissions.net`）**只在入口 URL 校验一次**：

```rust
// http.rs:112-127  relay()
let host = parsed.host_str()...;
if !host_allowed(allowed_hosts, &host) {
    return Err(...);   // ← 唯一的一次校验
}
```

但跳转交给了 reqwest 的客户端级策略：

```rust
// http.rs:251-267  client_for()
.redirect(reqwest::redirect::Policy::limited(limit))   // ← 跟随任意域
```

`Policy::limited` 不区分目标域。响应回来后虽然取了最终 URL：

```rust
// http.rs:191
let final_url = response.url().to_string();
```

但它**只被写进返回值和缓存条目**（第 223、236 行），**从未重新校验**。

### 可利用性（已查实，非推测）

| 事实 | 证据 |
|---|---|
| 这是**默认路径**，不是边缘情况 | `dispatcher.ts:448` 的 `redirectLimit` 默认 **10**；`followRedirects` 默认为真 |
| 影响面广 | **34 个**插件声明了 `net` 权限，**26 个**会发起请求，全部走这条路 |
| 攻击路径 | 声明 `net: ["e-hentai.org"]` 的插件，只要上游返回 302 指向任意站点，即可到达未声明的域 |

### 边界说明（避免误判）

**入口校验本身是严密的**，不需要改动：

```rust
// http.rs:298  host_allowed()
entry == host || host.ends_with(&format!(".{entry}"))
```

用 `.{entry}` 后缀匹配，正确规避了同后缀冒充。测试已覆盖：

- ✅ `e-hentai.org` / `api.e-hentai.org` / `API.E-Hentai.ORG`（大小写无关）
- ✅ 拒绝 `not-e-hentai.org`
- ✅ 拒绝 `e-hentai.org.evil.test`

**缺口只存在于跳转之后。**

### 建议修法

改用 `reqwest::redirect::Policy::custom`，在每一跳的回调里对目标 host 执行 `host_allowed`，不通过则 `attempt.stop()`：

```rust
let allowed = allowed_hosts.to_vec();   // 回调需要 'static
Policy::custom(move |attempt| {
    let ok = attempt.url().host_str()
        .is_some_and(|h| host_allowed(&allowed, &h.to_ascii_lowercase()));
    if !ok {
        attempt.stop()
    } else if attempt.previous().len() >= limit {
        attempt.error("too many redirects")
    } else {
        attempt.follow()
    }
})
```

注意：这会改变 `client_for` 的缓存键——策略现在依赖 `allowed_hosts` 而不只是 `limit`，按 limit 复用客户端的做法需要相应调整（或改为每次请求构建，权衡性能）。

### 测试缺口

`http.rs` 现有 5 个测试，**没有任何一个覆盖重定向**：

- `a_declared_host_covers_itself_and_its_subdomains`
- `the_request_key_covers_method_url_and_body`
- `only_revalidatable_successes_are_stored`
- `an_unresolvable_host_fails_within_the_connect_budget`
- `error_messages_do_not_leak_query_strings`

修复后应补一个：**跳转到未声明域时请求被拒**。

---

## 三、✅ 审查通过的部分

### 缩略图降质循环（`crates/lanrurugi-scanner/src/thumbnail.rs:229-266`）

逻辑正确，无死循环风险：

- `target_height` 单调递减，触及 `MIN_THUMBNAIL_HEIGHT` 后必定 `break`
- 即使所有尝试都超出原图字节数，仍会存下**最小的那个**，不会丢缩略图
- 不放大（`THUMBNAIL_HEIGHT.min(img.height())`），避免了 legacy ImageMagick 几何放大的问题

### HTTP 缓存的存储条件（`http.rs:316-328  storable()`）

判断准确：

- 非 2xx → 不存
- 带 `Set-Cookie` → 不存（避免跨会话串用）
- `cache-control: no-store` → 不存
- 只用 `If-None-Match`/`If-Modified-Since` 换 304，**不做基于时间的陈旧猜测**

### 乱码修复模块（`crates/lanrurugi-api/src/mojibake.rs`）

改动用户数据时的谨慎到位：

- 默认 dry-run（`apply: false` 只报告不写）
- 写入需显式请求，UI 先展示 before/after 对照
- 复用既有的严格检测器，而非另写一套

### 订阅预览的范围差异（`runner.rs` + `subscriptions.previewWindow`）

预览只读 1 页、定时检查读 4 页——**界面明确说明了这个差异**：

> 预览只读取最新 1 页（共 {{checkPages}} 页）以尽量减少对源站的请求；整点检查会读取全部 {{checkPages}} 页。数据均为实时抓取。

没有让用户误以为预览等同真实检查。这正是此前反复强调的要求。

### 额度策略选项的处理

此前审查指出 `credit_policy` 是**空设置**（`InsufficientCredit` 无任何生产调用方，只在测试里出现）。本批已按要求**移除 UI 控件**，数据字段保留并固定传 `"pause"` —— 不再误导用户，也不破坏数据结构。处理得当。

---

## 四、⚠️ 判定为既有问题（非本批引入）

### Rust 测试 1 项失败

```
stamps::tests::delete_one_of_two_stamps_on_page_keeps_the_bookmark
  panicked at crates/lanrurugi-api/src/stamps.rs:611:9:
  assertion failed: state.bookmarks.is_bookmarked(&archive_id, 4).await.unwrap()
```

**判定依据**：

1. `stamps.rs` **不在这三个提交的改动范围内**（最后改动是更早的 `c8fde14`）
2. 仓库已有提交 `9967e70` 标题为「stamps.rs 图章测试补充配置字段锁，**避免并行测试竞争 LRR_CONFIG 全局字段**」—— 同一文件的同类竞争问题有前科
3. 失败断言涉及 `stampautobookmark` / `stampautounbookmark` 两个**全局配置字段**，与并行测试共享 Redis 的已知模式吻合

**结论**：这是测试间的并发竞争，不是本批功能缺陷。但值得单独跟进——同一文件已经为此修过一次锁，说明覆盖不完整。

### i18n 仅补了 en / ja / zh

**与现状一致，不是缺陷**：

| 语言 | key 数 | 相对 en 缺失 |
|---|---|---|
| en / ja / zh | 2004 | 0 |
| zh_Hant | 948 | 1064 |
| ko | 869 | 1142 |
| 其余 8 种 | 290–781 | 1229–1715 |

只有这三个语言本来就是完整的，其余 11 种在本批之前就缺 1000+ 条，且 i18next 配置了 `fallbackLng: "en"` 兜底，不会白屏。

---

## 五、结论

**整体质量高**：类型检查、lint 干净，前端测试从 111 涨到 172，缓存/循环/数据改写等易错处的判断都正确，设计文档（spec/tasks）同步更新。

**唯一阻塞项**是 §二 的重定向权限绕过 —— 这是真实的安全缺口，默认路径可达，影响 26 个会发网络请求的插件。建议修复后再合入。

其余两项（stamps 测试竞争、i18n 覆盖）均为既有问题，不阻塞本批。
