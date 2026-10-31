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
  independently by this project. **CPU execution provider always available as the explicit
  fallback; CUDA added as of issue #103 (see 2026-09-12 update at the end of this section) — no
  DirectML/OpenVINO GPU acceleration** (see Rationale).

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
  **CPU-only via `ort`'s default execution provider was the correct initial scope** — it avoided
  inheriting the EP-hang risk class entirely rather than partially mitigating it. `ort`'s own
  `download-binaries` feature (used as-is in jellyfin-suite, not a custom acquisition service)
  fetches a working standard CPU/CUDA12 build automatically, with no manual ONNX Runtime install
  step for self-hosted users.
- GPU acceleration MAY be added later as an explicit, feature-flagged, opt-in enhancement once
  real performance data shows CPU is insufficient at the target library scale — at that point,
  reuse jellyfin-suite's asset-key-gating pattern for OpenVINO specifically (never attempt it
  without a verified-compatible asset active), not a "try and catch" approach.

**2026-09-12 update (issue #103) — CUDA added, this section's OpenVINO/DirectML judgment
unchanged**: The paragraph above about "CPU-only initial scope" described the very first cut of
this feature. Issue #103 later added NVIDIA CUDA specifically (not OpenVINO, not DirectML) as an
optional per-session accelerator, with CPU kept as the always-available explicit fallback — this
is exactly the class of EP this section already called safe two paragraphs up ("CUDA and DirectML
fail cleanly when unavailable... a clean CPU fallback works"), not a reversal of the OpenVINO-hang
finding. Full design + a real hardware spike (including a since-fixed CUDA-session-`Drop` crash
unrelated to the EP-hang risk this section discusses — root-caused to NVIDIA's own driver
userspace component, not to this project, `ort`, or ONNX Runtime) live in
`.debug-scratch/GPU_EP_INTEGRATION_PLAN.md` and `GPU_EP_FINAL_SUMMARY.md`; the real code lives in
`lanrurugi-ocr::recognize`/`bubble_segment` and `lanrurugi-inpaint`'s
`build_session_with_gpu_fallback` functions. OpenVINO/DirectML remain out of scope, unchanged from
the original decision above — this update only concerns CUDA.

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
  above; CPU-only was both simpler and safer to ship first. (CUDA specifically was added later,
  issue #103 — see the 2026-09-12 update above; this bullet describes the initial-scope decision,
  not the current state.)
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
  on disk) with no extra round-trip. See §18 for which existing Phase 1 disk-cache mechanism this
  image cache shares quota with (not the thumbnail cache, despite this document's earlier wording
  — thumbnails are permanent, unquota'd generated content, a different case entirely).
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

## 13. Per-block color/boldness estimation (added 2026-09-06, `/speckit-clarify`)

**Decision**: Estimate each `Detected Text Region`'s own foreground color, background color, and
boldness via a lightweight heuristic pass over its cropped pixels — run once per region alongside
the existing rayon-batched OCR pass (§2), not as a separately-scheduled step — rather than
retraining or fine-tuning `manga-ocr` to output these attributes directly. Each of the three
attributes is estimated and stored independently; a low-confidence result on any one does not
block the others (spec.md FR-008a).

**Rationale**: Researched against `zyddnys/manga-image-translator`
(GPL-3.0 — cloned locally to `~/manga-image-translator` for architecture reference only, following
this project's existing Koharu precedent from §1; never a dependency, no code reused) — the most
prominent open-source prior art for this exact problem:

- That project's own OCR recognition model (`model_48px_ctc.py`) is a **multi-head** architecture:
  a shared backbone feeds both a `char_pred` head (text recognition) and a `color_pred1` head
  (per-character foreground+background RGB, 6 float outputs per timestep) in a single forward
  pass — genuinely zero extra inference cost, because color comes from the same model call as
  recognition. This is the ideal architecture, but requires a model trained with that second head
  from the start.
- `manga-ocr` (kha-white, this project's chosen recognition model per §1) has no such color-output
  head, and retraining/fine-tuning it to add one is out of scope for this phase (would require
  assembling a labeled color-annotated manga-text training set, well beyond "use an existing
  Apache-2.0 model" per this project's original §1 scoping). A separate lightweight heuristic pass
  over each region's cropped pixels — informed by `sift-ocr`'s published approach (KMeans
  clustering over pixels sampled from the region to find dominant foreground/background color) —
  gets most of the same practical benefit without a training investment, at the cost of one small
  additional per-region computation (folded into the existing rayon batch, §2, not a new
  sequential stage).
- **Boldness has no working automatic-detection precedent to adopt.** `manga-image-translator`'s
  own data model (`utils/textblock.py`) carries a `bold: bool` field, but grep across its entire
  OCR/text-line-merge pipeline confirms it is never actually computed anywhere — it exists as a
  constructor parameter, always left at its default. No open-source manga-translation project
  found during this research implements real bold detection. Given no working prior art exists to
  adopt, LANrurugi uses its own simple heuristic (stroke-width-to-glyph-height ratio, computed from
  the same cropped-pixel pass as color) rather than leaving the field entirely unaddressed — this
  is explicitly a first attempt with no external validation, more likely to need revision later
  than the color estimation (which has real prior art behind its approach) — see spec.md FR-008a's
  independent-fallback design, which exists specifically so a wrong/low-confidence boldness guess
  never blocks the (better-grounded) color estimate for the same block.

**Alternatives considered**:
- Retraining/fine-tuning `manga-ocr` with an added color-output head, matching
  `manga-image-translator`'s architecture exactly — rejected for this phase on scope grounds (see
  Rationale); worth revisiting if a pre-trained Apache-2.0/MIT recognition model with a built-in
  color head is ever identified, which would obsolete this heuristic pass entirely.
- Treating boldness as out of scope entirely (leaving `is_bold` always absent) — considered and
  rejected in favor of a first-attempt heuristic, since spec.md FR-008a's independent-fallback
  design already absorbs the cost of that heuristic being wrong without harming the other two
  attributes.
- A generic per-pixel color histogram without clustering — rejected in favor of the KMeans-based
  approach `sift-ocr` publishes, since a histogram alone doesn't cleanly separate foreground
  (glyph) pixels from background (bubble) pixels the way clustering on spatially-sampled pixels
  does.

## 14. Translation consistency: Terminology Glossary + advisory context, not a bigger cache

**Decision**: Two related but distinct gaps — name/term consistency and tone/style
consistency — are both addressed by populating the previously-defined-but-never-populated
`context` field on the LLM provider adapter's normalized request (`contracts/llm-provider-
adapter.md`), assembled server-side from three independently-optional sources, in order:
(1) exact-substring Terminology Glossary hits (deterministic, free, no LLM judgment involved),
(2) the volume's other known glossary names (names only) so the backend can itself recognize a
nickname/initialism variant, and (3) the current page's other already-translated blocks (source +
translation) as tone/style reference. See spec.md FR-007a–e and Clarifications (Session
2026-09-06) for the full requirement text and the reasoning behind each.

**Rationale**:

*Why a hybrid (exact match + LLM judgment) for names, not either alone*: A first pass at this
design considered only exact-substring matching against a glossary — cheap, deterministic, and
sufficient for a name/term that recurs verbatim. But real manga text doesn't only recur verbatim:
a character introduced as さゆき may later be called さっちゃん (a nickname with no substring
relationship to the original — Japanese nickname formation from a given name is a
cultural/contextual convention, not a derivable string transform), and a Western full name
introduced once ("Axxx Bxxx Cxxx") is often abbreviated later ("A.B.C."/"ABC"). Extending the
substring rule to also catch these would require guessing at nickname-formation patterns or
initialism rules — and worse, a purely shape-based initialism rule risks false-positive matching
against a genuine, unrelated all-caps acronym that was never introduced as a name at all (the user
explicitly flagged this exact failure mode during spec review). Recognizing "this is the same
entity, referred to differently" is fundamentally a semantic/contextual judgment, which is exactly
what an LLM is well-suited for and a fixed string rule is not — so the design keeps the free
deterministic path for the case it actually handles well (exact recurrence) and defers the harder
case (variant recognition) to the backend's own language understanding, rather than trying to
encode ad-hoc pattern rules for an open-ended set of natural-language nickname/abbreviation
conventions.

*Why tone/style consistency reuses the same `context` mechanism instead of its own cache*: Unlike
a name (a discrete value with one correct answer per entity, cacheable in a lookup table), tone is
continuous and scene-dependent — the same character can be playful in one scene and serious in
another, so there is no fixed "this character's tone" value to store the way Volume Font Pattern
stores a fixed font. Attempting to build a tone-classification/caching system would require
LANrurugi to itself perform tone classification (out of scope — this project has no NLP
sentiment/register-classification component and adding one is a much larger undertaking than this
consistency gap warrants) and would still need per-scene overrides, defeating the purpose of
caching. Supplying the page's other already-translated blocks as advisory reference material lets
the backend infer tone from real surrounding context on every request, at a bounded cost (one
page's worth of text, not the volume's), without LANrurugi needing to model tone as a first-class
concept at all.

*Why the glossary is scoped per-volume (or per-archive if ungrouped), mirroring Volume Font
Pattern*: Character names and established terminology are volume-level facts (a series' cast list
doesn't reset per page), and this project already has a working precedent for volume-scoped state
that accumulates during reading and persists in Redis (Volume Font Pattern, §4) — reusing that
scoping keeps the two entities' lifecycle/reset semantics easy to reason about together, even
though (per spec.md FR-007d) the glossary deliberately does *not* copy Volume Font Pattern's
bulk-reset affordance (FR-010), since an individual wrong glossary entry is independent of the
others and a bulk reset would discard already-correct entries for no benefit.

**Alternatives considered**:
- Sending the LLM the entire glossary on every request, letting it figure out relevance itself —
  rejected; grows unboundedly as a volume's glossary accumulates entries, directly conflicting
  with this feature's existing cost-aware/budget-visible design (FR-013/FR-014) for what would
  often be a majority-irrelevant context payload (most text blocks don't mention most characters).
- A per-character tone/style preference the user or system maintains explicitly (e.g. "Character X
  = playful") — rejected; scene-dependent tone shifts would make any fixed per-character value
  wrong some of the time by design, and authoring such preferences manually is exactly the kind of
  added friction this feature's existing low-friction posture (FR-007) argues against.
- Requiring user confirmation before a new glossary entry takes effect — rejected; consistent with
  the same FR-007 low-friction reasoning already applied to Volume Font Pattern's own auto-voting
  design (no confirmation gate there either), and a wrong auto-captured entry is correctable after
  the fact (FR-007d) rather than needing to be prevented up front.
- Sending each text block as its own fully independent LLM request, with no batching at all —
  superseded; see §15 (revised after initial design discussion — batching by small fixed group is
  the final decision, not per-block requests).

## 15. Translation request batching, prefetch integration, and prompt-cache alignment

**Decision**: A translation request batches a **small, fixed-size group of pages** (2–4 pages,
matching the OCR batching group size already established in T009, for design consistency between
the two pipeline stages) into one LLM call, rather than one call per text block. Look-ahead
prefetch (US3) drives this batching directly — as pages enter the look-ahead window, they're
grouped into fixed-size batches and each batch becomes one translation request — rather than
prefetch and request-granularity being decoupled. Every request's content is still ordered
stable-content-first, dynamic-content-last (`contracts/llm-provider-adapter.md`): Terminology
Glossary matches/names first, then same-page/same-batch tone-reference material, then the
batch's own text blocks (each tagged with a `block_id`) last. The normalized request/response
shapes become arrays of tagged blocks rather than single strings (see the updated contract). A
third adapter, DeepSeek, is added alongside the existing OpenAI-compatible and Anthropic adapters,
since DeepSeek's own caching behavior (see Rationale) is distinct enough to document explicitly
even though it's also OpenAI-Chat-Completions-shaped at the wire level.

**Rationale**:

*Why batching won out over one-request-per-block*: The original per-block design (§5) was
correctness-first (simple response parsing, no ID-mapping bugs possible, and a failure only ever
loses one block, keeping FR-019/FR-020's failure isolation trivial) but pays a fixed per-request
overhead (HTTP round-trip, and every request repeating the same Terminology-Glossary-derived
prefix content) once per text block — often 3-8+ times per page. Batching a small fixed group
amortizes that fixed overhead across several blocks/pages per call, at the cost of the response
now needing real `block_id` tags to map translations back to their originating block (unlike the
single-string shape, which relied entirely on "the caller knows which block it asked about because
it made the call"), and of a failure now needing to be handled at *batch* granularity for
FR-019/FR-020 (a whole batch's pages show the original-page fallback if the batch's LLM call
fails, not just one page) — accepted as a reasonable, bounded cost given the batch size is
deliberately small and capped, not unbounded.

*Why the batch size mirrors T009's OCR batch size (2–4, within T009's already-established 4–8
range)*: Reusing an already-decided, already-justified batch-size range keeps this pipeline
internally consistent rather than introducing a second, independently-tuned constant for the same
general "how many pages of look-ahead work to group together" question; the exact value within
that range is a tasks-phase tuning parameter, not fixed here.

*Why stable-prefix ordering and prompt caching still apply, and still don't rescue an unbounded/
sliding-everything design*: All three adapters' underlying providers cache via **exact-prefix
matching**, not similarity or semantic matching — confirmed directly against each provider's
current documentation, not assumed, including a live fetch of DeepSeek's own official docs
(`api-docs.deepseek.com`) specifically because this project's actual configured/tested provider
(the `deepseek-recommend-architecture` memory) is DeepSeek, not a hypothetical. DeepSeek's own
worked example states explicitly that a request with prefix `A+B` and a request with prefix `A+C`
cannot hit each other's cache — only a byte/token-identical shared prefix (`A`) is reusable.
Anthropic's `cache_control` and OpenAI's automatic caching work the same way (research.md's
earlier per-provider findings, unchanged by this revision). What batching changes is only *how
much* content sits after the stable prefix on each call, not whether the prefix itself needs to be
identical to hit — the Terminology Glossary content, being genuinely stable across many
consecutive requests for the same volume (only grows, rarely edited — FR-007d), remains the right
thing to keep first regardless of batch size, and remains cacheable at that position.

*Why DeepSeek gets called out as its own adapter documentation, not folded silently into the
"OpenAI-compatible" adapter's existing entry*: DeepSeek's context caching is meaningfully
different from the other two providers in ways worth documenting explicitly rather than assuming
"OpenAI-compatible wire shape" implies "same caching characteristics": (1) it requires **zero
opt-in** — no `cache_control` field, no request parameter, fully automatic and disk-backed, unlike
Anthropic's explicit breakpoints; (2) its cache-hit discount is substantially larger than either
other provider's (~31x cheaper on a hit vs. a miss for `deepseek-v4-flash`, vs. Anthropic/OpenAI's
~90% (~10x) discount) — verified against DeepSeek's current pricing, not the deprecated
`deepseek-chat`/`deepseek-reasoner` names (already known stale per the `006-ai-plugin-wizard`
research this project's constitution work already flagged); (3) it creates cache units at
specific points (end of user input, end of model output, detected common prefixes, and fixed
token-interval cut points in long content) rather than only at explicit user-placed breakpoints,
which doesn't change what this project needs to do (still: put stable content first) but is worth
recording since it means DeepSeek may cache *more* opportunistically than Anthropic's
explicit-breakpoint-only model without any extra work on LANrurugi's part.

**Alternatives considered**:
- Keeping one-request-per-block (the original §5/§14 design) — superseded per Rationale above;
  not wrong, just a different point on the correctness-simplicity-vs-cost tradeoff that this
  revision moves along after weighing the fixed-overhead cost more heavily.
- Batching an entire look-ahead window (however large the user configures it) into one request,
  rather than a small fixed group — rejected; unbounded batch size means unbounded failure blast
  radius (FR-019/FR-020) and unbounded Usage Budget "pre-spend" risk (FR-013/FR-014) against pages
  the user may never reach, for a diminishing marginal fixed-cost-amortization return past a few
  pages.
- Putting `source_text`/batch content first and glossary/context last (i.e. not reordering
  anything) — rejected; this is the shape that fails to cache at all, since the one thing that's
  actually stable (glossary) would sit after the one thing that changes every request (batch
  content), so no usable prefix would ever repeat.
- Treating DeepSeek as just another instance of the OpenAI-compatible adapter with no dedicated
  documentation — rejected; its caching behavior differs enough (automatic, no opt-in, much larger
  discount) that a future maintainer reading only the OpenAI-compatible adapter's Anthropic-style
  "needs explicit breakpoints" framing would draw the wrong conclusion about what DeepSeek actually
  needs (nothing extra).
- Explicit application-level caching of LLM responses beyond what `Translation Cache Entry`
  (data-model.md) already does — rejected as redundant; `Translation Cache Entry` already caches
  the *final rendered output* per (page, target language, backend), which is a stronger guarantee
  than provider-side prompt caching (zero LLM call at all on a hit, vs. a cheaper-but-still-billed
  call) — provider prompt caching is a cost optimization for the calls `Translation Cache Entry`
  doesn't already avoid (i.e. genuinely new page/language/backend combinations), not a replacement
  for it.
- An **append-only, per-volume translation session** modeled directly on `deepseek-ai/deepseek-
  harness`'s own session architecture (MIT-licensed; cloned locally to `~/deepseek-harness` for
  architecture reference only, same reference-clone handling as `manga-image-translator` in §13) —
  investigated in depth, then rejected once its actual precondition was checked against this
  feature's own design rather than assumed to transfer. `dsh`'s ~99% cache-hit rate
  (`packages/compaction/compaction-basic/src/region.ts`'s own doc comment: each request replays
  "the last routed request's cacheable prefix... so the call is a genuine prefix of the
  conversation and reuses the provider's KV cache") works because a long-running agent session
  keeps calling the LLM repeatedly against a continuously-growing history — exactly the shape
  where an ever-growing, never-reordered prefix pays off, and `dsh`'s `compaction` mechanism exists
  specifically to keep that history from growing unbounded over a long session (summarizing old
  turns, never applicable to translation text anyway, since summarizing would destroy the
  verbatim source/translation correspondence future pages' variant-recognition (FR-007c) and tone
  reference (FR-007e) depend on). But this feature's actual call pattern doesn't have a long-
  running-session shape at all: `Translation Cache Entry`/`Detected Text Region` (data-model.md,
  revised per §16) mean a given page is translated by an LLM call **at most once** — every
  subsequent view of that page, however many times the user re-reads it or across however many
  devices, is served from the persisted result with zero LLM call. There is no repeated,
  long-running sequence of calls against the same volume for an append-only session to amortize
  a prefix over in the first place — the "session" that would exist is, at most, as long as a
  single first-read-through of one volume, after which it simply stops accumulating further calls
  entirely. Introducing session state, history-window truncation, and the associated failure/
  resume semantics purely for a session shape this feature doesn't actually have was judged not
  worth the added complexity — see §14/§15's actual solution to the same underlying goal (name/
  tone consistency): a stable Terminology-Glossary-derived prefix reused across the batch-level
  requests already scoped in §15, without needing a modeled "session" at all.

## 16. Translation result persistence: `Detected Text Region` is authoritative, rendered image is a re-derivable cache

**Decision**: Reversing this document's original position, `Detected Text Region` — including its
`translated_text` and resolved `font`/`fg_color`/`bg_color`/`is_bold` once translated
(data-model.md) — is now the long-term-persisted, authoritative Redis record. `Translation Cache
Entry` (the rendered/composited page image) becomes a re-derivable performance cache: it MAY be
evicted or expired at any time without data loss, regenerated by re-running compositing (not OCR,
not translation) against the already-persisted `Detected Text Region` records for that page.

**Rationale**: Raised directly during spec review as a storage-footprint concern — a rendered page
image (even compressed) is orders of magnitude larger than the text/position/style data needed to
reproduce it, so keeping every translated page's *image* around indefinitely (the original design)
wastes far more space than keeping the underlying text+style data around indefinitely and treating
the image as disposable. This also happens to compose cleanly with the rest of this feature's
design: `Detected Text Region`'s per-region style attributes (FR-008a) and the Terminology
Glossary's translations (FR-007a) are exactly the data compositing needs to reproduce an
identical rendered result, so nothing new needs to be computed to regenerate a cache-evicted page
— only the same server-side (T029) or client-side (T050) compositing step this feature already
implements, run again against data that was never thrown away.

**Alternatives considered**:
- Keeping the original design (rendered image is authoritative, text/style data is disposable) —
  rejected per the storage-footprint concern above; also the less resilient choice, since losing
  the rendered image cache trivially costs one compositing pass to regenerate, while losing the
  text/style data would require re-running OCR and re-paying for a fresh LLM translation call.
- Persisting both the image and the text/style data as equally-authoritative, neither disposable —
  rejected as the worst of both options: still incurs the original image storage cost, with no
  compensating benefit over making the image properly disposable.

## 17. Cross-cutting integration: backup/export and Activity audit log

**Decision**: Two of Phase 1's existing cross-cutting mechanisms are extended to cover this
feature's new Redis-persisted entities, as additive calls into already-shipped Phase 1
infrastructure (not edits to `specs/001-lanrurugi-full-rewrite`'s own spec/plan/tasks — permitted
under constitution Principle VI, which prohibits altering Phase 1's planning documents or gating
Phase 1 delivery, not calling Phase 1's already-implemented code from Phase 2):

1. **Backup/export** (`crates/lanrurugi-backup/src/build.rs`): `Terminology Glossary`,
   `Detected Text Region` (per §16, now the authoritative translation record), and `Volume Font
   Pattern` are added to `build()`'s existing explicit per-entity enumeration (alongside its
   current `ArchiveRepository`/`CategoryRepository`/`GroupingRepository`/`StampRepository`/
   `BookmarksRepository` calls) and to `BackupDocument`'s field set, each with its own
   `to_backup_*` conversion function following the existing per-entity pattern. `Translation Cache
   Entry` and `Usage Budget` are deliberately excluded — the former is a re-derivable rendering
   cache (§16) with nothing lost by omitting it from a backup, and the latter is a
   point-in-time consumption counter, not durable user-authored state (mirroring why Phase 1's own
   backup doesn't capture e.g. in-flight job status).
2. **Activity audit log** (`crates/lanrurugi-storage/src/activity.rs`): a new `translation.*`
   `action_type` namespace records this feature's user-initiated writes — glossary entry
   auto-capture and user edit/delete (FR-007a/d), Volume Font Pattern reset (FR-010), and backend/
   target-language selection changes (FR-002/003/004) — following the same call-a-shared-recording-
   function-at-the-write-site pattern every existing namespace (`archive.*`, `plugin.*`,
   `tankoubon.*`, etc.) already uses, so these actions appear in the existing Activity page
   (`apps/frontend/src/pages/Activity/`) without that page needing feature-specific changes (per
   `activityTarget.ts`'s existing genericism — a new namespace just needs its own entries in that
   file's namespace-order/label/target-link tables, the same onboarding step every prior namespace
   went through).

**Rationale**: This project's constitution Principle I treats user data loss as a correctness bug,
not an inconvenience — a user's accumulated Terminology Glossary (potentially dozens of manually-
corrected character names across a long-running series) and the OCR/translation results
(`Detected Text Region`, now authoritative per §16) represent real, hard-to-reproduce user
investment (the glossary) and real spent cost (paid-backend LLM calls already made) respectively;
omitting them from backup/restore would silently discard both on any restore-from-backup, directly
matching the kind of data-loss risk Principle I already treats as unacceptable for Phase 1's own
entities. Activity coverage follows the same "this project already has a general mechanism for
this concern, extend it rather than inventing a parallel one" reasoning this feature's plan.md
already applies to concurrency (Principle III's rayon/spawn_blocking bridge) and secrets
(Principle V) — a user correcting a wrong glossary entry, for instance, is exactly the kind of
"what changed and when" question Activity already answers for every other mutable entity in this
project.

**Alternatives considered**:
- Leaving Phase 2 data out of backup/export entirely, documenting it as a known limitation —
  rejected; the data in question (glossary corrections, paid translation results) is exactly the
  kind of user investment Principle I's rationale already argues must not be silently lost.
- A separate, Phase-2-only backup mechanism instead of extending Phase 1's existing one — rejected;
  would give users two different backup artifacts/flows to manage for what should be one coherent
  library snapshot, and duplicates infrastructure (snapshot consistency under concurrent
  modification, FR-010 of Phase 1) that `lanrurugi-backup` already solves correctly.
- Skipping Activity integration since it's "just" a nice-to-have visibility feature, not a
  correctness one — rejected; the cost of following the existing per-write-site call pattern is
  small and consistent with how every other Phase 1 feature already integrated with Activity, and
  skipping it would make this feature's mutations the one silent, unaudited exception in an
  otherwise fully-covered system.

## 18. Server-side rendered-image cache quota: shares the reader's resize cache, not the thumbnail cache

**Decision**: The server-composited `Translation Cache Entry` image cache (data-model.md, the
re-derivable rendering cache per §16) shares the existing reader page **resize cache**'s quota
(`tempmaxsize`, a Redis-`LRR_CONFIG`-stored, user-configurable setting — default 500MB — enforced
by a periodic sweep that deletes oldest-`modified`-first once the total exceeds it,
`crates/lanrurugi-api/src/download_manager/ingest.rs`/`crates/lanrurugi-server/src/main.rs`), not
the thumbnail cache — no new setting is introduced.

**Rationale**: This document's earlier wording (§6, before this correction) called the server-side
translated-image cache a sibling of "Phase 1's existing thumbnail cache," without having actually
verified what that mechanism is or whether it has a quota at all. Checked directly against the
running code, not assumed: Phase 1 has **two** separate, unrelated disk-cache mechanisms, not one
generic "image cache" concept —
- **Thumbnail cache** (`state.library.thumb_dir`, generated by `lanrurugi-scanner/src/
  thumbnail.rs`): permanent generated content (a cover/page thumbnail is generated once and kept
  indefinitely, overwritten only by an explicit regen). No sweep, no eviction, no size quota exists
  for it anywhere in the codebase — it is conceptually closer to "part of the archive's own
  metadata" than to a cache with a hit/miss/eviction lifecycle.
- **Reader resize cache** (`<temp_dir>/resize_page/...`, WebP-resized page images the reader
  requests on demand): explicitly quota-managed and evictable — governed by `tempmaxsize`, swept
  every 15 minutes, oldest-first eviction once over budget.
`Translation Cache Entry`'s server-side image is the correct match for the **second** case, not
the first: per §16, it is explicitly re-derivable at any time (re-run compositing against the
persisted `Detected Text Region`), the exact same "safe to evict, regenerate on next miss"
property the resize cache already has and the thumbnail cache does not. Reusing `tempmaxsize`
rather than introducing a dedicated new quota setting also avoids asking a user to reason about
and configure two separate "how much disk space can ephemeral page-rendering cache use" numbers
for what is, from the user's perspective, the same kind of thing (temporary, regenerable,
page-rendering-related disk usage) — one budget, one sweep mechanism, one setting to understand.

**Alternatives considered**:
- Treating it as a thumbnail-cache sibling, per this document's own original (now-corrected)
  wording — rejected once actually verified; thumbnails have no quota/eviction lifecycle at all,
  so there was nothing to actually "share" in the first place, making the original wording not
  just imprecise but not implementable as stated.
- A dedicated new `translation_cache_max_size_mb`-style setting, quota-managed independently of
  `tempmaxsize` — rejected; both caches hold the same *kind* of thing from a user's perspective
  (disposable, regenerable, page-image-shaped disk usage attached to reading), so splitting them
  into two independently-sized budgets adds a setting for a distinction a user has no real reason
  to care about, without a corresponding benefit (no scenario was identified where a user would
  want translation image cache to have a meaningfully different size policy from resize cache).
- Making `Translation Cache Entry` unbounded/unmanaged (no quota at all) — rejected; unlike the
  thumbnail cache's one-per-page-ever footprint, a translated page's cache entry is keyed by
  (page, target language, backend) per data-model.md, so switching target language or backend
  repeatedly could accumulate multiple cached renders per page — an unbounded-growth risk the
  thumbnail cache's simpler "one thumbnail per page" shape doesn't share, making some quota
  mechanism actually necessary here, not just a nice-to-have consistency choice.
