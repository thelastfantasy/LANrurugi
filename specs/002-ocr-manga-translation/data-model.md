# Phase 1 Data Model: On-Page Manga Translation (Phase 2)

Server-side state is stored in Redis (reused from Phase 1, per constitution Principle I) under new,
additive key namespaces — nothing here alters a Phase 1 archive/category/tankoubon key. Some state
is deliberately client-side (browser) rather than server-side; each entity below states which.

## Translation Backend Selection

Split across two storage locations by design (research.md §8) — this is not one record, it's two:

| Location | Field | Notes |
|---|---|---|
| Server (Redis) | `category` | `cloud` \| `local` (as a server-wide default) |
| Server (Redis) | `provider` | e.g. `openai-compatible`, `anthropic` |
| Server (Redis) | `endpoint` | Non-secret connection detail (base URL etc.) |
| Server (Redis) | `credential_ref` | Opaque reference to the actual secret; the secret itself is never returned in any API response body |
| Client (`localStorage`) | `local_override` | Present only when this specific device has its own locally-hosted backend configured; when present, takes precedence over the server default for reads made from this device |

**Resolution rule**: reading "the active backend" on a given device checks `localStorage` first;
if absent, falls back to the server-stored default.

## Target Language Preference

| Field | Type | Notes |
|---|---|---|
| `target_language` | string (BCP-47 tag), optional | LTR languages only (FR-004); absent means "use the browser's own language setting" at read time — not itself persisted as a value, resolved dynamically. |

Stored server-side alongside Translation Backend Selection's server-side fields (it's not
device-specific the way the local-backend override is, since a target language choice isn't tied
to a particular device the way "my Ollama is at 127.0.0.1" is).

## Detected Text Region

| Field | Type | Notes |
|---|---|---|
| `archive_id` | string | Foreign key to Phase 1's Archive entity |
| `page_number` | integer | |
| `bounding_box` | (x, y, w, h) | Post-merge (research.md §3), not raw per-line boxes |
| `source_text` | string | Original-language text recognized in this region |
| `is_cover` | boolean | Set from Phase 1's own cover/page metadata, not inferred from OCR — used to exclude this region from Volume Font Pattern voting |

Not persisted long-term as its own Redis record necessarily — may be recomputed per look-ahead
request and only the derived Translation Cache Entry persisted; exact persistence strategy (cache
vs. recompute) is a tasks-phase implementation choice, not fixed here.

## Volume Font Pattern

| Field | Type | Notes |
|---|---|---|
| `volume_id` | string | Keyed per volume/grouping (Phase 1's Grouping entity, or per-archive if ungrouped) |
| `is_locked` | boolean | Gate for the two-sequential-checks design (research.md §4) |
| `vote_pool` | map<font, count> | Populated only from non-cover pages during the voting stage; frozen once locked |
| `golden_set` | list<font> (2–3 entries) | Selected from `vote_pool` once locked |
| `meltdown_tally` | map<font, count> | **Separate** from `vote_pool` — meltdown re-classifications accumulate here only, never in `vote_pool` (research.md §4, the resolved review concern) |

**State transitions**: `unlocked` (accumulating `vote_pool`) → `locked` (routing via cheap
features among `golden_set`, outliers go to meltdown and update only `meltdown_tally`) → (user
action) `reset` → back to `unlocked` with both `vote_pool` and `meltdown_tally` cleared (FR-010).

## Translation Cache Entry

Two variants, matching the compositing-location split (research.md §6):

| Variant | Key | Storage | Notes |
|---|---|---|---|
| Server-composited (cloud backend) | (`archive_id`, `page_number`, `target_language`, `provider`) | Redis (metadata) + server-side image cache, alongside Phase 1's existing thumbnail cache mechanism | Populated by the look-ahead prefetch pipeline; served directly on subsequent requests for the same key |
| Client-composited (local backend) | same key tuple | Browser IndexedDB/Cache API (Blob/PNG) | Never leaves the device; a different device with the same local backend configured builds its own cache independently |

A change in any key component (page, target language, or provider/backend) MUST NOT reuse an
entry keyed under a different combination (FR-016).

## Usage Budget

| Field | Type | Notes |
|---|---|---|
| `provider` | string | Only tracked for metered (cloud) backends — locally-hosted has no cost to track (research.md §10) |
| `limit` | number | User-configured cap (FR-013) |
| `consumption_current_page` | number | |
| `consumption_current_archive` | number | |
| `consumption_today` | number | |
| `consumption_current_week` | number | |

Stored server-side (Redis), since it's inherently tied to the server-proxied cloud call path.

## Out of scope for this data model

Anything from Phase 1 (Archive, Category, Grouping, Reading Progress, Extension, Stamp — see
`specs/001-lanrurugi-full-rewrite/data-model.md`) is referenced by foreign key where needed
(e.g. Detected Text Region → Archive) but not redefined here. RTL-specific text-layout fields are
intentionally absent (research.md §11).
