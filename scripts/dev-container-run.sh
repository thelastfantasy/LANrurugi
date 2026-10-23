#!/usr/bin/env bash
# Starts (or replaces) the local full-stack dev container via a direct `podman run` — shared by
# `mise run dev-up` and `dev-rebuild-auto` so the flag list only needs to be kept in one place.
#
# Not `podman compose -f compose.dev.yaml up -d` — this host's `docker-compose` fallback binary (no
# `podman-compose` installed) parses GPU device syntax (`deploy.resources.reservations.devices`,
# and the newer top-level `gpus:` field — both tried) without error but attaches zero real devices
# to the resulting container (confirmed live via `podman inspect`'s `Devices: []` either way); a
# direct `podman run --device nvidia.com/gpu=all` on this same host attaches them correctly. Needed
# since issue #103's CUDA EP integration.
#
# Every flag below mirrors `compose.dev.yaml`'s `services.lanrurugi-dev` section (kept there as
# documentation + the source of truth for `dev-rebuild`'s image build); if that file's volumes,
# environment, or healthcheck ever change, update both. `--replace` makes this idempotent (safe to
# call whether or not a `lrr-dev` container already exists).
#
# `--health-timeout 30s`, not 5s (2026-09-15): a GPU-inference-heavy request can briefly block the
# health endpoint's own response on this same process; a 5s timeout hit that window and
# `--restart unless-stopped` below killed the container mid-translation — confirmed live,
# `RestartCount` climbed to 2 during a single translation verification pass.
set -euo pipefail

# `/dev/dri` (the DRM render-node device Intel/AMD GPUs expose on Linux — see
# `gpu_worker_client`'s own module doc on why Intel is worth detecting at all) is conditionally
# attached: unlike `--device nvidia.com/gpu=all` (the NVIDIA Container Toolkit's own CDI device,
# always safe to request since the toolkit itself no-ops when no NVIDIA GPU is present),
# `podman run --device /dev/dri` fails outright if that path doesn't exist on the host at all
# (a pure-NVIDIA host with no Intel/AMD graphics stack). Building the flag array conditionally
# keeps this script working on hosts without `/dev/dri`, not just this one (which happens to have
# both).
device_flags=(--device nvidia.com/gpu=all)
if [ -d /dev/dri ]; then
  device_flags+=(--device /dev/dri)
fi

podman run -d --replace \
  --name lrr-dev \
  --network host \
  "${device_flags[@]}" \
  --restart unless-stopped \
  -v ./data/manga:/home/koyomi/lanrurugi/content \
  -v ./data/thumb:/home/koyomi/lanrurugi/thumb \
  -v ./data/database:/home/koyomi/lanrurugi/database \
  -v ./data/models:/home/koyomi/lanrurugi/models \
  -v lrr-dev-log:/log \
  -v lrr-dev-temp:/temp \
  -v ./apps/frontend:/workspace/apps/frontend \
  -v lrr-dev-frontend-node-modules:/workspace/apps/frontend/node_modules \
  -v ./plugins:/usr/local/share/lanrurugi/plugins \
  -v lrr-dev-cargo-target:/build/target \
  -v lrr-dev-geoip:/var/lib/GeoIP \
  -e DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY:-}" \
  -e LANRURUGI_MANGA_OCR_MODEL_DIR=/home/koyomi/lanrurugi/models/manga-ocr \
  -e LANRURUGI_INPAINT_MODEL_DIR=/home/koyomi/lanrurugi/models/inpaint \
  -e LANRURUGI_BUBBLE_SEG_MODEL_DIR=/home/koyomi/lanrurugi/models/bubble-seg \
  -e LANRURUGI_GEOIP_ACCOUNT_ID="${LANRURUGI_GEOIP_ACCOUNT_ID:-}" \
  -e LANRURUGI_GEOIP_LICENSE_KEY="${LANRURUGI_GEOIP_LICENSE_KEY:-}" \
  --health-cmd "curl -fsS http://127.0.0.1:3001/health" \
  --health-interval 15s \
  --health-timeout 30s \
  --health-retries 3 \
  --health-start-period 30s \
  localhost/lanrurugi-dev-full:latest
