#!/usr/bin/env bash
# Fetches and converts the manga speech-bubble segmentation model used by `lanrurugi-ocr`'s bubble
# detection (Phase 2 on-page manga translation, specs/004-ocr-manga-translation) — precise bubble
# masks for `lanrurugi-inpaint::Inpainter::erase_region_mask`, replacing the rectangular-bbox
# erasure `erase_region` alone provides.
#
# Unlike scripts/fetch-ocr-model.sh / fetch-inpaint-model.sh, this one is NOT a pure download: the
# upstream repo (huyvux3005/manga109-segmentation-bubble, Apache-2.0) only publishes Ultralytics
# `.pt` weights, no pre-exported ONNX. `dmMaze/comic-text-detector` — the other real candidate
# researched — and the whole `manga-image-translator` ecosystem it lives in are GPL-3.0 and
# incompatible with this project's MIT license; excluded regardless of integration cost, not
# considered further.
#
# The `.pt` → `.onnx` conversion is `ultralytics`'s own standard, officially-documented
# `model.export(format="onnx")` — not a from-scratch reimplementation, not model training. It runs
# in a throwaway `uv`-managed venv (bundles its own CPython, needs no system `python3-venv`
# package or sudo) so it never touches the host's real Python environment. True byte-for-byte
# reproducibility of the *exported .onnx* isn't meaningful the way a plain file download's
# checksum is (`onnxslim`'s optimisation passes and library versions can shift output bytes
# between runs even for identical input weights) — so what's actually pinned here is the input
# (`.pt`, real commit + checksum, like every other model this project fetches) and the exact
# `ultralytics`/`onnx`/`onnxruntime`/`onnxslim` versions used for the conversion step, which is
# the closest meaningful notion of reproducibility this ecosystem's own export tooling supports.
set -euo pipefail

REPO="huyvux3005/manga109-segmentation-bubble"
REV="f9a4108c4955136a810e5e92207972f3fb3a65fd"
PT_SHA256="4028152940f7c910f40192f46ede3b3f6c7129e5c76849c324d3564f8ac50198"

# Pinned versions used to originally produce/verify this export (2026-09-07) — not a moving
# `ultralytics` latest, per the constitution's dependency-pinning rule.
ULTRALYTICS_VERSION="8.4.142"
ONNX_VERSION="1.22.0"
ONNXRUNTIME_VERSION="1.29.0"
ONNXSLIM_VERSION="0.1.96"

# Fixed at the upstream model card's own training resolution (README: "Image Size 1600x1600") —
# exporting at a different size than training risks accuracy loss from an unfamiliar receptive
# field, not just a slower/faster model.
EXPORT_IMGSZ=1600

DEST="${1:-${LANRURUGI_BUBBLE_SEG_MODEL_DIR:-models/bubble-seg}}"
HF_ENDPOINT="${HF_ENDPOINT:-https://huggingface.co}"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

mkdir -p "$DEST"

if [ -f "$DEST/bubble_seg.onnx" ]; then
  echo "already present: ${DEST}/bubble_seg.onnx (delete it to force reconversion)"
  exit 0
fi

if ! command -v uv >/dev/null 2>&1; then
  echo "ERROR: uv is required (https://github.com/astral-sh/uv) — no system Python venv needed," >&2
  echo "       but uv itself must already be installed." >&2
  exit 1
fi

echo "Downloading best.pt into ${WORKDIR}/"
curl -fL --retry 3 --progress-bar \
  "${HF_ENDPOINT}/${REPO}/resolve/${REV}/best.pt" -o "${WORKDIR}/best.pt"
if ! echo "${PT_SHA256}  ${WORKDIR}/best.pt" | sha256sum --check --status; then
  echo "ERROR: checksum mismatch for best.pt — refusing to convert." >&2
  exit 1
fi

echo "Building a throwaway conversion venv (uv, isolated — nothing touches system Python)..."
uv venv "${WORKDIR}/venv" >/dev/null
uv pip install --python "${WORKDIR}/venv/bin/python" \
  "ultralytics==${ULTRALYTICS_VERSION}" \
  "onnx==${ONNX_VERSION}" \
  "onnxruntime==${ONNXRUNTIME_VERSION}" \
  "onnxslim==${ONNXSLIM_VERSION}" >/dev/null

echo "Exporting to ONNX (imgsz=${EXPORT_IMGSZ}, opset=17)..."
"${WORKDIR}/venv/bin/python" -c "
from ultralytics import YOLO
model = YOLO('${WORKDIR}/best.pt')
path = model.export(format='onnx', imgsz=${EXPORT_IMGSZ}, opset=17, simplify=True, dynamic=False)
print('exported:', path)
"

mv "${WORKDIR}/best.onnx" "${DEST}/bubble_seg.onnx"
echo "Done. Point LANRURUGI_BUBBLE_SEG_MODEL_DIR at ${DEST} if it is not on the default search path"
echo "(see crates/lanrurugi-ocr/README.md)."
