# Quickstart: Validating Restricted Guest Access Mode

Prerequisites: Phase 1 (`001-lanrurugi-full-rewrite`) built and running with this feature's
changes applied — no new deployable, extends the existing `lanrurugi-server` binary and
`apps/frontend` app. At least one archive and one category exist in the library.

## 1. Password login can no longer be disabled (US1)

```
lanrurugi serve --redis-url <redis> --library-path <library>
```

**Expected**: no `--no-pass` flag exists (removed); opening the Settings page shows no "enable
password"/"No-Fun mode" toggle anywhere. Every request to a management endpoint without a valid
session is rejected:

```
curl -i http://localhost:3000/api/settings
curl -i -X POST http://localhost:3000/api/database/drop
```

**Expected**: both return `401`/`403` (not `200`) with no prior configuration able to change that
(FR-001, FR-002).

## 2. Pre-existing "fully open" instance requires real login after upgrade (US1, edge case)

Simulate an upgraded instance whose `LRR_CONFIG` hash still has `enablepass` set to `0` from
before this feature shipped (leave the stale field in place — do not manually clean it up, per
FR-013's migration requirement).

```
curl -i http://localhost:3000/api/settings
```

**Expected**: `401`, not `200` — the stale `enablepass=0` field is inert and no longer read
(FR-013, spec Edge Cases).

## 3. Guest mode off → unauthenticated visitor redirected to login (US2)

```
curl -i http://localhost:3000/api/login/status
```

**Expected**: `{"logged_in": false, "using_default_password": ..., "guest_mode_enabled": false}`;
opening the site in a fresh unauthenticated browser session redirects to `/login` (US2 Scenario 4,
FR-006).

## 4. Guest mode on but no category marked guest-visible → still redirected (US2)

In Settings, enable the guest-mode switch. Do not mark any category as guest-visible.

```
curl -s http://localhost:3000/api/login/status | jq .guest_mode_enabled
```

**Expected**: `false` (the switch alone, without at least one guest-visible category, does not
flip `guest_mode_enabled` to `true`) — the unauthenticated browser session still redirects to
`/login`, with no confusing empty-library state shown instead (US2 Scenario 3, FR-006).

## 5. Guest mode on + one category marked guest-visible → scoped browsing (US2, US3)

In Settings, keep guest mode enabled. In Categories, mark one existing category (containing at
least one archive) as visible to guests; leave at least one other archive outside every
guest-visible category.

```
curl -s http://localhost:3000/api/login/status | jq .guest_mode_enabled
# expect: true

curl -s http://localhost:3000/api/search | jq '.data | length'
# expect: only archives belonging to the guest-visible category are present
```

**Expected**: opening the site unauthenticated shows the library scoped to the guest-visible
category's archives, with no forced redirect; the site navigation shows a discoverable login entry
point that leads to the existing admin login flow (US2 Scenarios 1-2, FR-005, FR-007).

## 6. Guest visitor can browse, read, and search within scope (US3)

Using the same unauthenticated session as step 5, open one of the guest-visible archives in the
reader and confirm pages load. Then search/filter by a tag known to exist on both an in-scope and
an out-of-scope archive.

```
curl -s "http://localhost:3000/api/archives/<in-scope-id>/page/1"
curl -s "http://localhost:3000/api/search?filter=<tag>" | jq '.data[].tags'
```

**Expected**: the reader page request succeeds (`200`, real image bytes); search results include
only the in-scope archive, never the out-of-scope one, even though both carry the searched tag
(US3 Scenario 1-2, FR-008, FR-012, SC-002).

## 7. Guest visitor cannot access an out-of-scope archive directly (US3)

```
curl -i "http://localhost:3000/api/archives/<out-of-scope-id>/metadata"
```

**Expected**: `404`, identical in shape/status to requesting a genuinely nonexistent archive ID —
not `403`, and not distinguishable from "doesn't exist" (US3 Scenario 3, FR-011, research.md §6).

## 8. Guest visitor cannot bookmark, save progress, or download (US3)

```
curl -i -X POST "http://localhost:3000/api/archives/<in-scope-id>/bookmarks/1"
curl -i -X PUT "http://localhost:3000/api/archives/<in-scope-id>/progress/1"
curl -i "http://localhost:3000/api/archives/<in-scope-id>/download"
```

**Expected**: all three return a non-2xx status (bookmark/progress denied because they're
non-`GET` and `guest_visitor`'s policy is GET-only allow; download denied via its explicit deny
rule despite being `GET`) — even though the same archive's `/page`/`/metadata` GET requests
succeeded in steps 6-7 (US3 Scenario 4, FR-009).

## 9. Guest visitor cannot reach any administrative endpoint (US3)

```
curl -i http://localhost:3000/api/plugins
curl -i http://localhost:3000/api/activity
curl -i http://localhost:3000/api/stats
```

**Expected**: all return a non-2xx status for the unauthenticated guest session, regardless of
guest mode being on (US3 Scenario 5, FR-010).

## 10. Configuration change takes effect immediately, mid-session (spec Edge Cases)

With an active unauthenticated guest session already browsing (per step 5), turn guest mode off
(or un-mark the last guest-visible category) via an authenticated admin session in another window.

```
curl -s http://localhost:3000/api/search   # using the guest session's own cookies/headers
```

**Expected**: the very next guest request reflects the new state (denied/redirected) — no stale
snapshot from earlier in that browsing session persists (FR-015).

## 11. `devmode` no longer configurable at runtime (US1-adjacent, FR-017)

```
lanrurugi serve --redis-url <redis> --library-path <library> --disable-update-check
curl -s http://localhost:3000/api/info | jq '.debug_mode, .nofun_mode'
```

**Expected**: `debug_mode` is `true` (reflects the CLI flag, not a Settings-page toggle — the
Settings page has no "Debug Mode" checkbox anymore); `nofun_mode` is `null`/absent (field removed
from the response entirely, per contracts/guest-access-api.md).
