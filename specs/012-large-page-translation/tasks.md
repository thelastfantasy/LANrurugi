---

description: "Task list for feature 012-large-page-translation"
---

# Tasks: Translation and Erase Support for Pages Beyond the Worker Payload Limit

**Input**: Design documents from `/specs/012-large-page-translation/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included. The feature specification requires them rather than merely permitting them:
SC-003 requires pixel-identical output verified "by automated comparison over the same sample, not by
inspection", SC-007 requires the existing suite to pass with no expected-output changes, and the
contracts pin the payload arithmetic with a test against real serialization. Tasks below therefore
pair each behavioural change with the test that makes it checkable.

**Organization**: Grouped by user story. US1 and US2 are both P1 and US2 is the gate on US1, so they
are sequenced rather than parallelised.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to
- Exact file paths are given in every task

## Path Conventions

Existing Cargo workspace under `crates/`. No new crate. Paths below are repository-relative.

---

## Phase 1: Setup

**Purpose**: The one dependency change the feature needs, before any code.

- [x] T001 Add `bincode` as a **dev-dependency** of `lanrurugi-gpu-ipc` in `crates/lanrurugi-gpu-ipc/Cargo.toml`, pinned to the exact version `tarpc`'s `serde-transport-bincode` feature already resolves to (read `Cargo.lock`, do not guess). Add a comment recording that the pin is deliberate: the test exists to reproduce the transport's own framing, so a different major version would verify the wrong thing (plan.md, Constitution Check → Technology Stack Constraints).

**Checkpoint**: `mise run check-crate -- lanrurugi-gpu-ipc` still passes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The two shared pieces every user story builds on — a single implementation of the
reduction rule, and knowledge of what a request will weigh.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [x] T002 [P] In `crates/lanrurugi-inpaint/src/lib.rs`: make the working-resolution constant public and add `pub fn work_dims(width: u32, height: u32) -> (u32, u32)` holding the existing reduction rule verbatim. Refactor `Inpainter::erase_page` to call it instead of keeping its own copy (contracts/inpaint-geometry-api.md — a second copy inside the same crate defeats the extraction). Behaviour at every input size must be unchanged.

- [x] T003 In `crates/lanrurugi-inpaint/src/lib.rs`: add `pub fn downscale_to_work(page: &RgbImage, model_mask: &[bool]) -> Result<(RgbImage, GrayImage), InpaintError>`, extracted verbatim from `erase_page`'s current mask-materialisation and resize steps (`CatmullRom` for the page, `Nearest` for the two-level mask), and refactor `erase_page` to call it. Depends on T002 (same file, same function).

- [x] T004 [P] Add unit tests for the geometry API in `crates/lanrurugi-inpaint/src/lib.rs`: `work_dims` is identity at or below the working resolution; the longest edge reduces to exactly the working resolution when above it; neither component exceeds its input nor reaches 0; `work_dims` is idempotent; `downscale_to_work` returns data identical to its input when no reduction is needed. Depends on T002, T003.

- [x] T005 [P] In `crates/lanrurugi-gpu-ipc/src/lib.rs`: add `ERASE_REQUEST_FRAMING_SLACK`, `pub fn erase_request_bytes(width: u32, height: u32) -> u64`, and `pub fn erase_request_provably_fits(width: u32, height: u32) -> bool` per contracts/worker-boundary.md. Also correct the stale comment that describes the working resolution as 2048px — it is 1024, and that wrong figure is the assumption this defect survived on.

- [x] T006 [P] Add a test module in `crates/lanrurugi-gpu-ipc/src/lib.rs` pinning `erase_request_bytes`' `48 + 5wh` term against real `bincode` serialization of a page plus two masks at several `(w, h)` pairs, so the arithmetic cannot drift from the wire format unnoticed. Depends on T001, T005.

**Checkpoint**: `mise run check-crate -- lanrurugi-inpaint lanrurugi-gpu-ipc` passes, and `work_dims` has exactly one implementation in the workspace.

---

## Phase 3: User Story 1 - Any page translates, regardless of its pixel dimensions (Priority: P1) 🎯 MVP

**Goal**: A page above the boundary's payload limit is translated instead of silently skipped.

**Independent Test**: Translate a page whose `width * height` exceeds 13,421,772 and confirm a
translated page is produced (quickstart.md scenario 4).

### Tests for User Story 1

- [x] T007 [US1] Add a pure, worker-free decision helper (`erase_plan`) and its unit tests in `crates/lanrurugi-api/src/gpu_worker_client.rs`: for dimensions surveyed in research.md (6336x9504, 6084x3943, 4535x6307) it selects the reduced path; for small dimensions it selects the existing path; a page at or below the working resolution is never resampled. Keeping the decision pure is what makes US1 testable without a GPU, a model, or a live worker.

### Implementation for User Story 1

- [x] T008 [US1] Implement the reduced path in `GpuWorkerClient::erase_page` in `crates/lanrurugi-api/src/gpu_worker_client.rs`, driven by T007's decision: reduce the page and model mask with `lanrurugi_inpaint::downscale_to_work`, send an all-covering paste mask (research.md §4), receive the working-resolution model output, restore it to the page's original dimensions with the same filter, and composite using the **original full-resolution** paste mask into a buffer initialised from the original page. The existing full-resolution path must remain byte-for-byte the same code. Depends on T002, T003, T005, T007.

- [x] T009 [US1] Add a test in `crates/lanrurugi-api/src/gpu_worker_client.rs` asserting the reduced request's size is bounded by a constant for every surveyed over-limit dimension pair, and that it is at least 10x smaller than the full-resolution request it replaces (SC-004). Depends on T008.

**Checkpoint**: US1 is complete and its decision logic is independently tested. End-to-end confirmation is quickstart scenario 4.

---

## Phase 4: User Story 2 - Translations that already work do not change at all (Priority: P1)

**Goal**: Every page that translates today produces exactly the same pixels.

**Independent Test**: `erase_plan` selects the existing path for every dimension that fits, and
`Inpainter::erase_page` still satisfies the invariant that `lanrurugi-translate`'s composite step
depends on (quickstart.md scenario 6 confirms end-to-end).

### Tests for User Story 2

- [x] T010 [US2] Extend T007's tests in `crates/lanrurugi-api/src/gpu_worker_client.rs` to assert the existing path is selected for a dense sweep of dimensions across the whole fitting range, **including the band immediately below the limit** where the framing slack makes the decision conservative. A wrong predicate would silently move a working page onto the new path, and this is the test that catches it. Depends on T007.

- [x] T011 [US2] Assert, without a model, that restoring the reduced model output writes **only** the pixels the original full-resolution paste mask covers — every other pixel keeps the original page's value. Implemented by extracting the restore-and-composite step into `composite_restored_output` in `crates/lanrurugi-api/src/gpu_worker_client.rs` and testing it directly with a synthetic model output. **Revised during implementation**: this task originally called for a model-gated test against the real `Inpainter`. That was rejected on a concrete ground rather than for convenience — building an `Inpainter` inside `cargo test` creates a CUDA session, and *dropping* one is a documented ~35-60% process crash on this project's hardware (see `ManagedSession`'s own doc comment). A test that can abort the whole suite is worse than no test, which is why real-model checks in this repository live in `examples/`. The half of the invariant that a model would exercise is pre-existing behaviour in `Inpainter::erase_page` (it starts from `page.clone()` and writes only where the paste mask is set); the half this feature authored is the client-side restore, and that is what is tested here. Depends on T003.

**Checkpoint**: US2's guarantee holds by construction and is guarded by tests at both ends.

---

## Phase 5: User Story 3 - An oversized transfer is reported as a size problem (Priority: P2)

**Goal**: A size-limit failure is identifiable from its own output.

**Independent Test**: Force the too-large branch and read the error; it names the attempted size and
the limit, and is distinguishable from a model failure (quickstart.md scenario 1's sibling assertion).

### Implementation for User Story 3

- [x] T012 [US3] Add a dedicated error variant carrying the attempted size and the limit to `GpuWorkerError` in `crates/lanrurugi-api/src/gpu_worker_client.rs`, return it from the reduced path's defensive branch (unreachable in practice, since reduction bounds the message — required so an unforeseen case fails loudly rather than silently), and log the chosen path at a level that makes a real size failure distinguishable from a model-side failure. Depends on T008.

### Tests for User Story 3

- [x] T013 [US3] Add a test in `crates/lanrurugi-api/src/gpu_worker_client.rs` asserting the size-limit error's rendered message contains both the attempted size and the limit, and that its variant is not one of the model-side failure variants (FR-013, FR-014). Depends on T012.

**Checkpoint**: The failure mode this feature exists to remove can no longer be misread as a model problem.

---

## Phase 6: User Story 4 - Serving a translation does not balloon the main process's memory (Priority: P3)

**Goal**: The boundary message, and the number of full-page copies, are bounded.

**Independent Test**: The assembled request for the largest surveyed page is a constant, and the
full-resolution result is produced without an extra page copy.

### Implementation for User Story 4

- [x] T014 [US4] In `crates/lanrurugi-api/src/gpu_worker_client.rs`, ensure the composite reuses the page buffer the call already owns — consume the owned image (`into_raw`) rather than cloning it for the return value — so the feature removes a full-page copy rather than adding one (FR-011a, research.md §7). Depends on T008 (same function).

- [x] T015 [US4] Record the measured memory model from research.md §7 as a short doc comment on the reduced path's entry point, stating plainly that total process memory does not fall by an order of magnitude because a full-resolution page necessarily exists — so a future reader does not mistake SC-005 for a claim about total RSS. Depends on T008.

**Checkpoint**: The memory claim is bounded by what the design actually achieves.

---

## Phase 7: Polish & Cross-Cutting Concerns

- [x] T016 [P] Add `LANRURUGI_TEST_LARGE_PAGE_ARCHIVE` to `.env.example` with a blank value and a comment stating its purpose and that tests using it skip when it is unset. No real archive name may be committed (CLAUDE.md).

- [x] T017 Run the model-free quickstart scenarios (1, 2, 3, 7) via `mise run check-crate -- lanrurugi-inpaint lanrurugi-gpu-ipc lanrurugi-api` and record which assertions passed.

- [ ] T018 Run `mise run check` (rust-check + frontend-lint) and `mise run fmt-check`, and fix any failure. Never bypass a hook. **Satisfied as far as this tree allows — see the Implementation Record**: `mise run check` delegates to `lefthook run pre-push`, which skipped both hooks ("no matching push files") and verified nothing; the underlying `mise run clippy` cannot pass here for reasons outside this feature (a pre-existing `result_large_err` in `crates/lanrurugi-backup`, plus six more in untouched `lanrurugi-api` files). `mise run fmt` ran clean, `lanrurugi-inpaint`/`lanrurugi-gpu-ipc` are clippy-clean, and `gpu_worker_client.rs` produced zero diagnostics across lib and test targets.

- [x] T019 Update the SPECKIT status block in `CLAUDE.md`: move `012-large-page-translation` out of "Plan only" and record its actual checkbox completion counted from this file.

- [x] T020 Review `README.md`'s "Improvements over LANraragi" and Status sections against what this feature actually changed, and update if it makes a claim this fixes or should now make (a bug fix with user-facing impact is a candidate; routine internal refactor is not).

- [x] T021 Run the deployment-gated quickstart scenarios (4, 5, 6) against the dev container, or record explicitly that they remain unrun and why. Scenario 6 requires a reference capture from the pre-change revision and must not be reported as verified without it. **Done for scenario 6 (byte-for-byte, against a stashed pre-change build) and for scenario 4's substance (a real over-limit page through a real worker and real weights); scenario 5 is not runnable here and scenario 4's HTTP end was not exercised — see the Implementation Record.**

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies.
- **Foundational (Phase 2)**: depends on Setup (T006 needs T001). **Blocks every user story** — US1
  needs both the shared geometry and the size arithmetic, and neither can be worked around.
- **US1 (Phase 3)**: depends on Phase 2.
- **US2 (Phase 4)**: depends on Phase 2; T010 depends on T007 (US1), which is the only cross-story
  dependency. This is deliberate: US2's guarantee is a property *of* US1's decision, so the two cannot
  be fully independent here, and the spec sequences them accordingly.
- **US3 (Phase 5)**, **US4 (Phase 6)**: depend on T008 (US1).
- **Polish (Phase 7)**: depends on all desired stories.

### Within Each Phase

- T002 before T003 before T004 (one file, one function).
- T005 before T006.
- T007 before T008 before T009, T012, T014, T015.
- T012 before T013.

### Parallel Opportunities

- **T002/T004** (inpaint) and **T005/T006** (gpu-ipc) touch different crates and can proceed together.
- **T005** is independent of the whole inpaint chain.
- **T016** (`.env.example`) and **T020** (README review) touch files nothing else touches.
- US3 and US4 both depend only on T008 and touch different concerns; they can be done in either order.

```bash
# After Phase 1, these two chains are independent:
Task: "T002/T003/T004 — working-resolution API in crates/lanrurugi-inpaint/src/lib.rs"
Task: "T005/T006 — payload arithmetic in crates/lanrurugi-gpu-ipc/src/lib.rs"
```

---

## Implementation Strategy

### MVP

US1 alone (T001–T009) restores translation for the ~8% of archives that cannot be translated today.
That is the entire user-visible value of the feature.

### Incremental Delivery

1. Phases 1–2 → shared geometry and size arithmetic, no behaviour change yet.
2. US1 → over-limit pages translate (MVP). Validate with quickstart scenarios 2 and 4.
3. US2 → the safety gate: prove nothing that worked before moved onto the new path.
4. US3 → the failure is diagnosable if it ever recurs.
5. US4 → the memory claim is bounded and honest.
6. Polish → full checks, status bookkeeping, README review.

### Notes

- Every Rust build and test goes through `mise run`; heavier runs are launched detached from the
  agent's process tree, per CLAUDE.md.
- Tests are written before their implementation where the task orders them that way (T007 before T008,
  T012 before T013); the T011 test is the exception, since it guards an invariant of code written in
  T008 rather than new behaviour of its own.
- Scenario 6 is the one gate that cannot be satisfied by reasoning about the code, and must not be
  reported as verified unless a real pre-change capture was compared.

---

## Implementation Record (2026-10-13)

Filled in after implementation. Checkbox counts above are the authority for this feature's status.

### What was verified, and how

| Check | Result |
| --- | --- |
| `mise run check-crate -- lanrurugi-inpaint lanrurugi-gpu-ipc` | pass |
| `mise run check-crate -- lanrurugi-api` | pass |
| `mise run test-crate -- lanrurugi-inpaint lanrurugi-gpu-ipc lanrurugi-api` | 46 + 5 + 408 passed, 0 failed |
| `mise run build` | pass (produces `target/debug/lanrurugi-gpu-worker`) |
| `mise run fmt` / `mise run fmt-check` | both pass |
| `mise run clippy` (workspace) | **blocked by a pre-existing failure outside this feature — see below** |
| `mise run clippy-crate -- lanrurugi-inpaint lanrurugi-gpu-ipc lanrurugi-api` | 0 warnings in any file this feature touched; 6 pre-existing warnings remain in untouched `lanrurugi-api` files |
| quickstart scenarios 1–3, 7 (model-free) | covered by the unit tests |
| **quickstart scenario 4, at the boundary** (real worker + real model) | **verified — see §End-to-end below** |
| **quickstart scenario 6** (pages that already worked are unchanged) | **verified, byte for byte — see §End-to-end below** |
| quickstart scenario 5 (bubble detection above its own limit) | **not verifiable here**: this environment has no bubble-segmentation model installed at all (the worker logs "no bubble segmentation model installed"), so that call cannot be exercised |

### End-to-end verification against a real worker and a real model

Two opt-in integration tests were added, both driving an actual `lanrurugi-gpu-worker` subprocess over
the real Unix socket with the real LaMa weights:

- `crates/lanrurugi-api/tests/gpu_worker_boundary.rs` — erases a **synthetic 4000x3600 page**
  (14.4 MP), which is past the boundary's limit at full resolution, and then a fitting 2400x1500 one.
- `crates/lanrurugi-api/tests/gpu_worker_erase_capture.rs` — erases the fitting page and writes the
  returned raw bytes to a path from `LANRURUGI_GPU_WORKER_ERASE_CAPTURE`, using **only** API that
  exists on both sides of this feature's change, so the same file compiles and runs against a
  pre-change build.

Results from a run with `LANRURUGI_GPU_WORKER_BOUNDARY_E2E=1` (CPU execution provider; the
`Dockerfile.build` container this repo runs tests in has no GPU):

```text
over-limit 4000x3600: full-resolution request 72001072 bytes (refused) -> reduced to 1024x922, 4721712 bytes
  erase_page(4000x3600) succeeded in 34.68s
  1374965 pixels under the paste mask were rewritten by the model
  erase_page(2400x1500) succeeded in 17.38s
```

That is 68.7 MiB refused → 4.5 MiB accepted, for a page that previously could not be translated at
all, with the model genuinely running (1.37 M pixels rewritten) and the returned page asserted to be
identical to its input everywhere outside the paste mask.

**Scenario 6, measured rather than argued.** The capture test was run against the pre-change build
(the feature's four files stashed, then restored) and against the post-change build, on an identical
synthetic input (`input_fnv1a=0xf87b2b5dd755f8f9` in both runs). All three captures hash identically:

```text
32ecd7aff3acce1456b6ea07a26ce63829ba98cec9be29bff3d48152a29db08c  before.raw
32ecd7aff3acce1456b6ea07a26ce63829ba98cec9be29bff3d48152a29db08c  after.raw
32ecd7aff3acce1456b6ea07a26ce63829ba98cec9be29bff3d48152a29db08c  erase.raw  (final run)
```

scenario 6 as written asks for a real archive and a pre-change capture of a served image; what was
actually compared is the erase boundary's own output rather than an HTTP-served PNG. That is the
layer this feature changes, and it is a stronger comparison than a served image (no re-encoding in
the middle), but it is not literally the scenario's stated procedure, and the difference is recorded
here rather than glossed over.

### Three findings worth keeping

**1. The payload arithmetic in this feature's own artifacts was wrong, and the test caught it.** The
contract and research notes originally said `40 + 5wh`. Real `bincode` serialization says the
per-struct framing is 16 bytes each (`4 + 4` dimensions + an 8-byte `Vec` length prefix), so three
structs make it **`48 + 5wh`** — and T006 failed on its first run for exactly that reason. This is the
whole point of pinning the formula against the serializer instead of restating it, and it is also a
small illustration of the defect this feature fixes: an unchecked assumption about what a message
weighs, sitting next to the code that has to live with it.

**2. `mise run check` does not verify anything unless a push is in progress.** The repo's own
`mise run check` delegates to `lefthook run pre-push`, which skips both hooks with
`no matching push files` when nothing is staged for a push — it reported success in 0.00 seconds. The
underlying tasks (`mise run clippy`, `mise run fmt-check`) have to be run directly to get real
coverage. Worth knowing before trusting that gate again.

**3. `Dockerfile.build`'s image ships no ONNX Runtime, so no model-dependent test can run in it.**
`cargo-container-run.sh` runs `Dockerfile.build`'s `lanrurugi-dev`, which has *no* ONNX Runtime
anywhere on its filesystem, while `Dockerfile.dev`'s runtime image has both `/usr/local/lib/ort-cpu`
and the CUDA build. The worker's ONNX Runtime path is chosen by the *client* — vendor detection, or
`LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH` — and never falls back to an inherited `ORT_DYLIB_PATH`, so in
that container the worker was handed `/usr/local/lib/ort-cpu/libonnxruntime.so`, which does not exist,
and reported it as "no inpainting model loaded in this worker". Pointing
`LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH` at the copy mounted from the repository fixed it. Two things
follow: the misleading error (a *missing dylib* presented as a *missing model*) is worth a diagnostic
improvement of its own, and `mise run debug-erase-page` — whose own description requires
`LANRURUGI_INPAINT_MODEL_DIR` — cannot currently work in the container it runs in for the same reason.
Both are outside this feature's scope; neither was changed.

### Pre-existing workspace clippy failure (not caused by this feature)

`mise run clippy` fails on:

```text
error: the `Err`-variant returned from this function is very large
  --> crates/lanrurugi-backup/src/import_legacy.rs:361:6
```

`crates/lanrurugi-backup/` is untouched by this feature — `git status` shows nothing under it — so the
failure is inherent to the tree as it stands, not introduced here. It also means the workspace-wide
clippy run never reaches `lanrurugi-api`, which is why the scoped run above was needed. It was
deliberately **not** silenced with an `#[allow]` or a signature change: that is a decision about
another feature's in-flight work, and silently editing it would be exactly the kind of unrelated
change this project's commit discipline argues against.

### Not done, and stated plainly

- **The HTTP end of quickstart scenario 4 was not exercised.** The boundary call itself was verified
  end to end against a real worker and real weights (above), and the boundary call was the only thing
  blocking translation — but nothing in this feature was run through the reader's translate action,
  a real archive, the translation backend, or the served-image path. Doing that needs the dev
  container rebuilt with this change (`mise run dev-rebuild && dev-down && dev-up`), the dev
  container is built from a snapshot of the source rather than a bind mount, and for a model-dependent
  call it is the only image in this repository with an ONNX Runtime at all (see finding 3).
- **Scenario 5 could not be run here**: no bubble-segmentation model is installed in this environment,
  so that call cannot be exercised at all.
- **`mise run clippy`'s workspace gate remains red** for the pre-existing reasons above, and was left
  that way deliberately rather than silenced.
- **Bubble detection above ~22.4 megapixels** still degrades to rectangular erasure boxes. That is
  research.md §6's deliberate scope boundary, not a regression: those pages were previously not
  translated at all.
