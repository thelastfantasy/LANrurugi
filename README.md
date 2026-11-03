简体中文 | [English](./README.en.md) | [日本語](./README.ja.md)

# LANrurugi

[![CI](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml)
[![CodeQL](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

[主页](https://lrrg.dev) · [文档](https://docs.lrrg.dev) · [API 参考](https://api.lrrg.dev) · [插件 SDK](https://sdk.lrrg.dev)

自托管的漫画 / 同人志库管理器。Rust 后端 + React 前端，从零重写 [LANraragi](https://github.com/Difegue/LANraragi)。

## 与 LANraragi 的关系

沿用 LANraragi 的库目录、标签语法和 HTTP API，现有客户端、阅读器与迁移流程都不用改 —— 挂上同一个漫画目录、
导入备份 JSON 即可。重写是为了性能与可靠性：Rust + Tokio 让缩略图 / OCR / 图像对比真正跑满多核，下载从插件
沙箱搬进宿主进程，才有字节级进度与按域名限速。逐项差异（包括刻意保留的）见
[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md)。

## 能做什么

- **库与阅读**：分类、标签、阅读进度、全文搜索与标签过滤；书架 / 网格 / 列表视图，界面支持 14 种语言。
- **插件**：元数据、下载、登录三类插件跑在 Deno 子进程里，按命名空间隔离权限；也能让 AI 向导按站点生成新插件。
- **下载**：字节级进度、暂停 / 断点续传、按域名并发上限与限速；订阅自动追更（发现新作 → 规则判定 → 入队），候选先预览再决定。
- **分享**：把档案或单行本分享给别人，对方在浏览器里读、下载，或用自己的 LANrurugi 直接导入 —— 直连、打洞、
  中继兜底，不依赖第三方服务。
- **页面内翻译**：OCR → 擦除原文 → 排版回填，云端 API 或本地模型均可，字体按卷缓存。
- **电子书**：PDF、EPUB、MOBI/AZW3、DjVu、FB2、TXT 与压缩包一起入库、一样阅读。
- **回收站与安全密码**：删除先移入回收站（保留期 / 容量可配），恢复时带回标签、分类、书签和阅读进度；
  永久删除与清空回收站需要单独的四位安全密码（连续输错会锁定），忘记可用登录密码或命令行重置。
- **原生安装包**：Linux（x86_64 / aarch64）、macOS（Apple Silicon）、FreeBSD，解压即用，不需要容器。

## 快速开始

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/manga:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

打开 `http://<服务器地址>:3000`。镜像自带 Redis，不需要额外的数据库容器；更完整的 compose（含 NAS 面板、
反向代理、从 LANraragi 迁移的步骤）见[文档站「安装」](https://docs.lrrg.dev/guide/install/)。

镜像有两个通道：`:latest` 跟随发布版，`:edge` 跟随 `dev` 分支的最新代码（可能坏，别指向正式书库）。

> **默认管理员密码是 `kamimamita`**（与 LANraragi 一致）。首次登录后立刻在设置页改掉，不要把默认密码的实例
> 暴露到局域网外。

## 文档

- 用户指南：<https://docs.lrrg.dev/>
- API 参考：<https://api.lrrg.dev/> —— 由服务端路由生成的 OpenAPI
- 插件 SDK：<https://sdk.lrrg.dev/>
- 给 AI 工具读的入口：<https://lrrg.dev/llms.txt>

## 参与贡献

见 [CONTRIBUTING.md](./CONTRIBUTING.md)：工具链固定方式、构建与测试命令、提交前必须通过的检查。设计文档在
`specs/` 下，每个功能一份 `spec.md` / `plan.md` / `tasks.md`。

## 变更记录

每次打 `v*` tag，CI 会从提交历史生成中英双语发布说明（应用内升级提示读的也是它）：
[Releases](https://github.com/thelastfantasy/LANrurugi/releases)。

## 许可证

MIT —— 见 [LICENSE](./LICENSE)。
