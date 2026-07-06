# Phase 1 Data Model: LANrurugi — Full Rewrite

Source of truth remains Redis (constitution Principle I) — this document models the domain
entities and their Redis shape, not a new schema for a new store. Legacy field names are kept
where they already exist (verified against `lib/LANraragi/Model/*.pm` and
`lib/LANraragi/Utils/Database.pm`) so migration is a non-event rather than a transformation.

## Archive

The central entity — a single trackable manga/comic work (spec Key Entities).

| Field | Type | Notes |
|---|---|---|
| `id` | string (40 hex chars) | Primary key. Either the legacy `SHA-1(first 512KB)` or the new `SHA-1(first 512KB ++ u64 BE size)` — see research.md §1. Both forms coexist; the algorithm used to produce a given ID is not itself stored (it's implicit in whether the ID matches a legacy-migrated record or a freshly-scanned one). |
| `name` / `file` | string | Path/filename on disk. |
| `title` | string | User- or plugin-set display title (FR-002, FR-015). |
| `tags` | string (comma-separated `namespace:value` pairs) | Kept in the legacy flat-string shape rather than normalized into a separate table, to avoid a data-shape migration; namespace:tag parsing is a read-time concern for lanrurugi-search. |
| `summary` | string | Free text. |
| `pagecount` | integer | |
| `thumbhash` | string | Hash of the thumbnail image, used for thumbnail cache invalidation (mirrors legacy `Utils::Archive::extract_thumbnail`). |
| `isnew` | boolean flag | "Unread"/new marker (legacy `set_isnew`). |
| `extension` | string | Archive/container format (zip, rar, 7z, pdf, epub, ...). |

**Relationships**: optionally belongs to zero-or-more `Category` (via saved search or explicit
membership), optionally belongs to zero-or-one `Grouping`, has zero-or-more `Stamp`s, has one
`ReadingProgress`.

**Identity/uniqueness rule (FR-005, Clarifications)**: two files with different `id` are always
distinct archives. Two files that would compute the *same* `id` (byte-identical, or — pre-fix —
merely sharing a leading 512KB-and-size match) are the *same* archive record; the fix in FR-005
is about not letting non-identical content collide, not about disabling identity matching for
genuinely identical content.

## Category

| Field | Type | Notes |
|---|---|---|
| `catid` | string (`SET_<timestamp>` per legacy convention) | Primary key. |
| `name` | string | |
| `search` | string, optional | If present, this is a dynamic/saved-search category (archives are computed, not stored); if absent, `archives` is the authoritative membership list. |
| `archives` | array of Archive `id` | Only meaningful for static categories. |

Verified against `Model/Backup.pm`'s category-backup block (`SET_??????????` key glob, fields
`name`/`search`/`archives`).

## Grouping (Tankoubon)

| Field | Type | Notes |
|---|---|---|
| `tankid` | string (`TANK_<timestamp>` per legacy convention) | Primary key. |
| `name` | string | |
| `summary` | string | |
| `tags` | string | |
| `archives` | ordered array of Archive `id` | Order is significant (volume order, FR-002's "grouping and volume order are preserved"). |

Verified against `Model/Backup.pm`'s tankoubon-backup block and `Model/Tankoubon.pm`'s
`get_tankoubon_list`.

## Reading Progress

| Field | Type | Notes |
|---|---|---|
| `archive_id` | string | Foreign key to Archive; one record per archive (single-owner model — no per-user dimension, per Clarifications Q1). |
| `lastreadpage` | integer | |
| `progress_percent` / completion state | derived or stored | Exact legacy field name to confirm against `Utils::Database` during implementation; behavior (last page + completion) is fixed by FR-002/SC-001. |
| `lastreadtime` | timestamp | Used by the legacy search engine's "sort by last read" and its cache-busting rule (`Model/Search.pm`: history-sorted searches skip the cache) — this ordering-affects-caching behavior must be preserved by `lanrurugi-search`. |

## Stamp

Not named in the feature spec's Key Entities, but **verified to exist in legacy data**
(`Model/Backup.pm`'s `STAMPS_*` key glob) and therefore is in scope implicitly under FR-002's
"preserve all previously recorded ... data" — omitting it would violate Principle I even though
no user story mentions it by name. Flagging this explicitly so it isn't lost between spec and
implementation.

| Field | Type | Notes |
|---|---|---|
| `stamp_id` | string (`STAMPS_<id>`) | Primary key. |
| `content` | string | |
| `position` | string | |
| `archive_id` | string | Foreign key to Archive. |

## Extension (Plugin)

| Field | Type | Notes |
|---|---|---|
| `namespace` | string | Plugin identifier. |
| `type` | enum: `metadata` \| `login` \| `download` | Mirrors legacy `Plugin::{Metadata,Login,Download}` categories. |
| `parameters` | key/value map | User-supplied plugin config (API keys for the plugin's own target site, etc. — not to be confused with Principle V's LLM-provider secrets, which are Phase 2). |
| `enabled` | boolean | |
| `declared_permissions` | list of capability grants (e.g. allowed network hosts) | New in LANrurugi — legacy Perl plugins had no permission model; this is the concrete shape of constitution Principle IV / FR-014's "explicitly declare needing" requirement. |

**State/lifecycle**: `disabled → enabled → (per-invocation) running → succeeded | failed | timed
out`. A `failed`/`timed out` run MUST NOT change the archive's existing metadata (FR-013) and MUST
be isolated from other concurrently-running plugin invocations.

## Backup/Export document

The output shape of US5/FR-008–010, matching `Model/Backup.pm`'s `build_backup_JSON` (verified):

```text
{
  "categories":  [ { catid, name, search?, archives: [id, ...] }, ... ],
  "tankoubons":  [ { tankid, name, summary, tags, archives: [id, ...] }, ... ],
  "stamps":      [ { stamp_id, content, position, archive_id }, ... ],
  "archives":    [ { arcid, title, tags, summary, thumbhash }, ... ]
}
```

**Consistency rule (FR-010)**: this document MUST represent a single point-in-time snapshot; if
generation races with a concurrent write (new tag, new archive), the snapshot must not contain a
half-written intermediate state. Implementation approach (e.g. a read transaction/lock scope) is
a tasks-phase concern; the requirement itself is fixed here.

## Rebuild/Reindex operation (US6)

Not a persisted entity, but a stateful operation worth modeling explicitly since it's the
mechanism behind FR-011/FR-012:

1. For every existing Archive record, recompute the `id` using the new size-aware algorithm from
   the archive's current on-disk file.
2. If the recomputed `id` differs from the stored legacy `id` **and** no other archive already
   claims that new `id`: re-key the record (tags/summary/reading-progress/category-and-grouping
   membership all move to the new key) — this is the common case, just a legacy→new ID upgrade
   with no duplicate involved.
3. If a *second*, previously-unseen file on disk also maps to the same legacy `id` (the historical
   false-merge case) but produces a *different* new size-aware `id`: the file whose path was
   already tracked keeps its existing metadata under its (possibly re-keyed) `id`; the
   previously-invisible file is inserted as a **new** Archive record with no pre-existing
   metadata (per the Clarifications answer — this is "revealed as new," not "recovered").
4. Categories/Groupings/Stamps referencing a re-keyed Archive `id` are updated to the new `id` in
   the same operation (no dangling references).

## Concurrency-benchmark artifacts (US8)

Not domain data — supporting artifacts for FR-020–022/SC-011:

| Artifact | Shape |
|---|---|
| Synthetic library manifest | Reproducible generator config (archive count, size distribution) targeting the SC-008 scale (~100,000 archives). |
| Benchmark report | Per-operation (full scan/ingestion, duplicate-repair reindex) wall-clock time + throughput, for legacy LANraragi and LANrurugi, on identical hardware/library copy — plain-text/Markdown table. |

## Out of scope for this data model

Anything from spec.md User Stories 9–10 (Phase 2 translation: `Translation Backend Selection`
entity, OCR text regions, font-lock cache) — intentionally excluded per constitution Principle VI
and the plan's Summary. Not modeled here.
