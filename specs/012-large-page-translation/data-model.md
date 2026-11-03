# Phase 1 Data Model: Translation and Erase Support for Pages Beyond the Worker Payload Limit

No persisted entity is introduced, changed, or migrated. Everything below is in-flight data that
exists only for the duration of one page's erase call. This section exists because the feature's
correctness depends on the relationships between these in-flight values, particularly which
resolution each one lives at.

## Entities

### Page

A decoded image from an archive, at the resolution the archive stores it.

| Attribute | Meaning |
| --- | --- |
| `width`, `height` | The page's own pixel dimensions. The sole input that determines whether reduction is needed. |
| `rgb` | Row-major 3-bytes-per-pixel image data. |

**Validation rules**

- `rgb.len()` must equal `width * height * 3`. A mismatch is a hard error, never a silent resize.
- `width` and `height` are each at least 1.

**Notes**

- The page is the *only* entity whose size varies with the archive's content. Every other entity in
  this model is either bounded by a constant or proportional to a bounded one.

### Working resolution

A fixed upper bound on the longest edge of the image the erase model is asked to process. Existing
and unchanged by this feature.

| Attribute | Meaning |
| --- | --- |
| value | The longest edge, in pixels, that the model is asked to process. |
| derived dimensions | For a given page, `(work_w, work_h)`: the page's dimensions scaled so the longest edge equals the working resolution, both rounded to integers and floored at 1; or the page's own dimensions when the page is already at or below it. |

**Validation rules**

- Reduction happens if and only if the page's longest edge is *strictly greater* than the working
  resolution. A page exactly at the resolution is not reduced.
- The dimension that was the page's longest edge MUST reduce to exactly the working resolution. This
  is load-bearing: it is what makes the already-reduced page pass unchanged through the worker's own
  reduction step (§4 of research.md).
- Both derived dimensions MUST be at least 1, including for extreme aspect ratios where one rounds to 1.
- Neither derived dimension may exceed the corresponding page dimension.

### Model mask

One boolean per page pixel: whether the model is asked to reconstruct that location.

| Attribute | Meaning |
| --- | --- |
| `width`, `height` | The dimensions of the page this mask describes. |
| `mask` | `width * height` booleans, row-major. |

**Validation rules**

- `mask.len()` must equal `width * height`, and those dimensions must match the page's. A mismatch is a
  hard error.
- When the page is reduced before a transfer, this mask MUST be reduced to the same derived dimensions
  as the page, by nearest-neighbour selection, so that no location is invented or averaged into
  existence.

### Paste mask

One boolean per page pixel: whether that location's final value comes from the model's output rather
than from the page itself.

| Attribute | Meaning |
| --- | --- |
| `width`, `height` | The dimensions of the page this mask describes. |
| `mask` | `width * height` booleans, row-major. |

**Validation rules**

- Same length and dimension rules as the model mask.
- This mask MUST be at the page's **original** resolution at the moment the composite is applied,
  regardless of what resolution the page was transferred at. It is never reduced for transfer and
  never restored from a reduced copy.
- A paste mask with no set bits means nothing is to be erased. When combined with a model mask with no
  set bits, the boundary call is skipped entirely.

### Boundary message

One request or response crossing to the worker process.

| Attribute | Meaning |
| --- | --- |
| payload | The serialized page, and for an erase request the two masks. |
| size | Total serialized bytes, including field framing and the request envelope. |
| limit | The fixed maximum size a message may have. Existing and unchanged. |

**Validation rules**

- A message whose size exceeds the limit MUST NOT be transmitted. This is already enforced by the
  transport; the feature's requirement is to not *construct* one.
- The decision to reduce MUST be made from the message's size, before any transmission is attempted.
- When a message cannot be brought within the limit, the resulting error MUST name both the attempted
  size and the limit.

**Erase request size.** `48 + 5wh` bytes, plus the RPC envelope:

- page: 4 (width) + 4 (height) + 8 (length prefix) + `3wh`
- model mask: 4 + 4 + 8 + `wh`
- paste mask: 4 + 4 + 8 + `wh`

The `48 + 5wh` term MUST be pinned to reality by a test that serializes the same three values with the
transport's own serialization library. The envelope is bounded by a documented constant slack.

### Working-page transfer

A boundary message carrying an already-reduced page. Exists only in the reduced path.

| Attribute | Meaning |
| --- | --- |
| page | The page reduced to the derived dimensions. |
| model mask | The model mask reduced to the same dimensions. |
| paste mask | An all-covering mask at the derived dimensions (see the state machine below). |
| response | The model's output at the derived dimensions. |

**Validation rules**

- The reduced page's longest edge MUST equal the working resolution exactly, so the worker's own
  reduction does not run and its final restore is a no-op.
- The response MUST be at exactly the derived dimensions.
- The reduced page's dimensions MUST NOT be used to interpret the paste mask: the mask applied at
  composite time is the original full-resolution one.

## State machine: how one erase call resolves

```text
                    erase(page, model_mask, paste_mask)
                                  |
                                  v
                  +-------------------------------+
                  | full-size request provably    |
                  | within the limit?             |
                  +-------------------------------+
                     |                       |
                    yes                      no
                     |                       |
                     v                       v
        +------------------------+   +--------------------------------+
        | EXISTING PATH          |   | page already at/below the      |
        | send full-resolution   |   | working resolution?            |
        | page + both masks      |   +--------------------------------+
        | worker reduces         |        |                |
        | worker composites      |       yes               no
        | return full page       |        |                |
        +------------------------+        v                v
                     |            +--------------+  +---------------------+
                     |            | HARD ERROR:  |  | REDUCED PATH        |
                     |            | cannot fit   |  | reduce page + model |
                     |            | even reduced |  | mask; send with an  |
                     |            +--------------+  | all-covering paste  |
                     |                              | mask                |
                     |                              | restore response to |
                     |                              | original dimensions |
                     |                              | composite using the |
                     |                              | full-resolution     |
                     |                              | paste mask          |
                     |                              +---------------------+
                     |                                       |
                     +-------------------+-------------------+
                                         |
                                         v
                            full-resolution page, identical to the
                            input outside the paste mask
```

**Terminal-state invariants** (hold on every path, including the existing one):

1. Outside the paste mask's set pixels, the returned page equals the input page exactly.
2. Inside them, the returned value comes from the model's output for that location.
3. The returned page is at the input page's original dimensions.
4. Exactly one erase boundary call is made, or none at all when there is nothing to erase.

The reduced path reaches invariant 1 by never writing a pixel the paste mask does not cover, into a
buffer it initialized from the original page. The existing path reaches it because the worker itself
starts from its input page and writes only where the paste mask is set.

## Relationships

```text
Page 1---1 Model mask        (same dimensions)
Page 1---1 Paste mask        (same dimensions; stays at original resolution)
Page 1---0..1 Working-page transfer   (exists only in the reduced path)
Working-page transfer 1---1 reduced Page      (derived dimensions)
Working-page transfer 1---1 reduced Model mask (derived dimensions)
Working-page transfer 1---0..1 composed Page  (original dimensions)
```
