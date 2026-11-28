#!/bin/sh
# Builds the platform-independent part of every native package: the web interface, the
# documentation site as the server serves it under /docs (site + plugin SDK + Swagger UI), and the
# bundled plugins. Same steps as the image's frontend-builder/docs-builder stages.
#
# Usage: build-native-assets.sh <out-dir>
# Needs pnpm, node, deno and mise; apps/docs-site/public/openapi.json must exist
# (`cargo run -p lanrurugi-api-docgen`, or `mise run docs-api`).
set -eu

out="${1:?usage: build-native-assets.sh <out-dir>}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
case "$out" in /*) ;; *) out="$repo/$out" ;; esac

if [ ! -f apps/docs-site/public/openapi.json ]; then
    echo "build-native-assets: apps/docs-site/public/openapi.json is missing — run mise run docs-api" >&2
    exit 1
fi

pnpm install --frozen-lockfile
pnpm --filter lanrurugi-frontend run build
SDK_DOCS_URL=/docs/ mise run docs-sdk
pnpm --filter lanrurugi-docs run build:swagger
DOCS_BASE=/docs/ DOCS_SDK_URL=/docs/sdk/ pnpm --filter lanrurugi-docs run build

rm -rf "$out"
mkdir -p "$out/docs" "$out/plugins"
cp -R apps/frontend/dist "$out/frontend"
cp -R apps/docs-site/dist/. "$out/docs/"
cp -R apps/docs-site/dist-sdk "$out/docs/sdk"
cp -R apps/docs-site/dist-swagger "$out/docs/swagger"
# Tracked plugin files only: local AI-wizard drafts under plugins/custom/ stay out of packages.
git ls-files -z plugins | xargs -0 tar -cf - | tar -xf - -C "$out/plugins" --strip-components=1
node scripts/third-party-licenses.mjs --out "$out/THIRD_PARTY_LICENSES.txt"
echo "build-native-assets: $out"
