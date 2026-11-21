#!/usr/bin/env bash
# Memory baseline for image work (#330): the server's peak and settled RSS while it generates
# thumbnails and serves full pages of a fixed set of large JPEG pages. Run it before and after a
# change to compare. Needs the dev container (`mise run dev-up`) and Python with Pillow.
#
#   scripts/memory-baseline.sh [pages] [concurrency]
#
# It uploads one synthetic archive the first time ("[测试] 内存基线", 3000x4200 pages), and on every
# run deletes only that archive's own thumbnails and converted pages so both are produced again. The peak is reset
# (`/proc/<pid>/clear_refs`) at the start, so runs compare from the same baseline.
set -euo pipefail

PAGES="${1:-16}"
CONCURRENCY="${2:-8}"
BASE="${LANRURUGI_DEV_URL:-http://192.168.3.105:3000}"
CONTAINER="${LANRURUGI_DEV_CONTAINER:-lrr-dev}"
THUMB_DIR="${LANRURUGI_DEV_THUMB_DIR:-/home/koyomi/lanrurugi/thumb}"
TEMP_DIR="${LANRURUGI_DEV_TEMP_DIR:-/temp}"
SETTLE="${SETTLE_SECONDS:-15}"
TITLE="[测试] 内存基线"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
jar="$work/cookies"
curl -fsS -c "$jar" -o /dev/null -X POST -d "password=${LANRURUGI_DEV_PASSWORD:-kamimamita}" "$BASE/api/login"

# The library turns the leading `[测试]` into part of the title, so match on the rest.
KEY="内存基线"
find_archive() {
  curl -fsS -b "$jar" "$BASE/api/search?start=-1&groupby_tanks=false&filter=$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1]))' "$KEY")" |
    python3 -c 'import json,sys; d=json.load(sys.stdin)["data"]; print(next((a["arcid"] for a in d if sys.argv[1] in a["title"]), ""))' "$KEY"
}

id="$(find_archive)"
if [ -z "$id" ]; then
  echo "uploading a $PAGES-page fixture ..."
  python3 - "$work/fixture.zip" "$PAGES" <<'EOF'
import io, sys, zipfile
from PIL import Image, ImageDraw, ImageFilter
out, pages = sys.argv[1], int(sys.argv[2])
with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
    for n in range(1, pages + 1):
        # Noise plus shapes: compresses like a real scan (a few MB), not like a flat test card.
        img = Image.effect_noise((3000, 4200), 40 + n).convert("RGB")
        d = ImageDraw.Draw(img)
        for k in range(30):
            d.ellipse([k * 97 % 2800, k * 131 % 4000, k * 97 % 2800 + 300, k * 131 % 4000 + 300], outline=(n * 13 % 255, 80, 160), width=12)
        img = img.filter(ImageFilter.GaussianBlur(1))
        buf = io.BytesIO()
        img.save(buf, "JPEG", quality=90)
        z.writestr(zipfile.ZipInfo(f"{n:03d}.jpg", date_time=(2026, 1, 1, 0, 0, 0)), buf.getvalue())
EOF
  cp "$work/fixture.zip" "$work/$TITLE.zip"
  curl -fsS -b "$jar" -o /dev/null -X PUT -F "file=@$work/$TITLE.zip" "$BASE/api/archives/upload"
  for _ in $(seq 1 60); do id="$(find_archive)"; [ -n "$id" ] && break; sleep 2; done
  [ -n "$id" ] || { echo "the fixture did not appear in the library" >&2; exit 1; }
fi
echo "archive $id"

pid="$(podman exec "$CONTAINER" pgrep -f 'lanrurugi-server serve' | head -1)"
status() { podman exec "$CONTAINER" sh -c "grep -E '^(VmRSS|VmHWM):' /proc/$pid/status" | awk '{printf "%s %d MiB  ", $1, $2/1024}'; }

podman exec "$CONTAINER" sh -c "rm -rf '$THUMB_DIR/${id:0:2}/$id' '$TEMP_DIR/resize_page/$id'"
podman exec "$CONTAINER" sh -c "echo 5 > /proc/$pid/clear_refs"
echo "before:  $(status)"

curl -fsS -b "$jar" "$BASE/api/archives/$id/files" |
  python3 -c 'import json,sys; [print(p["url"]) for p in json.load(sys.stdin)["pages"]]' > "$work/pages"

( while :; do echo "  $(date +%T) $(status)"; sleep 0.5; done ) & sampler=$!
start=$(date +%s.%N)
seq 1 "$PAGES" | xargs -P "$CONCURRENCY" -I{} curl -fsS -b "$jar" -o /dev/null "$BASE/api/archives/$id/thumbnail?page={}"
thumbs_done=$(date +%s.%N)
echo "after thumbnails: $(status)"
sed "s|^|$BASE|" "$work/pages" | xargs -P "$CONCURRENCY" -I{} curl -fsS -b "$jar" -o /dev/null "{}"
pages_done=$(date +%s.%N)
kill "$sampler"

printf 'thumbnails: %.1fs   pages: %.1fs\n' "$(echo "$thumbs_done - $start" | bc)" "$(echo "$pages_done - $thumbs_done" | bc)"
echo "peak:    $(status)"
sleep "$SETTLE"
echo "after ${SETTLE}s idle: $(status)"
