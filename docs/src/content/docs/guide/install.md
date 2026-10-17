---
title: 安装（Docker / NAS）
description: 用 Docker 或 Compose 在服务器/NAS 上部署 LANrurugi。
---

LANrurugi 是单个二进制，前端静态资源、HTTP API、文件监视器与插件池都在同一个进程里。它复用你已有的
Redis，并且**不会**破坏性地迁移已有数据——`compose.yaml` 里挂载的 `data/database` 就是旧版 LANraragi
用的那份 Redis 数据目录。

## 用 Compose 启动

```sh
podman compose up -d      # 或 docker compose up -d
```

`compose.yaml` 使用的三个宿主机目录，与旧版 LANraragi 的 `data/` 布局一致：

| 宿主机路径 | 容器内路径 | 用途 |
|---|---|---|
| `./data/manga` | `/home/koyomi/lanrurugi/content` | 你的档案库 |
| `./data/thumb` | `/home/koyomi/lanrurugi/thumb` | 缩略图缓存 |
| `./data/database` | `/home/koyomi/lanrurugi/database` | Redis 数据（与旧版共用） |

容器使用 `network_mode: host`，服务监听 **3000** 端口（`LANRURUGI_BIND` 的默认值
`0.0.0.0:3000`）。

## 直接用 Docker 运行

```sh
docker build -t lanrurugi .
docker run -p 3000:3000 -v /path/to/library:/library lanrurugi
```

## 首次登录

一个全新实例——或者从一个从未修改过密码的旧版安装迁移过来的实例——使用的是旧版 LANraragi 自身的默认
管理员密码 **`kamimamita`**。

:::caution[第一件事就是改密码]
登录后请立刻在设置页面修改密码。不要让一个还在用默认密码的实例可以从本地网络之外访问。
:::

## 离线阅读本文档

文档站、API 正文参考、Swagger UI 和插件 SDK 参考都打包在镜像里，由实例自己在 `/docs` 下提供：

```text
http://<你的实例地址>:3000/docs/          指南与按分组的 API 正文参考（中文）
http://<你的实例地址>:3000/docs/en/       同上，英文版
http://<你的实例地址>:3000/docs/swagger/  Swagger UI，中文内容（可看 schema，可试发请求）
http://<你的实例地址>:3000/docs/swagger/en/  Swagger UI，英文内容
http://<你的实例地址>:3000/docs/sdk/      插件 SDK 参考
```

不需要外网：连 Swagger UI 的 JS 打包和 SDK 的搜索索引都在镜像里，没有任何资源来自 CDN。

## 工具链（从源码构建）

版本固定在 `.mise.toml` 里，`mise install` 会精确复现它们（Rust、Node、Deno、pnpm，以及
`sccache`/`mold`）。

```sh
# 后端
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# 前端开发服务器（把 /api 代理到上面的后端）
pnpm --filter lanrurugi-frontend run dev
```

### CLI 子命令

- `lanrurugi serve`——在同一个进程里运行 HTTP API、静态前端、文件监视器与插件池。
- `lanrurugi rebuild-index`——用大小感知算法重新计算每个档案的 ID，并发现历史上曾被误判合并而隐藏的
  文件。
- `lanrurugi bench`——生成合成库，并针对一个正在运行的旧版实例做并发/吞吐量对比。

## 下一步

- [快速开始](../quickstart/)
- [用文档站测试 API](../api-testing/)
