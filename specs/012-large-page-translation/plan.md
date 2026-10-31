# Implementation Plan: Translation and Erase Support for Pages Beyond the Worker Payload Limit

**Branch**: `012-large-page-translation` | **Date**: 2026-10-13 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/012-large-page-translation/spec.md`

## Summary

Page translation silently fails for large pages because the whole-page erase request carries the
page at **full resolution** across the GPU-worker boundary, against a fixed message limit. The
reduction to the model's working resolution already exists, but it happens *inside the worker, after
the transfer*. For pages above ~13.4 megapixels the request is refused before it is sent; every call
site treats a refused erase as "leave this page untranslated", so the user sees nothing.

The fix moves that reduction to **before** the transfer, for exactly those pages: the client reduces
the page and its model mask to the same working resolution, sends that, receives the model's output
at working resolution, restores it to the page's original dimensions, and composites using the
original full-resolution paste mask — which never leaves the client.

Pages whose current full-resolution transfer already fits take the **existing code path unchanged**,
which is what makes the "no visible change" requirement hold by construction rather than by
argument. No worker-side, protocol, or model change is needed: `Inpainter::erase_page` already
short-circuits its internal reduction when its input is at or below the working resolution.

Measured effect for the worst page in the survey (6336x9504): the request drops from ~287 MiB to
under 8 MiB, and the main server process stops assembling a 287 MiB message it is about to have
refused.

## Technical Context

**Language/Version**: Rust (workspace edition, toolchain pinned by the workspace)

**Primary Dependencies**: `image` (resampling), `tarpc` + `tokio-util`'s length-delimited codec (the
worker boundary), `ort` (ONNX Runtime, worker-side only). No new runtime dependency is introduced.

**Storage**: None. No Redis, on-disk, or cache shape changes — this feature holds nothing across
restarts.

**Testing**: `cargo test` through the repository's `mise run` tasks (`mise run test`,
`mise run check-crate -- <crate>`). Unit tests cover the dimension arithmetic, the size predicate,
and resampling invariants; an integration test asserts the payload-size guarantee for the surveyed
worst-case dimensions.

**Target Platform**: Same as the existing server — Linux, containerised, with the GPU worker as a
sibling subprocess of `lanrurugi-server`.

**Project Type**: Workspace crate modifications within the existing server binary; no new crate.

**Performance Goals**: The data crossing the worker boundary per erase call must be bounded by a
constant (independent of source page dimensions) for pages above the working resolution. Peak
resident memory of the main server process while translating the largest surveyed page must drop by
at least an order of magnitude (SC-004, SC-005).

**Constraints**: Pixel-identical output for every page that translates today (FR-008, SC-003). No
increase in the number of boundary calls per page (FR-019). The model's working resolution values
must not change (FR-020). No external API or persisted-data change (FR-017, FR-018).

**Scale/Scope**: Affects ~8% of a real library's archives (17 of 209 surveyed) — the subset above
~13.4 megapixels. Roughly three files change, plus tests.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Assessment |
| --- | --- |
| **I. Legacy Data & User-Trust Compatibility** (NON-NEGOTIABLE) | **Pass.** Nothing persisted changes: no Redis schema, no keying, no on-disk artifact. The feature is confined to an in-flight computation. Verified by inspection: the only data the change touches is a decoded page image and two in-memory masks. |
| **II. API Contract Fidelity (Phase 1)** | **Pass.** No HTTP route, request, or response shape changes (FR-017). The worker boundary is a private contract between two binaries shipped in the same image, not a user-facing API. |
| **III. Resource-Conscious, Genuinely Concurrent Single-Process Architecture** | **Pass, and improved.** The change *reduces* peak memory in the long-lived server process rather than adding any. The new resampling is CPU-bound and must not run on the async reactor: it runs inside the existing `run_blocking` closure that already owns the erase call, on a `block_on` thread that is documented to be a non-reactor thread. **No new `rayon` usage is introduced**, so the project's parallel-budget rule is not engaged. |
| **IV. Sandboxed, Language-Agnostic Plugin Extensibility** | **Pass.** The plugin runtime, Deno permission model, and plugin protocol are untouched. |
| **V. Secrets & Network Trust Boundaries** | **Pass.** No new network egress, no credential handling, no change to which hosts the server will talk to. |
| **VI. Phased Scope Discipline** | **Pass.** This is Phase 2 work inside an already-planned Phase 2 capability (on-page translation). It introduces no new deliverable, no new service, and no Phase 1 surface. |
| **VII. Frontend Engineering Discipline & Legacy UI Fidelity** | **Pass.** No frontend file is touched. |
| **Technology Stack Constraints** | **Pass.** One new **dev-dependency only** (`bincode`, pinned to the version the transport already uses) so a test can pin the payload-size arithmetic against real serialization; justified under the "documented, specific compatibility reason" clause, since the entire point is to reproduce the transport's own framing rather than to pick a fresh library. No new runtime dependency, no new crate. |
| **Engineering Workflow & Quality Gates** | **Pass.** `cargo check`/`fmt --check`/`clippy` run through the repository's `mise run` tasks and its resource-capped container script. The feature adds automated tests; no gate is weakened. |

No violations. Complexity Tracking is therefore not applicable and is omitted.

## Project Structure

### Documentation (this feature)

```text
specs/012-large-page-translation/
├── plan.md              # This file
├── spec.md              # Feature specification
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── worker-boundary.md
│   └── inpaint-geometry-api.md
├── checklists/
│   └── requirements.md  # Specification quality checklist
└── tasks.md             # Phase 2 output (/speckit-tasks — NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-inpaint/src/lib.rs           # working-resolution constant + dimension/resampling
│                                         # helpers become public and shared; erase_page consumes
│                                         # them (behaviour unchanged)
├── lanrurugi-gpu-ipc/src/lib.rs           # payload-size arithmetic for the worker boundary,
│                                         # expressed against the existing MAX_FRAME_BYTES
├── lanrurugi-api/src/gpu_worker_client.rs # chooses the reduced path only when the
│                                         # full-resolution transfer cannot be proven to fit;
│                                         # performs reduce → RPC → restore → masked composite
├── lanrurugi-gpu-worker/src/main.rs       # UNCHANGED
└── lanrurugi-translate/src/composite.rs   # UNCHANGED
```

**Structure Decision**: Changes are confined to the three crates that already participate in this
boundary, with the shared geometry living in `lanrurugi-inpaint` — the crate that already owns the
working-resolution rule — and consumed by `lanrurugi-api`, which already depends on it. This keeps a
single implementation of the reduction rule on both sides of the boundary (FR-006) without a new
crate and without making `lanrurugi-inpaint` depend on the transport.

Two crates are deliberately **not** touched. `lanrurugi-gpu-worker` needs no change because
`Inpainter::erase_page` already skips its internal reduction when its input is at or below the
working resolution, and the client therefore asks for exactly the model output it wants by sending
an all-covering paste mask. `lanrurugi-translate`'s composite step needs no change because the client
still returns a full-resolution page that is byte-identical to its input outside the paste mask —
the invariant that step already relies on.

## Complexity Tracking

Not applicable: the Constitution Check records no violations.

## Post-Design Constitution Re-check

*Re-evaluated after Phase 1 design (research.md, data-model.md, contracts/, quickstart.md).*

**No new violations, and two points are strengthened rather than weakened.**

- **Principle III (Resource-Conscious Architecture)** — research.md §7 models both paths at the worst
  surveyed page. The design removes one full-resolution page copy relative to today (the reduced path
  composites into the page buffer it already owns rather than into a fresh clone), and bounds the
  boundary message from ~287 MiB to under 8 MiB. The specification's SC-005 was corrected during this
  re-check to claim exactly that, and not the larger total-RSS reduction it originally stated: a
  full-resolution page necessarily exists in the process either way, so an order-of-magnitude claim
  about *total* memory would have been false. Stating a criterion the design cannot meet would have
  been the violation.
- **Principle VII / Engineering Workflow** — the design adds automated tests to every crate it touches
  and pins the payload arithmetic against real serialization rather than against its own restatement
  (contracts/worker-boundary.md). No gate is relaxed.
- **Technology Stack Constraints** — the single new dev-dependency is `bincode`, deliberately pinned
  to the version the transport already uses. The "prefer the latest stable release" rule is satisfied
  by its documented compatibility exception: the entire purpose is to reproduce the transport's own
  framing, so a different major version would verify the wrong thing. It is a test-only dependency and
  does not enter any shipped binary.

The design also **narrows** scope relative to a naive reading of the feature: bubble detection is
explicitly excluded (research.md §6) precisely because including it would break the pixel-identity
guarantee, which this project's standing constraint forbids. A smaller, provable change was preferred
over a larger, plausible one.

