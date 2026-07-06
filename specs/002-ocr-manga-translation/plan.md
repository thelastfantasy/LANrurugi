# Implementation Plan: On-Page Manga Translation (Phase 2)

**Branch**: `002-ocr-manga-translation` | **Date**: 2026-07-06 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/002-ocr-manga-translation/spec.md`

## Summary

Add optional on-page manga translation to LANrurugi: batched OCR detects and merges text regions
on manga pages; a user-selected LLM backend (an OpenAI-compatible provider, Anthropic, or a
locally-hosted model) translates the detected text; the result is rendered back onto the page in
a font matched to the volume's established body-text style (a three-stage vote/lock/meltdown
cache avoids re-running font classification on every block); a sliding-window prefetch keeps
translated pages ready ahead of the reader, capped by a user-visible, user-configurable budget for
metered backends. This plan extends the same Rust/React codebase delivered by
`specs/001-lanrurugi-full-rewrite` — it is additive to that workspace, not a separate deployable —
while remaining independent of and non-blocking to Phase 1's own plan/tasks, per constitution
Principle VI.

## Technical Context

**Language/Version**: Rust (stable, same `mise`-pinned toolchain as Phase 1) for new backend
crates; TypeScript (strict) for new frontend modules, within the existing Phase 1 workspace and
frontend app — this feature adds to that codebase rather than starting a new one.

**Primary Dependencies**:
- OCR text **detection**: `oar-ocr` (Apache-2.0, PP-OCR-based), which fetches and manages its own
  detection model — no custom acquisition/inference code needed for this half of OCR.
- OCR text **recognition**: a custom `ort` (ONNX Runtime's Rust bindings, CPU execution provider
  only — see research.md §1 for why GPU is out of scope this phase) integration against
  kha-white's Apache-2.0 `manga-ocr` model, chosen over generic recognition specifically for its
  accuracy on manga/vertical-Japanese text. Written independently — not a dependency on, or code
  derived from, Koharu (mayocream/koharu), a similar Rust manga-translation project that was
  cloned locally for architecture reference only and rejected as a dependency because its current
  code is GPL-3.0-only with `publish = false` (verified directly against the live repository; see
  research.md §1 and the `koharu-ocr-reference` memory).
- `image` crate (already a Phase 1 dependency) for page crop/decode operations feeding the font
  classifier and OCR pipeline.
- `rayon` + `tokio::task::spawn_blocking` (already established in Phase 1's constitution-mandated
  concurrency model) for OCR batching and font-classification work — this is CPU/GPU-bound work
  and MUST follow the same bridging rule Phase 1 already established, not a new pattern.
- LLM provider adapters: an OpenAI-Chat-Completions-shaped internal type (covers OpenAI-compatible
  providers and Ollama via its own OpenAI-compatible endpoint) plus a distinct Anthropic Messages
  API adapter — both already specified in the project constitution's Technology Stack Constraints,
  implemented concretely here for the first time.
- Frontend: Canvas/OffscreenCanvas (native browser API, no new dependency) for client-side
  translated-image compositing when a locally-hosted backend is selected; IndexedDB/Cache API
  (native) for client-side caching of that composited output; `localStorage` (native) for the
  device-local backend-selection override (FR-003).

**Storage**: Redis (reused, per constitution Principle I) for server-side state: Volume Font
Pattern, server-stored (cloud/API-key) Translation Backend Selection, Usage Budget and its
consumption tracking, and the server-composited Translation Cache Entry (cloud-backend case).
Browser-side storage (`localStorage`, IndexedDB/Cache API) for device-local state per FR-003 and
the client-composited cache case — this is a deliberate, spec-driven split, not an oversight: see
research.md.

**Testing**: `cargo test` for OCR/merging/font-cache/adapter logic; frontend tests (Vitest, per
this project's established convention) for the compositing/cache/settings UI; a small fixture set
of sample manga pages with known text regions for OCR/merging regression tests.

**Target Platform**: Same as Phase 1 (Linux server, Debian slim Docker image); browser-side work
targets modern Chromium/Firefox (Canvas, OffscreenCanvas, IndexedDB, Cache API are broadly
supported; Private Network Access behavior — relevant to the locally-hosted backend path — is
currently Chromium-specific and must be handled per constitution Principle V).

**Project Type**: Extension of the existing Phase 1 web application — new Rust crates added to
the same Cargo workspace, new modules added to the same frontend app. Not a new project.

**Performance Goals**: SC-002 (zero measurable regression when translation is off), SC-004 (95%
of ordinary forward-reading sessions have look-ahead pages ready in time), SC-007 (a failed/
unreachable backend adds no perceptible delay to reading).

**Constraints**: Constitution Principle III (OCR/font-classification batching must use the
rayon/spawn_blocking bridge, not block the async reactor); Principle V (the core constraint this
feature is built around — cloud credentials server-side only, local-backend traffic
browser-originated with graceful PNA degradation); Principle VI (this plan and its eventual tasks
must stay in `specs/002-ocr-manga-translation/`, must not modify `specs/001-lanrurugi-full-rewrite`
plan/tasks, and must not gate Phase 1).

**Scale/Scope**: Operates over the same library scale Phase 1 targets (up to ~100,000 archives,
per `specs/001-lanrurugi-full-rewrite`'s SC-008) — this feature doesn't change that scale target,
it adds a per-page, on-demand processing layer on top of it. 5 user stories, 21 functional
requirements, 7 success criteria (all from spec.md).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | New Redis keys (font pattern, cache, budget) must be additive, must not alter existing archive/category/tankoubon keys | **PASS** — data-model.md's new entities use their own key namespaces; nothing here touches Phase 1's archive identity or metadata keys |
| II. API Contract Fidelity (Phase 1) | Must not alter any existing `contracts/rest-api.md` (Phase 1) path | **PASS** — all new endpoints in `contracts/` here are new paths under a distinct namespace; zero edits to Phase 1's contract |
| III. Resource-Conscious, Genuinely Concurrent Architecture | OCR/font-classification (CPU/GPU-bound) must use the established rayon+spawn_blocking bridge, not run inline on an async worker thread | **PASS** — research.md's OCR/batching decision explicitly reuses Phase 1's bridging pattern rather than inventing a new concurrency model |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | N/A — this feature does not introduce or modify plugins | **PASS (not applicable)** — LLM provider adapters are a distinct subsystem from the Deno plugin runtime and must not be conflated with it |
| V. Secrets & Network Trust Boundaries | Cloud credentials server-side only; local-backend traffic browser-originated; graceful degradation on PNA failure | **PASS** — this is the principle this plan is built around; data-model.md and contracts/ implement the server/client storage and call-path split exactly as specified |
| VI. Phased Scope Discipline | Must not modify `specs/001-lanrurugi-full-rewrite`'s plan.md/tasks.md; must not block Phase 1 | **PASS** — this plan lives entirely under `specs/002-ocr-manga-translation/`; no Phase 1 artifact is touched |

No violations requiring justification — **Complexity Tracking is empty.**

**Post-design re-check** (after research.md, data-model.md, contracts/, quickstart.md): still
PASS across all six principles — data-model.md's new Redis key namespaces and browser-only
storage (`localStorage`/IndexedDB) are additive and don't touch any Phase 1 key (I); all
`contracts/` paths are new and don't alter `specs/001-lanrurugi-full-rewrite/contracts/rest-api.md`
(II); research.md §1–§4 keep OCR/font-classification on the established rayon+spawn_blocking
bridge (III); no plugin/Deno concept was introduced or modified (IV); research.md §6–§9 and
`contracts/llm-provider-adapter.md`/`client-compositing-cache.md` implement the server/client
trust-boundary split exactly, including graceful PNA degradation (V); nothing here touches
`specs/001-lanrurugi-full-rewrite`'s plan.md/tasks.md (VI). No new Complexity Tracking entries.

## Project Structure

### Documentation (this feature)

```text
specs/002-ocr-manga-translation/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md         # Phase 1 output (/speckit-plan command)
├── quickstart.md         # Phase 1 output (/speckit-plan command)
├── contracts/            # Phase 1 output (/speckit-plan command)
└── tasks.md              # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

This feature adds to the existing Phase 1 workspace/app (see
`specs/001-lanrurugi-full-rewrite/plan.md` § Project Structure) rather than creating a new one:

```text
crates/
├── lanrurugi-ocr/            # NEW: oar-ocr-based detection + custom manga-ocr recognition
│   └── src/                  # (recognize.rs) + IoU/geometric line-paragraph merging →
│                              # produces Detected Text Region records (source_text included)
├── lanrurugi-fontcache/       # NEW: Volume Font Pattern three-stage state machine
│   └── src/                  # (voting, excluding covers; locking; meltdown re-classification
│                              # without polluting the vote pool — see research.md)
├── lanrurugi-translate/       # NEW: LLM provider adapters (OpenAI-compatible + Anthropic),
│   └── src/                  # Translation Cache Entry (server-composited case), Usage Budget
├── lanrurugi-api/             # EXTENDED (existing Phase 1 crate): new translation-settings,
│   └── src/                  # usage, and backend-selection endpoints (additive only)
└── lanrurugi-server/          # EXTENDED (existing Phase 1 crate): wires the above into the
                                # same single binary — no new deployable process

frontend/
└── src/
    └── translation/           # NEW: reader overlay rendering; client-side Canvas/OffscreenCanvas
                                # compositing + IndexedDB/Cache API cache for the locally-hosted-
                                # backend path; localStorage-backed local backend-selection
                                # override; settings UI (backend, target language, usage/budget
                                # view with optional chart)
```

**Structure Decision**: No new crate-workspace or frontend app is created — this is additive to
Phase 1's existing structure, consistent with the constitution's single-binary/single-app
principle (III) and this feature's own non-blocking relationship to Phase 1 (VI). The
frontend/backend split for the *compositing* step specifically follows the backend-type branch
established in this feature's design discussion: server composites and caches for cloud backends
(no extra round-trip, server already holds the translated text after proxying the call);
`frontend/src/translation/` composites and caches client-side for the locally-hosted-backend case
(translated text never needs to leave the browser to be usable).

## Complexity Tracking

*No entries — Constitution Check produced no violations requiring justification.*
