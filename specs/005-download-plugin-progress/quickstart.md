# Quickstart: Validating Download Plugin Progress, Concurrency & Rate Limiting

Prerequisites: Phase 1 (`001-lanrurugi-full-rewrite`) built and running with at least one
download-type plugin installed (`plugins/download/chaika.ts`, `ehentai.ts`, or `pixiv.ts`, all
migrated to the `downloads[]` contract per `contracts/plugin-download-protocol.md`) — this feature
only adds to the existing `lanrurugi-server` binary and `apps/frontend` app, no new deployable.

## 1. Real progress bar during a download (US1)

```
lanrurugi serve --redis-url <redis> --library-path <library>
# In the UI: Plugins page → trigger a download via an installed download plugin against
# a real, reasonably large (≥50MB) target URL
```

**Expected**: opening `/jobs` while the download is in flight shows the job's progress bar
advancing through multiple intermediate states (not a single queued→finished jump) — verify via
`GET /api/jobs` directly too (`contracts/download-settings-api.md`'s extended job shape):
`downloaded_bytes` increases across repeated polls, `total_bytes` is populated once the response's
`Content-Length` is known (SC-001).

## 2. Indeterminate progress when size is unknown (US1, edge case)

Trigger a download against a target whose server response has no `Content-Length` header (e.g. a
test endpoint configured with chunked transfer encoding and no declared length).

**Expected**: the job's `downloaded_bytes` still increases across polls, but `total_bytes` stays
absent for the whole transfer; the Jobs page renders an indeterminate/spinner-style indicator
rather than a stuck or divide-by-zero percentage (spec Edge Case, FR-002).

## 3. Combined progress for a multi-resource download (US1)

Trigger a download via the Pixiv plugin against a multi-page artwork URL.

**Expected**: `GET /api/jobs` shows exactly one job entry for the whole request (not one per
page), whose `downloaded_bytes`/`total_bytes` are the sum across all of that artwork's pages —
confirm by comparing the final `total_bytes` against the artwork's actual combined page-image
sizes (FR-003).

## 4. Failed download leaves no half-cataloged archive (US1, edge case)

Trigger a download against a URL that returns a non-2xx status partway through, or point a
download plugin at an unreachable host.

**Expected**: the job ends in state `failed` with a human-readable `error` (`GET /api/jobs`); the
library (`GET /api/archives` or the Library page) shows no new, partial, or broken entry for that
attempt (FR-004).

## 5. Per-domain concurrency limiting (US2)

```
curl -X PUT localhost:<port>/api/plugins/<download-plugin-namespace>/options \
  -H "<auth header>" -d '{"domain_rules": [{"pattern": "*.example.com", "max_concurrent": 1}]}'
```

Trigger three downloads against different subdomains of `example.com` at roughly the same time.

**Expected**: `GET /api/jobs` shows only one of the three actually transferring bytes
(`downloaded_bytes` increasing) at a time; the other two remain `active`/queued for their turn,
none fail outright (US2 Acceptance Scenario 1). Confirm the wildcard rule's subdomain-wide sharing
by checking that all three (different subdomains, same `*.example.com` pattern) count against the
same limit, not three independent ones.

## 6. Exact-hostname rule overrides a wildcard rule (US2)

```
curl -X PUT localhost:<port>/api/plugins/<namespace>/options \
  -H "<auth header>" -d '{"domain_rules": [
    {"pattern": "*.example.com", "max_concurrent": 1},
    {"pattern": "cdn.example.com", "max_concurrent": 5}
  ]}'
```

Trigger several simultaneous downloads from `cdn.example.com` specifically.

**Expected**: up to 5 run concurrently against `cdn.example.com` (the exact-hostname rule), even
though the wildcard rule alone would have capped it at 1 — confirms precedence (FR-007).

## 7. Rate limiting caps observed throughput (US3)

```
curl -X PUT localhost:<port>/api/plugins/<namespace>/options \
  -H "<auth header>" -d '{"domain_rules": [{"pattern": "*", "max_bytes_per_sec": 1048576}]}'
```

Trigger a download of a file at least several times the size of the configured cap (e.g. ≥20MB
against a 1MB/sec cap).

**Expected**: elapsed time ÷ file size stays within 10% of the configured cap (SC-005) — verify via
the job's own `downloaded_bytes` progression over wall-clock time, or by timing the job from
`active` to `finished`.

## 8. No rate limit configured → unrestricted speed (US3, edge case)

```
curl -X DELETE localhost:<port>/api/plugins/<namespace>/options -H "<auth header>"
```

Trigger the same download again.

**Expected**: transfer proceeds at full available network speed, noticeably faster than step 7's
capped run (US3 Acceptance Scenario 2/FR-010).

## 9. Settings UI round-trip (US4)

```
# In the UI: Plugins page → find an installed download plugin that declares pluginOptions()
# → open its Settings
```

**Expected**: the form shows the plugin's current effective values (its own defaults, since step 8
just cleared any override), each with the human-readable description from `pluginOptions()`
(FR-011/FR-012). Change a value, save, and confirm — by reopening the settings, and via
`GET /api/plugins/{namespace}/options` directly — that the change persisted and is now
`"source": "user_override"` for that field (FR-013).

## 10. Invalid setting value is rejected (US4, edge case)

```
curl -X PUT localhost:<port>/api/plugins/<namespace>/options \
  -H "<auth header>" -d '{"domain_rules": [{"pattern": "*", "max_concurrent": -1}]}'
```

**Expected**: `422` response with a clear field-level explanation (`contracts/
download-settings-api.md`), not a silently-accepted or silently-discarded value; confirm via a
follow-up `GET` that the invalid value was never actually persisted (FR-014).

## 11. No settings UI for a plugin with nothing to configure (US4, edge case)

Open the Plugins page for a metadata- or login-type plugin, or a download plugin that exports no
`pluginOptions()` at all.

**Expected**: no Settings affordance is shown for that plugin at all (FR-015); a direct
`GET /api/plugins/{namespace}/options` call against it returns `404`.

## 12. Existing download plugins keep working with zero user setup (SC-004)

```
lanrurugi serve --redis-url <fresh redis> --library-path <library>
# Trigger a download via chaika.ts or ehentai.ts without ever opening any settings screen
```

**Expected**: the download completes exactly as it did before this feature shipped, plus the new
progress bar from step 1 — no behavior regression, no required configuration step.
