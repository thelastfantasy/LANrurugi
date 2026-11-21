#!/usr/bin/env bash
# Fetches the LaMa inpainting model used by `crates/lanrurugi-inpaint` (Phase 2 on-page manga
# translation, specs/004-ocr-manga-translation) — same fetch-and-verify shape as
# scripts/fetch-ocr-model.sh: pinned to an exact commit, checksum-verified, never a moving `main`.
#
# `ogkalu/lama-manga-onnx-dynamic`'s `lama-manga-dynamic.onnx`: an ONNX export of
# `dreMaz/AnimeMangaInpainting` (LaMa fine-tuned on manga) with dynamic `h`/`w` inputs — what
# `crates/lanrurugi-inpaint` has expected since 2026-09-08 (see its lib.rs). This script still
# fetched the fixed-512 `Carve/LaMa-ONNX` file until #329, under a name model discovery never
# looked for, so release images shipped a model nothing loaded and translation stayed
# unavailable. Apache-2.0, per the HF API's `cardData.license`. The file name must equal
# `model_discovery::MODEL_FILE`; a test in that module reads this script to keep them in step.
set -euo pipefail

# Verifies `<sha256>  <file>` lines on stdin (`shasum` on macOS, which has no `sha256sum`).
sha256_check() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum --check --status; else shasum -a 256 --check --status; fi
}

REPO="ogkalu/lama-manga-onnx-dynamic"
REV="ee4ed4a8447b6730fc41d34f90876b6c48af925a"
FILE="lama-manga-dynamic.onnx"
SHA256="de31ffa5ba26916b8ea35319f6c12151ff9654d4261bccf0583a69bb095315f9"

# Default target matches `model_discovery.rs`'s binary-relative candidate; override for the
# Dockerfile stage or a custom install.
DEST="${1:-${LANRURUGI_INPAINT_MODEL_DIR:-models/inpaint}}"
HF_ENDPOINT="${HF_ENDPOINT:-https://huggingface.co}"

mkdir -p "$DEST"

fetch() {
  local url="$1" out="$2" want="$3"

  if [ -f "$out" ] && echo "${want}  ${out}" | sha256_check 2>/dev/null; then
    echo "  already present and verified: $(basename "$out")"
    return 0
  fi

  echo "  downloading $(basename "$out") ..."
  curl -fL --retry 3 --progress-bar "$url" -o "${out}.part"

  if ! echo "${want}  ${out}.part" | sha256_check; then
    rm -f "${out}.part"
    echo "ERROR: checksum mismatch for $(basename "$out") — refusing to install." >&2
    exit 1
  fi
  mv "${out}.part" "$out"
}

echo "Fetching LaMa inpainting model into ${DEST}/"
fetch "${HF_ENDPOINT}/${REPO}/resolve/${REV}/${FILE}" \
  "${DEST}/${FILE}" "$SHA256"

echo "Done. Point LANRURUGI_INPAINT_MODEL_DIR at ${DEST} if it is not on the default search path"
echo "(see crates/lanrurugi-inpaint/README.md)."
