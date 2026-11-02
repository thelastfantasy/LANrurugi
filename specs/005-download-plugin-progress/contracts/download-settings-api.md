# Contract: Download Plugin Settings API

Additive endpoints (no legacy contract equivalent — matches 002-job-console's own precedent of
adding new, LANrurugi-only endpoints rather than touching the existing `/api/*` surface, per
constitution Principle II). Same authentication as the rest of the admin-facing API — standard
API-key/session middleware, no special-cased bypass.

Also documents the additive fields `GET /api/jobs` gains on each job entry (extending
`specs/002-job-console/contracts/jobs-api.md`'s existing shape, not replacing it).

## `GET /api/plugins/{namespace}/options`

Returns the plugin's current *effective* settings — its own declared defaults
(`pluginOptions()`, re-evaluated fresh each call, same cost profile as an existing `plugin_info`
call) merged with any persisted user override (data-model.md's `Download Plugin Settings`).

**Response `200`** (plugin declares options):

```json
{
  "namespace": "pixivdl",
  "domain_rules": [
    {
      "pattern": "*.pixiv.net",
      "max_concurrent": 2,
      "max_bytes_per_sec": null,
      "description": "Limit simultaneous downloads from Pixiv's CDN",
      "source": "plugin_default"
    }
  ],
  "bundle_as_archive": {
    "value": true,
    "default": true,
    "description": "Combine all downloaded pages into a single manga archive instead of one archive per page",
    "source": "plugin_default"
  }
}
```

`source` on each field ∈ `"plugin_default" | "user_override"` — lets the settings UI show which
values are user-customized vs. inherited from the plugin (FR-012's "custom value if set, otherwise
the plugin's own built-in default").

**Response `404`**: plugin has no `namespace`, or exports no `pluginOptions()` at all (FR-015 — no
settings interface is shown for such a plugin; the frontend simply doesn't render a settings
affordance rather than calling this endpoint in that case, but a direct call still returns `404`
rather than an empty `200` to keep the contract unambiguous).

## `PUT /api/plugins/{namespace}/options`

Persists a user override for one or more of the plugin's configurable fields. Partial updates are
supported — a field omitted from the request body is left at its current effective value (plugin
default, or a previously-set override), not reset.

**Request**:

```json
{
  "domain_rules": [
    { "pattern": "*.pixiv.net", "max_concurrent": 4 }
  ],
  "bundle_as_archive": false
}
```

**Response `200`**: the updated effective settings, same shape as `GET`'s response.

**Response `422`** (FR-014 — invalid value rejected, not silently clamped/discarded):

```json
{
  "error": "max_concurrent must be a positive integer",
  "field": "domain_rules[0].max_concurrent"
}
```

**Response `404`**: same condition as `GET` — plugin declares no configurable options at all.

## `DELETE /api/plugins/{namespace}/options`

Clears all user overrides for the plugin, reverting every field to the plugin's own declared
default. Idempotent — deleting when no override exists is a no-op `200`, not an error.

**Response `200`**: the plugin's now-all-defaults effective settings, same shape as `GET`.

## `GET /api/jobs` — extended job entry shape

Extends `specs/002-job-console/contracts/jobs-api.md`'s existing per-job object with two new,
optional fields (data-model.md's `JobStatus` extension):

```json
{
  "id": "b3f1...-uuid",
  "name": "download_url",
  "state": "active",
  "progress": 0.42,
  "downloaded_bytes": 47185920,
  "total_bytes": 112197632,
  "result": null,
  "error": null
}
```

`downloaded_bytes`/`total_bytes` are both absent (not `0`/`null` sentinels — genuinely absent
keys) for any job that isn't a byte-transfer download, and for a download job before its real
transfer phase has started. `total_bytes` may be absent even once `downloaded_bytes` is present
(server didn't report a size — spec Edge Case, FR-002) — the frontend renders an indeterminate
indicator in that case, not a `NaN`/zero-total percentage.
