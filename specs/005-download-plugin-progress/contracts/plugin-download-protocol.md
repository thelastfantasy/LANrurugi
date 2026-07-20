# Contract: Download Plugin SDK Extensions (`downloads[]`, `pluginOptions()`)

Extends `specs/001-lanrurugi-full-rewrite/contracts/plugin-protocol.md`'s existing `exec_download`
method result shape and adds one new, optional dispatcher method (`plugin_options`), parallel to
the existing `plugin_info`. Transport (NDJSON request/response over the subprocess's stdin/stdout)
is unchanged — see that contract for the envelope.

## `exec_download` result — extended `DownloadResult`

```json
{
  "downloads": [
    {
      "url": "https://example.com/archive.zip",
      "method": "GET",
      "headers": { "Referer": "https://example.com/artwork/123" },
      "filename_hint": "archive.zip"
    }
  ],
  "file_path": null,
  "error": null
}
```

| Field | Type | Notes |
|---|---|---|
| `downloads` | array, optional | **NEW.** One element = single-file download (e.g. Chaika/EHentai); more than one = a multi-resource download (e.g. Pixiv's per-page images), assembled per the plugin's `bundle_as_archive` setting (see `plugin_options` below). Absent when the plugin instead returns `file_path` (see below) or `error`. |
| `downloads[].url` | string | The real, directly-fetchable resource URL. |
| `downloads[].method` | string, optional | HTTP method. Defaults to `"GET"` when absent — every real corpus plugin (and legacy LANraragi's own `Model::Upload.pm::download_url`) only ever uses GET, but other methods remain representable for a future plugin that needs one. |
| `downloads[].headers` | object of string→string, optional | Extra request headers for this specific resource (e.g. Pixiv's anti-hotlink `Referer` header). Absent means no extra headers beyond whatever the host's own downloader sends by default. |
| `downloads[].filename_hint` | string, optional | A plugin-suggested filename. The host still prefers a `Content-Disposition` header from the real HTTP response when present (matching legacy `Model::Upload.pm::download_url`'s own behavior); this is a fallback for when the server provides no such header. |
| `file_path` | string, optional | **Pre-existing, unchanged.** A plugin that already downloaded/wrote a file itself and hands back a local path. Mutually exclusive with `downloads`; does not receive progress/concurrency/rate-limit treatment (the transfer already happened inside the plugin process). |
| `error` | string, optional | *(existing, unchanged)* |

Exactly one of `downloads`, `file_path`, or `error` MUST be present in a successful/failed result;
a `downloads` array, if present, MUST have at least one element.

## `plugin_options` method (new, optional)

A plugin MAY implement this method (backing the plugin-authored `export function pluginOptions()`
in its `.ts` source — see the SDK reference below). The host calls it the same way it already
calls `plugin_info` (a cheap, zero-extra-permission subprocess call — no `downloads`/`file_path`
side effects). A plugin that exports no `pluginOptions()` simply has no `plugin_options` method to
call; the host treats that as "no configurable options" (spec FR-015), not an error.

**Request**: `{"request_id": "...", "plugin": "namespace", "method": "plugin_options", "args": {}}`

**Response `result`** (`PluginOptionsResult`):

```json
{
  "domain_rules": [
    {
      "pattern": "*.pixiv.net",
      "max_concurrent": 2,
      "max_bytes_per_sec": null,
      "description": "Limit simultaneous downloads from Pixiv's CDN"
    }
  ],
  "bundle_as_archive": {
    "default": true,
    "description": "Combine all downloaded pages into a single manga archive instead of one archive per page"
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `domain_rules` | array, optional | The plugin's own declared default `Domain Rule`s (data-model.md) for the domain(s) it targets. Absent/empty means the plugin declares no concurrency/rate-limit defaults of its own — that domain is unmanaged unless a user override exists (FR-017). |
| `domain_rules[].pattern` | string | Exact hostname or wildcard (data-model.md's `Domain Rule.pattern`); omit for a general, non-domain-specific fallback rule. |
| `domain_rules[].max_concurrent` | integer, optional | Plugin's declared default concurrency cap for domains matching `pattern`. |
| `domain_rules[].max_bytes_per_sec` | integer, optional | Plugin's declared default rate limit for domains matching `pattern`. |
| `domain_rules[].description` | string | Human-readable explanation shown in the settings UI (FR-011). |
| `bundle_as_archive` | object, optional | Only meaningful for a plugin whose `exec_download` can return more than one `downloads[]` element. Absent for a single-resource-only plugin (no such setting shown at all). |
| `bundle_as_archive.default` | boolean | The plugin's own declared default (Pixiv: `true`). |
| `bundle_as_archive.description` | string | Human-readable explanation shown in the settings UI. |

A `PluginOptionsResult` with every field absent/empty is equivalent to not implementing
`plugin_options` at all — the host shows no settings UI for that plugin either way (FR-015).

## SDK reference addition (`crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`)

Plugin authors write this as a plain additional `export function`, parallel to the existing
`pluginInfo()`:

```ts
export function pluginOptions(): PluginOptionsResult {
  return {
    domain_rules: [
      { pattern: "*.pixiv.net", max_concurrent: 2, description: "..." },
    ],
    bundle_as_archive: { default: true, description: "..." },
  };
}
```

Omitting `pluginOptions()` entirely is valid and is the expected shape for every non-download
plugin, and for a download plugin with nothing to configure.
