---
title: 安装（Docker / NAS）
description: 用 Docker 或 Compose 在服务器/NAS 上部署 LANrurugi。
---

LANrurugi 的服务端是一个常驻进程，前端静态资源、HTTP API、文件监视器和后台任务都在里面；插件、电子书解析和
AI 推理在按需启动的子进程里运行。它有自己的数据库，不读取也不改动 LANraragi 的数据库，两者可以同时运行。

:::tip[从 LANraragi 迁移]
1. 在 LANraragi 的设置 →「Database Backup/Restore」→「Backup Database」下载备份 JSON，LANraragi 可以继续运行。
2. 启动 LANrurugi：书库挂载和 LANraragi 相同的漫画目录；缩略图和数据库也要挂载，但必须用新的目录，不要和
   LANraragi 的相同。[主页](https://lrrg.dev/#start)上的转换器能根据 LANraragi 的部署命令或
   compose 文件生成。
3. 用默认密码 `kamimamita` 登录，首次运行向导会提示你修改。
4. 在向导的「从 LANraragi 备份导入」一步上传 JSON（之后也能在设置 →「数据库备份/恢复」里导入）。标签、分类、
   合集和 stamps 会恢复，阅读进度不在备份里。
:::

不想用容器的话，也可以用 Linux、macOS（Apple Silicon）和 FreeBSD 的[原生安装包](../install-native/)。

## 用 Compose 部署（推荐）

把下面这段保存为 `compose.yaml`，按注释把左侧的宿主机目录改成你自己的，然后启动：

```yaml
services:
  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:latest
    container_name: lanrurugi
    ports:
      - "3000:3000"
    volumes:
      # 书库，可以直接指向旧版 LANraragi 的 content 目录
      - ./data/manga:/home/koyomi/lanrurugi/content
      # 缩略图缓存
      - ./data/thumb:/home/koyomi/lanrurugi/thumb
      # Redis 数据，LANrurugi 专用；LANraragi 的元数据通过备份 JSON 导入
      - ./data/database:/home/koyomi/lanrurugi/database
      # 推荐和单行本整理用的本地模型，首次用到时自动下载，非必需
      - ./data/models:/home/koyomi/lanrurugi/models
    environment:
      # 经 HTTPS 反向代理访问时改为 true
      - LANRURUGI_FORCE_SECURE_COOKIES=false
    restart: unless-stopped
```

```sh
docker compose up -d      # 或 podman compose up -d
```

在群晖 Container Manager、Portainer 等面板里新建一个 compose 项目，粘贴同一段文本即可。

启动后在浏览器打开 `http://<服务器地址>:3000`。镜像自带 Redis，不需要额外的数据库容器。四个挂载目录：

| 宿主机路径 | 容器内路径 | 用途 |
|---|---|---|
| `./data/manga` | `/home/koyomi/lanrurugi/content` | 你的档案库 |
| `./data/thumb` | `/home/koyomi/lanrurugi/thumb` | 缩略图缓存 |
| `./data/database` | `/home/koyomi/lanrurugi/database` | Redis 数据（LANrurugi 专用） |
| `./data/models` | `/home/koyomi/lanrurugi/models` | 推荐和单行本整理用的本地模型缓存 |

要通过域名和 HTTPS 访问，见[反向代理](../reverse-proxy/)。

## 直接用 Docker 运行

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/library:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

## 镜像来源

同一个镜像同时发布在两个仓库，内容完全相同，都包含 amd64 和 arm64 两种架构：

| 仓库 | 镜像名 |
| --- | --- |
| GitHub Container Registry（推荐） | `ghcr.io/thelastfantasy/lanrurugi` |
| Docker Hub | `jade913/lanrurugi` |

上面的示例用的是 GitHub 的仓库。Docker Hub 对未登录用户有拉取次数限制；如果你的 NAS 面板只能从 Docker Hub
搜索镜像，或者你更习惯用它，把镜像名换成 `jade913/lanrurugi` 即可。`latest` 指向最新的正式版本，
也可以写具体的版本号。

## 从源码构建镜像

仓库根目录的 `compose.yaml` 用于从源码构建并试运行（`network_mode: host`，监听 3000 端口）：

```sh
podman compose up -d --build      # 或 docker compose up -d --build
```

## 首次登录

首次打开时会出现首次运行向导：修改管理员密码、填写 AI 功能用的 DeepSeek API 密钥、从 LANraragi 备份导入
数据，每一步都可以跳过，之后也能在设置页面重新运行。

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
