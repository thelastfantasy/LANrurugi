# Research: native multi-platform builds

## 1. Target matrix and build hosts

- **Decision**: four release targets — `linux-x86_64` (GitHub `ubuntu-22.04`), `linux-aarch64`
  (`ubuntu-22.04-arm`), `macos-arm64` (`macos-14`, deployment target 13.0), `freebsd-x86_64`
  (FreeBSD 14.x inside `vmactions/freebsd-vm` on an Ubuntu runner).
- **Rationale**: Ubuntu 22.04 gives a glibc 2.35 baseline that runs on Debian 12, Ubuntu 22.04+,
  Fedora 36+, Arch, openSUSE Leap 15.5+; GitHub's free Apple Silicon and ARM Linux runners build
  natively, so no cross toolchains; FreeBSD has no hosted runner, the VM action is the standard route.
- **Alternatives**: musl static builds (rejected: ONNX Runtime and PDFium ship glibc-only binaries);
  cross-compiling macOS/BSD from Linux with zig (rejected: C dependencies built through CMake and
  Apple frameworks make it fragile); Intel macOS (out of scope per the user).

## 2. Third-party runtime components per platform

| Component | Linux | macOS arm64 | FreeBSD | Delivery |
|---|---|---|---|---|
| Redis ≥ 7.4 | distro / redis.io repo | `brew install redis` | `pkg install redis` | user installs |
| Deno 2.x (plugins) | official binary / `deno` pkg | `brew install deno` | `pkg install deno` | user installs |
| DjVuLibre CLI | `djvulibre-bin` / `djvulibre` | `brew install djvulibre` | `pkg install djvulibre` | user installs |
| CJK fonts | Noto CJK packages | system fonts (PingFang, Hiragino, Songti, Apple SD Gothic Neo) | `pkg install noto-serif-sc noto-sans-sc noto-serif-jp noto-sans-jp …` | user installs (macOS: none needed) |
| PDFium | bblanchon `pdfium-linux-{x64,arm64}` | bblanchon `pdfium-mac-arm64` | `pkg install pdfium` (`lib/libpdfium.so`) | bundled (Linux/macOS), pkg (FreeBSD) |
| ONNX Runtime 1.27 | official `linux-{x64,aarch64}` | official `osx-arm64` | `pkg install onnxruntime` | bundled (Linux/macOS), pkg (FreeBSD) |
| liblzma / liblz4 (libarchive codecs) | always present | `brew install xz lz4` | base / `pkg install lz4` | user installs (macOS, FreeBSD) |
| Models (OCR, inpaint, bubble segmentation) | download script | download script | download script | script |

- **Decision**: bundle only what may be redistributed and has an official prebuilt binary (PDFium
  BSD-3, ONNX Runtime MIT, with their licence files); everything else comes from the system package
  manager, so security updates flow through it.
- **Redis minimum**: the image ships 7.4; data written by the image needs ≥ 7.4 (RDB format), a fresh
  native install works with any 7.x. The guide asks for ≥ 7.4.
- Checksums: PDFium chromium/7543 `mac-arm64` `41c26972…`; ONNX Runtime 1.27.0 `osx-arm64`
  `545e81c5…`, `linux-x64` `547e40a4…`, `linux-aarch64` `3e4d83ac…`.

## 3. Linux-only code

| Site | Change |
|---|---|
| `archive_split.rs` reads `/proc/self/status` for RSS | use `sysinfo` (already a dependency) on every OS |
| `book-worker/backend/djvu.rs` `prctl(PR_SET_PDEATHSIG)` | Linux only; elsewhere the DjVu child is bounded by the request timeout and the worker's own exit |
| `book-worker/flow/typeset.rs` Noto-only families, Linux font dirs | per-language family fallback list (Noto CJK, Noto SC/JP/KR, Source Han, macOS system fonts), plus macOS/BSD font dirs |
| `gpu_worker_client.rs` CPU ORT default `/usr/local/lib/ort-cpu/libonnxruntime.so` | resolved from the install layout or `ORT_DYLIB_PATH`; GPU vendor probing stays Linux-only (`lspci`) |
| Unix-domain sockets, `std::os::unix` | portable across all targets — no change |

## 4. Finding resources without configuration

- **Decision**: at the very start of `main`, before argument parsing and before any thread exists,
  the server derives its install prefix (`<exe>/..` after resolving symlinks) and, for every
  location variable that is unset, sets it to the install-relative path **if that path exists**.
  The CLI's own `env`-backed defaults then pick them up, and every child worker inherits them.
- **Rationale**: one place, no change to the dozen crates that already read these variables, and
  explicit settings still win (FR-003). Inside the container nothing changes: the image sets them all.
- **Alternatives**: a wrapper shell script per platform (rejected: breaks when users call the binary
  directly or from a service manager); threading a prefix through every crate (rejected: invasive).

## 5. Optional components missing

- PDFium, ONNX Runtime, models, DjVuLibre, Deno each already fail per request with a specific error;
  the change adds one start-up summary log line listing what is unavailable (FR-006) and makes sure
  the book worker reports a missing PDFium as `Unreadable("PDF support unavailable: …")` (already the
  case through `Failure::internal("cannot load PDFium")` — reworded to say it is unavailable).

## 6. Release automation

- **Decision**: new workflow `native-release.yml` on `v*` tags: one job builds the frontend, docs
  and plugin bundle once (Ubuntu) and uploads them; four platform jobs build the Rust binaries,
  assemble the package with `scripts/package-native.sh`, run `scripts/smoke-native.sh` (start the
  server against a throwaway Redis and an empty library, wait for `/api/info`, stop), and upload; a
  final job attaches all packages and `SHA256SUMS` to the GitHub release.
- **Local verification**: the Linux x86_64 package is built through a mise task in the build
  container and smoke-tested in a clean `debian:bookworm-slim` container with Redis and Deno installed
  the way the guide says — the closest local stand-in for a user's machine. macOS and FreeBSD are
  verified only by the workflow, after a push.

## 7. HarmonyOS / OpenHarmony

- **Decision**: not supported in this feature; documented.
- **Findings**: HarmonyOS NEXT runs apps in a sandbox with no supported way to run a long-lived
  background server, and none of Redis, Deno, PDFium or ONNX Runtime has a HarmonyOS build. Rust has
  an `aarch64-unknown-linux-ohos` target, so the core could be cross-compiled, but without a database
  server it cannot run. The realistic HarmonyOS path is a native reader **client** for an existing
  LANrurugi server (a separate future project), not running the server on the device.
