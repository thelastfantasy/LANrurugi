---
title: 快速开始
description: 首次登录、建立 API 令牌、扫描库、导入旧数据。
---

## 1. 登录

打开 `http://<你的实例地址>:3000`，用默认密码 `kamimamita` 登录，然后**立刻在设置页面改掉它**。

## 2. 指向你的库

`--library-path`（或 `LANRURUGI_LIBRARY_PATH`）指向放档案的目录。默认是 `./library`；Compose 部署里
对应容器的 `/home/koyomi/lanrurugi/content`。

## 3. 扫描

在设置页面触发一次全量扫描。扫描是并发的（Rayon），并且**不会**自动删除档案记录——「扫描时发现文件消失」
只会清理路径簿记，档案记录本身归「数据库清理」管，是一个独立的、需要你明确触发的操作。

## 4. 建立 API 令牌

API 参考里的「Test it」面板需要令牌。在设置页面的令牌管理里创建一个，然后：

```sh
curl -H "Authorization: Bearer <你的令牌>" http://localhost:3000/api/archives
```

令牌按权限分为管理员与访客两类；访客令牌只能读取。

## 5. 从旧版 LANraragi 迁移

LANrurugi 用自己的数据库，不读取也不改动 LANraragi 的数据，两边可以同时运行。步骤见[安装](../install/)页的
「从 LANraragi 迁移」：书库挂载同一个漫画目录，缩略图和数据库用新的目录，再在首次运行向导里导入 LANraragi 的
备份 JSON。

导入按文件名把备份里的档案对应到 LANrurugi 扫描到的档案，不比较档案 ID：LANrurugi 用大小感知指纹
（`hash(前 512KB ++ 大端 u64 文件大小)`）作为 ID，和 LANraragi 的 `SHA-1(前 512KB)` 不同。同名文件有多个时
不会猜测，计入「匹配不唯一」。导入前请等书库扫描完成，还没扫描到的档案会被算作「无匹配而跳过」。
LANraragi 曾把开头字节相同的不同文件误判为重复并合并，这些文件在 LANrurugi 里会各自出现。

## 6. 试试 API

打开 [API 参考](../../api/)，在请求面板里把服务器地址改成你自己的实例（默认已经是
`http://localhost:3000`），填上令牌，然后直接发请求。请求从你的浏览器直接到你的实例，不经过本项目。

浏览器不允许的时候（例如文档站是 HTTPS 而你的实例是局域网里的纯 HTTP），每个端点都提供了一份可直接
复制的命令行。

细节见[用文档站测试 API](../api-testing/)。
