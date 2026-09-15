# lanrurugi-ocr

OCR for Phase 2's on-page manga translation (`specs/004-ocr-manga-translation`). Two independently
sourced stages, per research.md §1:

- **Detection** (*where* is the text): [`oar-ocr`](https://github.com/GreatV/oar-ocr) (Apache-2.0,
  PP-OCR based), driven against the official Apache-2.0 `PP-OCRv5_mobile_det` ONNX export.
- **Recognition** (*what does it say*): this crate's own `ort` (ONNX Runtime) integration against
  kha-white's Apache-2.0 [`manga-ocr`](https://github.com/kha-white/manga-ocr) weights, chosen over
  generic PP-OCR recognition for accuracy on vertical/stylized Japanese manga lettering.

## Recognition model files

> **Deviation from research.md §1**: that document assumed `oar-ocr` "fetches and manages its own
> detection model — no custom acquisition needed". Checked against the real 0.9.2 API at
> implementation time, it does not: `TextDetectionAdapterBuilder::build()` takes a `ModelSource`
> (path or bytes) the caller must supply. The detection model is therefore fetched and discovered
> by the same mechanism as the recognition model below.

The pipeline needs an ONNX export of `manga-ocr` plus its tokenizer vocabulary, and a PP-OCR
detection model:

| File | Purpose |
|---|---|
| `encoder_model.onnx` | ViT image encoder |
| `decoder_model.onnx` | Autoregressive text decoder |
| `vocab.txt` | WordPiece vocabulary for decoding token ids back to text |
| `detection_model.onnx` | PP-OCRv5 mobile text detection |

The two ONNX files come from `onnx-community/manga-ocr-base-ONNX` (an Apache-2.0 ONNX export of
kha-white's model); `vocab.txt` comes from the upstream `kha-white/manga-ocr-base` repo, which
publishes PyTorch weights only. Both are pinned to an exact commit and checksum-verified by the
fetch script — verified live at implementation time rather than assumed, since the upstream repo
turned out to ship no ONNX export of its own.

Fetch them for local development with:

```sh
./scripts/fetch-ocr-model.sh
```

Production images get the same files via a dedicated `Dockerfile` build stage, so the runtime image
never needs network access to run OCR.

## Model discovery

`model_discovery.rs` resolves the directory holding the files above by taking the **first match**
from this search path (mirroring `~/jellyfin-suite`'s `find_model()` pattern, research.md §1):

1. `LANRURUGI_MANGA_OCR_MODEL_DIR` — explicit override, wins over everything.
2. `<dir of the running binary>/models/manga-ocr/` — alongside a locally built binary.
3. `/var/lib/lanrurugi/models/manga-ocr/` — production config path.
4. `/app/models/manga-ocr/` — container path used by the `Dockerfile` stage.

If no candidate contains the required files, recognition is **unavailable** rather than fatal: the
crate reports a typed error the caller turns into "translation unavailable for this page"
(spec.md FR-019), and the rest of the server — including untranslated reading — is unaffected.

## Execution provider

CPU only. No CUDA/DirectML/OpenVINO execution provider is registered this phase — registering an EP
against an ONNX Runtime build lacking that provider's code can hang indefinitely inside
`session.run()` rather than failing cleanly (a real production incident documented in research.md
§1). GPU support, if added later, must gate on a verified-compatible runtime asset rather than
"try it and fall back on error".
