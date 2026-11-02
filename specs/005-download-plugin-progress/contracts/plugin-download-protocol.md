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
| `version_history` | array, optional | **NEW (issue #107).** Every revision of this download's series the plugin's site exposes — see below. Independent of `downloads`/`file_path`: a result may carry both. |
| `version_history[].source` | string | This revision's source URL, already normalized through this plugin's own `canonicalize_source` (see below). |
| `version_history[].posted_at` | string | ISO 8601 timestamp the site reports for this revision. Sole determiner of order. |
| `error` | string, optional | *(existing, unchanged)* |

Exactly one of `downloads`, `file_path`, or `error` MUST be present in a successful/failed result;
a `downloads` array, if present, MUST have at least one element. `version_history` is orthogonal to
that rule — it accompanies a result, it is never the result.

### `version_history` (issue #107)

The plugin enumerates what revisions its site says exist and reaches **no conclusion**; the host
compares that list against its own catalogue to decide whether this download supersedes (or is
superseded by) an existing archive, and applies the user's `relative_newer_policy`/
`relative_older_policy` accordingly.

Keeping the judgment host-side means one tested implementation shared by every plugin, rather than
each plugin — especially an AI-generated one (`specs/006-ai-plugin-wizard/`) — reimplementing a
multi-hop reachability walk at varying quality. Enumerating a list is mechanical I/O code; deciding
direction and distance is not.

Requirements:

- Every `source` MUST already be passed through this plugin's own `canonicalize_source`. The host
  compares by plain string equality, so an un-normalized entry silently fails to match with no
  error surfaced anywhere.
- The list MUST include the just-downloaded revision itself; the host anchors on it to measure
  direction. A list that omits it yields no relation at all.
- Order does not matter — the host sorts by `posted_at`.
- There is deliberately **no** "this one is current" flag: a second source of truth could contradict
  the `posted_at` sort, leaving the host no way to know which to believe.
- Absent when the site has no version-history concept, or this download has none (a first-ever
  upload with no parent and no later revision).

**E-Hentai's `namespace` parameter is per-call, not per-plugin.** `gdata` only returns `current_gid`
(the series tip) under `namespace: 0`, so the version-history walk must use that value. But
`namespace: 0` also returns every tag *stripped of its namespace prefix* — verified live against a
real gallery, where all 31 tags came back as `artist:<value>`/`male:<value>` under `namespace: 1`
and as bare `<value>` under `namespace: 0`.

So `plugins/download/ehentai.ts` uses `namespace: 0` (it reads only `gid`/`token`/`posted`/
`parent_gid`/`current_gid` and never touches `tags`), while `plugins/metadata/ehentai.ts`
deliberately **stays on `namespace: 1`** — switching it would collapse every E-Hentai tag into the
bare-tag "Other" bucket, since inferring a namespace back from a bare tag value needs a Redis
reverse index this project has not built. Two call sites, two values, on purpose.

Example (E-Hentai, whose `gdata` API exposes ancestors via `parent_gid` and the series tip via
`current_gid`):

```json
{
  "downloads": [{ "url": "https://...", "filename_hint": "1000003_cccccccccc.zip" }],
  "version_history": [
    { "source": "e-hentai.org/g/1000001/aaaa", "posted_at": "2026-01-02T03:04:05.000Z" },
    { "source": "e-hentai.org/g/1000002/bbbb", "posted_at": "2026-02-02T03:04:05.000Z" },
    { "source": "e-hentai.org/g/1000003/cccccccccc", "posted_at": "2026-03-02T03:04:05.000Z" }
  ]
}
```

## `plugin_introspect` method (new)

Reports what a plugin's *source* says it supports, without running its entry points. The host calls
it on every `GET /api/plugins/{namespace}/options` request, matching `discover_namespaces`' own
rescan-per-request behavior — so editing a plugin file is reflected on the next settings-page load,
with no download ever having run and nothing declared twice in `pluginOptions()`.

**Request**: `{"request_id": "...", "plugin": "namespace", "method": "plugin_introspect", "args": {}}`

**Response `result`**:

```json
{ "returns_version_history": true }
```

| Field | Type | Notes |
|---|---|---|
| `returns_version_history` | boolean | Whether `execDownload`'s body ever returns a `version_history` key. Drives whether the settings UI offers the two relative-revision policies at all. |

Whether the plugin exports `canonicalizeSource` is deliberately not reported here: the host learns
that from `canonicalize_source` answering `null`, at the moment it matters, so a second
separately-derived signal for the same fact would only be one more thing that can disagree.

Detection is a lexical scan of the plugin's own source with comments and string/template literals
blanked first, so the key must appear as real code — a `version_history` mentioned only in a doc
comment or an error message does not count. The dispatcher imports no external modules at all (see
`dispatcher.ts`'s own top-of-file docs), which rules out a full TS parser there; what remains is a
property-name match, which is exactly the question being asked.

Recognized as support: `version_history` as a shorthand property, an explicit `version_history:`
key, or a quoted `"version_history":` key. Deliberately *not* recognized: a mention in a line or
block comment, a mention inside a string literal, and a property *read* (`result.version_history`) —
only a key the plugin writes counts, since reading one says nothing about whether it reports one.

## `canonicalize_source` method (new, optional)

Runs URLs through a plugin's own optional `canonicalizeSource` export. Batched: a `source:` tag
comparison normalizes both sides of every candidate pair, so a per-URL call would mean one
subprocess round-trip per library archive.

**Request**: `{"request_id": "...", "method": "canonicalize_source", "args": {"urls": ["..."]}}`

**Response `result`**: `{"canonical": ["..."]}`, one entry per input in the same order, or `null`
when the plugin exports no such function (the host then applies only its own generic `trim_url`).
A response whose length does not match the input is rejected outright — a silent misalignment would
corrupt every comparison that follows.

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
| `relative_newer_policy` | object, optional | **NEW (issue #107).** What to do when the host determines this download is a *newer* revision of an already-catalogued archive. `default` is one of `always_overwrite` (delete the old archive regardless of filename) or `overwrite_if_same_name` (only when destination filenames collide; otherwise keep both). |
| `relative_older_policy` | object, optional | **NEW (issue #107).** What to do when the download is an *older* revision. `default` is one of `block` (refuse before any bytes transfer, reporting which archive is newer), `warn_then_conflict_menu` (confirm first — see below), or `silent_then_conflict_menu` (proceed silently). |

### `warn_then_conflict_menu` — the parked queue state

The item moves to `awaiting_revision_confirmation` **before any bytes transfer**, carrying a
`pending_revision_confirmation: {newer_archive_id, hops}` so the row can name and link to the
archive that supersedes it. That state is deliberately neither *startable* nor *in-flight*: the
ordinary Start button (and `start_all`/`start_selected`, which share the same predicate) cannot
bypass the question, and nothing treats the item as though a task were holding it.

The user answers via `POST /download_queue/{id}/confirm-older` with `{"proceed": true|false}`:

- `proceed: false` → the item becomes `cancelled`, the existing "user stopped this deliberately,
  still restartable" state. No new terminal state was added for this.
- `proceed: true` → `revision_confirmed` is persisted on the item and the download restarts. The
  restart runs through the identical version-history check and does *not* park again, because that
  flag is now set.

`revision_confirmed` is scoped to the single run it was granted for: every other entry into the
start path (plain Start, Start All, a retry after an error) clears it, so a later re-run of the same
URL asks again rather than silently inheriting a stale approval.

Distinct from the filename-collision conflict (issue #77), which happens *after* a download and
asks "overwrite or rename?". One item can hit both, in that order.

Both policy fields are optional **even for a plugin that populates `version_history`**: the host
decides whether to offer them from `plugin_introspect`, not from this declaration, so a plugin only
ever has to fill in `version_history` and never has to touch `pluginOptions()` at all. Declare one
only to express a different default than the built-in `always_overwrite` /
`warn_then_conflict_menu`.

A `PluginOptionsResult` with every field absent/empty is equivalent to not implementing
`plugin_options` at all. The host shows no settings UI for such a plugin (FR-015) **unless**
`plugin_introspect` reports `returns_version_history` — in which case the two policy pickers are
still rendered, on the built-in defaults.

## `canonicalizeSource` export (new, optional — issue #107)

```ts
export function canonicalizeSource(url: string): string;
```

Given any URL this plugin's site might expose (any domain alias, with or without query string,
trailing slash or scheme), returns the canonical form the host uses for matching against stored
`source:` tags — e.g. E-Hentai folding `exhentai.org`/`g.e-hentai.org`/`forums.e-hentai.org` all
down to one `e-hentai.org/g/{gid}/{token}` form. The host calls it before every `source:` tag
comparison instead of maintaining its own per-site domain-alias table, so a site's alias knowledge
lives in the one place that actually has it. This replaces (does not supplement) the hardcoded
E-Hentai alias special-case previously in `get_existing_archive_id_for_url`.

**Contract — required, not advisory.** This function MUST be idempotent and MUST produce
byte-identical output for every URL form referring to the same underlying resource, including:

1. a URL already stored in an archive's `source:` tag by a *past* version of this plugin, or by the
   host's own generic `trim_url()`;
2. a URL this plugin just resolved in `execDownload`;
3. every entry in `DownloadResult.version_history[].source`.

The host normalizes both sides of every comparison and then does plain string equality. If the same
real resource can normalize to two different strings depending on which historical URL shape it
arrived in, the comparison silently fails to match with nothing reported anywhere — far harder to
diagnose than an exception. When in doubt, normalize toward the most stable identifier the site
guarantees never changes (E-Hentai's numeric `gid`), not toward the most recently observed URL
shape.

Return the input unchanged if the plugin has no normalization beyond the host's generic
scheme/`www`/query/trailing-slash trimming. Omitting the export entirely is valid — the host then
applies only `trim_url()`.

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
