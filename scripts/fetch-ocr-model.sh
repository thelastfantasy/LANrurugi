#!/usr/bin/env bash
# Fetches the `manga-ocr` recognition model used by `crates/lanrurugi-ocr` (Phase 2 on-page manga
# translation, specs/004-ocr-manga-translation T004).
#
# Everything here is Apache-2.0: the ONNX export comes from `onnx-community/manga-ocr-base-ONNX`
# (an export of kha-white's Apache-2.0 `manga-ocr-base`), and the vocabulary comes from the
# upstream `kha-white/manga-ocr-base` repo itself, which ships PyTorch weights only — hence the two
# different sources. Both are pinned to an exact commit and checksum-verified, never a moving
# `main`, per the constitution's dependency-pinning rule.
#
# The PP-OCR **detection** model is fetched here too. research.md §1 assumed `oar-ocr` would fetch
# and manage its own detection model; verified against the real 0.9.2 API at implementation time,
# it does not — `TextDetectionAdapterBuilder::build()` requires a `ModelSource` (a path or bytes)
# the caller supplies. The official Apache-2.0 PaddlePaddle ONNX export is used, pinned the same
# way as the recognition model.
set -euo pipefail

ONNX_REPO="onnx-community/manga-ocr-base-ONNX"
ONNX_REV="f9023406bb2f6b17df67bc4a327c56ecd20611f0"
VOCAB_REPO="kha-white/manga-ocr-base"
VOCAB_REV="aa6573bd10b0d446cbf622e29c3e084914df9741"
DET_REPO="PaddlePaddle/PP-OCRv5_mobile_det_onnx"
DET_REV="e6f4fa85f00e168c862bc462aebca69eef9b3d3d"

ENCODER_SHA256="df35f64c2400ea860c70a2d06f2a1f99892374a78c89fcf35308076557a3863f"
DECODER_SHA256="31ca14d6dee6b3966144e128d0481d5a91f5083cbced81fe7a9571713fa50cd4"
VOCAB_SHA256="344fbb6b8bf18c57839e924e2c9365434697e0227fac00b88bb4899b78aa594d"
DET_SHA256="a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d"

# Default target matches `model_discovery.rs`'s binary-relative candidate; override for the
# Dockerfile stage or a custom install.
DEST="${1:-${LANRURUGI_MANGA_OCR_MODEL_DIR:-models/manga-ocr}}"
HF_ENDPOINT="${HF_ENDPOINT:-https://huggingface.co}"

mkdir -p "$DEST"

fetch() {
  local url="$1" out="$2" want="$3"

  if [ -f "$out" ] && echo "${want}  ${out}" | sha256sum --check --status 2>/dev/null; then
    echo "  already present and verified: $(basename "$out")"
    return 0
  fi

  echo "  downloading $(basename "$out") ..."
  curl -fL --retry 3 --progress-bar "$url" -o "${out}.part"

  if ! echo "${want}  ${out}.part" | sha256sum --check --status; then
    rm -f "${out}.part"
    echo "ERROR: checksum mismatch for $(basename "$out") — refusing to install." >&2
    exit 1
  fi
  mv "${out}.part" "$out"
}

echo "Fetching manga-ocr recognition model into ${DEST}/"
fetch "${HF_ENDPOINT}/${ONNX_REPO}/resolve/${ONNX_REV}/onnx/encoder_model.onnx" \
  "${DEST}/encoder_model.onnx" "$ENCODER_SHA256"
fetch "${HF_ENDPOINT}/${ONNX_REPO}/resolve/${ONNX_REV}/onnx/decoder_model.onnx" \
  "${DEST}/decoder_model.onnx" "$DECODER_SHA256"
fetch "${HF_ENDPOINT}/${VOCAB_REPO}/resolve/${VOCAB_REV}/vocab.txt" \
  "${DEST}/vocab.txt" "$VOCAB_SHA256"
fetch "${HF_ENDPOINT}/${DET_REPO}/resolve/${DET_REV}/inference.onnx" \
  "${DEST}/detection_model.onnx" "$DET_SHA256"

echo "Done. Point LANRURUGI_MANGA_OCR_MODEL_DIR at ${DEST} if it is not on the default search path"
echo "(see crates/lanrurugi-ocr/README.md)."
