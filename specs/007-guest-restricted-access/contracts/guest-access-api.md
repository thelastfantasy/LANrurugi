# Contract: Restricted Guest Access Mode

No new endpoints — this feature changes the behavior and response shape of several *existing*
endpoints, plus the Casbin route policy that gates all of them. Per constitution Principle II,
these are documented as an explicit, spec-mandated exception (research.md §5), not a silent
contract break.

## `GET /api/login/status`

**Before** (`login.rs:259-280`):

```json
{ "logged_in": true, "using_default_password": false }
```

`logged_in` was `true` whenever *either* a real session was valid *or* `enable_pass` was `false`
(fully-open instance) — conflating "you're authenticated" with "this instance doesn't require
authentication at all".

**After**:

```json
{
  "logged_in": true,
  "using_default_password": false,
  "guest_mode_enabled": true
}
```

- `logged_in` now means exactly one thing: a valid administrator session exists. The
  `!auth.enable_pass ||` half of the old expression is removed (there is no longer an
  `enable_pass` to check).
- `guest_mode_enabled` (new): whether the site-wide guest-mode switch is on **and** at least one
  category is currently `visible_to_guest` — i.e. whether an unauthenticated visitor will be
  routed into scoped browsing rather than redirected to `/login`. This is the field
  `RouteGuards.tsx`/`Layout.tsx` branch on to render a three-state (admin / guest / neither) UI
  instead of today's two-state (`logged_in` / not) one.

**Consumers**: First-party SPA only (`apps/frontend/src/RouteGuards.tsx`,
`apps/frontend/src/Layout.tsx`) — this endpoint has no legacy third-party contract equivalent
(`misc.rs`'s own comment on `LoginStatus`/`types.ts:605-606` already notes "am I logged in right
now" has no place in the legacy `ServerInfo` schema), so this is a free change with no Principle II
exposure.

## `GET /api/info`

**Before** (relevant fields only, `misc.rs:82-89`):

```json
{ "has_password": true, "debug_mode": false, "nofun_mode": false, "...": "..." }
```

**After**:

```json
{ "has_password": true, "debug_mode": false, "...": "..." }
```

- `has_password`: unchanged field name/type (boolean), now unconditionally `true` — password
  protection can no longer be disabled, so this is no longer a Redis-read value but a constant.
- `debug_mode`: unchanged field name/type, now reflects the deploy-time
  `--disable-update-check`/`LANRURUGI_DISABLE_UPDATE_CHECK` flag (`AppState.disable_update_check`)
  instead of the removed `LRR_CONFIG.devmode` Redis field.
- `nofun_mode`: **removed**. See research.md §5 for the Principle II justification.

**Consumers**: Documented legacy third-party API surface (Tachiyomi/Mihon-style extensions,
per constitution Principle II). `has_password`/`debug_mode` remain present and type-stable — a
client that merely reads these two fields (the expected/only known real-world usage) is
unaffected. A hypothetical client keyed specifically on `nofun_mode`'s presence would break; none
is known to exist.

## `GET/POST/PUT /api/categories*`

**Before** (`categories.rs:22-30`, response shape; `CreateCategoryParams`/`UpdateCategoryParams`,
request shape):

```json
{ "id": "SET_1234567890", "name": "Public Picks", "pinned": 1, "search": null, "archives": ["..."] }
```

**After**:

```json
{
  "id": "SET_1234567890",
  "name": "Public Picks",
  "pinned": 1,
  "visible_to_guest": 0,
  "search": null,
  "archives": ["..."]
}
```

- `visible_to_guest` (new, response): `0`/`1`, same integer-boolean convention as the existing
  `pinned` field (not a JSON `true`/`false` — matches this endpoint's own established style).
- `visible_to_guest` (new, request field on `POST /categories` and `PUT /categories/{id}`):
  optional boolean form field (`#[serde(default)]`, same as `pinned`'s existing handling) —
  omitted on an existing client's request means `false`, not an error, so no existing caller of
  these endpoints breaks by omitting the new field.

**Consumers**: First-party SPA (`Categories.tsx`) is the only known caller of the mutation
endpoints. `GET /categories`/`GET /categories/{id}` are also part of the legacy third-party
contract (per this file's own `//!` docstring, `categories.rs:1-2`, "verified against
`~/LANraragi/tools/openapi.yaml`") — adding a new, additional response field is additive per
Principle II ("New, LANrurugi-only functionality MAY freely add new endpoints" extends naturally
to additive new *fields* on an existing response; no existing field changed shape or meaning).

## Casbin route policy (`crates/lanrurugi-api/policy/route_policy.csv`)

Not a runtime-facing HTTP contract, but a compile-time-embedded authorization contract worth
documenting explicitly since it's this feature's primary enforcement mechanism.

- **Removed**: the entire `anonymous` subject block (route_policy.csv's existing lines covering
  `p, anonymous, /api/*, {GET,POST,PUT,DELETE}, allow` plus its own 15-entry deny list) — there is
  no longer any code path that produces a request with no `AuthContext` *and* full read/write
  access; that scenario (`enable_pass == false`) no longer exists.
- **Added**: the `guest_visitor` subject block — see research.md §3 for the exact policy lines and
  rationale (GET-only allow, `token_guest`'s existing deny list reused verbatim, plus one
  `guest_visitor`-specific deny for the raw-file download endpoint).
- **Unchanged**: `session`, `token_admin`, `token_guest` blocks — none of their existing rules are
  modified by this feature.

Enforced identically to how the four existing subjects already work — `guest_visitor` gets no
special-cased code path in `authz::check_route`, only a new match arm in `subject_role`
(research.md §1/§2) that produces the string `"guest_visitor"`, which the same generic
`enforcer.enforce((sub, obj, method))` call already handles.
