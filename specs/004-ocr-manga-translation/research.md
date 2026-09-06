# Phase 0 Research: On-Page Manga Translation (Phase 2)

Each entry: Decision / Rationale / Alternatives considered. This consolidates prior exploratory
design work (`specs/001-lanrurugi-full-rewrite/phase2-design-notes.md`) and the design discussion
that produced this feature's spec.md Clarifications, now finalized as concrete technical
decisions.

## 1. OCR inference runtime

**Decision**: OCR is split into two independently-sourced steps, not one generic detection model:

- **Detection** (finding *where* text is on a page): [`oar-ocr`](https://github.com/GreatV/oar-ocr)
  (Apache-2.0, verified), which bundles PP-OCR detection and manages its own model
  fetch/inference internally — no custom acquisition or `ort` code needed for this half.
- **Recognition** (transcribing the text *within* each detected region into `source_text`): a
  custom `ort` (ONNX Runtime's Rust bindings, pin starting from `2.0.0-rc.12` — the version
  already vetted and running in the user's own `~/jellyfin-suite` project's `frame-forge` crate,
  subject to revalidation at implementation time) integration against kha-white's **Apache-2.0**
  [`manga-ocr`](https://github.com/kha-white/manga-ocr) model weights (ONNX-exported), written
  independently by this project. **CPU execution provider only for this phase** — no
  CUDA/DirectML/OpenVINO GPU acceleration in the initial scope (see Rationale).

**Rationale**:

*Why two separately-sourced steps, not one generic OCR library*: General-purpose CJK OCR is not
good enough for manga specifically. DeepSeek-OCR (a strong general vision-language OCR model,
good general CJK handling) has a documented weakness on **vertically-written Japanese text**
(tategaki) — extremely common in manga lettering. PP-OCR-family models (including `oar-ocr`'s own
bundled recognition) are trained for regular documents, not manga's stylized/vertical/sound-effect
lettering. `manga-ocr` (kha-white) is trained specifically on Japanese manga text (vertical text,
stylized fonts, furigana, sound effects) and is verified **Apache-2.0** via GitHub's own license
API (`gh api repos/kha-white/manga-ocr` → `license.spdx_id: Apache-2.0`) — clean to depend on, and
the right accuracy tradeoff for this project's actual content. `oar-ocr`'s own detection is kept
for the detection half specifically because it's a reasonable, permissively-licensed, low-effort
v1 choice for "where is the text," a problem general document-detection models handle acceptably
even though manga-tuned detection would be better (see "Rejected" below for why manga-tuned
detection isn't available to depend on cleanly).

*Why recognition is custom `ort` code rather than another dependency*:
[Koharu](https://github.com/mayocream/koharu) is a complete, local-first Rust manga translator
(detection + `manga-ocr` recognition + inpainting + a manga-lettering-aware renderer) that
validated this project's layered architecture (specialized vision models for detection/
recognition, LLM only for translation) and specifically pointed at `manga-ocr` as the right
recognition model. It is **not usable as a dependency**, verified directly against the live
repository rather than crates.io search results: its current workspace (`Cargo.toml`, checked at
version `0.61.2`) sets `[workspace.package] license = "GPL-3.0-only"` and **`publish = false`** —
the maintainer does not intend these crates for external reuse, and every current sub-crate
(`koharu-llm`, `koharu-ml`, etc., checked directly) inherits GPL-3.0 via `license.workspace =
true`. Depending on this code would put LANrurugi's own licensing under real pressure (Rust's
static linking means GPL copyleft would likely extend to the whole binary). The Apache-2.0
packages that *do* exist on crates.io under Koharu-adjacent names (`koharu-models`,
`koharu-core`, `koharu-renderer`, `koharu-runtime`, all `v0.10.1`) are stale snapshots from
before the GPL-3.0 relicense — far behind the current `v0.61.2` — not the actively maintained
code; `manga-ocr`/`comic-text-detector` as standalone crates.io packages (`v0.5.1`,
crates.io-reported license `non-standard`) are even more orphaned and don't correspond to any
directory in the current repository at all. **Decision**: the Koharu repository was cloned
locally for architecture/approach reference only (pipeline structure, model choices, how it
handles vertical-CJK rendering) — informing this project's own independent implementation, never
as a build dependency and never as copied code. LANrurugi writes its own `ort` integration against
`manga-ocr`'s Apache-2.0 model weights directly.

*Why CPU-only for the custom recognition inference*: `~/jellyfin-suite/crates/frame-forge`
already runs `ort` in production for a different deep-learning task (panorama-stitching feature
matching) and has hard-won, documented lessons directly relevant to whatever `ort` code this
project writes (verified by reading `dl_match.rs`/`gpu_compat.rs` directly, not assumed):
- Model file discovery follows a clean, reusable search-path pattern (`find_model()` in
  `dl_match.rs`): an env var override, then next to the running binary, then a production config
  path, then a container path — first match wins. This project's `manga-ocr` model discovery
  follows the same shape.
- **GPU execution providers are not simply "try, then fall back on error."** CUDA and DirectML
  fail cleanly when unavailable (`commit_from_file` returns `Err`) and a clean CPU fallback
  works. But a documented production incident shows registering `OpenVINOExecutionProvider`
  against an ORT build that has no OpenVINO provider code compiled in doesn't fail cleanly — it
  **hangs indefinitely** inside `session.run()`. jellyfin-suite's fix requires a companion
  service (C#, part of the Jellyfin plugin host) that downloads a genuinely OpenVINO-enabled ORT
  build and signals its presence via an env var (`FRAME_FORGE_ORT_ASSET_KEY`) that must be checked
  *before* ever attempting that EP — "just try it and see" is unsafe for this specific provider.
- Given LANrurugi has no equivalent plugin-host service to run that kind of hardware-acceleration
  asset acquisition, and manga-page OCR recognition is a much lighter workload than frame-forge's
  video/image super-resolution use case (the reason it needed GPU acceleration at all),
  **CPU-only via `ort`'s default execution provider is the correct initial scope** — it avoids
  inheriting the EP-hang risk class entirely rather than partially mitigating it. `ort`'s own
  `download-binaries` feature (used as-is in jellyfin-suite, not a custom acquisition service)
  fetches a working standard CPU/CUDA12 build automatically, with no manual ONNX Runtime install
  step for self-hosted users.
- GPU acceleration MAY be added later as an explicit, feature-flagged, opt-in enhancement once
  real performance data shows CPU is insufficient at the target library scale — at that point,
  reuse jellyfin-suite's asset-key-gating pattern for OpenVINO specifically (never attempt it
  without a verified-compatible asset active), not a "try and catch" approach.

**Alternatives considered/rejected**:
- Full or hybrid adoption of Koharu's crates — rejected; see Rationale (GPL-3.0, `publish = false`,
  and the permissive packages under its name are stale/orphaned).
- Using `oar-ocr`'s own bundled recognition for both steps (a single-dependency, simpler option)
  — rejected on accuracy grounds for this project's actual content (manga, often vertical
  Japanese), per the CJK-accuracy findings above.
- `tract` (pure-Rust inference, no native ONNX Runtime dependency) for the custom recognition
  integration — lighter weight and avoids a native library dependency in the Docker image, but
  has historically narrower operator coverage than ONNX Runtime, and there's no equivalent
  in-house track record with it the way there is with `ort`; worth revisiting only if `ort`'s
  native dependency proves troublesome for the Debian-slim Docker image (constitution Technology
  Stack Constraints).
- A Python sidecar process for either step — rejected, reintroduces the multi-process complexity
  Principle III moved away from.
- Shipping GPU execution providers in the initial scope — rejected per the OpenVINO-hang finding
  above; CPU-only is both simpler and safer to ship first.
- This keeps LANrurugi's own font-matching cache (research.md §4) and server/client compositing
  split (research.md §6/§7) exactly as already designed — those were built around this project's
  own server/local-backend trust-boundary split (Principle V), which a single-user desktop app
  like Koharu never had to solve, so there was never a good reason to replace them.

## 2. OCR batching strategy

**Decision**: OCR detection runs on-demand as pages enter the look-ahead prefetch window (User
Story 3), not as a bulk pre-processing pass over an entire archive up front. Multiple pages
queued for look-ahead at the same time are batched together (batch size 4–8, matching the
original design sketch) into a single inference call when the queue allows it, falling back to
smaller batches when fewer pages are queued (e.g. the reader just opened an archive and only
page 1 is requested).

**Rationale**: Matches the existing sliding-window prefetch model (US3/FR-011) rather than
introducing a second, separate "pre-scan the whole archive" pipeline; batching only when multiple
pages are genuinely queued together avoids forcing an artificial wait for a full batch to fill
before starting.

**Alternatives considered**: Eager whole-archive OCR on first open — rejected; most users don't
read every page of every archive, so this would waste work and delay the first page unnecessarily
compared to just processing what the look-ahead window actually needs.

## 3. Text region merging (IoU / geometric)

**Decision**: Merge raw per-line detection boxes into paragraph-level regions when boxes have
high IoU overlap or sit within a small configurable horizontal/vertical distance threshold of
each other, producing the `Detected Text Region` records translation and font-matching operate
on.

**Rationale**: Directly from the original design sketch — translation quality depends on
receiving a coherent block of text, not fragments; font-matching likewise needs a stable per-block
unit to classify.

**Alternatives considered**: Translating each raw line independently — rejected, breaks sentence-
level context for the translation backend and would multiply the number of LLM calls per page.

## 4. Volume Font Pattern: three-stage cache, with the review concerns resolved

**Decision**: The three-stage design (voting → locking → meltdown) proceeds as originally
sketched, with the three concerns raised during design review now resolved as binding decisions:

- **Cover exclusion (already in spec.md FR-008)**: cover page(s) are excluded from the voting
  sample entirely — detected during ingestion metadata (Phase 1's archive model already
  distinguishes a cover) rather than inferred from OCR output.
- **Meltdown does not feed the primary vote pool.** A meltdown re-classification (triggered by a
  block whose cheap features are a clear outlier) is tracked in a *separate* tally, never merged
  into the pool that determined the locked golden set. Only if a meltdown outlier's classified
  font recurs often enough on its own (crossing its own, separate threshold) does it get
  considered for promotion into the golden set — a single deliberate design change from the
  original sketch, made specifically to prevent the failure mode identified in review (a
  recurring "special scene" font gradually displacing a legitimate low-frequency main font).
- **Two sequential checks, not one combined branch.** Implementation is a lock-state check
  (`is_locked` on the Volume Font Pattern record) followed, only if locked, by a cheap per-block
  feature classifier deciding "matches established pattern" vs. "outlier → meltdown" — modeled as
  two distinct functions/steps rather than one combined decision, per the review's clarity
  concern.
- **Fast path routes among the top 2–3 fonts, not just the single most common one.** The cheap
  per-block classifier (word count, bubble aspect ratio, punctuation) selects among all fonts in
  the locked golden set (typically 2–3), preserving legitimate intra-volume font variety (e.g.
  dialogue vs. sound-effect lettering) rather than collapsing everything to one font.

**Rationale**: These are exactly the three concerns raised when this design was first sketched and
reviewed; resolving them now (rather than leaving them as open questions) avoids relitigating them
during implementation.

**Alternatives considered**: A single shared vote pool for both initial voting and meltdown
results (the original naive sketch) — rejected per the above.

## 5. LLM provider adapters

**Decision**: Per constitution Technology Stack Constraints — an internal request/response type
modeled on the OpenAI Chat Completions shape, covering OpenAI-compatible providers directly and
Ollama via its own OpenAI-compatible endpoint (a configuration preset, not a third code path); a
distinct adapter for Anthropic's native Messages API (separate `system` field, content-block
array, `x-api-key`/`anthropic-version` auth, mandatory `max_tokens`, distinct SSE event shape).

**Rationale**: Already fixed at the constitution level; this plan implements it concretely for
the first time. Two adapters, not three, is the correct scope.

**Alternatives considered**: A bespoke third adapter for Ollama — rejected, Ollama's own
OpenAI-compatible endpoint makes this unnecessary.

## 6. Translated-page compositing location: split by backend type

**Decision**: Compositing (drawing translated text, in the matched font, over the original page)
happens in different places depending on which backend produced the translation:
- **Cloud-hosted backend**: composited **server-side**. The server already holds the translated
  text immediately after proxying the LLM call (constitution Principle V), so it composites and
  writes the result to the server-side `Translation Cache Entry` (Redis metadata + an image cache
  alongside Phase 1's existing thumbnail cache) with no extra round-trip.
- **Locally-hosted backend**: composited **client-side**, using Canvas/OffscreenCanvas. The
  translated text only exists in the browser after the direct call to the local model
  (Principle V — the server cannot reach the user's loopback device), so requiring it to be sent
  back to the server purely to be composited would add a pointless round-trip and burden server
  resources for work the user's own device is already positioned to do.

**Rationale**: Directly resolves the "does the local-model path need to round-trip through the
backend for compositing" question raised during design discussion, by following the same
trust-boundary split Principle V already establishes for the translation call itself — compositing
location follows where the translated text already lives, not a separate, uniform rule.

**Alternatives considered**: Always composite server-side (uniform architecture) — rejected;
would force local-backend translations to round-trip through the server for no benefit. Always
composite client-side — rejected for the cloud case; the server already has everything it needs
and a shared, server-side cache benefits every device the user reads from, which a browser-only
cache would not (see decision 7).

## 7. Client-side compositing does not need WASM; caching does not need filesystem write access

**Decision**: Client-side compositing (locally-hosted-backend case) uses plain Canvas/
OffscreenCanvas 2D drawing — no WASM module. The composited result, when cached for reuse, is
stored via IndexedDB or the Cache API (as a Blob/PNG), not written to any OS filesystem path.

**Rationale**: The compositing operation here is "draw text with a background box over an
image" — a lightweight 2D drawing task Canvas already handles natively at native-ish speed; WASM
would only earn its complexity if this project later adds something genuinely CPU-heavy client-
side (e.g. inpainting to cleanly remove original lettering before overlay), which is explicitly
out of scope for this phase (FR-005 only requires translated text positioned over the original
region, not the original text removed). IndexedDB/Cache API are standard, already-granted,
origin-scoped browser storage — using them sidesteps the "does the browser have write permission"
question entirely, since it's not filesystem access at all (unlike the File System Access API,
which does require an explicit user permission grant and would be the wrong tool here).

**Alternatives considered**: A WASM-compiled compositing module — rejected for this phase, no
identified workload here actually needs it. The File System Access API for a persistent on-disk
cache — rejected, requires user permission prompts for no benefit over IndexedDB/Cache API.

## 8. Backend-selection storage split and precedence

**Decision**: A locally-hosted backend selection (FR-003) is stored in `localStorage`, scoped to
the device/browser it was configured on. A cloud-hosted/API-key backend selection is stored
server-side (Redis), applying account-wide across the user's devices — consistent with how
Phase 1's own single-owner/single-instance model already works. On any given device, if a
`localStorage` selection is present, it takes precedence over the server-stored default for that
device only.

**Rationale**: A `127.0.0.1` (or any locally-hosted) selection is only meaningful on the specific
device it points at; silently applying it on a different device would be actively wrong (that
device's "localhost" is a different machine). Cloud/API-key selections have no such locality
constraint and belong with the rest of this project's server-stored, cross-device settings.

**Alternatives considered**: Storing everything server-side — rejected, breaks on any device
other than the one the local backend actually runs on. Storing everything client-side — rejected,
would scatter cloud API-key handling into the browser, directly conflicting with constitution
Principle V.

## 9. Local-backend browser connectivity (Private Network Access)

**Decision**: Ranked mitigation, cheapest-for-the-user first, matching
`phase2-design-notes.md` §3 and the constitution's existing PNA guidance:
1. Configure the local backend itself to answer the PNA preflight correctly (e.g. an `OLLAMA_ORIGINS`-equivalent setting plus, if/when supported, an `Access-Control-Allow-Private-Network: true` response) — zero extra install, the user already runs the local backend.
2. A companion bridge, shipped as a subcommand of the same `lanrurugi-server` binary (not a
   separate downloaded program) — fallback for setups where (1) isn't sufficient.
3. A browser extension — smoother one-time-install fallback, last resort.
The UI attempts a direct connection first and only surfaces steps 2/3 if that fails (FR-018).

**Rationale**: Already established project guidance (Principle V, `phase2-design-notes.md`);
restated here as this feature's concrete plan, not re-litigated.

**Alternatives considered**: Instructing users to disable browser security features — explicitly
rejected by the constitution.

## 10. Usage Budget tracking granularity and visualization

**Decision**: Usage consumption against a metered backend's budget is tracked and queryable at
four granularities — current page, current archive, today, current calendar week (FR-014) —
stored server-side (Redis, since budget/consumption relates to the server-proxied cloud call
path only; the locally-hosted path has no metered cost to track). A chart-style visualization is
a SHOULD, not a MUST — its specific charting approach is a frontend implementation detail for the
tasks phase, not fixed here.

**Rationale**: Directly from spec.md FR-014; scoping tracking to the cloud/metered path only
(rather than also instrumenting the free, locally-hosted path) avoids doing pointless bookkeeping
where there's no cost to track.

**Alternatives considered**: Tracking usage client-side only — rejected, the budget/consumption
data is inherently tied to the server-proxied cloud call path and belongs with that path's other
server-side state.

## 11. Target-language scope: LTR only

**Decision**: Target language selection (FR-004) is restricted to left-to-right languages this
phase. No bidi (bidirectional text) handling, no RTL-aware layout/justification is implemented.

**Rationale**: Directly from spec.md's Clarifications and Assumptions; keeps text-rendering logic
(positioning translated text within a detected region, line-wrapping) simpler for this phase by
construction, deferring RTL-specific typographic concerns to a later, separate increment.

**Alternatives considered**: Building RTL support now — explicitly deferred per the spec.

## 12. Not-yet-ready page display

**Decision**: When a page is reached before its look-ahead translation completes, the reader
shows the original page immediately with a small, non-blocking loading affordance (e.g. a
corner-anchored indicator, not a full-page overlay or spinner-over-content), then swaps in the
translated/composited version in place once it's ready — no layout shift, no blocking wait.

**Rationale**: Directly from spec.md FR-012/Clarifications; "non-obscuring or minimally
obscuring" rules out a centered full-page spinner, which is the most common default pattern and
would violate this requirement if implemented naively.

**Alternatives considered**: A blocking full-page loading state — rejected, spec explicitly
requires the original content to remain visible and usable while waiting.
