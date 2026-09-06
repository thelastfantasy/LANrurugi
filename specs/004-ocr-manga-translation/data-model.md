# Phase 1 Data Model: On-Page Manga Translation (Phase 2)

Server-side state is stored in Redis (reused from Phase 1, per constitution Principle I) under new,
additive key namespaces — nothing here alters a Phase 1 archive/category/tankoubon key. Some state
is deliberately client-side (browser) rather than server-side; each entity below states which.

**Note (constitution 1.8.0 alignment, 2026-09-06)**: Constitution 1.8.0 (ratified 2026-08-03,
after this document's original 2026-07-06 draft) added a Technology Stack Constraints bullet
requiring every domain entity identifier to be a newtype (e.g. `ArchiveId(String)`), never a raw
`String`, from first implementation. Every `string`-typed ID field below (`archive_id`,
`volume_id`, and any future entity's own primary key) MUST be implemented as the corresponding
newtype at tasks-phase implementation time — reusing Phase 1's own newtype for `archive_id` if one
already exists there, and introducing a new `VolumeId` newtype for `volume_id` if Phase 1 doesn't
already define one for its Grouping entity. The `string` notation in the tables below reflects this
document's original authoring date and is retained for readability; it is not an instruction to use
a raw `String` in the implementation.

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
| `fg_color` | (r, g, b), optional | Heuristically estimated from the region's cropped pixels (research.md §13); absent when the heuristic couldn't estimate it with confidence (FR-008a) — independent of `bg_color`/`is_bold` |
| `bg_color` | (r, g, b), optional | Same estimation pass as `fg_color`, independently nullable |
| `is_bold` | boolean, optional | Estimated via a stroke-width-to-glyph-height heuristic ratio (research.md §13); absent (not defaulted to `false`) when the heuristic couldn't estimate it with confidence, distinguishing "estimated not-bold" from "unknown" |
| `font` | string, optional | The matched font from the volume's `Volume Font Pattern` golden set, resolved at translation time (FR-008/FR-009) |
| `translated_text` | string, optional | Present once this region has been translated for a given (target_language, backend); absent for a region not yet translated (research.md §16) |

**Persistence (revised, research.md §16)**: `Detected Text Region` — including its translation
once produced — IS the long-term-persisted, authoritative record (Redis, per `archive_id`/
`page_number`/`target_language`/`backend`, mirroring `Translation Cache Entry`'s own key shape).
This is a deliberate change from this document's original position (which called the *rendered
image* the durable artifact and this record disposable) — see research.md §16 for why: a JSON
record of text + position + style is orders of magnitude smaller than a rendered page image, so
it's the cheaper thing to keep indefinitely, while the rendered image becomes a re-derivable
performance cache the system MAY evict/regenerate without any data loss (unlike evicting this
record, which would require re-running OCR/translation entirely).

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

## Terminology Glossary

| Field | Type | Notes |
|---|---|---|
| `volume_id` | string | Same scoping as Volume Font Pattern — per volume/grouping, or per-archive if ungrouped (research.md §14) |
| `entries` | map<source_term, translation> | Auto-populated on first translation of each name/term (FR-007a); no confirmation gate |

**Resolution at translation-request time** (FR-007b/c, `contracts/llm-provider-adapter.md`):
1. Exact-substring match of the block's `source_text` against `entries`' keys → reuse that
   translation directly, no LLM judgment needed.
2. The glossary's other known source-term names (keys only, not their translations) are included
   in the request's `context` so the backend can itself recognize a nickname/initialism variant.
3. A name/term not resolved by (1) or recognized by (2) is translated normally and its result
   becomes a new `entries` record.

**User edit/delete** (FR-007d): modifies or removes a single `entries` record; takes effect on the
next translation request referencing that source term. No bulk-clear operation, unlike Volume Font
Pattern's `reset` (FR-010) — see research.md §14 for why.

## Translation Cache Entry

Two variants, matching the compositing-location split (research.md §6). **Revised (research.md
§16): this entity is now a re-derivable rendering cache, not the authoritative store** — the
authoritative translation result is the `Detected Text Region` records above (text + style +
position). A `Translation Cache Entry` MAY be evicted/expired at any time without data loss; it is
regenerated by re-running compositing (not OCR, not translation) against the already-persisted
`Detected Text Region` records for that page.

| Variant | Key | Storage | Notes |
|---|---|---|---|
| Server-composited (cloud backend) | (`archive_id`, `page_number`, `target_language`, `provider`) | Redis (metadata) + server-side image cache on disk, sharing the reader's resize-page cache quota (`tempmaxsize`, research.md §18) — NOT the thumbnail cache, which has no quota/eviction at all | Populated by the look-ahead prefetch pipeline; served directly on subsequent requests for the same key; evictable/regenerable from `Detected Text Region` |
| Client-composited (local backend) | same key tuple | Browser IndexedDB/Cache API (Blob/PNG) | Never leaves the device; a different device with the same local backend configured builds its own cache independently; evictable/regenerable the same way, from the server's `Detected Text Region` records (fetched via `text-regions`, `contracts/translation-api.md`) |

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
