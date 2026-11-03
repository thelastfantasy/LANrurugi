#!/bin/sh
# Assembles a native release package (layout: specs/015-native-platforms/contracts/package-layout.md).
#
# Usage: package-native.sh <platform> <version> <bin-dir> <assets-dir> <out-dir>
#   platform    linux-x86_64 | linux-aarch64 | macos-arm64 | freebsd-x86_64
#   bin-dir     directory holding the release binaries (target/release)
#   assets-dir  output of scripts/build-native-assets.sh (frontend/, docs/, plugins/)
#   out-dir     where lanrurugi-<version>-<platform>.tar.gz is written
set -eu

usage="usage: package-native.sh <platform> <version> <bin-dir> <assets-dir> <out-dir>"
platform="${1:?$usage}"
version="${2:?$usage}"
bin_dir="${3:?$usage}"
assets="${4:?$usage}"
out="${5:?$usage}"
repo="$(cd "$(dirname "$0")/.." && pwd)"

case "$platform" in
    linux-x86_64 | linux-aarch64 | macos-arm64) bundle_libs=yes ;;
    freebsd-x86_64) bundle_libs=no ;;
    *) echo "package-native: unknown platform '$platform'" >&2; exit 1 ;;
esac

name="lanrurugi-${version}-${platform}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
root="$stage/$name"
mkdir -p "$root/bin" "$root/share/lanrurugi" "$root/share/licenses" "$root/scripts"

for binary in lanrurugi-server lanrurugi-book-worker lanrurugi-embed-worker lanrurugi-gpu-worker; do
    if [ ! -x "$bin_dir/$binary" ]; then
        echo "package-native: missing $bin_dir/$binary" >&2
        exit 1
    fi
    cp "$bin_dir/$binary" "$root/bin/"
done

for dir in frontend docs plugins; do
    if [ ! -d "$assets/$dir" ]; then
        echo "package-native: missing $assets/$dir (run scripts/build-native-assets.sh)" >&2
        exit 1
    fi
    cp -R "$assets/$dir" "$root/share/lanrurugi/$dir"
done
mkdir -p "$root/share/lanrurugi/models"

if [ "$bundle_libs" = yes ]; then
    libs="$stage/libs"
    pdfium_platform="$platform"
    [ "$platform" = macos-arm64 ] && pdfium_platform=mac-arm64
    sh "$repo/scripts/fetch-pdfium.sh" "$pdfium_platform" "$libs/pdfium"
    sh "$repo/scripts/fetch-onnxruntime.sh" "$platform" "$libs/onnxruntime"
    mkdir -p "$root/lib" "$root/share/licenses/pdfium" "$root/share/licenses/onnxruntime"
    cp -P "$libs"/pdfium/lib/* "$libs"/onnxruntime/lib/* "$root/lib/"
    cp -R "$libs/pdfium/LICENSE" "$libs/pdfium/licenses" "$root/share/licenses/pdfium/"
    cp "$libs/onnxruntime/LICENSE" "$libs/onnxruntime/ThirdPartyNotices.txt" "$root/share/licenses/onnxruntime/"
fi

cp "$repo/scripts/fetch-ocr-model.sh" "$repo/scripts/fetch-inpaint-model.sh" \
    "$repo/scripts/fetch-bubble-seg-model.sh" "$root/scripts/"
cp -R "$repo/packaging/service" "$root/service"
cp "$repo/packaging/README.md" "$repo/packaging/fetch-models.sh" "$repo/packaging/install-service.sh" "$root/"
cp "$repo/LICENSE" "$root/"
chmod +x "$root"/scripts/*.sh "$root/fetch-models.sh" "$root/install-service.sh" "$root/service/lanrurugi.rc"

mkdir -p "$out"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
echo "package-native: $out/$name.tar.gz"
