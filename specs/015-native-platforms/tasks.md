# Tasks: Native multi-platform builds and releases

**Input**: Design documents from `/specs/015-native-platforms/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

## Phase 1: Setup

- [X] T001 [P] Extend `scripts/fetch-pdfium.sh` with `mac-arm64` and a portable sha256 check (`sha256sum` or `shasum -a 256`), keeping the Linux checksums
- [X] T002 [P] Add `scripts/fetch-onnxruntime.sh <platform> <dest>` (linux-x64, linux-aarch64, osx-arm64; pinned 1.27.0 with checksums from research §2) that extracts the real library file plus its symlinks and LICENSE
- [X] T003 [P] Fix the ARM64 container image: arch-aware ONNX Runtime and geoipupdate downloads in `Dockerfile` (done before this spec)

## Phase 2: Foundational (portability; blocks every story)

- [X] T004 Replace the `/proc/self/status` read with `sysinfo` in `crates/lanrurugi-api/src/archive_split.rs`
- [X] T005 [P] Make `prctl(PR_SET_PDEATHSIG)` Linux-only in `crates/lanrurugi-book-worker/src/backend/djvu.rs`
- [X] T006 [P] Per-language CJK font fallback list (Noto CJK, Noto SC/JP/KR, Source Han, macOS system fonts) and macOS/BSD font directories, with a unit test, in `crates/lanrurugi-book-worker/src/flow/typeset.rs`
- [X] T007 Install-prefix discovery that fills unset location variables per `contracts/discovery.md`, with unit tests, in `crates/lanrurugi-server/src/install_layout.rs`; call it first in `crates/lanrurugi-server/src/main.rs`
- [X] T008 CPU ONNX Runtime path for the GPU worker from `ORT_DYLIB_PATH` before the Linux default in `crates/lanrurugi-api/src/gpu_worker_client.rs`
- [X] T009 Start-up capability summary (PDFium, ONNX Runtime, DjVuLibre, Deno, models) logged once in `crates/lanrurugi-server/src/main.rs`; PDFium load failure reads as "PDF support unavailable" in `crates/lanrurugi-book-worker/src/backend/pdf.rs`

## Phase 3: User Story 1 — native Linux (P1)

**Goal**: one package that runs on a clean glibc Linux with system packages only.
**Independent test**: quickstart "Local Linux package".

- [X] T010 [US1] `scripts/package-native.sh <platform> <version> <out-dir>` assembling the layout of `contracts/package-layout.md` from built binaries, `apps/frontend/dist`, `plugins/`, docs output, fetched PDFium/ONNX Runtime and licences
- [X] T011 [P] [US1] Service definitions and env example in `packaging/service/{lanrurugi.service,org.lanrurugi.server.plist,lanrurugi.rc,lanrurugi.env.example}`, package README in `packaging/README.md`, model script `packaging/fetch-models.sh`
- [X] T012 [US1] `scripts/smoke-native.sh <package.tar.gz>`: unpack, start a throwaway Redis, start the server with only library/data locations, wait for `/api/info` and `/`, check the start-up summary, stop
- [X] T013 [US1] `package-native` task in `.mise.toml` building release binaries in the build container and calling `scripts/package-native.sh` for linux-x86_64
- [X] T014 [US1] Run the quickstart locally: package, then smoke-test in a clean `debian:bookworm-slim` container with Redis/Deno/DjVuLibre/fonts installed per the guide, including the "PDFium removed" variant

## Phase 4: User Story 2 — native macOS arm64 (P1)

**Goal**: same package flow on Apple Silicon. **Independent test**: release workflow macOS job.

- [X] T015 [US2] macOS job in `.github/workflows/native-release.yml` (`macos-14`, `brew install redis deno xz lz4 djvulibre`, `MACOSX_DEPLOYMENT_TARGET=13.0`), packaging and smoke test

## Phase 5: User Story 3 — native FreeBSD (P2)

**Goal**: core features on FreeBSD; PDF/OCR through pkg. **Independent test**: FreeBSD job.

- [X] T016 [US3] FreeBSD job in `.github/workflows/native-release.yml` (`vmactions/freebsd-vm`, `pkg install` toolchain and runtime deps, rustup-pinned Rust), packaging (no bundled libs) and smoke test

## Phase 6: User Story 4 — documentation (P2)

- [X] T017 [P] [US4] Native install guide (zh) `docs/src/content/docs/guide/install-native.md`: per-platform prerequisites with commands, unpack/run, service setup, models, feature availability table, HarmonyOS status
- [X] T018 [P] [US4] English guide `docs/src/content/docs/en/guide/install-native.md`; sidebar entry in `docs/astro.config.mjs`; links from both install pages
- [X] T019 [US4] Homepage start section: container and native routes, per-platform note, link to the guide, in `homepage/src/index.html` and `homepage/src/copy.mjs` (zh/en/ja)

## Phase 7: User Story 5 — release automation (P3)

- [X] T020 [US5] Linux x86_64/aarch64 jobs, shared frontend/docs build job and release-attach job with `SHA256SUMS` in `.github/workflows/native-release.yml`

## Phase 8: Polish

- [X] T021 `mise run check`, affected-crate tests and clippy; README (zh/en/ja) deployment sections and "Improvements" entry; CLAUDE.md feature list
- [X] T022 Restore `.specify/feature.json` to the feature that was active before this one

## Dependencies

Phase 1 and 2 before all stories. US1 (T010–T014) defines the package and smoke test that US2, US3
and US5 reuse. US4 can proceed in parallel with US2/US3. T020 completes the workflow started in T015/T016.

## Implementation strategy

MVP = Phases 1–3 (verifiable locally). macOS/FreeBSD/aarch64 are verified by the workflow after a
push; the report states that explicitly.

## Implementation Record (2026-10-19)

Verified locally:

- `mise run package-native` builds `lanrurugi-<version>-linux-x86_64.tar.gz` (55 MB). In a clean
  `debian:bookworm-slim` container with only the guide's packages: no missing shared libraries; the
  server finds its frontend, plugins, docs, PDFium and ONNX Runtime from the install prefix while
  started from `/`; `/api/info`, `/` and `/docs/` answer; a sample PDF and a chaptered CJK text book
  are catalogued with no unreadable file (strict mode). With PDFium removed the server still starts,
  the start-up summary lists PDF as unavailable, and the PDF reports why.
- The dev container is unaffected (every location is set explicitly; nothing is filled from the prefix).
- Tests: lanrurugi-server (install-prefix discovery, component checks), lanrurugi-book-worker (font
  fallback), lanrurugi-api (portable memory readings), lanrurugi-storage (fresh-instance config,
  against a real scratch Redis); workspace clippy clean; `mise run check` passes.

Found and fixed while verifying: the release binary linked `libxml2` (libarchive's XAR support,
picked up from the build machine) and would not start on a minimal system — release builds now use
`packaging/libarchive-portable.cmake`; a brand-new instance (empty Redis) answered every request
with 500 because `LRR_CONFIG.guestmode` is required — bootstrap now seeds it only when `LRR_CONFIG`
does not exist at all (an existing config missing the field is still left to the 007 migration);
the ARM64 container image bundled x86 ONNX Runtime and an amd64 geoipupdate package.

Not verified: macOS arm64, FreeBSD x86_64 and Linux aarch64 — they are built and smoke-tested only
by `.github/workflows/native-release.yml`, which has not run (needs a push). FreeBSD's `pdfium`
package is chromium 8066 while the bindings target 7543; the workflow's strict smoke test with a
real PDF is what will show whether they are compatible.
