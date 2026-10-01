#!/usr/bin/env bash
# Fetches the LaMa inpainting model used by `crates/lanrurugi-inpaint` (Phase 2 on-page manga
# translation, specs/004-ocr-manga-translation) — same fetch-and-verify shape as
# scripts/fetch-ocr-model.sh: pinned to an exact commit, checksum-verified, never a moving `main`.
#
# `lama_fp32.onnx` (not `lama.onnx`) per Carve/LaMa-ONNX's own README: it's the recommended
# variant — the other one (exported via `torch.onnx.dynamo_export`) is explicitly marked
# "NOT RECOMMENDED" there (slower, less-optimized ONNX graph). Both the ONNX export (Carve) and
# the original model (advimman/lama) are Apache-2.0 — verified directly via the HF API's
# `cardData.license` field, not inferred from a repo badge.
set -euo pipefail

REPO="Carve/LaMa-ONNX"
REV="c3c0c9e468934d62e79c329e35d82dd09ff8c444"
SHA256="1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6"

# Default target matches `model_discovery.rs`'s binary-relative candidate; override for the
# Dockerfile stage or a custom install.
DEST="${1:-${LANRURUGI_INPAINT_MODEL_DIR:-models/inpaint}}"
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

echo "Fetching LaMa inpainting model into ${DEST}/"
fetch "${HF_ENDPOINT}/${REPO}/resolve/${REV}/lama_fp32.onnx" \
  "${DEST}/lama_fp32.onnx" "$SHA256"

echo "Done. Point LANRURUGI_INPAINT_MODEL_DIR at ${DEST} if it is not on the default search path"
echo "(see crates/lanrurugi-inpaint/README.md)."
