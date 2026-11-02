---
title: Plugin authoring
description: Write LANrurugi's metadata / login / download plugins in TypeScript.
---

A LANrurugi plugin is a **single `.ts` file**, placed under `plugins/<category>/` or uploaded through
the settings page / `POST /api/plugins/upload` (which lands in `plugins/custom/<category>/`). It runs
in a **separate Deno subprocess** holding only the permissions the plugin itself declares
(`--allow-net` / `--allow-read` and so on), with no access to anything in the host process.

## Minimal plugin

A plugin file must export exactly two things:

1. `pluginInfo()` — synchronous, side-effect free, returning only the plugin's own metadata:
   namespace, name, parameters, declared permissions. The host calls it in a **zero-permission**
   one-shot subprocess purely to read that metadata, and only then requests the real permissions it
   declared.
2. The execution function for its category — `execMetadata` (metadata fetching), `execLogin` (login),
   or `execDownload` (downloading).

For the exact signatures, the types, and what every field means, see the **[SDK reference](../sdk/)**:
`deno doc` generates it directly from the same TypeScript the runtime loads, so it cannot drift from
the real behaviour.

## Permissions

A plugin has to **declare up front** what it needs, and the host grants exactly that. This is what
stops a misbehaving or malicious plugin from taking down the host process or reading files it should
not. Any network or file access beyond the declaration fails.

## Debugging

- The settings page's plugin list shows each plugin's `pluginInfo()` result, including its declared
  permissions.
- When a plugin call fails, the host records the subprocess's stderr alongside it; the logs page can
  be filtered by category.
- HTTP requests are relayed through the host and minimised on demand — plugin requests are proxied by
  the host, so a plugin needs nothing beyond `--allow-net`, and they can be rate-limited and logged in
  one place.

## Next

- [SDK reference](../sdk/) — every exported type and function, with its documentation comments.
- [API reference](../../api/) — the endpoints under `/api/plugins*` (list, upload, execute, export).
