---

description: "Task list for On-Page Manga Translation (Phase 2)"
---

# Tasks: On-Page Manga Translation (Phase 2)

**Input**: Design documents from `/specs/004-ocr-manga-translation/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md (all present)

**Tests**: Not explicitly requested in spec.md (no TDD mandate). Each story ends with a task that
runs its `quickstart.md` scenario as the acceptance check, consistent with
`specs/001-lanrurugi-full-rewrite/tasks.md`'s convention for this project.

**Scope**: This feature is additive to the Phase 1 codebase (`specs/001-lanrurugi-full-rewrite`)
— new crates and frontend modules only. No task here modifies any Phase 1 file, per constitution
Principle VI. Building this feature assumes Phase 1 is already implemented and running.

**Revision note (2026-07-06, second pass)**: Researching OCR options (triggered by verifying
`ort`'s version against the constitution's "latest stable, verified" rule) surfaced Koharu
(mayocream/koharu), a complete Rust manga-translation project. Its current code is
**GPL-3.0-only and explicitly `publish = false`** (verified directly via `gh api`) — not usable as
a dependency — but it validated the detection+recognition architecture and pointed at
kha-white's **Apache-2.0** `manga-ocr` model as the right recognition choice for manga/vertical-
Japanese accuracy. Detection now uses `oar-ocr` (Apache-2.0, PP-OCR-based); recognition is a
custom `ort` integration against `manga-ocr`'s weights, written independently (not derived from
Koharu's GPL code — Koharu was cloned locally for architecture reference only, per the user's
instruction, never as a build dependency). This added one Foundational task (recognition
inference) and adjusted T002–T004/T006–T007's content. Task IDs were renumbered accordingly
(previous T001–T064 → current T001–T065); this is the authoritative numbering.

**Revision note (2026-09-06, `/speckit-clarify`)**: A per-block visual-attribute-fidelity gap was
identified — the original design only matched font *family* at the volume level (T011–T014), with
no per-block color/boldness fidelity to the original text at all. Researched against
`zyddnys/manga-image-translator` (GPL-3.0, cloned locally to `~/manga-image-translator` for
architecture reference only, same Koharu-precedent handling as above) — see spec.md Clarifications
Session 2026-09-06 and research.md §13 for the full finding, including that even that project's
own `bold` field is never actually computed by any of its code. Added T010a (heuristic color/
boldness estimation, Foundational) and updated T034/T035/T050 (server compositing, text-regions
endpoint, client compositing) to consume and apply the three new independently-nullable
`Detected Text Region` attributes. Added spec.md FR-008a and SC-003a. No existing task ID was
renumbered — T010a is inserted between T010 and T011 without shifting T011 onward, consistent with
this project's task-ID stability convention.

**Revision note (2026-09-06, second `/speckit-clarify` pass, same day)**: A translation-*content*
consistency gap was identified — every text block's translation request was fully independent, so
the same character name/term could render with a different translation each time it recurred
across a volume, and per-block requests had no visibility into surrounding dialogue for
tone/style consistency either. Addressed via spec.md FR-007a–e (Terminology Glossary +
advisory context assembly) and SC-003b — see spec.md Clarifications (Session 2026-09-06, the two
entries after the visual-fidelity one above) and research.md §14 for the full reasoning, including
why tone consistency is handled as advisory context rather than a cacheable entity like the
glossary. Added T019a (glossary entity/storage), T019b (context assembly: exact-match lookup,
variant-recognition name list, same-page tone reference), T032a (glossary management endpoints),
and T032b (glossary UI); updated T028's description to route through T019b. No existing task ID
was renumbered.

A follow-up cross-path gap was then caught during self-review: the locally-hosted-backend path
(US4) never routes its translation call through the server at all (constitution Principle V), so
without deliberate wiring it would receive none of FR-007a–e's consistency benefit, and any
name/term it discovered would never reach the shared, server-stored glossary. Fixed by updating
T035/T049 to carry `context` to/from the local-backend path and adding T049a (a lightweight
"record this translation" endpoint, no LLM call) and T049b (wiring the local path to call it) —
see `contracts/client-compositing-cache.md` and `contracts/translation-api.md`'s
`/translation/record` entry for the full shape.

**Revision note (2026-09-06, third `/speckit-clarify` pass, same day)**: Three further gaps
surfaced through direct user questioning about prefetch/batching/caching mechanics and cross-cutting
integration:
1. **Request batching + prompt-cache alignment (research.md §15)**: the original one-request-
   per-block design was superseded by small-fixed-batch translation (2–4 pages, mirroring T009's
   OCR batch size), with request content ordered stable-prefix-first so Terminology Glossary
   content is cacheable across requests. An append-only per-volume session (modeled on
   `deepseek-ai/deepseek-harness`, cloned locally for reference) was investigated and rejected —
   its precondition (a long-running sequence of repeated LLM calls to amortize a growing prefix
   over) doesn't hold here once §16 below is accounted for. Added T015 batched-shape update,
   T016/T017 cache-ordering notes, and **T017a** (a third adapter: DeepSeek, whose caching is
   automatic/disk-backed and needs no explicit marker, unlike Anthropic's `cache_control`).
2. **Persistence reversal (research.md §16)**: `Detected Text Region` (text + resolved style,
   including a new `font` field) is now the authoritative, long-term-persisted record; the
   rendered/composited page image (`Translation Cache Entry`) becomes a re-derivable performance
   cache instead of the durable artifact this document originally assumed. Updated T028 (checks
   persisted `translated_text` before sending a block in a batch) and T029/T034 (compositing reads
   from and writes resolved style back onto the persisted record, rather than being the only place
   translation output exists).
3. **Cross-cutting integration (research.md §17)**: this feature's new Redis entities were
   entirely absent from Phase 1's existing backup/export and Activity audit mechanisms — a real
   data-loss/auditability gap, not a stylistic one, per constitution Principle I. Added spec.md
   FR-022/FR-023 and Polish-phase tasks **T064a** (backup/restore enumeration), **T064b** (Activity
   `action_type` write-site wiring), **T064c** (Activity page's namespace-table entries). LLM-call-
   specific audit fields (token usage, cache-hit rate, estimated cost) were identified as a
   cross-project gap beyond this feature's own scope and tracked in GitHub issue #100 instead of
   being designed here.
4. **Disk-cache quota correction (research.md §18)**: a direct follow-up question about whether
   the server-composited `Translation Cache Entry` image cache shares quota with an existing Phase
   1 cache surfaced that this document's own earlier wording ("alongside Phase 1's existing
   thumbnail cache") was never actually verified against the code and is wrong — the thumbnail
   cache (`thumb_dir`) has no size quota or eviction at all; the mechanism that actually has one is
   the reader's resize-page cache (`tempmaxsize`). Corrected T020's description and research.md §6
   accordingly; no new task, no FR change — a factual correction to an implementation detail, not
   a new requirement.
No existing task ID was renumbered in any of the four.

**Implementation regression note (2026-09-06)**: A first implementation pass marked T001–T065
(plus all inserted sub-tasks) as `[X]` complete, and `mise run clippy`/`fmt-check`/`tsc`/ESLint/184
Rust unit tests all passed clean. An independent code-consistency audit against spec.md/research.md
then found that despite this, the server-side orchestration connecting the individually
well-implemented pieces was **never actually wired**: `get_page_translation` (T028) checks the
rendered-image cache and returns `not_ready()` on a miss, but never calls
`lanrurugi_ocr::batch::run_batch`, `lanrurugi_translate::pipeline::translate_batch`, or
`composite_page` (T029/T034); `PrefetchScheduler` (T039) is defined and tested but never
instantiated outside its own test module, with no `tokio::spawn` registration anywhere in
`lanrurugi-server` (contrast with `main.rs`'s real periodic tasks, which *are* spawned). The
practical effect: enabling translation would never produce a real translated page — every request
polls until timeout and falls back to the FR-019 failure path, indistinguishable from a genuinely
broken backend. T028/T029/T034/T039 have been reverted to `[ ]` to reflect this; every other
`[X]` task (OCR/font-classification/adapter/glossary/context-assembly/backup/Activity units, all
independently verified as correctly implemented and genuinely tested) is unaffected. This is
recorded as a lesson for this feature's remaining work, not just a status correction: passing
tests and clean lints verify that written units behave as designed in isolation — they do not
verify those units were ever actually invoked by the request-handling code path a user's action
goes through. That distinction needs its own explicit check (e.g. tracing a handler's call graph
down to the primitives it's supposed to invoke) before marking an orchestration-shaped task done.

**Orchestration wiring note (2026-09-06, third pass — fixes the regression above)**: T028/T029/
T034/T039 are now genuinely done. What was actually missing was never the individual pieces (all
correct and tested) but the code that calls them, plus two parts that had no implementation at all:

1. **`crates/lanrurugi-api/src/translation_pipeline.rs`** (new) holds the whole chain —
   `ensure_detected` (→ `run_batch` → font voting/routing → `save_detected`), `translate_and_persist`
   (→ context-assembled `translate_batch` → `save_translated` → glossary capture →
   `BudgetRepository::record`), and `composite_and_cache` (→ `composite_page` →
   `TranslationImageCache::put`). `translate_page` sequences them, persisting after each step so an
   interruption only costs the steps after it (research.md §16).
2. **`crates/lanrurugi-translate/src/fonts.rs`** (new) was a genuine hole: `composite_page` takes a
   borrowed `FontSet`, and *nothing outside `composite.rs`'s own test module had ever built one* —
   no production code path loaded a font file at all. `FontLibrary` discovers a CJK-capable
   fallback plus per-style faces once per process.
3. **Lazy runtime + scheduler on `AppState`**: `TranslationRuntime` loads the OCR models and fonts
   on first actual use (translation is off by default per FR-007, and a deployment with no model
   files must still boot), and `TranslationScheduler` drives `PrefetchScheduler` as described in
   T039 above.
4. **The local-backend path (US4) had the same defect** and was not in the original regression
   list: `get_page_text_regions` returned not-ready forever on a detection miss, waiting on a
   look-ahead that this path doesn't have. It now triggers `ensure_detected` itself — detection
   only, no provider call and no credential, since the translated text on that path only ever
   exists in the browser (constitution Principle V).

**Verification method (the lesson from the regression, applied)**: passing tests and clean lints
were what produced the false "done" last time, so this pass added two checks they don't cover.
First, a `grep` over `crates/lanrurugi-api/` and `crates/lanrurugi-server/` confirming every key
primitive (`run_batch`, `translate_batch`, `composite_page`, `save_detected`,
`TranslationImageCache::put`, `BudgetRepository::record`, `PrefetchScheduler::new`) has at least one
*non-test* call site. Second — and this is the one that actually mattered — reading the call graph
top-down from the HTTP handler to each primitive, confirming reachability rather than mere presence.
That second check is what caught three real bugs a green test run would have shipped: the
`VolumeId` scope mismatch noted under T034; a `Default`-derived semaphore that would have had zero
permits and deadlocked every spawned page; and a page stuck permanently at "not ready" after one
transient failure, because `PrefetchScheduler` deliberately never auto-retries a `Failed` page and
nothing distinguished that rule from an explicit user request for the page they're looking at now.

**Independent re-audit (2026-09-06, fourth pass)**: Given the third pass was itself a fix for a
"looked done but wasn't" failure, its own claim of being fixed was not taken on faith — a second,
independent audit re-traced the same five things from scratch (HTTP handler down to each
primitive, `PrefetchScheduler` actually being driven, the three bug fixes above, the local-backend
detection trigger, and re-running `mise run check-crate` for `lanrurugi-api`/`lanrurugi-translate`/
`lanrurugi-server` itself rather than trusting the prior pass's pasted output). **Confirmed
genuinely fixed** — no further orchestration gaps found. Three minor, non-blocking items surfaced
during this re-audit and are recorded here rather than actioned, since none affect correctness:
- Font resolution failures (`resolve_fonts`) are logged via `tracing::warn!` and silently degrade
  to a fallback font rather than surfacing as a per-page FR-019 failure — a deliberate "don't let
  font trouble block translation" choice, but it means a persistently broken golden-set would be
  invisible to a user, diagnosable only from server logs.
- `get_page_translation`'s cache-miss branch always records a `record_lookahead(false)` telemetry
  event even when the miss is for the *current* page rather than a genuine look-ahead page — a
  naming/semantics mismatch in the SC-004 telemetry (T063), not a functional bug.
- These changes remain uncommitted in the working tree (verified via `git status`) — noted as a
  process reminder, not a code issue.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1–US5, per spec.md)
- Every task names an exact file path, per plan.md's Project Structure

## Path Conventions

New Rust crates under `crates/lanrurugi-{ocr,fontcache,translate}/src/...`, extending the
existing Phase 1 Cargo workspace; extensions to the existing `crates/lanrurugi-api/src/...`;
new frontend code under `apps/frontend/src/translation/...` and new components alongside Phase 1's
existing `apps/frontend/src/components/`, `apps/frontend/src/pages/`. See `plan.md` § Project Structure.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add this feature's new crates/modules to the existing Phase 1 workspace/app, and get
a real OCR recognition model file onto disk (detection models are fetched by `oar-ocr` itself).

- [X] T001 Add `lanrurugi-ocr`, `lanrurugi-fontcache`, `lanrurugi-translate` members to the
      Cargo workspace in `Cargo.toml`
- [X] T002 [P] Add `oar-ocr` (PP-OCR text detection, Apache-2.0) and `ort` + `ndarray` (for the
      custom `manga-ocr` recognition inference, CPU-only per research.md §1) dependencies to
      `crates/lanrurugi-ocr/Cargo.toml`
- [X] T003 [P] Document the `manga-ocr` recognition model's file placement and the
      model-discovery env var convention (research.md §1, mirroring `~/jellyfin-suite`'s
      `find_model()` pattern) in `crates/lanrurugi-ocr/README.md` — detection models are fetched
      automatically by `oar-ocr` and don't need this convention
- [X] T004 Add a model-acquisition step that places kha-white's **Apache-2.0** `manga-ocr`
      recognition model (an existing ONNX export of its weights) at the path T003 documents — a
      Dockerfile build stage for production images plus a `scripts/fetch-ocr-model.sh` for local
      dev — downloading a pinned release/version with checksum verification (exact ONNX artifact
      source to be verified against the live repository/hosting at implementation time, not
      assumed) in `Dockerfile` and `scripts/fetch-ocr-model.sh`
- [X] T005 [P] Create the `apps/frontend/src/translation/` directory skeleton with barrel exports in
      `apps/frontend/src/translation/index.ts`

**Checkpoint**: Workspace builds with the new (empty) crates; a real `manga-ocr` recognition
model file is present at the documented path; frontend module skeleton in place.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core OCR, font-cache, and translation-adapter infrastructure every user story
depends on.

**⚠️ CRITICAL**: No user story task may begin until this phase is complete.

- [X] T006 Implement model-file discovery for the `manga-ocr` recognition model (env var →
      binary-relative → config path, first match wins; research.md §1) in
      `crates/lanrurugi-ocr/src/model_discovery.rs`
- [X] T007 Integrate `oar-ocr` for text-region detection (Apache-2.0, research.md §1) in
      `crates/lanrurugi-ocr/src/detect.rs`
- [X] T008 Implement custom CPU-only `ort` Session initialization and encoder/decoder inference
      for kha-white's Apache-2.0 `manga-ocr` recognition model (research.md §1 — chosen for
      manga/vertical-Japanese accuracy over generic recognition; written independently, not
      derived from Koharu's GPL-licensed wrapper code — see Notes) in
      `crates/lanrurugi-ocr/src/recognize.rs`
- [X] T009 [P] Implement rayon-batched OCR inference (detection + recognition, batch size 4–8) —
      the whole batch MUST be dispatched through a single `rayon`-parallel call (e.g. this
      codebase's own `parallel_map` helper) bridged via one `tokio::task::spawn_blocking`
      (reusing `crates/lanrurugi-core/src/concurrency.rs` from Phase 1), never a `for` loop issuing
      one `spawn_blocking` call per page — that shape is the exact anti-pattern constitution
      Principle III names and forbids (previously shipped once in this codebase as a thumbnail-
      regeneration bug) — in `crates/lanrurugi-ocr/src/batch.rs`
- [X] T010 Implement IoU/geometric line-paragraph merging producing `Detected Text Region`
      records in `crates/lanrurugi-ocr/src/merge.rs`
- [X] T010a [P] Implement the per-region `fg_color`/`bg_color`/`is_bold` heuristic estimation pass
      (FR-008a, research.md §13 — KMeans-style clustering over each region's cropped pixels for
      color, stroke-width-to-glyph-height ratio for boldness; each of the three attributes
      independently nullable on low confidence, folded into the same rayon batch as T009, not a
      separate sequential stage) in `crates/lanrurugi-ocr/src/style_estimate.rs`
- [X] T011 [P] Define the `Volume Font Pattern` entity and Redis schema (`vote_pool`,
      `golden_set`, `meltdown_tally`, `is_locked`) in `crates/lanrurugi-fontcache/src/entities.rs`
- [X] T012 Implement cover-excluded `vote_pool` accumulation (voting stage, FR-008) — this runs
      the full font classifier per block and MUST be bridged via `tokio::task::spawn_blocking`
      (reusing `crates/lanrurugi-core/src/concurrency.rs`), the same rule T009 follows, per
      constitution Principle III and plan.md's Technical Context — in
      `crates/lanrurugi-fontcache/src/voting.rs`
- [X] T013 Implement the two-sequential-checks routing — lock-state check, then a cheap
      per-block feature classifier among `golden_set` (research.md §4) — in
      `crates/lanrurugi-fontcache/src/routing.rs`
- [X] T014 Implement meltdown re-classification into a separate `meltdown_tally` that never
      feeds `vote_pool` (research.md §4, the resolved review concern); like T012, this re-runs
      the full classifier and MUST use the same `spawn_blocking` bridge — in
      `crates/lanrurugi-fontcache/src/meltdown.rs`
- [X] T015 [P] Define the normalized LLM provider adapter trait — batched request/response shape
      (array of `block_id`-tagged blocks, research.md §15) — in
      `crates/lanrurugi-translate/src/adapter.rs`
- [X] T016 Implement the OpenAI-compatible adapter (covers OpenAI-compatible providers and the
      Ollama preset); no explicit cache marker needed, relies on stable-prefix ordering
      (research.md §15) in `crates/lanrurugi-translate/src/openai_compat.rs`
- [X] T017 Implement the Anthropic adapter (`system` field, content-block array,
      `x-api-key`/`anthropic-version`, mandatory `max_tokens`), marking a `cache_control`
      breakpoint after the stable Terminology-Glossary-derived prefix (research.md §15) in
      `crates/lanrurugi-translate/src/anthropic.rs`
- [X] T017a [P] Implement the DeepSeek adapter (OpenAI-Chat-Completions-shaped wire format, no
      explicit cache opt-in — its context caching is automatic and disk-backed, research.md §15)
      in `crates/lanrurugi-translate/src/deepseek.rs`
- [X] T018 Implement server-side `credential_ref` resolution so a secret is never logged or
      returned in any response (constitution Principle V) in
      `crates/lanrurugi-translate/src/credentials.rs`
- [X] T019 Implement server-side Translation Backend Selection + Target Language Preference
      Redis storage in `crates/lanrurugi-translate/src/settings.rs`
- [X] T019a [P] Define the `Terminology Glossary` entity and Redis schema (`entries: map<source_term,
      translation>`, scoped per volume/per-archive-if-ungrouped like Volume Font Pattern) in
      `crates/lanrurugi-translate/src/glossary.rs`
- [X] T019b Implement `context` assembly (FR-007b/c/e, `contracts/llm-provider-adapter.md`,
      research.md §14) — exact-substring glossary lookup against a block's `source_text` (FR-007b),
      the volume's other known glossary source-term names for variant recognition (FR-007c), and the
      current page's other already-translated blocks for tone reference (FR-007e); a name/term not
      resolved by exact match becomes a new `entries` record after translation (FR-007a) — in
      `crates/lanrurugi-translate/src/context_assembly.rs`
- [X] T020 Implement the server-composited `Translation Cache Entry` variant (Redis metadata +
      on-disk image cache sharing the reader resize-page cache's `tempmaxsize` quota and periodic
      sweep, research.md §18 — reuses `crates/lanrurugi-api/src/download_manager/ingest.rs`'s
      existing sweep mechanism rather than introducing a new one) in
      `crates/lanrurugi-translate/src/cache.rs`
- [X] T021 Implement Usage Budget tracking at page/archive/day/week granularity (FR-014) in
      `crates/lanrurugi-translate/src/budget.rs`
- [X] T022 [P] Implement the `localStorage` wrapper and device-local precedence resolution
      (FR-003, research.md §8) in `apps/frontend/src/translation/settings.ts`
- [X] T023 [P] Implement the IndexedDB/Cache API wrapper for the client-composited cache
      (research.md §7) in `apps/frontend/src/translation/cache.ts`

**Checkpoint**: OCR detection+recognition, font-pattern voting/routing/meltdown, both LLM
adapters, and the backend-selection/cache storage split are all available to every subsequent
story.

---

## Phase 3: User Story 1 - Read a page with on-page translation (Priority: P1) 🎯 MVP

**Goal**: Core enable/select/translate/render flow, cloud-backend path end-to-end.

**Independent Test**: `quickstart.md` §1 — enable translation, select a cloud backend, open a
page, confirm translated text renders and no credential reaches the browser.

- [X] T024 [US1] Implement the enable/disable toggle for on-page translation (FR-001) in
      `apps/frontend/src/components/TranslationSettings.tsx`
- [X] T025 [P] [US1] Implement `GET`/`PUT /translation/settings` endpoints (FR-002, FR-003) in
      `crates/lanrurugi-api/src/translation_settings.rs`
- [X] T026 [US1] Implement the backend-category selection UI (cloud vs. locally-hosted) in
      `apps/frontend/src/components/TranslationSettings.tsx`
- [X] T027 [US1] Implement the target-language selection UI with browser-language fallback
      (FR-004) in `apps/frontend/src/components/TranslationSettings.tsx`
- [X] T028 [US1] Implement `GET /archives/{id}/page/{page}/translation`, orchestrating OCR +
      context assembly (T019b) + batched translate (research.md §15 — checks each block's
      `Detected Text Region.translated_text` first per §16 and only sends still-untranslated
      blocks in the batch) + composite + cache for the cloud-backend path
      (`contracts/translation-api.md`) in `crates/lanrurugi-api/src/translation.rs`.
      **REGRESSION (verified 2026-09-06 via independent code-consistency audit)**: the handler
      exists but only checks `TranslationImageCache::get` and returns `not_ready()` on a miss — it
      never calls `lanrurugi_ocr::batch::run_batch`, `lanrurugi_translate::pipeline::translate_batch`,
      or compositing. No orchestration was ever wired; this task is NOT actually done despite the
      prior `[X]`. **FIXED (2026-09-06, second pass)**: the orchestration now lives in the new
      `crates/lanrurugi-api/src/translation_pipeline.rs` and is reached from the handler on every
      cache miss — see this file's own "Orchestration wiring" revision note below.
- [X] T029 [US1] Implement server-side compositing (draw translated text over the original page
      image) reading from the persisted `Detected Text Region.translated_text`/style fields
      (research.md §16 — compositing is a re-derivable step over already-persisted data, not the
      point where translation happens) in `crates/lanrurugi-translate/src/composite.rs`.
      **REGRESSION (verified 2026-09-06)**: `composite_page` exists and is unit-tested, but is
      never called from any API handler or background task in `lanrurugi-api`/`lanrurugi-server` —
      only from its own test module. Reverted to not-done. **FIXED (2026-09-06, second pass)**:
      called from `translation_pipeline::composite_and_cache`, which also required building the
      font-loading layer (`crates/lanrurugi-translate/src/fonts.rs`) that never existed — nothing
      outside `composite.rs`'s own tests had ever supplied a real `FontSet`.
- [X] T030 [US1] Audit that no code path logs, returns, or otherwise exposes a cloud credential
      to the browser (FR-006) across `crates/lanrurugi-translate/`
- [X] T031 [US1] Ensure translation-disabled reading takes zero extra code paths or latency
      (FR-007) in `crates/lanrurugi-api/src/translation.rs`
- [X] T032 [US1] Wire the reader to request and display the translated overlay when enabled in
      `apps/frontend/src/pages/Reader.tsx`
- [X] T032a [P] [US1] Implement `GET`/`PUT`/`DELETE /volumes/{id}/terminology-glossary[/{term}]`
      (FR-007d, `contracts/translation-api.md`) in
      `crates/lanrurugi-api/src/terminology_glossary.rs`
- [X] T032b [US1] Implement the terminology glossary view/edit/delete UI in
      `apps/frontend/src/components/TerminologyGlossaryControl.tsx`
- [ ] T033 [US1] Run `quickstart.md` §1 and confirm SC-001/SC-002/SC-003b

**Checkpoint**: A user can enable translation, pick a cloud backend, and read a translated page —
demoable MVP for this feature.

---

## Phase 4: User Story 2 - Translated text looks like it belongs on the page (Priority: P2)

**Goal**: Translated text renders in a font matched to the volume's established style.

**Independent Test**: `quickstart.md` §2 — read several pages of a volume, confirm a small,
consistent set of matched fonts is used once the pattern locks.

- [X] T034 [US2] Wire `lanrurugi-fontcache`'s routing (T013/T014) into server-side compositing
      (T029) so translated text draws in the matched golden-set font, persisting the resolved
      `font` onto the `Detected Text Region` record (data-model.md, research.md §16) rather than
      recomputing it on every render, and applying each region's own `fg_color`/`bg_color`/
      `is_bold` from T010a with independent per-attribute fallback (FR-008a) in
      `crates/lanrurugi-translate/src/composite.rs`.
      **REGRESSION (verified 2026-09-06)**: depends entirely on T029/T028 actually running
      compositing, which they don't — `accumulate_votes`/`classify_batch`/`lock_golden_set` are
      never called from any API handler either. Reverted to not-done. **FIXED (2026-09-06, second
      pass)**: `translation_pipeline::resolve_fonts` runs the voting or routing stage per page and
      persists the resolved `font` onto the region record at detection time; compositing reads it
      back. Tracing this call chain also caught a scope bug worth recording — font patterns were
      being written under `VolumeId::from_archive` but read back under the Tankoubon-aware
      `resolve_volume_id`, so for any *grouped* volume the golden set would have been written and
      never read, silently disabling font matching for exactly the multi-archive case it matters
      most for.
- [X] T035 [P] [US2] Implement `GET /archives/{id}/page/{page}/text-regions`, exposing detected
      regions (including each region's `fg_color`/`bg_color`/`is_bold` from T010a), the volume's
      current golden font set, and the same `context` assembly from T019b (so the local-backend
      path gets FR-007a–e's consistency benefit, `contracts/client-compositing-cache.md`) in
      `crates/lanrurugi-api/src/text_regions.rs`
- [X] T036 [US2] Implement `POST /volumes/{id}/font-pattern/reset` (FR-010) in
      `crates/lanrurugi-api/src/font_pattern.rs`
- [X] T037 [P] [US2] Implement a "reset font pattern" UI control in
      `apps/frontend/src/components/VolumeFontPatternControl.tsx`
- [ ] T038 [US2] Run `quickstart.md` §2 and confirm SC-003/SC-003a

**Checkpoint**: Font matching is visibly consistent within a volume and resettable if wrong.

---

## Phase 5: User Story 3 - Reading stays fast while translation is on (Priority: P2)

**Goal**: Sliding-window prefetch, non-blocking not-yet-ready state with a real client-side
ready/swap mechanism, budget enforcement and visibility.

**Independent Test**: `quickstart.md` §3 — read forward with look-ahead enabled, confirm no
visible delay within the window and a working usage breakdown.

- [X] T039 [US3] Implement the sliding-window look-ahead scheduler (reusing Phase 1's rayon/
      tokio concurrency bridge) in `crates/lanrurugi-translate/src/prefetch.rs`.
      **REGRESSION (verified 2026-09-06)**: `PrefetchScheduler` is defined and unit-tested, but
      `PrefetchScheduler::new()` is never instantiated outside its own test module — no
      `tokio::spawn` registration exists anywhere in `lanrurugi-server`/`lanrurugi-api` (contrast
      with `main.rs`'s real periodic tasks: `sweep_stale_queue_items`, `sweep_resize_cache_size`,
      etc., which ARE spawned). The scheduler is inert. Reverted to not-done. **FIXED (2026-09-06,
      second pass)**: `PrefetchScheduler` is a pure state machine — it answers "which pages still
      need work" but has no way to run any, and (the point the original pass missed) no input
      source telling it where the reader actually is. Both gaps are now filled by
      `translation_pipeline::TranslationScheduler`, which owns one `PrefetchScheduler` per reading
      session, receives the reader's position from `get_page_translation` on every cache miss
      (`on_reader_at`), and `tokio::spawn`s the per-page work under a
      `precompute_worker_budget()`-sized semaphore. A periodic `tokio::spawn` in `main.rs`
      (`sweep_finished_translation_sessions`, alongside the existing sweeps) drops finished
      sessions so the per-archive state doesn't accumulate for the process's whole uptime.
      Deliberately *not* a periodic scan of its own: a look-ahead window is only meaningful
      relative to where a reader currently is.
- [X] T040 [US3] Implement the not-yet-ready response for the translation endpoint (T028) when a
      page is still processing (FR-012) in `crates/lanrurugi-api/src/translation.rs`
- [X] T041 [P] [US3] Implement the non-obscuring loading indicator component in
      `apps/frontend/src/translation/components/LoadingIndicator.tsx`
- [X] T042 [US3] Implement client-side ready-detection polling that, upon seeing T040's
      not-yet-ready response, retries until the translated page is available and then swaps it
      in seamlessly in place of T041's indicator (FR-012's "seamless replace" behavior,
      previously unassigned) in `apps/frontend/src/translation/readyPoller.ts`
- [X] T043 [US3] Implement usage-budget enforcement capping look-ahead activity for metered
      backends (FR-013) in `crates/lanrurugi-translate/src/prefetch.rs`
- [X] T044 [P] [US3] Implement `GET /translation/usage` (FR-014) in
      `crates/lanrurugi-api/src/translation_usage.rs`
- [X] T045 [P] [US3] Implement the usage panel (page/archive/day/week breakdown, optional chart)
      in `apps/frontend/src/translation/components/UsagePanel.tsx`
- [X] T046 [US3] Implement abandonment of in-flight look-ahead requests on navigate-away
      (FR-015) in `apps/frontend/src/translation/prefetchController.ts`
- [X] T047 [US3] Wire `Translation Cache Entry` reuse keyed by (page, target language, backend)
      into the prefetch and serve paths (FR-016) in `crates/lanrurugi-translate/src/cache.rs`
- [ ] T048 [US3] Run `quickstart.md` §3 and confirm SC-004/SC-005

**Checkpoint**: Reading with translation enabled feels as fast as without it, within budget, with
a real (not just designed) not-ready-to-ready transition.

---

## Phase 6: User Story 4 - Using a locally-hosted model works without installing extra software (Priority: P3)

**Goal**: Browser-direct local-backend calls, client-side compositing/caching, PNA guidance.

**Independent Test**: `quickstart.md` §4 — configure a locally-hosted backend per the documented
path, confirm pages translate, then confirm blocked connections show guided fallback.

- [X] T049 [US4] Implement the browser-side direct call to the locally-hosted backend, including
      the `context` received from `text-regions` (T035) in the direct call the same way the server
      includes it for the cloud path (FR-007b/c/e, `contracts/client-compositing-cache.md`) in
      `apps/frontend/src/translation/localBackend.ts`
- [X] T049a [P] [US4] Implement `POST /archives/{id}/page/{page}/translation/record` — records a
      locally-hosted-backend translation result into the shared, server-stored Terminology
      Glossary (FR-007a, `contracts/translation-api.md`) — no LLM call, a lightweight write only —
      in `crates/lanrurugi-api/src/translation.rs`
- [X] T049b [US4] Wire T049's local-backend translation result to call T049a's record endpoint
      after each successful local translation in `apps/frontend/src/translation/localBackend.ts`
- [X] T050 [US4] Implement client-side Canvas/OffscreenCanvas compositing (no WASM, research.md
      §7), consuming `text-regions` (T035) including each region's `fg_color`/`bg_color`/`is_bold`
      with independent per-attribute fallback (FR-008a, `contracts/client-compositing-cache.md`)
      in `apps/frontend/src/translation/composite.ts`
- [X] T051 [P] [US4] Wire the client-composited result into the IndexedDB/Cache API wrapper
      (T023) in `apps/frontend/src/translation/localCache.ts`
- [X] T052 [US4] Implement Private-Network-Access failure detection and guided fallback UI
      (FR-018, research.md §9) in
      `apps/frontend/src/translation/components/LocalBackendGuidance.tsx`
- [X] T053 [P] [US4] Write the zero-extra-install configuration documentation (e.g.
      `OLLAMA_ORIGINS`/PNA settings) in `docs/translation-local-backend.md`
- [X] T054 [US4] Confirm the device-local backend selection (T022) takes precedence over the
      server-stored default end-to-end in `apps/frontend/src/translation/settings.ts`
- [ ] T055 [US4] Run `quickstart.md` §4 and confirm SC-006

**Checkpoint**: A locally-hosted backend works with zero extra installed software in the common
case, and fails gracefully with guidance when it can't connect.

---

## Phase 7: User Story 5 - Translation failures never take down reading (Priority: P3)

**Goal**: Normalized failure handling across both backend paths; reading never blocks on a
translation problem.

**Independent Test**: `quickstart.md` §5 — point the backend at an unreachable endpoint, confirm
the original page reads immediately with a clear per-page indicator.

- [X] T056 [US5] Implement normalized error kinds (`unreachable`, `auth_failed`, `rate_limited`,
      `malformed_response`) across both adapters in `crates/lanrurugi-translate/src/adapter.rs`
- [X] T057 [US5] Implement fallback-to-original-page display on any translation failure (FR-019)
      in `crates/lanrurugi-api/src/translation.rs` and `apps/frontend/src/pages/Reader.tsx`
- [X] T058 [US5] Ensure per-page/per-archive failure isolation — no shared mutable state that
      could cascade a single page's failure (FR-020) in
      `crates/lanrurugi-translate/src/prefetch.rs`
- [X] T059 [US5] Implement a guided configuration prompt when translation is enabled with no
      backend configured (FR-021) in `apps/frontend/src/components/TranslationSettings.tsx`
- [ ] T060 [US5] Run `quickstart.md` §5 and confirm SC-007

**Checkpoint**: Translation failures degrade gracefully on both the cloud and local-backend
paths; reading is never blocked.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [X] T061 [P] Update `docs/`/`README.md` to reflect the shipped Phase 2 feature set
- [X] T062 Code cleanup and refactoring pass across `crates/lanrurugi-ocr/`,
      `crates/lanrurugi-fontcache/`, `crates/lanrurugi-translate/`, `apps/frontend/src/translation/`
- [X] T063 [P] Add lightweight telemetry logging for the font-match rate (SC-003) and
      look-ahead-readiness rate (SC-004) over real usage, so these "at least X%" claims are
      actually measurable beyond a single qualitative quickstart run, in
      `crates/lanrurugi-translate/src/telemetry.rs`
- [X] T064 [P] Security hardening pass — credential handling audit
      (`crates/lanrurugi-translate/src/credentials.rs`), CORS/PNA review for the local-backend
      endpoints
- [X] T064a [P] Add Terminology Glossary, translated `Detected Text Region`, and Volume Font
      Pattern to `lanrurugi-backup`'s existing per-entity backup/restore enumeration (FR-022,
      research.md §17) in `crates/lanrurugi-backup/src/build.rs`
- [X] T064b [P] Wire glossary entry capture/edit/delete (FR-007a/d), Volume Font Pattern reset
      (FR-010), and backend/target-language selection changes (FR-002/003/004) into a new
      `translation.*` Activity `action_type` namespace at each write site (FR-023, research.md
      §17), following the existing per-namespace call pattern in
      `crates/lanrurugi-storage/src/activity.rs`
- [X] T064c [P] Add the new `translation.*` namespace's ordering/label/target-link entries to
      `apps/frontend/src/pages/Activity/activityTarget.ts` so T064b's events render correctly on
      the existing Activity page with no page-specific changes
- [ ] T065 Run the full `quickstart.md` end-to-end across all 5 user stories on a clean checkout

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately. Now includes real
  `manga-ocr`-recognition-model acquisition (T004), not just discovery code.
- **Foundational (Phase 2)**: Depends on Setup. **Blocks all user stories.**
- **User Stories (Phase 3–7)**: All depend on Foundational. Priority order (P1 → P2 → P3) already
  matches the real dependency order for this feature — no exception needed (contrast with
  `specs/001-lanrurugi-full-rewrite/tasks.md`'s US8, which did need one):
  - US2 (P2) wires into US1's compositing (T029) — naturally scheduled right after US1.
  - US3 (P2) prefetches US1's translation endpoint (T028) — naturally scheduled after US1.
  - US4 (P3) consumes US2's `text-regions` endpoint (T035) — naturally scheduled after US2.
  - US5 (P3) wraps failure handling for both the cloud (US1) and local (US4) paths — naturally
    scheduled last.
- **Polish (Phase 8)**: Depends on all Phase 2 user stories (US1–US5) being complete.

### User Story Dependencies

- **US1 (P1)**: Foundational only.
- **US2 (P2)**: Foundational + US1's T029 (server-side compositing) to wire into.
- **US3 (P2)**: Foundational + US1's T028 (translation endpoint) to prefetch/cache around.
- **US4 (P3)**: Foundational (T022, T023) + US2's T035 (`text-regions` endpoint).
- **US5 (P3)**: Foundational's adapters (T015–T017) + US1 (cloud path) + US4 (local path) to wrap
  failure handling around both.

### Parallel Opportunities

- Setup tasks marked `[P]` (T002, T003, T005) can run in parallel once T001 exists; T004 (model
  acquisition) can proceed in parallel with T005 but should land before Foundational's T006–T008
  are exercised against a real model.
- Within Foundational: the OCR group (T006–T010), font-cache group (T011–T014), and adapter group
  (T015–T019) touch entirely different crates and can be staffed in parallel; T020/T021 depend on
  T015–T019's adapter/credential work; T022/T023 (frontend) are independent of all the Rust work.
- Once Foundational is done, US1 must go first (everything else wires into it), but once US1's
  T028/T029 exist, US2 and US3 can proceed in parallel with each other.

---

## Parallel Example: Foundational Phase

```bash
# These three groups touch different crates and can be staffed in parallel:
Task: "Implement model-file discovery in crates/lanrurugi-ocr/src/model_discovery.rs"
Task: "Define the Volume Font Pattern entity in crates/lanrurugi-fontcache/src/entities.rs"
Task: "Define the normalized LLM provider adapter trait in crates/lanrurugi-translate/src/adapter.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1 (Setup, including real model acquisition) and Phase 2 (Foundational).
2. Complete Phase 3 (US1).
3. **STOP and VALIDATE**: run `quickstart.md` §1.
4. This alone is a demoable increment: translation works end-to-end for the cloud-backend path.

### Incremental Delivery

1. Setup + Foundational → foundation ready.
2. US1 → validate → demo (core translation, cloud path).
3. US2 → validate → demo (font fidelity).
4. US3 → validate → demo (prefetch/performance/budget visibility, real ready→swap transition).
5. US4 → validate → demo (local-backend path, zero-install).
6. US5 → validate → demo (resilience across both paths).
7. Polish (including basic SC-003/SC-004 telemetry).

### Parallel Team Strategy

After Foundational, US1 should land first since US2/US3/US4/US5 all wire into pieces of it. Once
US1's endpoint/compositing exist, US2 and US3 can proceed in parallel; US4 waits on US2's
`text-regions` endpoint; US5 waits on both US1 and US4 being available to wrap.

---

## Notes

- `[P]` tasks touch different files (or independent crates) and have no unfinished-task
  dependency.
- `[Story]` labels map every implementation task to spec.md's user stories for traceability.
- No dedicated TDD test-writing phase per story (not requested by spec.md); each story ends with
  its `quickstart.md` scenario as the acceptance checkpoint.
- This feature adds to, and never edits, the Phase 1 codebase/crates/tasks — see
  `specs/001-lanrurugi-full-rewrite/tasks.md` for that separate task list, per constitution
  Principle VI.
- The `ort` CPU-only-initial-scoping decision, the model-discovery pattern (T006), the
  `manga-ocr`-model-acquisition source to verify (T004), and the OpenVINO EP-hang lesson behind
  the initial CPU-only scoping are all recorded in `research.md` §1 and the `jellyfin-suite-
  tooling-reference` memory. CUDA was added later per this same pattern (issue #103, see
  research.md §1's 2026-09-12 update) — consult both before adding any *further* GPU execution
  provider (e.g. OpenVINO, DirectML).
- **On Koharu (mayocream/koharu)**: cloned locally for architecture/approach reference only
  (its detection+recognition+LLM pipeline structure, its choice of `manga-ocr` for recognition) —
  it is **not** a dependency and no code from it is used. Its current codebase is
  `GPL-3.0-only` with `publish = false` (verified directly against the live repository, not
  assumed from crates.io search results), so T007/T008 are independent implementations against
  Apache-2.0 model weights (`oar-ocr` for detection, kha-white's `manga-ocr` for recognition), not
  ports of Koharu's own GPL-licensed wrapper code. See `research.md` §1 for the full licensing
  finding.
- T012 and T014 (font classification during voting and meltdown) both run the full, heavy font
  classifier and both MUST use the same `spawn_blocking` bridge as T009 — this was inconsistently
  stated before the 2026-07-06 `/speckit-analyze` remediation and is now explicit on all three.

**Revision note (2026-09-06, `/speckit-analyze` re-run against constitution 1.8.0)**: The
constitution advanced from the version current at this document's 2026-07-06 authoring to 1.8.0
(ratified 2026-08-03) — adding Principle VII (frontend engineering discipline) in full, a new
Principle III anti-pattern bullet (a loop of single-item `spawn_blocking` calls is not real
parallelization), and two new Technology Stack Constraints bullets (shared-helper extraction,
domain-entity-ID newtypes) — none of which this document had been checked against until now.
Re-analysis found two real gaps, both now addressed: (1) T009's wording was ambiguous enough to be
implemented as the newly-forbidden loop-of-single-item-`spawn_blocking` anti-pattern — reworded to
require one batch-wide `rayon` dispatch, matching T012/T014's already-explicit wording for the same
concern. (2) `data-model.md`'s `archive_id`/`volume_id` fields were typed as raw `string`, conflicting
with the newtype-ID constraint — `data-model.md` now carries a note requiring the newtype at
implementation time. No task numbering changed. Principle VII's frontend-file-organization and
verification-discipline bullets were checked against this document's frontend tasks (T024–T027,
T032, T037, T041–T042, T045, T049–T054, T059) and found already compliant: no task here defines a
second component/hook inline in a page's own `index.tsx`, and T033/T038/T048/T055/T060/T065 already
require running real `quickstart.md` browser scenarios rather than accepting a pure-function unit
test as verification.
