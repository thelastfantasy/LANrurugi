# Contract: GPU Worker Boundary (erase call)

**Status**: modified by feature `012-large-page-translation`
**Crate**: `lanrurugi-gpu-ipc`
**Consumer**: `lanrurugi-api::gpu_worker_client`, `lanrurugi-gpu-worker`

This is a private contract between two binaries shipped in the same image. It is not a user-facing
API and no external compatibility obligation attaches to it. It is documented here because the
feature's correctness depends on precisely what may cross it.

## Existing guarantees (unchanged)

- **G-1** The boundary carries one request and one response per erase call. A response corresponds to
  its own request; a cancelled or failed call never leaves the connection in an ambiguous state.
- **G-2** Every call carries a deadline. A call that exceeds it fails rather than blocking the caller.
- **G-3** A message exceeding `MAX_FRAME_BYTES` is rejected by the transport. On the encode side this
  happens before any byte is written.
- **G-4** A transport-level failure discards the session, so the next call starts from a fresh worker
  rather than reusing a connection whose stream state is unknown.

## New guarantees (this feature)

- **G-5 (bounded payload)** For any page whose longest edge exceeds the working resolution, the data
  the originating process assembles for the erase boundary is bounded by a constant and does not grow
  with the page's pixel dimensions.
- **G-6 (no construction of an over-limit message)** The originating process MUST decide in advance
  whether a message will fit, and MUST NOT construct one that will not. Relying on the transport to
  reject it is explicitly insufficient: by then the message's bytes have already been allocated and
  serialized.
- **G-7 (diagnosable rejection)** When a message cannot be brought within the limit, the resulting
  error MUST name both the attempted size and the limit, and MUST be distinguishable from a
  model-side failure.
- **G-8 (unchanged call count)** The number of erase boundary calls per page is unchanged: exactly one
  when there is something to erase, none when there is not.
- **G-9 (response also bounded)** In the reduced path the response is bounded by the same constant as
  the request. The response MUST NOT be assumed small merely because the request was.

## Size arithmetic

Serialized argument sizes for an erase request, at page dimensions `w` x `h`:

| Value | Bytes |
| --- | --- |
| page image | `4 + 4 + 8 + 3wh` |
| model mask | `4 + 4 + 8 + wh` |
| paste mask | `4 + 4 + 8 + wh` |
| **arguments total** | **`48 + 5wh`** |

The `48 + 5wh` term is normative and is pinned by a test that serializes the same three values with
`bincode` — the exact library and default configuration `tarpc`'s bincode transport uses. A test that
merely re-states the arithmetic in the implementation would verify nothing.

The RPC envelope (call context, request identity, method selector) adds a small, `w`/`h`-independent
amount. This contract defines:

```text
ERASE_REQUEST_FRAMING_SLACK = 1024 bytes   // generous, not measured tight
erase_request_bytes(w, h)   = 48 + 5*w*h + ERASE_REQUEST_FRAMING_SLACK
```

The slack is deliberately generous rather than tight for the same reason `MAX_FRAME_BYTES` itself
documents: the cost of over-estimating is that a page marginally inside the limit takes the reduced
path (and still produces a correct result), while the cost of under-estimating is the silent failure
this feature exists to remove. Those two costs are not symmetric.

### Decision predicate

```text
erase_request_provably_fits(w, h) = erase_request_bytes(w, h) <= MAX_FRAME_BYTES
```

The originating process takes the existing full-resolution path **only** when this predicate is true.
Otherwise it takes the reduced path, or fails with a diagnosable error if reduction cannot bound the
message.

### Band width

The predicate can be conservative by at most `ERASE_REQUEST_FRAMING_SLACK` bytes, i.e. an area of
`1024 / 5 ≈ 205` page pixels — a patch roughly 14 x 14. The number of real pages that fall in this
band is negligible, and a page in the band is routed to the reduced path, which **works**, rather than
to the full-resolution path, which might not. The requirement that already-working pages be unchanged
is therefore stated against the predicate (`provably fits`), which is knowable in advance, not against
the unknowable exact serialized length.

## Invariants the reduced path must hold

- **R-1** The reduced page's longest edge equals the working resolution exactly, so the worker's own
  reduction does not execute and its final restore is a no-op.
- **R-2** The reduced model mask uses the same reduction filter as the worker would have used, and
  carries no location that did not exist in the original mask.
- **R-3** The full-resolution paste mask never crosses the boundary.
- **R-4** The response is restored to the page's original dimensions before any composite is applied.
- **R-5** The final composite writes only pixels the full-resolution paste mask covers, into a buffer
  initialized from the original page. Consequently the returned page equals its input outside that
  mask — the invariant `lanrurugi-translate`'s composite step already depends on.

## Out of scope

- The **bubble-detection call** keeps its current behaviour at every page size, including its failure
  above its own higher limit. See research.md §6 for why reducing it cannot satisfy R-5's equivalent.
- **`MAX_FRAME_BYTES` itself** is not changed, and the working resolution is not changed.
