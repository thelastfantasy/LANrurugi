#!/bin/sh
# Fetches the pinned CPU ONNX Runtime shared library (official microsoft/onnxruntime release) and
# verifies it. The embedding, OCR and inpaint models load it at run time through `ort`'s
# `load-dynamic` feature, so its version only has to satisfy `ort`'s minimum.
#
# Usage: fetch-onnxruntime.sh <linux-x86_64|linux-aarch64|macos-arm64> <dest-dir>
#   → <dest-dir>/lib/libonnxruntime.* (the real file and its symlinks), LICENSE, ThirdPartyNotices.txt
set -eu

VERSION=1.27.0
usage="usage: fetch-onnxruntime.sh <linux-x86_64|linux-aarch64|macos-arm64> <dest-dir>"
platform="${1:?$usage}"
dest="${2:?$usage}"

case "$platform" in
    linux-x86_64 | amd64)
        name=linux-x64 sha256=547e40a48f1fe73e3f812d7c88a948612c23f896b91e4e2ee1e232d7b468246f ;;
    linux-aarch64 | arm64)
        name=linux-aarch64 sha256=3e4d83ac06924a32a07b6d7f91ce6f852876153fc0bbdf931bf517a140bfbe48 ;;
    macos-arm64 | mac-arm64)
        name=osx-arm64 sha256=545e81c58152353acb0d1e8bd6ce4b62f830c0961f5b3acfedc790ffd76e477a ;;
    *) echo "fetch-onnxruntime: no official build for '$platform'" >&2; exit 1 ;;
esac

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL -o "$work/ort.tgz" \
    "https://github.com/microsoft/onnxruntime/releases/download/v${VERSION}/onnxruntime-${name}-${VERSION}.tgz"
actual="$(sha256_of "$work/ort.tgz")"
if [ "$actual" != "$sha256" ]; then
    echo "fetch-onnxruntime: checksum mismatch for onnxruntime-${name}-${VERSION}.tgz (got $actual)" >&2
    exit 1
fi
tar -xzf "$work/ort.tgz" -C "$work"
src="$work/onnxruntime-${name}-${VERSION}"
mkdir -p "$dest/lib"
# Only the runtime itself (cp -P keeps the soname symlinks); no headers, cmake files or dSYMs.
for f in "$src"/lib/libonnxruntime.so* "$src"/lib/libonnxruntime*.dylib; do
    [ -e "$f" ] || [ -L "$f" ] || continue
    cp -P "$f" "$dest/lib/"
done
# The macOS tarball ships the unversioned name as a second full copy; link it instead.
if [ -f "$dest/lib/libonnxruntime.dylib" ] && [ ! -L "$dest/lib/libonnxruntime.dylib" ]; then
    ln -sf "libonnxruntime.${VERSION}.dylib" "$dest/lib/libonnxruntime.dylib"
fi
cp "$src/LICENSE" "$src/ThirdPartyNotices.txt" "$dest/"
echo "fetch-onnxruntime: ONNX Runtime ${VERSION} (${name}) -> ${dest}/lib"
