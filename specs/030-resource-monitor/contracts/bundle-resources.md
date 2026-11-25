# Contract: 反馈包里的资源快照

## 选择

`crates/lanrurugi-api/src/diagnostics/bundle.rs` 的 `Categories` 新增：

```rust
#[serde(default = "default_true")]
pub resources: bool,
```

默认 `true`（用户在反馈包页签里勾上「资源占用」即导出）。缺省字段的旧客户端因此也获得该文件——这是刻意的：报障时最缺的就是它。

## 产物

| 项 | 值 |
|---|---|
| 文件名 | `resources.json` |
| 内容 | `ResourceSnapshot` 的缩进 JSON（与 `GET /api/resources` 同一形状） |
| 大小上限 | 64 KB；超限按 V7 截断并写 `truncations` |
| 记录位置 | `manifest.json` 的 `files[]`（path/bytes/entries）与 `selection.categories.resources` |
| README | 新增一行说明"resources.json — 生成那一刻的进程/cgroup/队列视图" |

## 同时修复的既有缺陷

`diagnostics.json` 与 `config.redacted.json` 目前**只算大小、不写内容**：`bundle.rs:402` 写入的是 `Vec::new()`，`config.redacted.json` 根本没写进 ZIP。本特性把 `Plan` 改为携带待写文本（`extra_files: Vec<(String, String)>`），三个文件（`diagnostics.json`、`config.redacted.json`、`resources.json`）都由它写盘，预览与产物因此一致。

## 脱敏

`resources.json` 生成后整体过 `crate::diagnostics::scrub`：

- 库/temp 绝对路径 → `<path>`；
- 含 `?key=` / `&token=` 的 URL → 去掉 query；
- 环境变量名以外的值（`DATABASE_URL` 一类）不出现——本快照从不读环境变量值；
- 插件 namespace 保留（它不含用户数据，且是定位所必需）。

## 失败模式

采集失败（`/proc` 不可读、`sysinfo` 构造失败）→ 仍写出 `resources.json`，只含 `{"at": …, "unavailable": "…"}`，并让预览里该行字节数非零；不得让整个反馈包失败。
