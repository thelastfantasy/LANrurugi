#!/bin/sh
# Writes a tiny library for smoke-native.sh: a one-page PDF (exercises PDFium) and a chaptered CJK
# text novel over the 8 KB threshold (exercises the typesetter and its fonts). Plain sh + printf,
# so it runs on every build host, the FreeBSD VM included.
#
# Usage: smoke-sample-library.sh <dir>
set -eu
dir="${1:?usage: smoke-sample-library.sh <dir>}"
mkdir -p "$dir"

stream='BT /F1 24 Tf 40 300 Td (LANrurugi) Tj ET'
{
    printf '%%PDF-1.4\n'
    printf '1 0 obj <</Type /Catalog /Pages 2 0 R>> endobj\n'
    printf '2 0 obj <</Type /Pages /Kids [3 0 R] /Count 1>> endobj\n'
    printf '3 0 obj <</Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R'
    printf ' /Resources <</Font <</F1 5 0 R>>>>>> endobj\n'
    printf '4 0 obj <</Length %d>> stream\n%s\nendstream endobj\n' "${#stream}" "$stream"
    printf '5 0 obj <</Type /Font /Subtype /Type1 /BaseFont /Helvetica>> endobj\n'
    printf 'trailer <</Root 1 0 R /Size 6>>\n%%%%EOF\n'
} > "$dir/smoke-sample.pdf"

{
    for chapter in 一 二 三; do
        printf '第%s章 春天\n' "$chapter"
        i=0
        while [ "$i" -lt 40 ]; do
            printf '春天来了，小明在公园里散步，看见樱花开得很漂亮。The quick brown fox.\n'
            i=$((i + 1))
        done
    done
} > "$dir/smoke-sample.txt"
echo "smoke-sample-library: $dir"
