# Phase 1 — Data Model: Direct Download Domain

The feature persists nothing new. The model below describes the two existing values the validator reads and the one
derived result it returns.

## Setting: `direct_download_url`

| Property | Value |
|---|---|
| Owner | general settings (`LRR_CONFIG`) |
| Type | string, may be empty |
| Meaning | absolute URL of the downloads-only host; empty disables the split (all downloads on the page host) |
| Validation | absolute; scheme `http` or `https`; has a host; no userinfo, fragment or query; path component allowed but normalized to no trailing slash |
| Cross-check | if `share_public_url` is an `https://` origin, the scheme must be `https` |

## Derived: effective download base

```
download_base =
    direct_download_url (trimmed, trailing "/" removed)   if set
    otherwise the page base (share_public_url or the request origin)
```

Used by: `GET /api/archives/{id}/download-link`, the share page's download base, `apps/frontend/src/api/download.ts`.
Images and thumbnails never use it.

## Derived: validation result

```rust
struct DownloadUrlCheck {
    ok: bool,
    error: Option<DownloadUrlError>,   // Shape | SchemeMismatch | HasCredentials | HasQueryOrFragment
}
```

| Code | When | HTTP |
|---|---|---|
| `Shape` | not an absolute URL, or no host, or unsupported scheme | 400 |
| `HasCredentials` | `user:pass@` present | 400 |
| `HasQueryOrFragment` | `?` or `#` present | 400 |
| `SchemeMismatch` | page origin is HTTPS and the value is HTTP | 400 |

`SchemeMismatch` is never raised for an HTTP page origin.

## State transitions

```
unset ──save(valid)──▶ set
set   ──save(invalid)─▶ set (unchanged; the error is returned, nothing is stored)
set   ──page origin changed to HTTPS and value is http──▶ set + warning (value kept)
```

The warning path exists because refusing the page URL would block the edit that is trying to fix the deployment.
