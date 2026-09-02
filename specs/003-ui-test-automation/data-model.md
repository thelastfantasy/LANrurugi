# Data Model: Automated UI Test Coverage

This feature introduces no persisted application data model — it adds test tooling, not product
functionality (constitution Principle VI; spec FR-008). The entities below are the spec's own Key
Entities, made concrete enough to drive `tasks.md`. There is no `contracts/` directory (see
plan.md's Project Structure note) since this feature adds no new REST/external interface; the
"Scenario Contract" section at the end stands in for it.

## Entities

### Test Scenario

A single named, independently-runnable check (spec Key Entities).

| Field | Type | Notes |
|---|---|---|
| `name` | string | Human-readable, describes the behavior under test, not the mechanism (e.g. `"category pinned-field save persists across reload"`, not `"test 3"`) |
| `layer` | `unit \| e2e` | Which of the two test layers (spec User Story 1 Acceptance Scenario 3/4) this scenario belongs to |
| `tags` | string[] | Enables running a meaningful subset (spec FR-005) — at minimum a tag per page/flow area (`reader`, `categories`, `upload`, `archive-formats`) so `--grep`/tag-filtered runs are possible without inventing a separate scoping mechanism |
| `regression_fixture` | Regression Fixture reference (nullable) | Set when this scenario exists specifically to guard a known-bad case (spec SC-001); null for scenarios that are pure new-coverage (e.g. a Story 4 format-matrix entry with no prior bug) |
| `retries` | `0 \| 1` | E2E scenarios inherit `1` (FR-012) from Playwright config; unit scenarios are `0` — flakiness in pure logic is a real bug in the test or the code, not something to paper over with a retry |

### Regression Fixture

The known-bad case a scenario exists to guard against (spec Key Entities) — this is a *documentation*
entity (traceable-to-a-bug metadata), not a runtime data structure.

| Field | Type | Notes |
|---|---|---|
| `bug_description` | string | One line, e.g. `"category pinned field sent as '1'/'0', backend Form extractor required literal 'true'/'false', save always 422'd"` |
| `fixed_in` | string | Reference to how the fix is locatable (commit message subject or PR — this project's own convention already writes descriptive commit subjects, e.g. "Fix CI: install libarchive build deps before cargo test") |
| `revert_to_reproduce` | string | A short, concrete instruction for how a maintainer would deliberately reintroduce this bug to verify the test actually catches it (spec User Story 1's Independent Test) — e.g. "revert the `'true'/'false'` string literals back to `'1'/'0'` in `Categories.tsx`'s `saveDetails`" |

Known Regression Fixtures at plan time (from this project's own bug-fix history, per spec's
Assumptions "starts from the specific known-bad scenarios already paid to discover once"):

1. Category `pinned` field save 422 (form-encoded bool serialization mismatch)
2. Large archive upload failing with "Error parsing `multipart/form-data` request" (missing
   `DefaultBodyLimit` override)
3. Archive delete leaving orphaned `LRR_TANKGROUPED`/`LRR_UNTAGGED`/`LRR_NEW`/`LRR_TITLES`/
   `INDEX_<tag>` entries (non-atomic/incomplete index cleanup), surfacing as `/search/random`
   returning empty results
4. Reader toolbar icon spacing (0px instead of legacy's 3px — JSX doesn't preserve the whitespace
   text nodes between sibling elements that legacy's hand-indented HTML template produces)
5. Reader dead whitespace below a page image shorter than 75vh (`.loading` class never removed
   after the image finished loading, since it was applied unconditionally rather than tracking
   per-page load state)
6. CJK archive-entry filename mojibake for non-UTF-8-flagged legacy zip entries, **plus** the
   separate UTF-8-locale failure this project's own fix uncovered (`archive_read_next_header`
   rejecting valid UTF-8-flagged non-ASCII names entirely under a bare `C`/`POSIX` process locale)
7. Upload-vs-watcher ingestion race causing a spurious "This file already exists in the Library"
   409 on a genuinely first-time upload (found while implementing this feature's own e2e tests,
   not a previously-known bug): `POST /archives/upload` wrote the uploaded bytes directly into the
   watched `archive_dir` *before* calling `ingest_file` itself, racing the notify-based file
   watcher's own independent `ingest_file` call on the same newly-appeared file — whichever
   catalogued first "won," and the upload handler's own call then frequently saw the ID as already
   tracked. Fixed by writing to a `temp_dir` staging path (never watched) first, ingesting there,
   then moving the file into `archive_dir` and fixing up the Archive record's/filemap's stored path
   afterward — mirroring legacy's own `handle_incoming_file` order (`~/LANraragi/lib/LANraragi/
   Model/Upload.pm`), which registers the archive in Redis before moving the file into the watched
   content folder specifically "so Shinobu doesn't do it" (its own comment).

### Fixture Archive

A prepared, reusable archive file representing one format/variant the library must handle (spec
Key Entities).

| Field | Type | Notes |
|---|---|---|
| `format` | string | One of the 13 `lanrurugi-scanner`-supported extensions, or `multivolume` / `encrypted` / `non-ascii-filename` for the higher-risk shapes (FR-010) |
| `path` | string | Relative to `test-fixtures/archives/` (research.md §7) |
| `expected_behavior` | `ingest-and-read \| current-behavior-locked` | Plain formats (FR-009) expect successful ingestion + readable pages; the three higher-risk shapes (FR-010) only assert whatever this project's *current* behavior actually is (spec User Story 4 Acceptance Scenarios 2-3) — this field exists precisely so a future behavior change to multi-volume/encrypted handling is a deliberate, reviewed update to this field, not a silently-adjusted assertion |
| `page_count` | integer | Fixtures use 1-2 tiny placeholder images (research.md §8) — enough to exercise ingestion/pagination, not real manga-page content |

## Scenario Contract

In place of a REST/API contract (none is introduced by this feature — see plan.md), this section
fixes the vocabulary/shape that `tasks.md` and the actual test files must conform to, so scenario
naming and tagging stay consistent across both layers rather than drifting per-file.

- **Unit-level scenario file naming**: `apps/frontend/tests/unit/<module-under-test>.test.ts(x)`,
  one file per module (`useReaderSettings.test.ts`, `crossArchiveNav.test.ts`, etc.) — matches
  Vitest's own convention and this project's existing Rust `#[cfg(test)] mod tests` per-file
  pattern (research.md §1, no new convention invented).
- **End-to-end scenario file naming**: `apps/frontend/tests/e2e/<flow-area>.spec.ts` — one file per
  user-facing flow area (`login`, `categories`, `upload`, `reader`, `archive-lifecycle`,
  `archive-formats`), matching Playwright's own `.spec.ts` convention.
- **Fixture reference shape**: any e2e test referencing `test-fixtures/archives/*` uses a shared
  `fixturePath(name: string)` helper (`apps/frontend/tests/e2e/fixturePath.ts`) rather than each
  test file hand-rolling its own relative path — a single point of truth if the fixtures directory
  ever moves. The unit layer has no equivalent need: its scenarios are pure logic with no I/O (spec
  User Story 1 Acceptance Scenario 3), so no unit test references fixture archives at all.
- **Regression traceability**: any test file covering a Regression Fixture entry above states which
  one, in a comment directly above the test (not just in a separate tracking doc), so `git blame`/
  code review on the test file itself surfaces the "why" without cross-referencing this document —
  matching this project's own established commenting convention (explain the non-obvious "why," not
  restate the "what").
