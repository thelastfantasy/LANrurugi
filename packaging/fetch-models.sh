#!/usr/bin/env bash
# Downloads the models used by in-page translation into this package's share/lanrurugi/models/,
# where the server finds them without configuration. Checksums are verified; re-running skips
# files that are already present and intact.
#
# Usage: ./fetch-models.sh [--with-bubbles]
#   --with-bubbles  also builds the speech-bubble segmentation model (needs `uv`); without it,
#                   translation erases text with rectangles instead of exact bubble shapes.
set -euo pipefail

prefix="$(cd "$(dirname "$0")" && pwd)"
models="$prefix/share/lanrurugi/models"
"$prefix/scripts/fetch-ocr-model.sh" "$models/manga-ocr"
"$prefix/scripts/fetch-inpaint-model.sh" "$models/inpaint"
if [ "${1:-}" = "--with-bubbles" ]; then
    "$prefix/scripts/fetch-bubble-seg-model.sh" "$models/bubble-seg"
fi
echo "fetch-models: models are in $models"
