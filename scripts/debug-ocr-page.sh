#!/bin/sh
# Runs the real OCR pipeline on one page image, inside the dev container.
#
# `GpuWorkerClient` spawns `lanrurugi-gpu-worker` from the directory of the *running* binary, and a
# cargo example lives in `target/<profile>/examples/`, so the worker would never be found from here.
# Building it and linking it next to the example reproduces the production layout (both binaries in
# one directory) instead of special-casing the lookup.
set -eu

cargo build -p lanrurugi-gpu-worker
mkdir -p target/debug/examples
ln -sf ../lanrurugi-gpu-worker target/debug/examples/lanrurugi-gpu-worker

# The models are inside the repo mount (`./data/models`), which is where the dev container's own
# `LANRURUGI_*_MODEL_DIR` values point at too.
export LANRURUGI_MANGA_OCR_MODEL_DIR="${LANRURUGI_MANGA_OCR_MODEL_DIR:-/workspace/data/models/manga-ocr}"
export LANRURUGI_INPAINT_MODEL_DIR="${LANRURUGI_INPAINT_MODEL_DIR:-/workspace/data/models/inpaint}"
export LANRURUGI_BUBBLE_SEG_MODEL_DIR="${LANRURUGI_BUBBLE_SEG_MODEL_DIR:-/workspace/data/models/bubble-seg}"

exec cargo run -p lanrurugi-api --example debug_ocr_page -- "$@"
