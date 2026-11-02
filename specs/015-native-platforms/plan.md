# Implementation Plan: Native multi-platform builds and releases

**Branch**: `015-native-platforms` | **Date**: 2026-10-19 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/015-native-platforms/spec.md`

## Summary

Make the existing single server binary and its helper workers run outside the container on Linux
(x86_64, aarch64), macOS (Apple Silicon) and FreeBSD (x86_64), and publish one release package per
platform. Three kinds of work: (1) remove the few Linux-only code paths and add macOS/BSD font
fallbacks; (2) have the server find its own resources relative to its install prefix when nothing is
configured, by filling unset location variables at start-up; (3) a packaging script, a smoke test,
a tag-triggered release workflow, service definitions, and per-platform install documentation on the
homepage and docs site. The ARM64 container image's x86-only ONNX Runtime/geoipupdate downloads are
already fixed. HarmonyOS is documented as unsupported (research §7).

## Technical Context

**Language/Version**: Rust 1.99 (workspace, pinned in `.mise.toml`); POSIX `sh` for packaging scripts;
GitHub Actions YAML

**Primary Dependencies**: existing crates; `sysinfo` (already a dependency) replaces `/proc` reads;
bundled PDFium chromium/7543 and ONNX Runtime 1.27.0 per platform (research §2)

**Storage**: Redis (user-installed, ≥ 7.4); filesystem library/thumb/temp dirs as today

**Testing**: `cargo test` through `mise run test-crate`; new unit tests for install-prefix discovery
and font fallback; `scripts/smoke-native.sh` run per platform in CI and locally in a clean Debian
container

**Target Platform**: Linux glibc ≥ 2.35 (x86_64, aarch64), macOS 13+ arm64, FreeBSD 14 x86_64

**Project Type**: web service (single Rust binary + helper processes + static frontend)

**Performance Goals**: no change; native runs avoid container/VM overhead on macOS

**Constraints**: explicit configuration always wins over discovery; container behaviour unchanged;
no new runtime dependency in the main process

**Scale/Scope**: 4 release targets; ~6 code sites; 3 new scripts; 1 workflow; docs in zh/en (site)
and zh/en/ja (homepage)

## Constitution Check

| Principle | Status |
|---|---|
| I. Legacy data compatibility | Unaffected; the guide states Redis ≥ 7.4 so container-written data opens natively. |
| II. API contract fidelity | No API change. |
| III. Single-process architecture | Unchanged: the same one server process; workers stay subprocesses. |
| IV. Sandboxed plugins | Unchanged; Deno remains a subprocess. The constitution's pinned-Deno rule targets the production *image* (still pinned); native users install Deno from their package manager and the guide states the minimum version — justified in Complexity Tracking. |
| V. Secrets & network trust | No change. |
| VI. Phased scope | Additive to Phase 1 deployment; no Phase 2 dependency. |
| VII. Frontend discipline | Homepage/docs copy only; no app UI change. |

Gate: **pass** (one justified deviation, below). Re-checked after Phase 1 design: unchanged.

## Project Structure

### Documentation (this feature)

```text
specs/015-native-platforms/
├── plan.md
├── research.md
├── data-model.md          # package layout + platform capability matrix
├── quickstart.md
├── contracts/
│   ├── package-layout.md  # what is where inside a release package
│   └── discovery.md       # which variable is filled from which install-relative path
└── tasks.md
```

### Source Code (repository root)

```text
crates/lanrurugi-server/src/install_layout.rs   # new: prefix discovery, fills unset location vars
crates/lanrurugi-server/src/main.rs             # calls it first; start-up capability summary
crates/lanrurugi-api/src/archive_split.rs       # /proc → sysinfo
crates/lanrurugi-api/src/gpu_worker_client.rs   # CPU ORT path from ORT_DYLIB_PATH / layout
crates/lanrurugi-book-worker/src/backend/djvu.rs  # prctl Linux-only
crates/lanrurugi-book-worker/src/flow/typeset.rs  # font family fallbacks, macOS/BSD dirs
scripts/fetch-pdfium.sh                         # + mac-arm64, portable sha256
scripts/fetch-onnxruntime.sh                    # new
scripts/package-native.sh                       # new: assemble a release package
scripts/smoke-native.sh                         # new: start + probe + stop
packaging/                                      # service definitions + package README
.github/workflows/native-release.yml            # new
.mise.toml                                      # package-native task (build container)
docs/src/content/docs/{,en/}guide/install-native.md
homepage/src/{index.html,copy.mjs}              # start section
```

**Structure Decision**: no new crate; discovery lives in the server binary because only it starts
everything else, and children inherit the filled environment.

## Complexity Tracking

| Deviation | Why needed | Simpler alternative rejected because |
|---|---|---|
| Deno for native installs comes from the user's package manager, not a pinned, verified download | Native packages must not bundle a second copy of a JS runtime the OS already packages and patches | Bundling a pinned Deno per platform would ship an unpatched runtime outside the OS update path |
