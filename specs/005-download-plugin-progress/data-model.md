# Phase 1 Data Model: Download Plugin Progress, Concurrency & Rate Limiting

Source of truth remains Redis for persisted settings (constitution Principle I — additive
namespace, no existing key shape touched) and the existing in-process `JobRegistry` (unchanged
storage model, extended fields) for job/progress state.

## Download Job (extension of the existing `JobStatus`)

`lanrurugi_core::jobs::JobStatus` gains two new optional fields alongside the existing plain
`progress: f32`:

| Field | Type | Notes |
|---|---|---|
| `id` | string | *(existing, unchanged)* |
| `name` | string | *(existing, unchanged)* |
| `state` | enum (`queued`\|`active`\|`finished`\|`failed`) | *(existing, unchanged)* |
| `progress` | f32 (0.0–1.0) | *(existing, unchanged)* — for a download job, kept in sync with `downloaded_bytes / total_bytes` when both are known, so existing non-download-aware UI (if any) still gets a sane fraction. |
| `downloaded_bytes` | `Option<u64>` | **NEW.** Bytes transferred so far, updated incrementally as the Rust-side streaming download proceeds (FR-001). `None` until the download has actually started transferring bytes (i.e. still queued/resolving). |
| `total_bytes` | `Option<u64>` | **NEW.** Total expected size, taken from the response's `Content-Length` when present. `None` when the server doesn't report a size (FR-002) — the frontend renders an indeterminate indicator in that case rather than treating `None` as zero. For a multi-resource download (Pixiv-style), this is the *sum* across all resources once each one's size becomes known (FR-003: combined, not per-resource). |
| `result` | JSON, optional | *(existing, unchanged)* — still only populated at `finish()` time. |
| `error` | string, optional | *(existing, unchanged)* |

**New `JobRegistry` method**: `set_download_progress(&self, id: &str, downloaded: u64, total: Option<u64>)`
— sibling to the existing `set_progress(&self, id: &str, progress: f32)`, called by the download-
manager on each streamed chunk (or at a throttled interval, to avoid excessive lock contention on
very fast local-network transfers — exact throttling interval is an implementation-time tuning
detail, not a data-model concern).

**Lifecycle**: `Queued` → `Active` (plugin resolving `downloads[]`/`file_path`) → `Active` with
`downloaded_bytes` incrementing (Rust performing the real transfer(s)) → `Finished` (archive(s)
cataloged) or `Failed` (network error, non-2xx response, or cataloging failure — FR-004: no
partial/half-cataloged archive is left behind on failure).

## Download Plugin Settings

Per-plugin, persisted in Redis under a new, additive namespace (not part of any existing
legacy-compatible key shape). Two logical layers, kept separate so a plugin's own declared default
remains recoverable (spec Key Entities):

| Field | Type | Notes |
|---|---|---|
| `plugin_namespace` | string | Primary key component — the plugin's own `namespace` field from `pluginInfo()` (already the stable per-plugin identifier used elsewhere, e.g. `use_plugin_sync`). |
| `declared_defaults` | `PluginOptionsResult` (see contracts/) | **Not stored** — recomputed by calling the plugin's `pluginOptions()` export fresh each time (cheap: same zero-permission subprocess pattern already used for `plugin_info()` calls elsewhere, e.g. `upload_plugin`'s verification call). Documented here as a logical layer, not a Redis field, to make clear the plugin's own default is never persisted/duplicated — only the user's override is. |
| `user_overrides` | JSON object, optional | **NEW Redis field**, keyed by `plugin_namespace`. Present only for fields the user has actually changed; a field the user never touched is absent here and the plugin's own `declared_defaults` value applies (FR-012's "custom value if set, otherwise the plugin's own built-in default"). Shape mirrors `PluginOptionsResult` minus its descriptive/label metadata (which only ever comes from the plugin, never user-editable). |

**Validation rule (FR-014)**: a concurrency limit or rate limit MUST be a positive integer (bytes/
sec, or max simultaneous downloads) if present at all; a negative or zero value is rejected at
save time with an explanation, not silently clamped or discarded. `bundle_as_archive` is a plain
boolean, always valid.

**No system-level default (FR-017)**: absence of both a plugin-declared default and a user
override for a given `Domain Rule` means that domain is unmanaged (unlimited) — this is a
computed absence, not a stored "unlimited" sentinel value.

## Domain Rule

A single rule pairing a domain pattern with a concurrency limit and/or rate limit. Not a
standalone Redis-keyed entity — always nested inside one plugin's `Download Plugin Settings`
(either as part of its `declared_defaults`, computed fresh from `pluginOptions()`, or as part of
its `user_overrides`).

| Field | Type | Notes |
|---|---|---|
| `pattern` | string | Either an exact hostname (`"example.com"`) or a wildcard covering subdomains (`"*.example.com"`). Case-insensitive; no port/scheme component (per spec Assumptions). A pattern of `"*"` (or the rule with no `pattern` field at all) represents the general, non-domain-specific fallback (FR-009's "general rate limit usable as a fallback"). |
| `max_concurrent` | `Option<u32>` | Per-domain concurrency cap (FR-005/FR-006/FR-007). Absent = no concurrency limit from this rule. |
| `max_bytes_per_sec` | `Option<u64>` | Per-domain rate limit (FR-009). Absent = no rate limit from this rule. |

**Matching precedence (FR-007, FR-009)**: for a given download's target hostname, an exact-
hostname rule match always wins over a wildcard rule match, which always wins over the general
(`"*"`/pattern-less) fallback rule. Concurrency and rate-limit resolution are independent —
a target could match its rate limit via a wildcard rule while its concurrency limit comes from a
separate exact-hostname rule, if that's how the plugin/user configured the two lists.

## Relationships

- One **Download Job** may involve one or more real HTTP transfers (single-URL or Pixiv-style
  multi-resource), each of which consults the initiating plugin's current effective **Download
  Plugin Settings** (declared defaults merged with any user override) to resolve the **Domain
  Rule** governing its target hostname, for both concurrency admission and rate-limit throttling.
- **Download Plugin Settings** belongs to exactly one plugin (`plugin_namespace`); a plugin with no
  `pluginOptions()` export at all has no settings row and shows no settings UI at all (FR-015).
