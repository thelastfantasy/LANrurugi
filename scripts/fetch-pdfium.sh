#!/bin/sh
# Fetches the PDFium shared library `lanrurugi-book-worker` loads (prebuilt by
# bblanchon/pdfium-binaries) and verifies it. The one place its version and checksums live:
# VERSION must match the `pdfium_<n>` feature of `pdfium-render` in
# crates/lanrurugi-book-worker/Cargo.toml, or the bindings and the library disagree on the API.
#
# Usage: fetch-pdfium.sh <amd64|arm64|mac-arm64> <dest-dir>
#   → <dest-dir>/lib/libpdfium.so (Linux) or libpdfium.dylib (macOS), plus LICENSE and licenses/
set -eu

VERSION=7543
usage="usage: fetch-pdfium.sh <amd64|arm64|mac-arm64> <dest-dir>"
arch="${1:?$usage}"
dest="${2:?$usage}"

case "$arch" in
    amd64 | x64 | x86_64 | linux-x86_64)
        name=linux-x64 lib=libpdfium.so sha256=9329a3c4b19b3c8d0a93af5440f44be84e4bd879a204e47b1a7a160e96809da4 ;;
    arm64 | aarch64 | linux-aarch64)
        name=linux-arm64 lib=libpdfium.so sha256=4965a4c0b64c45b5edefa1072e2b483bf90b4d25d7deec44f104dcbdecf05c3e ;;
    mac-arm64 | macos-arm64 | darwin-arm64)
        name=mac-arm64 lib=libpdfium.dylib sha256=41c269723b4711793de70ff34e65c00fa79907d6c023741837579e906b846faa ;;
    *) echo "fetch-pdfium: unsupported platform '$arch'" >&2; exit 1 ;;
esac

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL -o "$tmp" "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/${VERSION}/pdfium-${name}.tgz"
actual="$(sha256_of "$tmp")"
if [ "$actual" != "$sha256" ]; then
    echo "fetch-pdfium: checksum mismatch for pdfium-${name}.tgz (got $actual)" >&2
    exit 1
fi
mkdir -p "$dest"
tar -xzf "$tmp" -C "$dest" "lib/${lib}" LICENSE licenses
echo "fetch-pdfium: PDFium chromium/${VERSION} (${name}) -> ${dest}/lib/${lib}"
