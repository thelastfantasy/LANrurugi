# 参与贡献

LANrurugi 的后端是 Rust（`crates/` 下的 Cargo workspace），前端是 React 19 + TypeScript（`apps/frontend/`），
插件跑在 Deno 子进程里。这份文档讲**怎么在本地构建、跑测试、以及提交前必须过哪些检查**；产品用法、特性
与安装见 [README](./README.md) 和[文档站](https://docs.lrrg.dev/)。

## 环境准备

- `mise install` —— 工具链版本固定在 `.mise.toml`（Rust、Node、Deno、pnpm，以及构建加速用的 `sccache`/`mold`）
- `podman` 或 `docker` —— Rust 的构建与测试都跑在容器里：`libarchive2-sys` 需要 cmake + libclang，这些依赖不进宿主机
- 首次运行前构建一次开发镜像：`mise run ci-image`

## 常用任务

| 命令 | 作用 |
|---|---|
| `mise run check` | 提交/推送前必跑：clippy + fmt-check + 前端 tsc/eslint |
| `mise run test` | 全量 `cargo test --workspace`（带 `--no-fail-fast`） |
| `mise run test-crate -- <crate>` | 只测某个 crate，迭代时用 |
| `mise run clippy` / `mise run fmt` | 单独跑 lint / 格式化 |
| `mise run test-frontend-unit` | Vitest + React Testing Library，不需要后端 |
| `mise run test-frontend-e2e` | Playwright，真实后端 + Redis，Chromium + Firefox |
| `mise run measure-compare [-- <页数>]` | 合成档案跑一次图片对比，打印峰值内存（`VmHWM`）与各阶段耗时；改对比/解码路径后用它对比前后 |
| `mise run dev-rebuild` / `dev-up` / `dev-down` | 开发容器（`compose.dev.yaml`，前端走 Vite 热重载） |

## 技术栈

- **后端**：Rust（Tokio 异步运行时、Axum Web 框架、Rayon 用于 CPU 密集型并行计算），一个位于
  `crates/` 下的 Cargo workspace，产出单一二进制文件 `lanrurugi-server`，带有
  `serve` / `rebuild-index` / `bench` / `formats` 四个子命令。
- **数据存储**：Redis，键结构沿用旧版（五个逻辑数据库——见
  `crates/lanrurugi-storage/src/redis.rs`）。LANrurugi 用自己的数据库目录，LANraragi 的元数据通过其备份 JSON
  导入。
- **前端**：React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query，位于
  `apps/frontend/` 下。
- **插件**：沙箱化的 Deno 子进程，每个插件命名空间一个（`crates/lanrurugi-plugin/`）。
- **页面内翻译（Phase 2）**：同一 workspace 下的 `crates/lanrurugi-{ocr,fontcache,translate}` 与
  `apps/frontend/src/translation/`，不是独立部署单元——OCR 检测用 `oar-ocr`、识别用 `ort` +
  `manga-ocr`（均为 Apache-2.0，仅 CPU），翻译适配 OpenAI 兼容 / Anthropic / DeepSeek 三种后端。
- **基准测试工具**：`crates/lanrurugi-bench/`——一个合成库生成器、`criterion` 微基准测试，以及一个同时驱动
  本二进制文件和一个真实旧版 LANraragi 实例进行并排对比的跨系统对比测试工具。


## 从源码构建

**从源码构建**：
工具链版本固定在 `.mise.toml` 中（`mise install` 会精确复现它们：Rust、Node、Deno、pnpm，以及用于加速
构建的 `sccache`/`mold`）。

```sh
# 后端
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# 前端（开发服务器，将 /api 代理到上面的后端）
cd apps/frontend && pnpm install && pnpm run dev
```

或者通过 Docker（将构建好的前端打包进同一个镜像）：

```sh
docker build -t lanrurugi .
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/library:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  lanrurugi
```

预构建镜像发布在 `ghcr.io/thelastfantasy/lanrurugi` 和 Docker Hub 的 `jade913/lanrurugi`。从 LANraragi 迁移：书库挂载
同一个漫画目录，缩略图和数据库用新的目录，再在首次运行向导里导入 LANraragi 的备份 JSON（见文档的安装页）。

一个全新的实例（或者一个从从未修改过密码的旧版安装迁移过来的实例）启动时，仍然使用的是旧版 LANraragi
自身的默认管理员密码 `kamimamita`（`crates/lanrurugi-api/src/auth.rs::DEFAULT_PASSWORD_HASH` 的
bcrypt 哈希对应的明文，与旧版 `Model/Config.pm::get_password` 的默认值一致）。**首次登录后请立即修改
密码**，通过设置页面——不要让一个使用默认密码的实例可以从本地网络外部访问。

### CLI 子命令

- `lanrurugi serve`——在同一进程中运行 HTTP API、静态前端、文件监视器和插件池。
- `lanrurugi rebuild-index`——用大小感知算法重新计算每个归档文件的 ID，并发现任何曾被历史误判合并隐藏的
  文件（用户故事 6）。
- `lanrurugi bench`——生成一个合成库，并针对一个已运行的旧版实例执行并发/吞吐量对比测试（用户故事 8；
  参见 `specs/001-lanrurugi-full-rewrite/quickstart.md` 第 8 节）。
- `lanrurugi formats`——只读普查库里实际存在的页面格式，并按"所有浏览器都能显示 / 取决于客户端能力 /
  没有主流浏览器能显示且本服务也无法转换"分类；最后一类（例如 HEIC、JPEG 2000、MNG、WebP2）是唯一
  无法靠转换挽救的情况，会被单独列出并给出示例档案。`--magic-samples-per-archive N` 会额外按内容
  识别每个档案中的 N 个条目，用于发现扩展名说谎的页面（例如被改名成 `.jpg` 的 HEIC）。


## 测试

```sh
mise run fmt
mise run clippy
mise run test -- -p lanrurugi-translate --lib   # 实际会跑完 workspace；见 .mise.toml 的 --no-fail-fast 说明
```

`scripts/cargo-container-run.sh` 会在 `127.0.0.1:6379` 没有 Redis 时自动起一个临时实例；**但仓库当前的
`.env.test.local` 会把 `LANRURUGI_TEST_REDIS_URL` 覆盖为 `redis://host.docker.internal:16380`**。如果
`16380` 上没有 Redis，`lanrurugi-storage` 的 `bootstrap::...` 测试会失败，表现为该测试二进制
`81 passed; 1 failed`——这是测试环境缺少 16380 Redis，而不是代码失败。启动一个监听 16380 的 Redis 后
即可得到 82/82。

若走裸 `cargo test`，不要设置 `LANRURUGI_TEST_REDIS_URL`，依赖 Redis 的测试会被优雅跳过；设置时请确保
它指向真实可连的 Redis，而不是照抄 README 旧示例里的端口。

### 前端测试（`specs/003-ui-test-automation/`）

```sh
mise run test-frontend-unit   # Vitest + React Testing Library——快速，无需后端
mise run test-frontend-e2e    # Playwright——真实后端 + Redis，Chromium + Firefox
```

`test-frontend-e2e` 会先构建后端，然后为每个测试 worker 启动其自己独立隔离的 Redis 实例、后端进程和
前端预览服务器（见 `apps/frontend/tests/e2e/fixtures.ts`），每次都从一个干净的状态开始。设置
`KEEP=1 mise run test-frontend-e2e` 可以在某一次运行中跳过清理步骤，以便之后检查其环境（Redis/库状态）
——这不会在那一次运行之外持续存在。完整验证指南见 `specs/003-ui-test-automation/quickstart.md`。


## 设计文档（`specs/`）

每个功能在自己的 `specs/<编号>-<名字>/` 下带 `spec.md`、`plan.md`、`tasks.md`（以及 `research.md`、
`data-model.md`、`contracts/`、`quickstart.md`）。`tasks.md` 的勾选状态是「实现到哪了」的唯一可信来源。

- [`specs/011-api-docs-site/`](./specs/011-api-docs-site/) —— Phase 1 附加功能（增量添加，已实现）：
- [`specs/001-lanrurugi-full-rewrite/`](./specs/001-lanrurugi-full-rewrite/) —— Phase 1 的
- [`specs/002-job-console/`](./specs/002-job-console/) —— Phase 1 附加功能（增量添加，已实现）：
- [`specs/003-ui-test-automation/`](./specs/003-ui-test-automation/) —— Phase 1 附加功能（增量添加，
- [`specs/005-download-plugin-progress/`](./specs/005-download-plugin-progress/) —— Phase 1
- [`specs/007-guest-restricted-access/`](./specs/007-guest-restricted-access/) —— Phase 1 附加
- [`specs/004-ocr-manga-translation/`](./specs/004-ocr-manga-translation/) —— Phase 2（依赖于
- [`docs/translation-local-backend.md`](./docs/translation-local-backend.md) —— 用本地模型
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) —— 项目治理、架构原则和

- [`docs/translation-local-backend.md`](./docs/translation-local-backend.md) —— 用本地模型（如 Ollama）跑页面内翻译的零额外安装配置指南。
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) —— 项目治理、架构原则和技术栈决策。

## 许可证

MIT —— 见 [LICENSE](./LICENSE)。提交即表示你同意以同一许可证授权你的贡献。
