# lanrurugi-inpaint

Erases a manga page's original lettering before the translated text is composited on top
(`lanrurugi-translate::composite`), regenerating the erased area from surrounding real pixels
rather than covering it with a flat-colour rectangle or a contrasting outline stroke (both already
shipped there as the pre-inpainting fallback, kept as the graceful-degradation path when this
crate's model isn't installed).

## Model file

| File | Purpose |
|---|---|
| `lama_fp32.onnx` | LaMa inpainting network, fixed 512x512 input |

From [`Carve/LaMa-ONNX`](https://huggingface.co/Carve/LaMa-ONNX) — an Apache-2.0 ONNX export of
[`advimman/lama`](https://github.com/advimman/lama) (itself Apache-2.0), pinned to an exact commit
and checksum-verified by the fetch script.

Fetch it for local development with:

```sh
./scripts/fetch-inpaint-model.sh
```

## Model discovery

`model_discovery.rs` resolves the directory holding `lama_fp32.onnx` by taking the **first match**
from this search path (same pattern as `lanrurugi-ocr::model_discovery`):

1. `LANRURUGI_INPAINT_MODEL_DIR` — explicit override, wins over everything.
2. `<dir of the running binary>/models/inpaint/` — alongside a locally built binary.
3. `/var/lib/lanrurugi/models/inpaint/` — production config path.
4. `/app/models/inpaint/` — container path.

If no candidate contains the model, inpainting is **unavailable** rather than fatal — callers fall
back to the existing flat-fill/outline-stroke compositing, the same graceful-degradation shape
already established for the OCR recognition model (FR-019).

## Whole-page erasure

`Inpainter::erase_page` runs a single LaMa inference per page: `lanrurugi-translate::composite`
merges every text region's own mask (a real speech-bubble outline where
[`huyvux3005/manga109-segmentation-bubble`](https://huggingface.co/huyvux3005/manga109-segmentation-bubble)
found a confident match, intersected with a glyph-colour stroke mask where available, falling back
to the plain OCR bounding box otherwise) into one page-sized mask, then calls this crate once for
the whole page. Matches how real production manga translators do this (e.g.
`zyddnys/manga-image-translator`'s own `dispatch_inpainting`) — an earlier per-region version of
this crate (one LaMa call per text region against a small padded crop) was tried and abandoned
after hitting cross-region context pollution and irregular-small-hole degradation in practice; see
`Inpainter::erase_page`'s own doc comment for the full history.

The page is letterboxed (aspect-ratio-preserving scale + centred mid-grey pad), not stretched,
into the model's fixed 512x512 input — matching `zyddnys/manga-image-translator`'s own
`resize_keep_aspect`. The output is cropped back to the real content and resized back up to the
page's own true size before masked pixels are pasted back in.

## Execution provider

CPU-only, explicitly registered — mirrors `lanrurugi-ocr::recognize`'s own reasoning: a
GPU-flavoured `libonnxruntime.so` can hang indefinitely in `commit_from_file` when no execution
provider is registered at all (research.md §1's OpenVINO incident). No GPU EP is registered.
