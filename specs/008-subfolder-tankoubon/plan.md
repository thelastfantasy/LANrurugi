# Implementation Plan: Subfolder → Tankoubon (Native Feature)

**Status**: Planning  
**Scope**: Native server-side + minimal frontend entry. Includes best-effort LLM naming/author
enrichment when a DeepSeek API key is configured.  
**Non-goal (this iteration)**: Plugin SDK hooks, scan-time auto-creation, automatic deletion of
existing Tankoubons.

## Summary

Add an explicit maintenance operation: scan the library directory and, for every **first-level subdirectory** that directly contains
recognized archive files, create one **Tankoubon** for that directory. All archives found
anywhere under that first-level directory — including archives inside deeper nested
subdirectories — are added to the same Tankoubon. Nested subdirectories do not become separate
Tankoubons. This mirrors the existing `subfolders_to_categories` behavior,
but creates `Grouping` records instead of `Category` records.

The feature is deliberately **explicit/manual** rather than auto-running during a scan, to avoid
accidental groupings from directory trees that are not actually “one series per folder”.

When a DeepSeek API key is configured, the operation may also use the LLM to:

- infer a better Tankoubon name from the directory tree + member filenames/titles;
- infer an `artist:` or `circle:` author tag for the Tankoubon.

If the LLM key is missing, the LLM call fails, or the LLM cannot confidently produce a result, the
operation still succeeds using the folder name as the Tankoubon name and skipping author tagging.

## User Story

As a library owner, I can run a “Subfolders to Tankoubons” maintenance action so that a folder
layout like:

```text
library/
├── My Series/
│   ├── Vol 1.zip
│   └── Vol 2.zip
└── Another Series/
    └── Vol 1.cbz
```

becomes:

```text
Tankoubon "My Series"   -> [Vol 1.zip, Vol 2.zip]
Tankoubon "Another Series" -> [Vol 1.cbz]
```

With an LLM key configured, names and author tags may be improved relative to the raw folder name.

## API Contract (new additive endpoint)

### `POST /api/database/scripts/subfolders-to-tankoubons`

Form/query-compatible with the existing maintenance-script style. Accepts optional query params:

| Param | Type | Default | Meaning |
|---|---|---|---|
No deduplication in v1: repeated runs may create new Tankoubons even when an existing one has the
same name. Duplicate Tankoubon names are allowed by the data model.

Example response:

```json
{
  "operation": "subfolders_to_tankoubons",
  "success": 1,
  "created_tankoubons": ["TANK_1234567890", "TANK_1234567891"],
  "llm_used": true,
  "elapsed_ms": 12
}
```

## Feature Switch

A new `LRR_CONFIG` boolean field `subfolders_to_tankoubons` controls the feature. It defaults to
`true` and appears in the Settings page as a checkbox. Disabling it makes the native endpoint a
non-destructive no-op and disables the Plugins-page maintenance button.

## Prompt Centralization

All LLM system prompts used by the API crate are now managed in
`crates/lanrurugi-api/src/llm_prompts.rs`, including the Subfolders-to-Tankoubons prompt, the
Tankoubon rename prompts, recommendation rerank, artist/coser backfill, plugin-wizard login
analysis, trial-run classification, and plugin generation. Feature modules keep their dynamic user
content, but prompt wording and output-shape rules are centralized.

## Implementation

### Backend

1. **Add route** in `crates/lanrurugi-api/src/scripts.rs`

   ```rust
   Router::new()
       .route("/database/scripts/subfolders-to-categories", post(subfolders_to_categories))
       .route("/database/scripts/subfolders-to-tankoubons", post(subfolders_to_tankoubons))
   ```

2. **Use a first-level-only directory walk**

   Add `walk_first_level_subfolders(root, out)`: it lists the library root's immediate child
   directories as grouping units, then recursively collects every archive file under each one.
   Nested subdirectories are not grouping units, but their archives are included in the first-level
   directory's Tankoubon.

3. **Build `PathBuf -> ArchiveId` map**

   Same pattern as `subfolders_to_categories`:

   ```rust
   let id_by_path: HashMap<PathBuf, ArchiveId> =
       state.repos.archives.list_all().await?
           .into_iter()
           .map(|a| (PathBuf::from(a.file), a.id))
           .collect();
   ```

4. **Build the “directory tree” context for the LLM**

   For each `(folder_name, paths)` group:

   - Map `paths` to existing `ArchiveId`s, dropping files that are not in the DB.
   - Keep the relative path, folder name, member archive titles/filenames.

   If `lanrurugi_llm::resolve_api_key(&state.redis.config).await` succeeds, send one batch JSON
   prompt containing all groups to `lanrurugi_llm::json_chat`:

   ```json
   [
     {
       "folder": "My Series",
       "files": ["My Series/Vol 1.zip", "My Series/Vol 2.zip"],
       "titles": ["Volume 1", "Volume 2"]
     }
   ]
   ```

   The LLM response should be:

   ```json
   [
     {
       "folder": "My Series",
       "tank_name": "My Series",
       "artists": ["Some Author"],
       "circles": ["Some Circle"]
     }
   ]
   ```

   - `tank_name` is optional: use folder name when absent.
   - `artists` and `circles` are optional arrays. Artist and circle names may coexist; there may be
     multiple artists (anthology works). All author/circle names must be English/Latin-script
     (romanized when the original is CJK); omit names when absent or when a reliable romanization
     cannot be produced.
   - Any LLM failure or parse error must be non-fatal: fall back to folder names and no author
     tags for all groups.

5. **Create Tankoubon per subfolder**

   For each `(folder_name, paths)` group:

   - Skip empty groups (all files stale/unimported).
   - Generate a unique Tank ID using the same `TANK_<timestamp>` convention as
     `tankoubons.rs::create_or_rename_tankoubon`.
   - Build `Grouping` with:
     - `name` = LLM-returned name if available, otherwise `folder_name`
     - `tags` = `artist:`/`circle:` tags built from LLM-returned `artists`/`circles` arrays, or
       empty when the LLM returns none
     - `archives = archive_ids`
     - empty `summary`
     - default `chapter_names`, thumbnail fields, timestamps
   - Save through `state.repos.groupings.save(...)`.
   - Keep search index consistent:
     - `lanrurugi_search::indexer::add_tank_to_index(...)`
     - `lanrurugi_search::indexer::update_title_index(..., "", &name)`
     - `lanrurugi_search::indexer::update_tag_indexes(..., "", &tags)`
     - `lanrurugi_search::indexer::sync_tank_membership(..., &joined, &[])`
   - Best-effort thumbnail follow: call the same first-archive cover sync used by the existing
     Tankoubon update path. If the helper is private in `tankoubons.rs`, make it `pub(crate)` or
     move it to a shared module.

6. **Do not delete or deduplicate existing Tankoubons**

   Every run creates new Tankoubons. Duplicate names are allowed and are not treated as errors.

7. **Activity / logging**

   Existing `subfolders_to_categories` does not write activity entries. For consistency and to
   keep v1 small, the new endpoint may skip activity logging; if desired, record one
   `tankoubon.create`-style manual event per created tank as a follow-up.

### Frontend

1. Add a maintenance-script entry next to “Subfolders to Categories” in
   `apps/frontend/src/pages/Plugins/PluginsPage.tsx`.

2. Reuse the existing `runScript` helper with path `subfolders-to-tankoubons`.

3. After execution, invalidate:

   - `["archives"]`
   - `["categories"]` (unchanged)
   - `["tankoubons"]`
   - search queries so the new Tankoubon appears in grouped search results.

4. Add i18n strings for the new button/description. The button can show whether LLM enrichment was
   used based on the response (`llm_used`).

## Tests

### Server integration test

Add a test to `crates/lanrurugi-server/tests/contract_api.rs` modeled on
`subfolders_to_categories_creates_a_category_visible_in_list_all`:

1. Create a temp library dir with two subfolders, each containing one or more fake archive files.
2. Insert corresponding `Archive` records.
3. `POST /api/database/scripts/subfolders-to-tankoubons`
4. Assert:
   - response is `success: 1`
   - `created_tankoubons` contains two IDs
   - `GET /api/tankoubons` returns both Tankoubons
   - each Tankoubon has the correct `archives` membership
   - each Tankoubon is discoverable through the search/grouped path.

### LLM tests

- Unit-test the LLM prompt/parse code with a mocked/forced JSON response, or split the parsing
  into a pure function that can be tested without a network call.
- No LLM key → endpoint still succeeds and uses folder names + empty tags.
- LLM returns missing/empty `tank_name`/`artists`/`circles` → falls back to folder name and skips
  author tags.
- LLM call errors → non-fatal, same fallback.

### Edge cases

- No subfolders → zero created tanks, success.
- Direct children of the library root are excluded (same as FolderToCat).
- Subfolder contains only unimported/stale files → no tank created.
- Same subfolder run twice → two Tankoubons may be created; duplicate names are allowed.

## Open Decisions

1. **LLM batching**: one batch call per endpoint is proposed. If the directory tree is very large,
   we may need to cap prompt size or fall back to per-folder calls.
2. **Author tagging**: v1 only writes a single `artist:` or `circle:` tag to the Tankoubon itself,
   not to each member archive.
3. **Naming**: use LLM when available; otherwise raw folder name.
4. **Auto common tags beyond author**: v1 does not do general common-tag intersection; only the
   LLM-inferred author tag is written.

## Follow-ups (out of scope here)

- Plugin SDK `ScriptResult.tankoubons_to_create` mirror.
- Scan-time automatic creation behind a disabled-by-default setting.
- Optional `delete_old_tankoubons` / conflict handling.
