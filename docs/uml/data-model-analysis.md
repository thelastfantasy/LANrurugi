# 前后端全部数据模型 UML 图形化 + 冗余/漏洞分析

issue #49 第三块范围的产出物。配套三张 UMLet 图（`.uxf`，用 VSCode 的 UMLet 扩展打开）：

- `01-core-domain.uxf` —— 核心领域模型：`Archive`/`Category`/`Grouping`/`Stamp`/`TocEntry`，以及
  `lanrurugi_core::ids` 的四个 ID newtype。
- `02-jobs-and-queue.uxf` —— 后台任务与下载队列子系统：`JobStatus`/`DownloadQueueItem`/
  `QueueError`/`PendingFilenameConflict`。
- `03-plugin-options-redundancy.uxf` —— 单独一张图，聚焦 `DomainRule` 三重重复这一个具体的
  冗余发现。

字段级完整清单（哪个类型有哪些字段、精确到类型）由本次普查的 Explore agent 逐个源文件核实
产出，篇幅原因不在本文档重复贴出全部字段表，需要时可重新触发同等范围的普查。本文档聚焦
**结论**：冗余在哪、缺口在哪、潜在漏洞在哪。

## 一、总体现状

前后端类型对应关系大致分三档：

1. **两侧完整类型化，逐字段核对一致**（推荐范式，应该继续照这个来）：
   `JobStatus`/`JobRecord`、`DownloadQueueItem`、`QueueError`（15/15 变体核对一致）、
   `PendingFilenameConflict`、`EffectivePluginOptions` 一家（`EffectiveDomainRule`/
   `EffectiveBundleAsArchive`/`EffectiveOverwriteOnDuplicate`）、`StampJson`。
2. **形状一致但后端没有具名 struct**，只是散落在各个 handler 里手写的 `json!{}` 宏：
   `Category`→`CategoryMetadata`、`Grouping`→`TankoubonMetadata`、`ServerInfo`、
   `ShinobuStatus`、`StatTag`、`BookmarkLinkResponse` 等。今天字段数对得上，但没有编译器背书，
   后端改一个字段名前端不会报错，纯靠人工同步。
3. **有真实缺口/发现的**：见下面第二、三节。

## 二、冗余发现

### 1. `DomainRule` 被独立定义了三次（详见图 3）

`lanrurugi_plugin::protocol::DomainRule`、`lanrurugi_api::download_manager::domain_rules::
DomainRule`、`lanrurugi_storage::plugin_options::DomainRuleOverride`（少一个 `description`
字段）——同样的 `pattern`/`max_concurrent`/`max_bytes_per_sec` 三个核心字段手写了三份，
`download_manager::settings` 模块自己的文档注释承认"kept in exact sync by the round-trip
test"，也就是说这个一致性完全靠一个测试撑着，不是类型系统保证的。

**建议**：把唯一权威的 `DomainRule` 挪进 `lanrurugi-core`（三个 crate 目前都没有理由不能依赖
它），另外两处改成重新导出或者一个薄 newtype 包装，而不是各自重复声明四个字段。

### 2. `Extension`/`ExtensionType`/`ExtensionState` 是孤立死代码

`lanrurugi_core::entities::Extension` 定义了完整的字段（`namespace`/`kind`/`parameters`/
`enabled`/`declared_permissions`），但普查确认 `lanrurugi-api` 里没有任何 repository、Redis
key、路由引用它。真实的插件配置实现走的是完全不同的 `plugin_options` 形状。这看起来是早期
`data-model.md` 设计遗留、后来被换掉但没清理干净的类型。

**建议**：确认真的没有任何引用后直接删除（或者如果是为未来功能预留，加一个明确的
`// TODO` 说明用途和排期，而不是悄悄躺在那）。

### 3. `Settings`（前端）在后端没有对应的具名 struct

后端 `GET/PUT /settings` 完全靠 `settings.rs` 里的三个 const 表（`STRING_FIELDS`/
`NUMBER_FIELDS`/`BOOL_FIELDS`）驱动一个 `serde_json::Value`，没有一个真正的 `Settings`
struct。这是本次普查里"前后端类型耦合最弱"的一处——见下面第三节，这不只是冗余问题，
也是一个真实的校验缺口。

### 4. `BackupArchive`/`BackupCategory`/`BackupTankoubon`/`BackupStamp`/`BackupDocument`
在前端完全没有对应类型

`GET /database/backup`、`POST /database/restore` 这两个端点的完整 wire 契约，前端
`api/types.ts` 里一个字都没有类型化。功能能跑是因为前端大概率把响应当 `unknown`/`any`
处理，但这意味着备份/恢复这条链路完全没有编译期保护。

### 5. `PluginInfo` 在传给前端之前被悄悄重塑过

`lanrurugi_plugin::protocol::PluginInfo` 里的 `PluginParameter{name,description,required,
param_type}`，真正发到前端的却是 `{name,desc,type?}`——`description` 改名成了 `desc`，
`required` 整个字段被丢弃。`declared_permissions`/`sidecar_files` 也完全没有发给前端，前端/
UI 上用户根本看不到某个插件申请了哪些权限。这看起来是有意为之的精简投影，不是 bug，但
"用户看不到插件权限申请"本身可能是个值得补的 UX 缺口（issue #49 范围之外，这里只作记录）。

## 三、潜在漏洞/校验缺口发现

以下两条是这次分析里**真正落地验证过**（不是猜测）的发现，比冗余问题更值得优先处理。

### 1. `PUT /settings` 没有字段白名单，也没有值校验

`settings.rs::put_settings` 的实现（第 205-274 行）：

```rust
for (key, value) in fields {
    if key == "password" || key == "session_secret" { continue; }
    let stored = match &value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => ...,
        Value::Number(n) => n.to_string(),
        _ => continue,
    };
    conn.hset(CONFIG_KEY, &key, stored).await?;
}
```

请求体里**任意** key（只要值是 string/bool/number）都会被原样写进 `LRR_CONFIG` 这个 Redis
hash，不检查这个 key 是不是 `STRING_FIELDS`/`NUMBER_FIELDS`/`BOOL_FIELDS` 里声明过的 24 个
已知字段之一，也不检查值本身是否落在该字段的合法取值范围内（比如 `theme` 字段没有对着
5 个真实主题文件名做枚举校验）。

顺着 `theme` 字段追下去，前端 `theme.ts:206` 是这样用的：

```ts
ensureLink(LEGACY_THEME_CSS_ID, `/legacy/themes/${theme}`)
```

`theme` 的值未经校验直接拼进 `<link href>` 路径。触发这条路径需要先有 `PUT /settings` 的
写权限（管理员级别的可信边界，不是匿名可达），所以不是一个开箱即用的漏洞，但这是一处
真实的纵深防御缺口——**服务端目前完全没有对设置字段做白名单/取值校验**，前端的类型系统
不能替代后端校验。

**建议**：`put_settings` 改成对着 `STRING_FIELDS`/`NUMBER_FIELDS`/`BOOL_FIELDS` 的 key 集合
做白名单过滤，未知 key 直接拒绝或忽略；`theme` 这类有限枚举的字段单独加合法值检查。

### 2. 插件自己声明的字符串（`description`/`title`/`tags`）未经清洗直接进了
`dangerouslySetInnerHTML`

确认过的真实代码路径：

- `apps/frontend/src/pages/Plugins/PluginCard.tsx:179`：
  `<span dangerouslySetInnerHTML={{ __html: plugin.description }} />`——`plugin.description`
  来自 `protocol::PluginInfo.description`，这是**插件自己**（一段 Deno 沙箱里跑的第三方
  TypeScript 文件）在自己的元数据里声明的字符串，后端原样转发，前端原样注入 DOM。
- `apps/frontend/src/pages/Edit.tsx:117,125`：metadata 插件跑完之后，`toast({text:
  result.title, ...})` / `toast({text: result.tags, ...})`——`result` 来自插件的执行结果，
  同样未经清洗。
- `apps/frontend/src/toast.tsx:64`：`toast()` 组件本身就是用
  `dangerouslySetInnerHTML={{__html: c.text}}` 渲染 `text` 的，所以任何调用 `toast()` 时
  传入插件可控字符串的地方都继承这个风险。

插件运行在 Deno 沙箱里（constitution Principle IV 管的是网络/文件系统权限），但这只约束了
插件能不能**读写外部资源**，完全不约束插件**返回的字符串内容**——一个恶意或者被篡改的插件
只要在 `description`/`title`/`tags` 里塞一段 `<img src=x onerror=...>`，就能在安装/运行这个
插件的用户浏览器里执行任意脚本，这是一条真实、可复现的存储型 XSS 路径。

**建议**：这几处的 `dangerouslySetInnerHTML` 要么改成纯文本渲染（React 默认转义，插件元数据
/执行结果没有理由需要渲染真实 HTML），要么在写入 DOM 前过一层 HTML 清洗（如 DOMPurify）。
`toast.tsx` 本身支持 HTML 是给项目自己受信任的调用点用的（比如 Library 里的书签登录链接），
不应该被插件可控内容复用同一条渲染路径。

## 四、总结

- 冗余发现 5 条（`DomainRule` 三重重复是最值得先动手的一条），漏洞相关发现 2 条（插件字符串
  XSS 风险优先级高于 Settings 校验缺口，因为前者不需要管理员权限就能触发——只要装一个
  恶意/被篡改的插件）。
- 三张 UMLet 图 + 本文档是这次分析的完整产出；具体修复本身不在这次范围内（issue #49 第三块
  只要求"画图 + 分析"），要不要动手修，以及先修哪个，留给你决定。
