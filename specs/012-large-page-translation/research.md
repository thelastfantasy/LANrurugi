# Phase 0 Research: Translation and Erase Support for Pages Beyond the Worker Payload Limit

## §1 What actually fails, and where

**Finding.** The whole-page erase request carries `(page, model_mask, paste_mask)` at the source
page's **full resolution**. The reduction to the model's working resolution is real, but it is
performed *inside the worker*, four steps after the payload has already crossed the boundary.

**Evidence.**

- `PageErasePlan::erase_request` returns `(&self.pristine, &self.page_model_mask,
  &self.page_paste_mask)`, and `pristine` is `page.clone()` — the full-resolution decoded page
  (`crates/lanrurugi-translate/src/composite.rs`).
- The caller gets that page from `load_page_image`, which decodes and returns it without any
  resizing (`crates/lanrurugi-api/src/translation_pipeline.rs`).
- `Inpainter::erase_page` computes `(work_w, work_h)` and resizes, but only *after* its arguments
  exist in the worker process (`crates/lanrurugi-inpaint/src/lib.rs`).
- The boundary's limit is enforced by `tokio-util`'s `LengthDelimitedCodec`, which **rejects an
  over-limit frame on the encode side** — the message is never sent. `tarpc` surfaces this as a
  transport error, which the client turns into a discarded session, which the call site turns into
  `.ok()` plus a warning.

**Consequence.** The failure is silent from the user's point of view: the erase "failed", and the
existing, deliberate policy for a failed erase is to leave the page untranslated rather than draw
over un-erased lettering. The policy is correct; the *cause* here is an artificial transport limit,
not a model failure.

**Payload arithmetic.** `RawRgbImage` is `4 + 4 + 8 + 3wh` bytes; each `RawMask` is `4 + 4 + 8 + wh`.
The erase request is therefore `48 + 5wh` bytes plus the RPC envelope. Surveying 209 real archives
(three pages each):

| Threshold | Pages affected | Archives affected |
| --- | --- | --- |
| `48 + 5wh` exceeds the limit (~13.4 MP) | 17 | 8.1% |
| page image alone exceeds it (~22.4 MP) | 12 | 5.7% |

Worst observed: 6336x9504 (60.2 MP) → ~287 MiB request, ~4.5x the limit.

## §2 Decision: reduce on the client, before the transfer

**Decision.** When, and only when, the full-resolution erase request cannot be proven to fit within
the boundary's limit, the client reduces the page and the model mask to the working resolution
itself, sends that, and restores the model's output to the page's original dimensions for a
full-resolution masked composite.

**Rationale.** It removes the entire class of failure without touching the model, the worker, the
protocol, or the working resolution. It also removes the oversized allocation from the long-lived
server process instead of merely making it acceptable.

**Alternatives considered.**

| Alternative | Rejected because |
| --- | --- |
| Raise the boundary's message limit | Only moves the wall. The 60.2 MP page would need ~287 MiB per in-flight call, and the limit exists to bound exactly that. It also does nothing about the main process assembling the message. |
| Reduce in the worker, but stream the page in chunks | Requires a new framed protocol for partial images and reassembly in the worker — a large protocol change to fix a case where the reduction discards ~99% of the transferred data anyway. |
| Shared memory between the two processes | Would make the transfer cheap but not bounded: the container's `/dev/shm` is 64 MiB by default, *smaller* than the existing limit. It also conflicts with this worker's design, where the process is deliberately terminated abruptly to reclaim VRAM — a shared segment owned by a process that gets `SIGKILL`ed needs leak/reconstruction handling that the socket transport does not. |
| Send the page already reduced, always (not only when required) | Would change the produced image for pages that work today, violating the pixel-identity requirement. Rejected in favour of confining the new path to pages that currently fail (§3). |
| Reduce, then let the worker paste at working resolution and upscale | The worker would blend model output with the *working-resolution* page before upscaling, so pasted pixels at the mask boundary would mix in working-resolution original pixels. Reducing and then compositing at full resolution with the full-resolution mask is strictly cleaner and keeps the "only paste-mask pixels ever change" invariant exact. |

## §3 Decision: the new path is taken only when the old one cannot be proven to work

**Decision.** The existing full-resolution path stays the default. The reduced path is used only when
the full-resolution request's size cannot be shown to fit.

**Rationale.** This is what makes the "no visible change for anything that works today" requirement
hold *by construction*: pages that currently succeed never execute a single new line, so their output
is identical for the trivial reason that the code producing it is the same code. The alternative —
arguing that a new path happens to produce the same pixels — is a much weaker guarantee, and this
project has already been burned once by treating a plausible equivalence as a verified one.

**The size predicate.** The request size is `48 + 5wh + envelope`. The envelope is small but not
knowable without serialising, so the plan uses a documented conservative slack:

- *Provably fits*: `48 + 5wh + SLACK <= limit` → take the existing path.
- *Provably does not fit*: `48 + 5wh > limit` → take the reduced path.
- *Ambiguous band*: narrower than `SLACK` bytes (a few tens of pixels of page area). The reduced path
  is taken, so an ambiguous page still **works**; the alternative would be to risk the hard failure
  this feature exists to remove.

The `48 + 5wh` term is pinned to reality by a test that serialises the same three arguments with the
transport's own serialization library and asserts the arithmetic matches, so the formula cannot drift
away from the wire format unnoticed.

## §4 Decision: obtain the model's working-resolution output without changing the worker

**Decision.** In the reduced path, the client sends an **all-covering paste mask** and performs the
real masked composite itself at full resolution.

**Rationale.** `Inpainter::erase_page` starts from `page.clone()` and writes only where the paste mask
is set, so an all-covering mask asks it for precisely "the model's output over the whole working
image" — a legitimate use of the existing, documented meaning of that mask, not a trick. This keeps
the worker, the RPC trait, and the protocol **entirely unchanged**, which is the single largest
reduction in blast radius available here.

**Verified consequence of sending an already-reduced page.** `Inpainter::erase_page` reduces only when
`longest_edge > MAX_INPAINT_SIZE`. The shared dimension function guarantees the reduced page's longest
edge equals the working resolution *exactly* (the dimension that was the longest edge rounds to the
constant itself), so the worker's own reduction does not run, and its final `resize(..., pw, ph)`
becomes a no-op. The worker therefore returns exactly the model's working-resolution output — which is
what the client needs. This is asserted by a test rather than left as an argument.

**How the client asks for it.** With an all-covering mask the worker's per-pixel paste loop rewrites
every pixel with the freshly resized output; it never reads a pixel of the working-resolution input
page. That is why the client's own upscale-and-composite is the only place original pixels are
combined with model output, and why the full-resolution paste mask stays authoritative.

**A stronger property than expected, confirmed while implementing.** Because the client reduces with
the *same* shared implementation the worker would have used, the model's input is bit-identical to
what it would have been had the full-resolution page crossed the boundary successfully: the same
resampling filter on the same source bytes gives the same working-resolution page, and the mask
survives the round trip exactly (nearest-neighbour selection on a two-level mask can only pick a value
that was already there, so reducing to booleans and re-materialising the two-level image is lossless).
The reduced path therefore does not trade away any model quality for the pages it rescues.

What it does change is the *composite*: today the worker pastes at working resolution and then
upscales; the reduced path upscales the model's output and pastes at the full-resolution mask. Those
differ only at mask boundaries. That difference is unreachable for any page that works today, because
those pages take the old path — which is exactly why the split-path design was chosen over applying
the reduction unconditionally.

## §5 Decision: one implementation of the reduction rule, shared by both sides

**Decision.** The working-resolution constant and the reduction rule become public API of
`lanrurugi-inpaint`, computed by a single function; `lanrurugi-api`'s client calls it. The worker's
`erase_page` calls the same function.

**Rationale.** FR-006 requires a single derivation. If the client and worker each computed the reduced
dimensions separately, a divergence would not raise an error — it would silently hand the model a
differently-shaped page and mask, producing a wrong result with no diagnostic. `lanrurugi-api` already
depends on `lanrurugi-inpaint`, so sharing costs no new dependency and no new crate.

**Alternatives considered.** Putting the geometry in the protocol crate (`lanrurugi-gpu-ipc`) would
force that crate to depend on `image` and on the model's resolution policy, which its own documentation
explicitly avoids (it deliberately depends on neither `lanrurugi-ocr` nor `lanrurugi-inpaint`).
Duplicating the constant in both crates with a cross-crate assertion test is a weaker form of the same
guarantee and was rejected in favour of there being nothing to assert.

## §6 Decision: `segment_bubbles` is out of scope

**Decision.** Only the erase call is changed. Bubble detection keeps its current behaviour, including
its failure above its own higher limit.

**Rationale.** Its payload is the page image alone, so its effective limit is higher (22.4 MP rather
than 13.4 MP), and its failure mode is an *already accepted* degradation — rectangular erasure boxes —
rather than a total loss of the page's translation. Bounding it would require reducing the page and
then mapping the returned bubbles' bounding boxes **and per-bubble masks** back into the original
frame. That mapping cannot be made exactly equivalent to today's output (the detector letterboxes into
a fixed square, and re-deriving the same content dimensions from an already-reduced page is not
guaranteed to reproduce the original integers), so it would break the pixel-identity requirement for
pages that currently succeed.

**Consequence, stated plainly.** Pages above ~22.4 MP keep rectangular erasure boxes. They already do
today, and this feature strictly improves them (their translation starts working at all). This is
recorded as a known limitation, not silently dropped; a follow-up feature may address it.

## §7 Measured memory effect, and the limit of it

Both paths are modelled at the worst surveyed page (6336x9504, 60.2 MP, 181 MiB RGB):

| Path | Main-process buffers attributable to this call | Boundary message |
| --- | --- | --- |
| Existing (today, for a fitting page of the same size) | page clone 181 MiB + two masks 120 MiB + serialized request ~287 MiB + returned page 181 MiB | ~287 MiB out, ~181 MiB back |
| Reduced (this feature) | reduced page ~5 MiB + reduced mask ~1.5 MiB + returned reduced page ~5 MiB + upscaled composite ~181 MiB | under 8 MiB out, ~5 MiB back |

The composite buffer is unavoidable: the caller needs a full-resolution result. It is written into the
page buffer the client already owns rather than into a fresh clone, so the change also removes one
full-page copy relative to today. Peak resident memory therefore improves by roughly the size of the
eliminated message and masks — a large absolute reduction, but **not** an order of magnitude of total
process memory, because a full-resolution page necessarily exists in the process either way. The
specification's success criterion is worded to match this, not to overclaim.

**Not attempted.** Pasting directly out of the resampling kernel would avoid the temporary upscaled
buffer (~181 MiB) but requires hand-writing the resampling filter, replacing a well-tested library
call with custom numeric code in a quality-critical path. Rejected: the memory it saves is real but
this feature does not need it, and the risk is not commensurate.

## §8 Why the transport itself is not changed

Recorded because it was the original question this work started from. The existing transport is a Unix
domain socket with length-delimited framing, and the alternatives evaluated were:

- **ByteDance `shmipc`** — no maintained Rust binding exists, so adopting it means porting the protocol
  (ring buffer, event notification, segment lifetime) into this codebase and owning it.
- **`iceoryx2`** — a mature Rust shared-memory library, but a publish/subscribe paradigm for
  fixed-rate streams, not request/response. Adopting it means re-implementing per-call deadlines,
  cancellation, and session recovery that the current transport provides, and reintroducing exactly the
  three gaps an earlier hand-rolled version of this protocol was reviewed and found to have.

Both would also have made this defect *worse*, not better: the payload that cannot fit today is
287 MiB, and the container's `/dev/shm` is 64 MiB.

Once the page is reduced before the transfer, the largest message this boundary ever carries is under
8 MiB — at which point the transport costs a fraction of a millisecond and no transport change has any
headroom to recover. The half of the original question that asked whether a different IPC library
would help is answered by the fix itself.
