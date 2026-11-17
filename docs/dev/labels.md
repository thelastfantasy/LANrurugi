# Issue / PR 标签

每个 issue 和 PR 都要打上**一个类型标签**，以及**至少一个范围标签**（涉及多个范围就都打上）。

## 类型（选一个）

| 标签 | 含义 |
|---|---|
| `type: feature` | 开发：新增此前没有的功能、页面、接口或设置项 |
| `type: fix` | 修复：现有行为有误、崩溃、数据错误或与预期不符 |
| `type: improvement` | 强化：已有功能的体验、性能、稳定性或代码结构改进，不是修 bug 也不是新功能 |
| `type: docs` | 文档：README、开发文档、宪章、文档站内容，不改代码行为 |

## 范围（可多选）

| 标签 | 含义 |
|---|---|
| `area: frontend` | `apps/frontend`（React/TS/CSS、主题文件、前端测试） |
| `area: backend` | `crates/` 下的 Rust 代码 |
| `area: infra` | CI/workflow、Docker/compose、mise 任务、发布打包、部署与 Cloudflare |
| `area: deno` | `plugins/` 下的插件、插件 SDK、插件运行时 |

## 其他

- `dependencies`、`docker`、`github_actions`、`javascript`、`rust` 由 Dependabot 自动使用，不手动打。
- `duplicate`、`invalid`、`wontfix`、`question`、`help wanted`、`good first issue` 按 GitHub 的通常含义使用。
- PR 的标签按实际改动的范围打，不一定和它关闭的 issue 完全相同。
