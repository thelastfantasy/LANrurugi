---
title: Testing the API from these docs
description: Send requests from your browser straight to your own running LANrurugi instance — preconditions, security boundaries, and what to do when it does not work.
---

Every endpoint in the [API reference](../../api/) has a request panel: enter a server address, enter a
token, press send. This page explains exactly where that request goes, what it requires, and how to
continue when the browser blocks it.

## Where the request goes

**From your browser, straight to the address you type into the panel.**

- The address and token you enter below live only in your browser's local storage, and are sent only
  to that address.
- This project has **no** intermediate server and **no** proxy. There is no path by which a user's
  token or internal address reaches this project's infrastructure.
- The one exception is the page itself: the API description document (`openapi.json`) and the
  JavaScript that renders it are served from this documentation site's own origin, not a CDN.

This feature therefore needs no "public demo instance" — it was always aimed at your own instance.

## Preconditions

### 1. Your instance must be reachable from this browser

The address you enter must be reachable from **this browser**. If you are running the docs site on one
machine and the instance on another, then `localhost` means the machine the browser is running on, not
the machine the instance is running on.

### 2. Enable cross-origin access on the instance

**"Enable CORS for the client API"** in the settings page must be on. It is off by default, and the
browser will refuse cross-origin requests while it is.

Once enabled, the instance permits the headers a browser needs to preflight — including the
`Content-Type` that any JSON-bodied request requires. If your instance is on an older version and
calls with a body always fail, that is usually this setting not covering it.

### 3. An HTTPS page can only call HTTPS instances and `http://localhost`

This rule is enforced by the browser, not by LANrurugi:

| Docs site | Instance address you enter | Result |
|---|---|---|
| `https://…` | `https://lrr.example.com` | ✅ works |
| `https://…` | `http://localhost:3000` | ✅ works (browsers treat `localhost` as a trusted origin) |
| `https://…` | `http://192.168.1.10:3000` | ❌ blocked as mixed content |
| `http://…` | anything | ✅ works (the site itself is HTTP) |

A LAN IP does **not** get `localhost`'s exemption. To test a LAN instance from an HTTPS docs site,
there are two routes:

- **Give the instance HTTPS**: a reverse proxy (Caddy / Traefik / nginx), a Cloudflare Tunnel, or a
  certificate-bearing entry point inside a VPN.
- **Forward the port to localhost**:
  ```sh
  ssh -L 3000:127.0.0.1:3000 <the machine running the instance>
  ```
  then enter `http://localhost:3000` in the panel. The browser treats it as a trusted origin and it
  works directly.

## When the browser will not allow it: use the command line

**Every endpoint has a copyable command line (cURL)** in its code-sample panel. It is generated from
the endpoint itself and depends on neither the browser, CORS, nor HTTPS, so it always works — reach
for it whenever sending from the panel fails.

```sh
curl -H "Authorization: Bearer <your token>" \
  "http://192.168.1.10:3000/api/archives?start=0&count=10"
```

## Reading a panel error

- **Network error / failed to fetch**: check the three preconditions above first. Most often it is
  CORS not being enabled, a wrong address, or the mixed-content combination.
- **401**: the token is wrong, or it has no permission for this endpoint.
- **403**: the token is a guest token and this endpoint is administrator-only.
- **404**: the path or the archive ID is wrong.

## Tokens

Create one under token management in the settings page. The token field in the panel only puts it into
an `Authorization: Bearer <token>` header, stores it in your browser's local storage, and sends it only
to the address you entered.

:::caution[Do not enter a token on somebody else's documentation site]
The premise of this page is that the documentation site is a static page. But "static page" is not the
same as "trusted page" — only enter a token on a deployment you trust. If you are unsure whether a
docs site is an official deployment, use cURL instead.
:::
