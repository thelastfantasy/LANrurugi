# Feature Specification: Translation and Erase Support for Pages Beyond the Worker Payload Limit

**Feature Branch**: `012-large-page-translation`

**Created**: 2026-10-13

**Status**: Draft

**Input**: User description: "确认放弃换 IPC 库（UDS 已在用、shmipc 无 Rust 生态、iceoryx2 范式不匹配且与 worker 被 SIGKILL 回收的设计冲突）与 IPC 微优化；改为修复测量中发现的真实缺陷——发给 GPU worker 的请求超出 64MiB 帧上限，导致大尺寸页静默无法翻译。要求：不能有任何负优化，不能造成现有逻辑中功能特性缺失。"

## Context

This feature exists because a measurement pass over the project's own library found that page
translation **silently fails for large pages**. The GPU worker boundary carries a fixed maximum
message size. The page image crossing that boundary is the **full-resolution** page, because the
reduction to the model's working resolution happens *inside the worker, after the transfer*. For
any page large enough, the request is rejected before it is ever sent, and because every call site
treats a failed erase as "leave this page untranslated", the user sees nothing happen.

Measured against 209 archives in a real library (first three pages of each):

| Boundary call | Fails when the page exceeds | Archives affected |
| --- | --- | --- |
| Whole-page erase | ~13.4 megapixels | 17 (8.1%) |
| Bubble detection | ~22.4 megapixels | 12 (5.7%) |

The worst observed page (6336 x 9504, 60.2 MP) produces a request about 4.5x the permitted size.
One archive in the sample has every page at 4535 x 6307, so the entire volume is untranslatable
rather than a single page.

The same defect also means the **whole request is materialised in the main server process** before
being refused, so an oversized page temporarily allocates hundreds of megabytes in the process that
serves every other request.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Any page translates, regardless of its pixel dimensions (Priority: P1)

A reader opens a high-resolution archive, requests translation for a page, and gets a translated
page — the same as they would for an ordinary-resolution archive.

**Why this priority**: This is the actual feature loss. Today ~8% of a real library's archives
cannot be translated at all, and the failure is silent apart from a server log line. Everything else
in this feature is secondary to this.

**Independent Test**: Translate the worst-case page found in the survey (6336 x 9504) and confirm a
translated page is produced, with the translation drawn on an erased backdrop rather than on
untouched original lettering.

**Acceptance Scenarios**:

1. **Given** an archive whose pages are 6336 x 9504, **When** a reader requests translation for one
   of its pages, **Then** the page is translated and the result is served.
2. **Given** an archive whose pages are 4535 x 6307 (every page of the volume), **When** a reader
   requests translation for several of its pages, **Then** each one is translated, not just the
   first.
3. **Given** any page size at all, **When** translation is requested, **Then** the outcome does not
   depend on the page's pixel dimensions being below some undocumented threshold.

---

### User Story 2 - Translations that already work do not change at all (Priority: P1)

An archive that translates correctly today continues to translate to exactly the same result, pixel
for pixel.

**Why this priority**: Equal to US1. The change touches the boundary that carries page imagery for
*every* translation, so an unverified "it should be equivalent" is how a working pipeline gets
quietly degraded. Under this project's standing constraint that no change may be a negative
optimisation or cost an existing capability, this story is a hard gate on US1 rather than a nicety.

**Independent Test**: Translate a representative set of pages that succeed before the change, capture
the outputs, and compare them pixel for pixel against the outputs after the change.

**Acceptance Scenarios**:

1. **Given** any page that translated successfully before the change, **When** it is translated
   after the change, **Then** the produced image is identical, pixel for pixel.
2. **Given** a page whose working resolution requires no reduction (already small enough for the
   model), **When** it is translated, **Then** no image data is resampled that was not resampled
   before.
3. **Given** the automated test suite as it stands before the change, **When** it is run after the
   change, **Then** it passes with no changes to any expected output.

---

### User Story 3 - An oversized transfer is reported as a size problem, not an unexplained failure (Priority: P2)

An operator seeing a translation fail can tell from the logs alone that the cause was a message
exceeding the boundary's limit, and what the size and the limit actually were.

**Why this priority**: The reason this defect survived is that its symptom was indistinguishable
from a legitimate model failure. Diagnosability is what stops the next instance of it.

**Independent Test**: Force a payload past the limit and read the resulting log/error output; it
must name both the actual size and the permitted limit without any further investigation.

**Acceptance Scenarios**:

1. **Given** a request that cannot be delivered within the limit even after reduction, **When** it is
   attempted, **Then** the recorded error names the actual payload size and the permitted limit.
2. **Given** a genuine model-side failure (no model loaded), **When** it occurs, **Then** it is still
   distinguishable from a size-limit failure.

---

### User Story 4 - Serving a translation does not balloon the main process's memory (Priority: P3)

Translating a very large page does not make the process that serves all other requests allocate
memory proportional to that page's size.

**Why this priority**: A real operational standard for this project is that the main server process
stays within a modest resident-memory budget. Today an oversized page allocates hundreds of megabytes
in that process to build a message that is then refused.

**Independent Test**: Measure the main process's peak resident memory while translating the largest
page in the survey, before and after the change.

**Acceptance Scenarios**:

1. **Given** the largest page in the survey, **When** it is translated, **Then** the data the main
   process assembles for the worker boundary is bounded by a constant rather than by the page's
   dimensions.

---

### Edge Cases

- A page reduced by a non-integer factor, where the reduced dimensions must be derived identically on
  both sides of the boundary — any disagreement produces a wrong-sized image or mask.
- A page whose longest edge equals the working resolution exactly (must take the unreduced path, since
  "reduce only if strictly larger" is the existing rule).
- A page already smaller than the working resolution (must not be resampled at all — no upscaling).
- A page with an extreme aspect ratio, where one reduced dimension rounds to 1.
- An archive whose pages have *different* dimensions (observed: 2933x4000, 9918x4000, 9899x4000 in one
  volume) — each page must be judged on its own dimensions, not on the volume's first page.
- A page with no model mask set at all (nothing to erase) — the existing short-circuit that skips the
  boundary call entirely must still apply.
- A page whose reduction succeeds but whose result, once returned, must be mapped back onto the
  original full-resolution page using the original full-resolution mask.
- A page large enough that the *response* would also exceed the limit, not only the request.
- Bubble detection on a page above its own (separate, higher) limit — see Assumptions; this remains a
  pre-existing degradation and is explicitly not changed by this feature.
- A worker cold start coinciding with a first large-page request.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST translate a page regardless of the page's pixel dimensions. No page may
  fail to translate for the sole reason that its dimensions exceed a boundary's message limit.

- **FR-002**: The volume of page-image data crossing the worker boundary MUST NOT grow in proportion
  to the source page's pixel dimensions. It MUST be bounded by a constant.

- **FR-003**: The volume of mask data crossing the worker boundary MUST likewise be bounded by that
  same constant.

- **FR-004**: The full-resolution paste mask MUST remain on the originating side of the boundary, so
  that the final composite can be performed at full resolution.

- **FR-005**: When a page would otherwise cross the boundary at its full resolution and that transfer
  would exceed the limit, the system MUST reduce the page and its model mask to the model's own
  working resolution before the transfer, and MUST restore the result to the page's original
  dimensions afterwards.

- **FR-006**: The working resolution used for that reduction MUST be derived by the same single
  implementation on both sides of the boundary. Duplicating the derivation in two places is
  prohibited, because a divergence between the two would silently produce a wrong-sized image or mask
  rather than an error.

- **FR-007**: The reduction applied before the transfer MUST use the same resampling filter for the
  image and the same resampling filter for the mask as the pipeline already uses for that reduction
  today, including the same treatment of mask values as a two-level quantity.

- **FR-008**: For a page whose full-resolution transfer already fits within the limit, the system MUST
  take the existing path unchanged, so that the produced image is identical, pixel for pixel, to the
  image produced before this feature.

- **FR-009**: For a page whose dimensions are already at or below the working resolution, the system
  MUST NOT resample the page at all.

- **FR-010**: Outside the pixels the paste mask covers, the resulting page MUST consist of the
  original page's own pixels. The system MUST NOT take any pixel from the model's output outside that
  mask.

- **FR-011**: After restoring a reduced result to the page's original dimensions, the composite MUST
  be applied using the original full-resolution paste mask, not a reduced or restored approximation
  of it.

- **FR-011a**: Producing the full-resolution result MUST NOT require an additional full-resolution copy
  of the page beyond the one the boundary call already owns. A full-resolution page necessarily exists
  in the process because the caller needs one; the feature MUST NOT increase how many times it is
  duplicated.

- **FR-012**: A page with no model mask set MUST continue to skip the boundary call entirely, exactly
  as it does today.

- **FR-013**: The system MUST detect, before transmitting, that a message would exceed the boundary's
  limit, and MUST report that condition with an error naming the actual size and the permitted limit.

- **FR-014**: A size-limit failure MUST remain distinguishable in the logs from a model-side failure,
  and MUST NOT be reported as a successful translation.

- **FR-015**: When a boundary call fails for any reason, the existing behaviour MUST be preserved: the
  page is left with its original lettering and no translation is drawn over un-erased text.

- **FR-016**: A page whose full-resolution transfer would exceed the limit but whose reduced transfer
  succeeds MUST NOT be reported as a failure at all.

- **FR-017**: The feature MUST NOT change the project's external HTTP API surface.

- **FR-018**: The feature MUST NOT change the shape or keying of any persisted data.

- **FR-019**: The feature MUST NOT increase the number of boundary calls made per page. The existing
  one-bubble-detection-call and one-erase-call-per-page structure MUST be preserved, since an earlier
  incident in this project was caused precisely by two such calls contending for the same device.

- **FR-020**: The feature MUST NOT change the model's working-resolution values themselves. What the
  model is asked to process is out of scope; only where the reduction happens is in scope.

- **FR-021**: The feature MUST NOT alter the behaviour of bubble detection. Pages above that call's
  own (higher) limit continue to degrade to rectangular erasure boxes exactly as they do today; this
  feature deliberately does not attempt to change it (see Assumptions).

### Key Entities

- **Page**: A decoded image from an archive, at its original resolution. Its dimensions are the sole
  input that determines whether reduction is needed.
- **Working resolution**: The fixed upper bound on the longest edge that the erase model is asked to
  process. Existing and unchanged; this feature only moves where the page is reduced to it.
- **Model mask**: The per-pixel set of page locations the model is asked to reconstruct. Reduced
  alongside the page when a reduction is needed.
- **Paste mask**: The per-pixel set of locations whose final value is taken from the model's output.
  Never leaves the originating side of the boundary.
- **Boundary message**: One request or response crossing to the worker process. Has a fixed maximum
  size that no page may be allowed to exceed.
- **Working-page transfer**: A boundary message carrying an already-reduced page. Replaces the
  full-resolution transfer only when the full-resolution transfer would not fit.

No new persisted entity is introduced: nothing about this feature is stored across restarts.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every page in the survey sample that previously failed to translate now translates —
  measured against the recorded baseline of 17 of 209 archives (8.1%).

- **SC-002**: Of the pages in that sample, zero fail for a reason attributable to a message
  exceeding the boundary's limit.

- **SC-003**: For every page that translated before the change, the produced image is identical,
  pixel for pixel, to its pre-change output. This is verified by automated comparison over the same
  sample, not by inspection.

- **SC-004**: The volume crossing the worker boundary for the largest surveyed page drops by at least
  an order of magnitude, and no longer varies with the page's pixel dimensions in the failing range.

- **SC-005**: The data the main server process assembles for the worker boundary while translating the
  largest surveyed page drops by at least an order of magnitude — from roughly the size of the page's
  full-resolution image plus two full-resolution masks, to a constant — and the number of times a
  full-resolution page is duplicated in that process does not increase.

- **SC-006**: A size-limit failure is identifiable from its own log output, without reproducing it:
  the log names both the attempted size and the permitted limit.

- **SC-007**: The project's full automated test suite passes with no change to any expected output
  other than newly added cases.

- **SC-008**: The number of boundary calls made while translating one page is unchanged from the
  pre-change measurement.

## Assumptions

- **Bubble detection's higher limit is deliberately out of scope.** That call's payload is the page
  image alone, so its effective limit is higher, and its failure mode is an *already accepted*
  degradation (rectangular erasure boxes) rather than a total loss. Its result geometry is expressed
  in the page's own coordinate frame and includes per-bubble masks, so bounding it would require
  remapping coordinates and masks back to the original frame — work that cannot be made exactly
  equivalent to today's output and therefore cannot satisfy US2. It is recorded here as a known,
  pre-existing limitation that this feature neither improves nor worsens. A follow-up feature may
  address it.

- **The model's working resolution values are intentional** and represent a deliberate
  quality-versus-cost trade-off made by the existing implementation. This feature does not revisit
  them.

- **"Identical, pixel for pixel" is achievable for pages that currently succeed by construction**,
  because those pages take the existing code path unchanged. The feature does not rely on an argument
  that a new path happens to produce the same result; it relies on the new path not being taken for
  those pages at all.

- **Reduced-path output for currently-failing pages has no prior result to compare against.** Those
  pages produced nothing before, so their success is measured by "a translated page now exists", not
  by pixel comparison. What *is* established is that the model receives bit-identical input to what it
  would have received had the transfer succeeded, because both sides reduce by the same shared rule;
  only the final composite differs, and only at mask boundaries.

- **The boundary limit is a transport-level safety bound** and is not raised by this feature. The fix
  is to stop producing messages that exceed it, not to permit larger ones.

- **Real archive images from the operator's own library are used as test inputs**, referenced through
  the project's existing environment-variable convention for real content; no real title, handle, or
  URL is committed to source.

- **Model weights, model behaviour, and graphics-driver behaviour are unchanged** by this feature.

- **The worker's process lifecycle is unchanged**: it is still started on demand, still reclaims its
  resources when idle, and still may be terminated abruptly to release them.

- **No legacy-compatibility surface is involved.** Page translation is a Phase 2 capability with no
  counterpart in the legacy system, so neither the external API contract nor stored-data
  compatibility is affected.
