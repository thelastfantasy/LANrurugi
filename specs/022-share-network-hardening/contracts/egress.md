# Contract: Plugin Egress

## When the shim loads

Only when an interface is pinned (`plugin_bind_interface` or `share_bind_interface` non-empty) and the platform
supports it. The mere presence of `liblanrurugi_egress.so` no longer enables it.

## What the shim must do

| Hook | Behaviour |
|---|---|
| `socket()` | remember the socket; do not bind yet |
| `connect()` / `sendto()` / `sendmsg()` / `sendmmsg()` | set `SO_BINDTODEVICE` once per socket before the first send |
| `getaddrinfo` path (UDP DNS via `sendto`) | covered by the first-send binding |
| link-local IPv4 `169.254/16` | skipped like IPv6 link-local |

Unable to bind → return the error to the caller. Never send on the default route.

## What egress mode must not change

The Deno-side dispatcher keeps, in both relay and egress mode:

- per-host and global concurrency limits,
- request coalescing/singleflight,
- ETag revalidation and its cache,
- per-request and connect timeouts,
- response size cap,
- the TLS root store (`with_roots`).

## Interface switch

1. Pool enters `draining`: new invocations wait.
2. In-flight invocations run to completion, bounded by the plugin timeout.
3. Workers restart on the new interface; waiting invocations resume.

## Platforms

| Platform | Mechanism | Failure mode |
|---|---|---|
| Linux | `SO_BINDTODEVICE` via LD_PRELOAD | request fails with `interface_bind_failed` |
| FreeBSD | `setfib` wrapper for the Deno subprocess | subprocess not started if the FIB is missing |
| macOS | none (needs entitlements) | pinning refused at settings save with an explanation |

## Observable state

`GET /api/settings` reports `egress_binding: "pinned" | "auto" | "unsupported" | "failed"` per interface, so the UI can
say whether the guarantee is in force.
