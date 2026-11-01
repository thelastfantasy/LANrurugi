# Data Model: Recycle Bin and Safety Password

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Research**: [research.md](./research.md)

All new state is additive: no existing key, field or on-disk layout changes. The only existing structures touched
are the archive record (`archive` DB) and the config hash (`LRR_CONFIG`), both by **new** fields only.

## Entities

### RecycleEntry

One recycled archive. Authoritative copy lives in the Redis hash; an identical `meta.json` is written inside the
entry's directory for crash recovery and on-disk self-description.

| Field | Type | Notes |
|---|---|---|
| `id` | `ArchiveId` | Newtype over the content-derived archive id; also the Redis hash field and the payload directory name. |
| `title` | `String` | Display title at deletion time (`Archive.title`). |
| `name` | `String` | Original filename stem (`Archive.name`). |
| `original_path` | `String` | Absolute path the file was deleted from — the restore target. |
| `file_name` | `String` | Basename including extension; payload lives at `payload/<file_name>`. |
| `patch_file_name` | `Option<String>` | Basename of the sidecar `.patch.zip` if `has_patch`; moved and restored with the payload. |
| `size` | `u64` | Archive file size in bytes at deletion (`Archive.arcsize`); drives capacity accounting. |
| `page_count` | `u32` | `Archive.pagecount`, for the listing. |
| `deleted_at` | `i64` | Unix seconds. Ordering key for eviction; expiry base. |
| `deleted_by` | `RecycleActor` | `{kind, id, label}` — the same shape activity entries use; `kind = session \| token \| system`. |
| `snapshot` | `RecycleSnapshot` | Everything needed to recreate the record (below). |
| `schema` | `u32` | Entry format version, currently `1`; an unknown/future version is reported as un-restorable rather than mis-parsed. |

`RecycleSnapshot`:

| Field | Type | Source / restore call |
|---|---|---|
| `tags` | `String` | `Archive.tags` verbatim (flat `namespace:value` string). |
| `summary` | `String` | verbatim. |
| `isnew` | `bool` | verbatim. |
| `lastreadpage` | `u32` | verbatim. |
| `lastreadtime` | `u64` | verbatim. |
| `thumbhash` | `Option<String>` | verbatim; lets an existing cover stay valid. |
| `toc` | `Vec<TocEntry>` | verbatim. |
| `has_patch` | `bool` | verbatim. |
| `corrupted_pages` | `Vec<String>` | verbatim. |
| `heal_failed_at` | `Option<u64>` | verbatim. |
| `version_history` | `Vec<(String, String)>` | `archives.version_histories_for`; restored with `set_version_history`. |
| `categories` | `Vec<CategoryId>` | `categories.for_archive`; restored with `add_archive_to_category`. |
| `bookmarks` | `Vec<BookmarkSnapshot>` | `{page, bookmarked_at, name?}` from `bookmarks.list_for_archive`; restored with `add` + `set_name`. |
| `stamps` | `Vec<Stamp>` | Full `Stamp` records for `Archive.stamp_ids`; restored with `stamps.restore_raw`. |

`meta.json` is exactly `RecycleEntry` serialized (same `schema`), inside `<id>/meta.json`.

### RecycleTank

One recycled Tankoubon. Stored in `LANRURUGI_RECYCLE_TANKS` keyed by tank id; its member archives are
ordinary `RecycleEntry` rows in `LANRURUGI_RECYCLE_BIN`, hidden from the top level while this record
exists.

| Field | Notes |
|---|---|
| `schema` | Entry format version. |
| `tank_id` | `TANK_…` id; also the list row's `id`. |
| `grouping` | The whole `Grouping` record (name, summary, tags, volume order, progress, cover choice), so restore is exact. |
| `size`, `page_count` | Sums over members, for the row and for capacity accounting. |
| `deleted_at`, `deleted_by` | Same semantics as an archive entry. |

Restore/purge always act on the whole tank; a member is never restored or evicted on its own.

### RecycleEntryView (API DTO)

Computed per request, never stored:

| Field | Notes |
|---|---|
| `entry` fields above (minus `snapshot`) | |
| `expires_at` | `null` when retention is 0; else `deleted_at + days*86400`. |
| `remaining_seconds` | `expires_at - now`, clamped at 0; `null` when retention is 0. |
| `restorable` | `false` when payload missing or its size no longer matches the recording, when `schema` is unknown, or when an archive with the same id exists again. |
| `conflict` | `null \| "identical_archive_exists" \| "payload_missing" \| "payload_size_mismatch" \| "unsupported_schema"`. |
| `thumbnail` | bool — whether a cover file currently exists for the id. |

### RecyclePolicy (settings)

Stored in `LRR_CONFIG` through the existing `NUMBER_FIELDS` mechanism:

| Setting | Default | Range | Meaning |
|---|---|---|---|
| `recycle_retention_days` | `30` | `0..=36500` | Days an entry is kept; `0` = forever. |
| `recycle_max_size_gb` | `0` (unlimited) | `0..=1_048_576` | Maximum total size in GiB; `0` = unlimited (the default). The MiB field this shipped with for one release is read once, rounded up, then ignored. |

Changing either value triggers an immediate sweep after the settings write succeeds.

### Safety password

| Field / key | Location | Value |
|---|---|---|
| `safety_password_hash` | `LRR_CONFIG` field | `{CRYPT}$2a$…` bcrypt hash, produced by `lanrurugi_core::password::hash_password`; absent = not configured. |
| `safety_password_set_at` | `LRR_CONFIG` field | Unix seconds; display/diagnostic only. |
| `LANRURUGI_SAFETY_FAIL:<family>` | `config` DB, TTL 300 s | Integer failure counter for one login family. |
| `LANRURUGI_SAFETY_LOCK:<family>` | `config` DB, TTL 300 s | Present = locked; value is the unlock timestamp in seconds. |

`<family>` is `AuthContext::session_family_id`, falling back to the literal `session` for a session cookie with no
`fid` claim. The hash is never serialized into any response, activity snapshot or log line.

## Redis keys

| Key | DB | Type | Content |
|---|---|---|---|
| `LANRURUGI_RECYCLE_BIN` | archive | Hash | field = archive id, value = JSON `RecycleEntry` |
| `LANRURUGI_RECYCLE_TANKS` | archive | Hash | field = tank id, value = JSON `RecycleTank` (grouping + its member ids live in the bin hash above) |
| `LRR_CONFIG` | config | Hash | two new settings fields + two safety-password fields (new field names only) |
| `LANRURUGI_SAFETY_FAIL:<family>` | config | String | failure counter |
| `LANRURUGI_SAFETY_LOCK:<family>` | config | String | lock marker |

No index key is needed: the listing is one `HGETALL` and in-memory sorting by `deleted_at`; the bin is bounded by
policy, so this stays small.

## Filesystem layout

```text
<library>/
├── .lanrurugi-recycle/           # reserved; excluded by watcher::is_watched_archive_path
│   └── <archive_id>/
│       ├── meta.json             # RecycleEntry, written before the Redis HSET
│       └── payload/
│           ├── <file_name>       # the moved archive file (same filesystem ⇒ rename)
│           └── <patch_file_name> # optional sidecar .patch.zip
├── …existing library files…
└── <restored archive at its original path>
```

Thumbnails are **not** moved: they stay in `thumb_dir/<shard>/<id>.*` and `thumb_dir/<shard>/<id>/` and are
read by the bin's thumbnail endpoint until the entry is permanently removed.

## State machine

```text
            delete (FR-001/002)
library ─────────────────────────▶ ready ──── restore (FR-006) ───▶ library
                                     │
                                     ├──── permanent delete (FR-010) ───▶ gone
                                     ├──── empty bin (FR-011) ──────────▶ gone
                                     └──── sweep: expired / over cap ───▶ gone
ready ──(payload lost / size mismatch / unknown schema)──▶ un-restorable
un-restorable ──permanent delete / sweep──▶ gone
```

- `ready → un-restorable` is derived at read time, never written back: a file restored out-of-band or an old
  backup reappearing makes the entry restorable again without a repair step.
- `gone` is terminal; the payload directory, `meta.json`, hash field and thumbnails are all removed.
- Restore is the only transition that re-creates library state; everything else only removes.

## Ordering invariants (Redis and the filesystem must agree)

Delete (recycle):

1. snapshot → 2. rename payload/patch into the entry directory → 3. write `meta.json` → 4. `HSET` the entry →
5. `HDEL LRR_FILEMAP[original_path]` (the file is no longer there; a future file at that path must be treated as
new, never re-keyed against this recycled id) → 6. delete the archive record → 7. remove index/cache/membership
rows → 8. activity.

Restore:

1. resolve the destination per research §4 → 2. rename payload/patch out (filename lock held) → 3. `HSET
LRR_FILEMAP[destination] = size_aware_id(payload)` — the **scanner-computed** id, which equals the stored id for
size-aware archives and differs only for legacy-id ones, where writing the stored id would make the watcher
re-key the record and orphan its stamps/bookmarks (research §4) → 4. `archives.save` → 5. re-apply
categories/bookmarks/stamps/version history → 6. `HDEL` the entry and remove the directory → 7. activity.

Permanent delete / sweep: delete payload and `meta.json` → thumbnails → `HDEL` the entry → activity. Nothing
outside the entry directory is touched.

Any step that fails before the rename leaves the library state untouched; a failure after it triggers the
best-effort rollback described in research §3, and startup reconciliation (research §3) is the final authority.

## Validation rules

| Rule | Where enforced |
|---|---|
| Safety password is exactly 4 ASCII digits | `POST /api/safety-password` (and reset/set-inline); frontend input is advisory only |
| Safety password required and correct for permanent delete / empty | shared verification helper called by both endpoints, server-side |
| No verification caching between requests | verification helper takes the PIN per call; no session flag is written |
| 5 failures → 5 minutes lock | helper, before bcrypt; counter/lock keys above |
| Retention and capacity are non-negative | existing `validate_setting_field` + range check |
| Restore never overwrites | destination resolution in §4 of research |
| Restore refuses an existing same-id archive | pre-flight `archives.get` |
| Recycled ids are not scannable | `is_watched_archive_path` component exclusion |
| Only session auth reaches the bin | `route_policy.csv` deny lines for both token roles |

## Activity types

Added to `lanrurugi_storage::activity::action_types`:

| Type | When | Target | `after` payload |
|---|---|---|---|
| `archive.recycle` | an archive moved into the bin | archive id + title | `{file, size}` |
| `archive.restore` | an entry restored | archive id + title | `{file}` (the actual destination) |
| `archive.purge` | one entry permanently deleted | archive id + title | `{file, reason: "manual"}` |
| `recycle.empty` | the bin emptied | no single target | `{count, bytes, failed: n}` |
| `recycle.sweep` | automatic expiry/capacity removal | no single target | `{reason: "expired" \| "over_capacity", count, bytes}` |
| `safety_password.set` / `safety_password.reset` | credential set, changed or reset (FR-036) | no archive target | `{changed: "set" \| "change" \| "reset"}`; never the PIN |

Sweeps are written with a `System` actor (`record_manual` with no `AuthContext`), so the activity log
distinguishes automatic bounds enforcement from a human's empty-bin action.
