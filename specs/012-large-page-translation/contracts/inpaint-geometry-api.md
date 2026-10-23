# Contract: `lanrurugi-inpaint` Geometry API

**Status**: additions to an existing crate's public surface
**Consumer**: `lanrurugi-api::gpu_worker_client` (client-side reduction), `lanrurugi-inpaint` itself
(the worker's own reduction)

## Why this is public

The client must reduce a page to exactly the resolution the worker would have reduced it to. If the
two derived those dimensions independently, a divergence would not raise an error — it would hand the
model a differently-shaped page and mask and produce a wrong result with no diagnostic. Making the
rule a single public function removes the possibility rather than testing for it.

`lanrurugi-api` already depends on `lanrurugi-inpaint`, so this costs no new dependency edge.

## Additions

### `pub const MAX_INPAINT_SIZE: u32`

The working resolution: the fixed upper bound on the longest edge the erase model is asked to process.
Changed from private to public. **Its value does not change.**

### `pub fn work_dims(width: u32, height: u32) -> (u32, u32)`

The derived dimensions for a page, per the existing (unchanged) rule:

```text
longest = max(width, height)
if longest > MAX_INPAINT_SIZE:
    scale = MAX_INPAINT_SIZE / longest          (f64)
    (round(width * scale).max(1), round(height * scale).max(1))
else:
    (width, height)
```

**Postconditions**

- `work_dims(w, h) == (w, h)` when `max(w, h) <= MAX_INPAINT_SIZE`, so a page already within the
  working resolution is never resampled.
- When `max(w, h) > MAX_INPAINT_SIZE`, `max(work_dims(w, h)) == MAX_INPAINT_SIZE` exactly. This is the
  postcondition the reduced path depends on: it makes the already-reduced page fail the worker's own
  `> MAX_INPAINT_SIZE` test, so the worker does not reduce it a second time.
- Neither component exceeds the corresponding input component, and neither is 0.

### `pub fn downscale_to_work(page: &RgbImage, model_mask: &[bool]) -> Result<(RgbImage, GrayImage), InpaintError>`

Produces the working-resolution page and model mask for a page.

- Reduces the page with `FilterType::CatmullRom` and the mask with `FilterType::Nearest`, or returns
  clones unchanged when `work_dims` says no reduction is needed.
- The mask is materialised as a two-level image (set locations to the maximum intensity, unset to
  zero) before reduction, so nearest-neighbour selection can only choose an existing location's value.
- Returns `InpaintError` when `model_mask.len()` does not equal `page.width() * page.height()`.

**Extraction requirement**: `Inpainter::erase_page` MUST be refactored to call this function rather
than keeping its own copy of the reduction. A second implementation of the same rule inside the same
crate would defeat the purpose of the extraction.

## What must not change

- `Inpainter::erase_page`'s signature, return type, and observable behaviour at every input size.
- `InpainterHandle`'s trait definition.
- The value of the working resolution, the padding rule, the model's input conventions, or anything
  about how the model is invoked.
- The rule that `erase_page` writes only where the paste mask is set, starting from a clone of its
  input page. The reduced path's correctness argument depends on this, so it is a contract, not an
  implementation detail.
