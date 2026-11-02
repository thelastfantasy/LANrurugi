# Implementation Plan: E-book Format Support

**Branch**: `013-ebook-formats` | **Date**: 2026-10-18 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/013-ebook-formats/spec.md`

## Summary

Generalise the (uncommitted) PDF support into one e-book layer: `lanrurugi-book` (format detection,
`BookInfo`, pooled synchronous worker client, caches) and `lanrurugi-book-worker` (one backend per
format plus a shared flow typesetter). The archive layer's page functions dispatch any e-book to
it, so every existing page consumer keeps working unchanged. Picture pages are served as stored
bytes; PDF/DjVu pages are rendered; reflowable text (EPUB, MOBI/AZW3, FB2, TXT) is typeset into
fixed pages. Each format's table of contents becomes the archive's initial chapter list.

## Technical Context

**Language/Version**: Rust 1.99 (workspace edition 2021); TypeScript/React 19 (two small frontend
touches)

**Primary Dependencies**: `pdfium-render` 0.9.4 + PDFium chromium/7543; DjVuLibre tools
(`djvused`, `ddjvu`); `zip` 8; `quick-xml` 0.38; `base64` 0.22; `memmap2` 0.9; `cosmic-text` 0.14;
`chardetng`/`encoding_rs`; `image` 0.25; `webpx`

**Storage**: Redis archive records unchanged; caches under `<temp>/book/<file-identity>/`

**Testing**: `cargo test` via `mise run test-crate`; generated fixtures for every format; live
verification in the dev container

**Target Platform**: Linux containers (amd64/arm64), Debian bookworm (prod) / Ubuntu 24.04 (dev)

**Project Type**: web service (Rust workspace) + web frontend

**Performance Goals**: first view of a text page < 2 s, cached < 100 ms; picture pages served
without re-encode

**Constraints**: main process memory unaffected (worker isolation); worker pool ≤ 30% of cores;
inspect timeout 300 s, page 60 s; no GPL linking

**Scale/Scope**: 6 formats, libraries of thousands of books, single books up to thousands of pages

## Constitution Check

| Principle | Status |
|---|---|
| I. Legacy data compatibility | ✅ No schema change; books are ordinary archive records; IDs unchanged (same size-aware ID over the file). Chapter import never overwrites user edits. |
| II. API contract fidelity | ✅ No existing endpoint shape changes; one additive query parameter (`inline`) already in place. |
| III. Resource-conscious architecture | ✅ C/C++ engines and the font-heavy typesetter isolated in subprocesses (same justification as the embed/GPU workers); pool capped at 30% of cores; streaming parsers, mmap, no whole-book buffering. The single long-lived server process is unchanged. |
| IV. Plugin sandboxing | n/a |
| V. Secrets & trust boundaries | ✅ Untrusted files parsed only in the worker; DjVu tools invoked with argument vectors (no shell). |
| VI. Phased scope | ✅ Additive to Phase 1; does not touch Phase 2 code paths. |
| VII. Frontend discipline | ✅ Two conditional render guards only; existing legacy classes and i18n keys. |
| Licensing | ✅ PDFium BSD; DjVuLibre GPL used as separate programs (aggregation); all crates MIT/Apache. |

Post-design re-check: unchanged — no violations, Complexity Tracking empty.

## Project Structure

### Documentation (this feature)

```text
specs/013-ebook-formats/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/{worker-protocol.md,http.md}
├── checklists/requirements.md
└── tasks.md
```

### Source Code

```text
crates/lanrurugi-book/                 (renamed from lanrurugi-pdf)
├── src/lib.rs                         public API: detect, inspect, read_page, page names
├── src/format.rs                      BookFormat + extension/content detection
├── src/protocol.rs                    frames, Request/Response, BookInfo, PageSource, OutlineItem
└── src/client.rs                      worker pool, timeouts, info/page caches
crates/lanrurugi-book-worker/          (renamed from lanrurugi-pdf-worker)
├── src/main.rs                        request loop, panic isolation, idle exit
├── src/backend/mod.rs                 Backend dispatch, open-document cache
├── src/backend/pdf.rs                 PDFium (moved from main.rs)
├── src/backend/djvu.rs                djvused/ddjvu
├── src/backend/epub.rs                OPF/spine/nav/NCX/encryption
├── src/backend/mobi.rs                PalmDB/MOBI/EXTH/PalmDOC over mmap
├── src/backend/fb2.rs                 streaming FB2
├── src/backend/txt.rs                 encoding detection + chapter detection
├── src/markup.rs                      tolerant (X)HTML → flow blocks (EPUB, MOBI)
├── src/flow/mod.rs                    FlowBook, Block, Lang
├── src/flow/typeset.rs                pagination + rasterisation (cosmic-text)
└── tests/*.rs                         per-format end-to-end tests with generated fixtures
crates/lanrurugi-scanner/src/{archive_format.rs,watcher.rs,pipeline.rs,full_scan.rs}
crates/lanrurugi-api/src/{archive_split.rs,archives.rs}
crates/lanrurugi-server/src/main.rs    configure cache dir
apps/frontend/src/pages/Reader/{Reader.tsx,ArchiveOverviewOverlay/ArchiveOverviewOverlayPanel.tsx}
Dockerfile, Dockerfile.dev, Dockerfile.build, scripts/fetch-pdfium.sh, .mise.toml
```

**Structure Decision**: two crates (library + worker binary) mirroring the existing
embed/GPU worker pattern; per-format backends as modules of the worker sharing one flow typesetter
and one markup parser.

## Complexity Tracking

None.
