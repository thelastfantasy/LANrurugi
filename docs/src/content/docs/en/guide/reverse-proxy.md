---
title: Reverse proxy
description: Serve LANrurugi on a domain over HTTPS with Nginx, Caddy, Traefik, Apache or Nginx Proxy Manager.
---

LANrurugi itself only speaks HTTP (by default on port 3000, IPv4 and IPv6). To serve it on a domain over
HTTPS, put a reverse proxy in front of it. Whichever proxy you use, it has to meet the conditions
below.

## What the proxy has to do

- **Serve it at the root of a host.** The frontend requests the API at absolute `/api/...` paths, so
  mounting it under a sub-path (such as `example.com/lanrurugi/`) is not supported. Use a dedicated
  domain or subdomain such as `manga.example.com`.
- **Forward three headers.** `X-Forwarded-Proto` (whether the request came in over HTTPS),
  `X-Forwarded-Host` (correct addresses for cross-domain login handoff and OpenSearch) and
  `X-Forwarded-For` (the real client IP in the activity log). Keep the original `Host` as well.
- **Don't buffer SSE.** Download-queue progress, the AI plugin wizard and archive-split suggestions
  stream over Server-Sent Events. If the proxy buffers the response, those screens stall and then
  update all at once when the request ends. The server sends a heartbeat every 15 seconds, so the
  usual 60-second read timeout is enough — but buffering has to be off.
- **Allow large uploads.** Archive uploads are sent in chunks of up to 64 MB, and a legacy data
  import can be up to 100 MB in one request. The proxy's body-size limit has to be at least that;
  the simplest choice is no limit.
- **Turn on secure cookies once you're on HTTPS.** Set `LANRURUGI_FORCE_SECURE_COOKIES=true` in the
  compose file. The server does not infer this from `X-Forwarded-Proto`: that header can be forged
  and this service has no trusted-proxy allowlist. Do **not** turn it on over plain HTTP — browsers
  would drop the login cookies and you could not sign in.

WebSockets are not used.

The examples assume LANrurugi and the proxy run on the same machine with LANrurugi on
`127.0.0.1:3000`; if they share a compose network, use `lanrurugi:3000` instead.

## Nginx

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name manga.example.com;

    ssl_certificate     /etc/ssl/manga.example.com/fullchain.pem;
    ssl_certificate_key /etc/ssl/manga.example.com/privkey.pem;

    # No upload size limit (chunked uploads send up to 64 MB per chunk)
    client_max_body_size 0;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;

        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-Host  $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header Connection        "";

        # SSE: no buffering, no timeout on long-lived streams
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

Caddy obtains certificates automatically, forwards `X-Forwarded-*` by default and flushes
`text/event-stream` responses immediately, so the configuration is as short as it gets:

```caddyfile
manga.example.com {
    reverse_proxy 127.0.0.1:3000
}
```

## Traefik

Add labels to the LANrurugi compose service (this assumes Traefik already has an entrypoint named
`websecure` and a certificate resolver named `letsencrypt`, on the same Docker network):

```yaml
services:
  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:latest
    # Traefik serves it, so port 3000 no longer needs publishing
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

Traefik forwards `X-Forwarded-*`, doesn't buffer responses and doesn't limit request bodies by
default, so no extra middleware is needed.

## Apache httpd

Enable `mod_proxy`, `mod_proxy_http`, `mod_headers` and `mod_ssl`:

```apache
<VirtualHost *:443>
    ServerName manga.example.com

    SSLEngine on
    SSLCertificateFile    /etc/ssl/manga.example.com/fullchain.pem
    SSLCertificateKeyFile /etc/ssl/manga.example.com/privkey.pem

    ProxyPreserveHost On
    # mod_proxy sets X-Forwarded-For and X-Forwarded-Host itself
    RequestHeader set X-Forwarded-Proto "https"

    # flushpackets=on delivers SSE events to the browser immediately
    ProxyPass        / http://127.0.0.1:3000/ flushpackets=on timeout=3600
    ProxyPassReverse / http://127.0.0.1:3000/

    # No upload size limit
    LimitRequestBody 0
</VirtualHost>
```

## Nginx Proxy Manager

Many NAS users manage certificates with it. Create a Proxy Host:

- **Details**: Domain `manga.example.com`, Scheme `http`, Forward Hostname / IP and Port pointing at
  LANrurugi and `3000`.
- **SSL**: request a certificate and tick *Force SSL*.
- **Advanced**: paste

```nginx
client_max_body_size 0;
proxy_buffering off;
proxy_request_buffering off;
proxy_read_timeout 1h;
proxy_set_header X-Forwarded-Host $host;
```

Nginx Proxy Manager already forwards `Host`, `X-Forwarded-Proto` and `X-Forwarded-For` by default.

## Checking the setup

1. Open `https://manga.example.com`; you can sign in, and you stay signed in after a refresh.
2. Add a task to the download queue; its progress bar should advance steadily rather than jump to
   100% only when it finishes.
3. On the activity log page (`/activity`), sign-ins show your client IP rather than the proxy's
   address.
