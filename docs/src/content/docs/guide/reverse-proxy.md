---
title: 反向代理
description: 用 Nginx、Caddy、Traefik、Apache 或 Nginx Proxy Manager 为 LANrurugi 配置域名与 HTTPS。
---

LANrurugi 本身只提供 HTTP（默认端口 3000，同时监听 IPv4 和 IPv6）。要通过域名和 HTTPS 访问，在它前面放一个反向代理。
无论用哪一种代理，都要满足下面几条。

## 代理需要满足的条件

- **挂在域名根路径下。** 前端以绝对路径 `/api/...` 请求接口，不支持挂在子路径（如
  `example.com/lanrurugi/`）下。请用独立的域名或子域名，例如 `manga.example.com`。
- **转发三个请求头。** `X-Forwarded-Proto`（判断是否走 HTTPS）、`X-Forwarded-Host`（跨域登录交接与
  OpenSearch 生成正确的地址）、`X-Forwarded-For`（活动记录里显示真实客户端 IP）。同时保留原始 `Host`。
- **不要缓冲 SSE。** 下载队列进度、AI 插件向导与拆包建议使用 Server-Sent Events 实时推送。代理一旦缓冲
  响应，这些界面就会卡住、直到请求结束才一次性刷新。服务端每 15 秒发送一次心跳，所以默认的读超时
  （通常 60 秒）够用，但缓冲必须关闭。
- **放开上传大小。** 档案上传以每片最多 64 MB 分片发送，旧版数据导入单次最多 100 MB。代理的请求体
  上限要不小于这个值，最简单的做法是不设上限。
- **走 HTTPS 后打开安全 Cookie。** 在 compose 里设置 `LANRURUGI_FORCE_SECURE_COOKIES=true`。服务端不会
  根据 `X-Forwarded-Proto` 自动推断这一项——那个请求头可以被伪造，而本服务没有可信代理白名单。只走 HTTP
  时**不要**打开它，否则浏览器会丢弃登录 Cookie，导致无法登录。

不需要 WebSocket。

下面的示例都假设 LANrurugi 与代理在同一台机器上，监听 `127.0.0.1:3000`；如果代理和 LANrurugi 在同一个
compose 网络里，把地址换成 `lanrurugi:3000`。

## Nginx

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name manga.example.com;

    ssl_certificate     /etc/ssl/manga.example.com/fullchain.pem;
    ssl_certificate_key /etc/ssl/manga.example.com/privkey.pem;

    # 不限制上传大小（分片上传每片最多 64 MB）
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;

        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-Host  $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header Connection        "";

        # SSE：不缓冲，长连接不超时
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 1h;
        proxy_request_buffering off;
    }
}

server {
    listen 80;
    server_name manga.example.com;
    return 301 https://$host$request_uri;
}
```

## Caddy

Caddy 自动申请证书、默认转发 `X-Forwarded-*`，并且会立即刷新 `text/event-stream` 响应，所以配置最短：

```caddyfile
manga.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

## Traefik

在 LANrurugi 的 compose 服务上加 labels（假设 Traefik 已经有名为 `websecure` 的 entrypoint 与名为
`letsencrypt` 的证书解析器，且两者在同一个 Docker 网络里）：

```yaml
services:
  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:latest
    # 由 Traefik 对外提供服务时，不需要再发布 3000 端口
    volumes:
      - ./data/manga:/home/koyomi/lanrurugi/content
      - ./data/thumb:/home/koyomi/lanrurugi/thumb
      - ./data/database:/home/koyomi/lanrurugi/database
      - ./data/models:/home/koyomi/lanrurugi/models
    environment:
      - LANRURUGI_FORCE_SECURE_COOKIES=true
    labels:
      - traefik.enable=true
      - traefik.http.routers.lanrurugi.rule=Host(`manga.example.com`)
      - traefik.http.routers.lanrurugi.entrypoints=websecure
      - traefik.http.routers.lanrurugi.tls.certresolver=letsencrypt
      - traefik.http.services.lanrurugi.loadbalancer.server.port=3000
    restart: unless-stopped
```

Traefik 默认转发 `X-Forwarded-*`、不缓冲响应、不限制请求体大小，无需额外中间件。

## Apache httpd

需要启用 `mod_proxy`、`mod_proxy_http`、`mod_headers` 与 `mod_ssl`：

```apache
<VirtualHost *:443>
    ServerName manga.example.com

    SSLEngine on
    SSLCertificateFile    /etc/ssl/manga.example.com/fullchain.pem
    SSLCertificateKeyFile /etc/ssl/manga.example.com/privkey.pem

    ProxyPreserveHost On
    # mod_proxy 会自动设置 X-Forwarded-For 与 X-Forwarded-Host
    RequestHeader set X-Forwarded-Proto "https"

    # flushpackets=on 让 SSE 事件立即送达浏览器
    ProxyPass        / http://127.0.0.1:3000/ flushpackets=on timeout=3600
    ProxyPassReverse / http://127.0.0.1:3000/

    # 不限制上传大小
    LimitRequestBody 0
</VirtualHost>
```

## Nginx Proxy Manager

许多 NAS 用户用它管理证书。新建一个 Proxy Host：

- **Details**：Domain 填 `manga.example.com`，Scheme 选 `http`，Forward Hostname / IP 与 Port 填 LANrurugi
  的地址和 `3000`。
- **SSL**：申请证书，勾选 *Force SSL*。
- **Advanced** 中粘贴：

```nginx
client_max_body_size 0;
proxy_buffering off;
proxy_request_buffering off;
proxy_read_timeout 1h;
proxy_set_header X-Forwarded-Host $host;
```

Nginx Proxy Manager 默认已经转发 `Host`、`X-Forwarded-Proto` 与 `X-Forwarded-For`。

## 检查是否配置正确

1. 打开 `https://manga.example.com`，能正常登录且刷新后仍保持登录。
2. 在下载队列里添加一个任务，进度条应当连续推进，而不是在结束时才一次跳到 100%。
3. 活动记录页（`/activity`）里，登录记录显示的是你的客户端 IP，而不是代理的地址。
