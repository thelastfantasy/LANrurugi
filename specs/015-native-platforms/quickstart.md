# Quickstart: validating native packages

## Automated

```sh
mise run test-crate -- lanrurugi-server lanrurugi-book-worker lanrurugi-api
```

Covers install-prefix discovery (unset variables filled only when the path exists, set variables
untouched, symlinked executables) and the CJK font fallback order.

## Local Linux package (stand-in for a user's machine)

1. `mise run package-native` — builds release binaries, the frontend, docs and plugins in the build
   container and writes `dist/native/lanrurugi-<version>-linux-x86_64.tar.gz`.
2. `scripts/smoke-native.sh dist/native/lanrurugi-<version>-linux-x86_64.tar.gz` inside a clean
   `debian:bookworm-slim` container that has only `redis-server`, Deno, `djvulibre-bin` and
   `fonts-noto-cjk` installed the way the install guide says.
3. Expected: the server starts with only the library location set, `/api/info` answers, the web
   interface is served, the start-up log lists no unavailable features, a PDF and a text e-book in the
   library get page counts, and a metadata plugin run succeeds.
4. Remove PDFium from `lib/` and repeat: the server still starts, the log lists PDF as unavailable,
   the PDF archive reports why it cannot be read.

## Release workflow (macOS, FreeBSD, Linux aarch64)

Push a `v*` tag to a fork. Expected: `native-release.yml` produces four packages and `SHA256SUMS`;
every platform job's smoke test (same script) passes. macOS and FreeBSD are only verified here.

## Container image on ARM64

Build the image for `linux/arm64` (`docker buildx build --platform linux/arm64 .`): the build
succeeds and `ldd`/`file` on `/usr/local/lib/libonnxruntime.so.1.27.0` reports AArch64.
