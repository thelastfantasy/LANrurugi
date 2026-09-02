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

- [ ] T001 Add `lanrurugi-ocr`, `lanrurugi-fontcache`, `lanrurugi-translate` members to the
      Cargo workspace in `Cargo.toml`
- [ ] T002 [P] Add `oar-ocr` (PP-OCR text detection, Apache-2.0) and `ort` + `ndarray` (for the
      custom `manga-ocr` recognition inference, CPU-only per research.md §1) dependencies to
      `crates/lanrurugi-ocr/Cargo.toml`
- [ ] T003 [P] Document the `manga-ocr` recognition model's file placement and the
      model-discovery env var convention (research.md §1, mirroring `~/jellyfin-suite`'s
      `find_model()` pattern) in `crates/lanrurugi-ocr/README.md` — detection models are fetched
      automatically by `oar-ocr` and don't need this convention
- [ ] T004 Add a model-acquisition step that places kha-white's **Apache-2.0** `manga-ocr`
      recognition model (an existing ONNX export of its weights) at the path T003 documents — a
      Dockerfile build stage for production images plus a `scripts/fetch-ocr-model.sh` for local
      dev — downloading a pinned release/version with checksum verification (exact ONNX artifact
      source to be verified against the live repository/hosting at implementation time, not
      assumed) in `Dockerfile` and `scripts/fetch-ocr-model.sh`
- [ ] T005 [P] Create the `apps/frontend/src/translation/` directory skeleton with barrel exports in
      `apps/frontend/src/translation/index.ts`

**Checkpoint**: Workspace builds with the new (empty) crates; a real `manga-ocr` recognition
model file is present at the documented path; frontend module skeleton in place.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core OCR, font-cache, and translation-adapter infrastructure every user story
depends on.

**⚠️ CRITICAL**: No user story task may begin until this phase is complete.

- [ ] T006 Implement model-file discovery for the `manga-ocr` recognition model (env var →
      binary-relative → config path, first match wins; research.md §1) in
      `crates/lanrurugi-ocr/src/model_discovery.rs`
- [ ] T007 Integrate `oar-ocr` for text-region detection (Apache-2.0, research.md §1) in
      `crates/lanrurugi-ocr/src/detect.rs`
- [ ] T008 Implement custom CPU-only `ort` Session initialization and encoder/decoder inference
      for kha-white's Apache-2.0 `manga-ocr` recognition model (research.md §1 — chosen for
      manga/vertical-Japanese accuracy over generic recognition; written independently, not
      derived from Koharu's GPL-licensed wrapper code — see Notes) in
      `crates/lanrurugi-ocr/src/recognize.rs`
- [ ] T009 [P] Implement rayon-batched OCR inference (detection + recognition, batch size 4–8,
      bridged via `tokio::task::spawn_blocking` reusing `crates/lanrurugi-core/src/concurrency.rs`
      from Phase 1) in `crates/lanrurugi-ocr/src/batch.rs`
- [ ] T010 Implement IoU/geometric line-paragraph merging producing `Detected Text Region`
      records in `crates/lanrurugi-ocr/src/merge.rs`
- [ ] T011 [P] Define the `Volume Font Pattern` entity and Redis schema (`vote_pool`,
      `golden_set`, `meltdown_tally`, `is_locked`) in `crates/lanrurugi-fontcache/src/entities.rs`
- [ ] T012 Implement cover-excluded `vote_pool` accumulation (voting stage, FR-008) — this runs
      the full font classifier per block and MUST be bridged via `tokio::task::spawn_blocking`
      (reusing `crates/lanrurugi-core/src/concurrency.rs`), the same rule T009 follows, per
      constitution Principle III and plan.md's Technical Context — in
      `crates/lanrurugi-fontcache/src/voting.rs`
- [ ] T013 Implement the two-sequential-checks routing — lock-state check, then a cheap
      per-block feature classifier among `golden_set` (research.md §4) — in
      `crates/lanrurugi-fontcache/src/routing.rs`
- [ ] T014 Implement meltdown re-classification into a separate `meltdown_tally` that never
      feeds `vote_pool` (research.md §4, the resolved review concern); like T012, this re-runs
      the full classifier and MUST use the same `spawn_blocking` bridge — in
      `crates/lanrurugi-fontcache/src/meltdown.rs`
- [ ] T015 [P] Define the normalized LLM provider adapter trait
      (`contracts/llm-provider-adapter.md`) in `crates/lanrurugi-translate/src/adapter.rs`
- [ ] T016 Implement the OpenAI-compatible adapter (covers OpenAI-compatible providers and the
      Ollama preset) in `crates/lanrurugi-translate/src/openai_compat.rs`
- [ ] T017 Implement the Anthropic adapter (`system` field, content-block array,
      `x-api-key`/`anthropic-version`, mandatory `max_tokens`) in
      `crates/lanrurugi-translate/src/anthropic.rs`
- [ ] T018 Implement server-side `credential_ref` resolution so a secret is never logged or
      returned in any response (constitution Principle V) in
      `crates/lanrurugi-translate/src/credentials.rs`
- [ ] T019 Implement server-side Translation Backend Selection + Target Language Preference
      Redis storage in `crates/lanrurugi-translate/src/settings.rs`
- [ ] T020 Implement the server-composited `Translation Cache Entry` variant (Redis metadata +
      image cache alongside Phase 1's existing thumbnail cache) in
      `crates/lanrurugi-translate/src/cache.rs`
- [ ] T021 Implement Usage Budget tracking at page/archive/day/week granularity (FR-014) in
      `crates/lanrurugi-translate/src/budget.rs`
- [ ] T022 [P] Implement the `localStorage` wrapper and device-local precedence resolution
      (FR-003, research.md §8) in `apps/frontend/src/translation/settings.ts`
- [ ] T023 [P] Implement the IndexedDB/Cache API wrapper for the client-composited cache
      (research.md §7) in `apps/frontend/src/translation/cache.ts`

**Checkpoint**: OCR detection+recognition, font-pattern voting/routing/meltdown, both LLM
adapters, and the backend-selection/cache storage split are all available to every subsequent
story.

---

## Phase 3: User Story 1 - Read a page with on-page translation (Priority: P1) 🎯 MVP

**Goal**: Core enable/select/translate/render flow, cloud-backend path end-to-end.

**Independent Test**: `quickstart.md` §1 — enable translation, select a cloud backend, open a
page, confirm translated text renders and no credential reaches the browser.

- [ ] T024 [US1] Implement the enable/disable toggle for on-page translation (FR-001) in
      `apps/frontend/src/components/TranslationSettings.tsx`
- [ ] T025 [P] [US1] Implement `GET`/`PUT /translation/settings` endpoints (FR-002, FR-003) in
      `crates/lanrurugi-api/src/translation_settings.rs`
- [ ] T026 [US1] Implement the backend-category selection UI (cloud vs. locally-hosted) in
      `apps/frontend/src/components/TranslationSettings.tsx`
- [ ] T027 [US1] Implement the target-language selection UI with browser-language fallback
      (FR-004) in `apps/frontend/src/components/TranslationSettings.tsx`
- [ ] T028 [US1] Implement `GET /archives/{id}/page/{page}/translation`, orchestrating OCR +
      translate + composite + cache for the cloud-backend path (`contracts/translation-api.md`)
      in `crates/lanrurugi-api/src/translation.rs`
- [ ] T029 [US1] Implement server-side compositing (draw translated text over the original page
      image) in `crates/lanrurugi-translate/src/composite.rs`
- [ ] T030 [US1] Audit that no code path logs, returns, or otherwise exposes a cloud credential
      to the browser (FR-006) across `crates/lanrurugi-translate/`
- [ ] T031 [US1] Ensure translation-disabled reading takes zero extra code paths or latency
      (FR-007) in `crates/lanrurugi-api/src/translation.rs`
- [ ] T032 [US1] Wire the reader to request and display the translated overlay when enabled in
      `apps/frontend/src/pages/Reader.tsx`
- [ ] T033 [US1] Run `quickstart.md` §1 and confirm SC-001/SC-002

**Checkpoint**: A user can enable translation, pick a cloud backend, and read a translated page —
demoable MVP for this feature.

---

## Phase 4: User Story 2 - Translated text looks like it belongs on the page (Priority: P2)

**Goal**: Translated text renders in a font matched to the volume's established style.

**Independent Test**: `quickstart.md` §2 — read several pages of a volume, confirm a small,
consistent set of matched fonts is used once the pattern locks.

- [ ] T034 [US2] Wire `lanrurugi-fontcache`'s routing (T013/T014) into server-side compositing
      (T029) so translated text draws in the matched golden-set font in
      `crates/lanrurugi-translate/src/composite.rs`
- [ ] T035 [P] [US2] Implement `GET /archives/{id}/page/{page}/text-regions`, exposing detected
      regions and the volume's current golden font set (`contracts/translation-api.md`) in
      `crates/lanrurugi-api/src/text_regions.rs`
- [ ] T036 [US2] Implement `POST /volumes/{id}/font-pattern/reset` (FR-010) in
      `crates/lanrurugi-api/src/font_pattern.rs`
- [ ] T037 [P] [US2] Implement a "reset font pattern" UI control in
      `apps/frontend/src/components/VolumeFontPatternControl.tsx`
- [ ] T038 [US2] Run `quickstart.md` §2 and confirm SC-003

**Checkpoint**: Font matching is visibly consistent within a volume and resettable if wrong.

---

## Phase 5: User Story 3 - Reading stays fast while translation is on (Priority: P2)

**Goal**: Sliding-window prefetch, non-blocking not-yet-ready state with a real client-side
ready/swap mechanism, budget enforcement and visibility.

**Independent Test**: `quickstart.md` §3 — read forward with look-ahead enabled, confirm no
visible delay within the window and a working usage breakdown.

- [ ] T039 [US3] Implement the sliding-window look-ahead scheduler (reusing Phase 1's rayon/
      tokio concurrency bridge) in `crates/lanrurugi-translate/src/prefetch.rs`
- [ ] T040 [US3] Implement the not-yet-ready response for the translation endpoint (T028) when a
      page is still processing (FR-012) in `crates/lanrurugi-api/src/translation.rs`
- [ ] T041 [P] [US3] Implement the non-obscuring loading indicator component in
      `apps/frontend/src/translation/components/LoadingIndicator.tsx`
- [ ] T042 [US3] Implement client-side ready-detection polling that, upon seeing T040's
      not-yet-ready response, retries until the translated page is available and then swaps it
      in seamlessly in place of T041's indicator (FR-012's "seamless replace" behavior,
      previously unassigned) in `apps/frontend/src/translation/readyPoller.ts`
- [ ] T043 [US3] Implement usage-budget enforcement capping look-ahead activity for metered
      backends (FR-013) in `crates/lanrurugi-translate/src/prefetch.rs`
- [ ] T044 [P] [US3] Implement `GET /translation/usage` (FR-014) in
      `crates/lanrurugi-api/src/translation_usage.rs`
- [ ] T045 [P] [US3] Implement the usage panel (page/archive/day/week breakdown, optional chart)
      in `apps/frontend/src/translation/components/UsagePanel.tsx`
- [ ] T046 [US3] Implement abandonment of in-flight look-ahead requests on navigate-away
      (FR-015) in `apps/frontend/src/translation/prefetchController.ts`
- [ ] T047 [US3] Wire `Translation Cache Entry` reuse keyed by (page, target language, backend)
      into the prefetch and serve paths (FR-016) in `crates/lanrurugi-translate/src/cache.rs`
- [ ] T048 [US3] Run `quickstart.md` §3 and confirm SC-004/SC-005

**Checkpoint**: Reading with translation enabled feels as fast as without it, within budget, with
a real (not just designed) not-ready-to-ready transition.

---

## Phase 6: User Story 4 - Using a locally-hosted model works without installing extra software (Priority: P3)

**Goal**: Browser-direct local-backend calls, client-side compositing/caching, PNA guidance.

**Independent Test**: `quickstart.md` §4 — configure a locally-hosted backend per the documented
path, confirm pages translate, then confirm blocked connections show guided fallback.

- [ ] T049 [US4] Implement the browser-side direct call to the locally-hosted backend in
      `apps/frontend/src/translation/localBackend.ts`
- [ ] T050 [US4] Implement client-side Canvas/OffscreenCanvas compositing (no WASM, research.md
      §7), consuming `text-regions` (T035) in `apps/frontend/src/translation/composite.ts`
- [ ] T051 [P] [US4] Wire the client-composited result into the IndexedDB/Cache API wrapper
      (T023) in `apps/frontend/src/translation/localCache.ts`
- [ ] T052 [US4] Implement Private-Network-Access failure detection and guided fallback UI
      (FR-018, research.md §9) in
      `apps/frontend/src/translation/components/LocalBackendGuidance.tsx`
- [ ] T053 [P] [US4] Write the zero-extra-install configuration documentation (e.g.
      `OLLAMA_ORIGINS`/PNA settings) in `docs/translation-local-backend.md`
- [ ] T054 [US4] Confirm the device-local backend selection (T022) takes precedence over the
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

- [ ] T056 [US5] Implement normalized error kinds (`unreachable`, `auth_failed`, `rate_limited`,
      `malformed_response`) across both adapters in `crates/lanrurugi-translate/src/adapter.rs`
- [ ] T057 [US5] Implement fallback-to-original-page display on any translation failure (FR-019)
      in `crates/lanrurugi-api/src/translation.rs` and `apps/frontend/src/pages/Reader.tsx`
- [ ] T058 [US5] Ensure per-page/per-archive failure isolation — no shared mutable state that
      could cascade a single page's failure (FR-020) in
      `crates/lanrurugi-translate/src/prefetch.rs`
- [ ] T059 [US5] Implement a guided configuration prompt when translation is enabled with no
      backend configured (FR-021) in `apps/frontend/src/components/TranslationSettings.tsx`
- [ ] T060 [US5] Run `quickstart.md` §5 and confirm SC-007

**Checkpoint**: Translation failures degrade gracefully on both the cloud and local-backend
paths; reading is never blocked.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [ ] T061 [P] Update `docs/`/`README.md` to reflect the shipped Phase 2 feature set
- [ ] T062 Code cleanup and refactoring pass across `crates/lanrurugi-ocr/`,
      `crates/lanrurugi-fontcache/`, `crates/lanrurugi-translate/`, `apps/frontend/src/translation/`
- [ ] T063 [P] Add lightweight telemetry logging for the font-match rate (SC-003) and
      look-ahead-readiness rate (SC-004) over real usage, so these "at least X%" claims are
      actually measurable beyond a single qualitative quickstart run, in
      `crates/lanrurugi-translate/src/telemetry.rs`
- [ ] T064 [P] Security hardening pass — credential handling audit
      (`crates/lanrurugi-translate/src/credentials.rs`), CORS/PNA review for the local-backend
      endpoints
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
- The `ort` CPU-only scoping decision, the model-discovery pattern (T006), the
  `manga-ocr`-model-acquisition source to verify (T004), and the OpenVINO EP-hang lesson behind
  the CPU-only scoping are all recorded in `research.md` §1 and the `jellyfin-suite-tooling-
  reference` memory — consult before adding any GPU execution provider later.
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
