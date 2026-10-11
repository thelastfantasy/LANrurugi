# Data Model: Restricted Guest Access Mode

## Category (extended)

Existing entity (`crates/lanrurugi-core/src/entities.rs:118-135`), gains one field.

| Field | Type | Notes |
|---|---|---|
| `catid` | `CategoryId` | Existing, unchanged. |
| `name` | `String` | Existing, unchanged. |
| `search` | `Option<String>` | Existing, unchanged — `Some` means a dynamic (saved-search) category; `archives` is meaningless when this is `Some`. |
| `archives` | `Vec<ArchiveId>` | Existing, unchanged — only meaningful for static categories. |
| `pinned` | `bool` | Existing, unchanged. |
| `visible_to_guest` | `bool` | **New.** Whether an unauthenticated guest visitor (when guest mode is on) can see archives belonging to this category. Defaults to `false` for any category record predating this feature (Redis Hash field absent → `false`, identical convention to `pinned`). |

**Redis representation**: Same `SET_<10-digit-unix-timestamp>` Hash as today. New field stored as
`"visible_to_guest"` → `"0"`/`"1"` string, read via
`fields.get("visible_to_guest").map(|v| v == "1").unwrap_or(false)` and written via
`("visible_to_guest", if category.visible_to_guest { "1" } else { "0" }.into())` —
copied verbatim from `pinned`'s own existing read/write code shape
(`repository.rs:310-316` read, `repository.rs:396-404` write).

**Validation rules**: None beyond the existing `CreateCategoryParams`/`UpdateCategoryParams`
`#[serde(default)]` boolean handling already used for `pinned` — an absent form field on
create/update defaults to `false`, not an error.

**Relationships**: Unchanged — an archive may belong to zero or more categories (many-to-many via
each category's own `archives: Vec<ArchiveId>`). Guest visibility of an archive is the union
across all its category memberships: an archive is guest-visible if *at least one* category it
belongs to has `visible_to_guest: true` (spec Edge Cases — visibility is a union, not an
intersection).

## Guest Mode Setting

A single new boolean field in the existing `LRR_CONFIG` Redis hash (same storage mechanism as
every other instance-wide setting — `theme`, `pagesize`, etc.), added to `settings.rs`'s
`BOOL_FIELDS` list under the key `guestmode`.

| Field | Type | Default | Notes |
|---|---|---|---|
| `guestmode` | `bool` | `false` | Instance-wide master switch. When `false`, guest access is never granted regardless of any category's `visible_to_guest` value (spec FR-006). When `true` **and** at least one category has `visible_to_guest: true`, unauthenticated requests are eligible for `guest_visitor` treatment (spec FR-005). |

**Migration**: Absent on any pre-existing `LRR_CONFIG` hash → defaults to `false` (same
`BOOL_FIELDS` default-handling mechanism every other boolean setting already uses) — satisfies
FR-013's requirement that upgraded instances default to the strictest posture.

## Guest Visitor (authentication concept, not a stored entity)

Not a persisted record — a per-request classification, computed fresh on every request (spec
Assumptions: no guest accounts, no cross-visit persistent identity).

| Concept | Representation |
|---|---|
| Auth method | New `AuthMethod::GuestVisitor` variant (`auth_context.rs`) — no payload, unlike `Token { id, role }`. |
| Casbin subject | `"guest_visitor"` string, returned by `authz::subject_role` for this `AuthMethod` variant. |
| Eligibility | Computed in `procedure.rs::require_api_key`: no valid `Session`/`Token` credential present, **and** `guestmode == true`, **and** at least one category has `visible_to_guest == true`. All three conditions must hold; otherwise the request proceeds as today's plain-unauthenticated (401/redirect) path. |
| Content scope | Not carried on the `AuthMethod` itself — computed per-request by the handler (search/archive endpoints) as the union of archive IDs across all `visible_to_guest: true` categories, via `CategoryRepository::list_all()` filtered to `visible_to_guest` and each category's own archive membership. |

**State transitions**: None — this is not a stateful entity with a lifecycle. A request either
qualifies for `guest_visitor` treatment at evaluation time or it doesn't; there is no "guest
session" that persists, expires, or is revoked independently of the two config values
(`guestmode`, and each category's `visible_to_guest`) that gate it on every single request (spec
FR-015: re-evaluated on every request, changes take effect immediately).

## Removed fields (no longer part of the data model)

- `LRR_CONFIG.enablepass` — removed from `BOOL_FIELDS`; password protection is no longer
  configurable (always effectively `true`). Pre-existing Hash field, if present on an upgraded
  instance, becomes inert (nothing reads it).
- `LRR_CONFIG.nofunmode` — removed from `BOOL_FIELDS`; the concept it described (force login even
  with password disabled) no longer applies once password protection can't be disabled. Same
  inert-on-upgrade handling as `enablepass`.
- `LRR_CONFIG.devmode` — removed from `BOOL_FIELDS`; replaced by the deploy-time
  `--disable-update-check` CLI flag / `LANRURUGI_DISABLE_UPDATE_CHECK` env var (not a Redis-backed
  field at all — see research.md §7). Same inert-on-upgrade handling.
