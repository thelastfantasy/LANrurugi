# Implementation Plan: Archive Split Suggestions

**Status**: Draft / waiting for user decisions  
**Scope**: Native feature.  
**Non-goal (this iteration)**: no automatic execution; LLM suggestions are saved and require manual
execution from the reader page.

## Summary

When an archive is ingested/scanned, do a lightweight structural analysis:

- multiple top-level directories
- one top-level directory containing multiple subdirectories
- nested archive files inside the archive

If such a structure is detected, build a JSON representation of the internal directory tree
(including the inner file list of any nested archives) and send it to the LLM. The LLM returns a
fixed JSON split/repackage recommendation whose target is to split the original archive into
multiple **flat, subdirectory-free ZIP archives**. The suggestion is validated, stored, and shown
on the reader page for **manual execution**.

## Feature Switches

Two Settings-page switches:

### Parent switch: `archive_split_suggestions_enabled`

- Default: **`true`**
- The switch is **disabled in the UI when no DeepSeek/LLM API key is configured**
- When disabled:
  - no ingestion-time structural analysis
  - no LLM calls for split suggestions
  - no saved suggestion used on the reader page
- When enabled and an LLM key is configured:
  - ingest/scan may automatically detect suspicious structures
  - LLM suggestions may be generated
  - reader page shows/manual-executes saved suggestions

### Child switch: `archive_split_delete_original_enabled`

- Default: **`false`**
- Only meaningful when the parent switch is enabled
- Controls whether a successful split:
  - deletes the original archive file
  - removes the original archive record from the library
  - removes the original archive from its Tankoubon membership
  - leaves the new split ZIPs in the same Tankoubon

## LLM Prompt

The prompt will live in `crates/lanrurugi-api/src/llm_prompts.rs`, consistent with the existing
centralized prompt policy. Input is a JSON directory tree plus nested archive contents; output is a
fixed JSON schema describing split ZIP groups. All prompt text will be **Chinese**; technical JSON
field names may remain in English.

## Reusable Existing Infrastructure

- `lanrurugi_scanner::archive_format::list_all_entries` for listing archive entries
- `lanrurugi_scanner::archive_format::read_entry` for extracting nested archives
- `lanrurugi_llm::json_chat` for structured LLM output
- existing Settings boolean-field machinery
- existing Repository patterns for storing per-archive metadata

## Resolved Decisions

1. **Trigger**
   - Ingest/scan automatic detection.
   - Not every archive triggers an LLM call: a local pre-filter must identify only structurally
     suspicious archives first, to limit cost/request volume.

2. **Nested archive handling**
   - Flatten nested archive contents into the output flat ZIPs.

3. **Execution**
   - Reader page click starts a server-side **background job**.
   - Progress is reported to the frontend via **SSE** while the reader page is open.
   - If the user chooses to delete the original, after execution the original archive is gone and
     the reader page will 404. The final result must be persisted in the job record so the user can
     see it after returning to the Library/home page. No Service Worker is strictly required: an
     in-app job console/list can poll or reconnect to a global SSE when the user returns. A Service
     Worker would only be needed for OS-level notifications when the tab is completely closed.

4. **Output ZIP naming**
   - LLM JSON includes suggested ZIP filenames.
   - If a generated filename collides on disk, insert the file's CRC32 before the extension:
     `SuggestionName_crc32.zip`.

5. **Output location**
   - Generated archives are written to the **same directory as the original archive**, including
     when the original is inside a subdirectory.

6. **Metadata inheritance and page mapping**
   - Every generated split ZIP inherits the original archive's tags, category memberships,
     bookmarks, and stamps.
   - Bookmark/stamp mapping is filename-based against the natural `list_pages` order, with the
     collision-renamed output name (`file_crc.ext`) also recorded; duplicate filenames from
     different source directories therefore keep the correct original page number.
   - Nested archives are flattened into the output ZIP; their files do not have an outer page
     number (they were not reader pages before the split), so no page mapping is attempted for
     them.

7. **Tree viewer**
   - `GET /archives/{id}/split-tree` returns a flat `ArchiveEntryInfo[]` with nested archives
     recursively expanded into synthetic directories (bounded to `MAX_NESTED_TREE_DEPTH`).
   - Both the single-archive split dialog and the Tankoubon split dialog expose the existing
     tree popover, so the user can inspect outer/nested/sub-sub archive contents before executing.
