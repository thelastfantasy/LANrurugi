# Tasks: E-book Format Support

**Input**: Design documents from `/specs/013-ebook-formats/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: included — every backend gets an end-to-end test with a generated fixture (quickstart).

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Setup

- [X] T001 Rename crates `crates/lanrurugi-pdf` → `crates/lanrurugi-book` and `crates/lanrurugi-pdf-worker` → `crates/lanrurugi-book-worker` (package/bin names, workspace members and `[workspace.dependencies]` in `Cargo.toml`, dependents in `crates/lanrurugi-scanner/Cargo.toml` and `crates/lanrurugi-server/Cargo.toml`)
- [X] T002 Add workspace dependencies `quick-xml`, `base64`, `memmap2`, `cosmic-text` in `Cargo.toml` and use them from `crates/lanrurugi-book-worker/Cargo.toml`; add `zip`, `chardetng`, `encoding_rs`
- [X] T003 [P] Install DjVuLibre tools (`djvulibre-bin`) in `Dockerfile`, `Dockerfile.dev` and `Dockerfile.build`; build/copy `lanrurugi-book-worker` instead of `lanrurugi-pdf-worker` in `Dockerfile`/`Dockerfile.dev`

## Phase 2: Foundational

- [X] T004 Add `BookFormat` with extension + content detection (`%PDF`, `AT&TFORM`, `BOOKMOBI`@60, EPUB zip mimetype, `<FictionBook`) in `crates/lanrurugi-book/src/format.rs`
- [X] T005 Generalise the protocol: `format` on requests, `PageSource::Image{kind}`, `BookInfo.format`, entry names for all image kinds, in `crates/lanrurugi-book/src/protocol.rs`
- [X] T006 Generalise the client (`inspect`/`read_page` take the detected format; cache dir `<temp>/book`; inspect timeout 300 s; disk-cache only `render` pages, stored pictures are re-read) in `crates/lanrurugi-book/src/client.rs` and `src/lib.rs`
- [X] T007 Split the worker into `src/main.rs` (loop, panic isolation, idle exit) and `src/backend/{mod.rs,pdf.rs}` with an open-document cache keyed by path+stamp in `crates/lanrurugi-book-worker/`
- [X] T008 Flow document model (`FlowBook`, `Block`, `Lang`, `ImageRef`) in `crates/lanrurugi-book-worker/src/flow/mod.rs`
- [X] T009 Typesetter: paginate a `FlowBook` into `PageStart`s and rasterise one page to WebP with cosmic-text (CJK font by language, headings, images placed/scaled, one-block memory bound) in `crates/lanrurugi-book-worker/src/flow/typeset.rs`
- [X] T010 Tolerant (X)HTML → blocks converter (headings, paragraphs, line breaks, images, entities, skip script/style) in `crates/lanrurugi-book-worker/src/markup.rs`
- [X] T011 Archive layer: replace the PDF branches with `is_book`/`BookFormat` dispatch in all seven functions in `crates/lanrurugi-scanner/src/archive_format.rs`; remove `epub` from `ARCHIVE_EXTENSIONS`
- [X] T012 Watcher: watch `mobi azw azw3 prc djvu djv fb2 txt`; ignore `.txt` under 8 KB in `crates/lanrurugi-scanner/src/watcher.rs`
- [X] T013 Generalise outline→ToC import and heal retry from PDF to all book formats in `crates/lanrurugi-scanner/src/pipeline.rs` and `crates/lanrurugi-scanner/src/full_scan.rs`

## Phase 3: User Story 1 - Image-based e-books (P1)

**Goal**: scanned PDF, fixed-layout EPUB, comic MOBI/AZW3, DjVu read like archives.

**Independent Test**: one file per format catalogued with correct page count; pages in book order; picture pages byte-identical.

- [X] T014 [US1] EPUB backend: container.xml → OPF, manifest, spine (linear), fixed-layout detection, image pages from spine in `crates/lanrurugi-book-worker/src/backend/epub.rs`
- [X] T015 [P] [US1] MOBI/AZW/AZW3 backend over mmap: PalmDB records, record-0 PalmDOC/MOBI/EXTH headers, image records (first-image index, magic sniffing, skip thumbnail EXTH 202), fixed-layout detection in `crates/lanrurugi-book-worker/src/backend/mobi.rs`
- [X] T016 [P] [US1] DjVu backend: page count/sizes via `djvused`, render via `ddjvu` (PPM → WebP), outline via `print-outline` in `crates/lanrurugi-book-worker/src/backend/djvu.rs`
- [X] T017 [US1] Tests: fixed-layout EPUB with spine order ≠ file order (byte-identical pages); comic MOBI fixture (PalmDB writer in test); DjVu (skipped without tools); existing PDF test in `crates/lanrurugi-book-worker/tests/`

## Phase 4: User Story 2 - Text e-books (P2)

**Goal**: reflowable EPUB, text MOBI/AZW3, FB2, TXT typeset into pages.

**Independent Test**: each text book catalogued; pages show CJK + Latin text and illustrations.

- [X] T018 [US2] EPUB flow mode: spine XHTML → blocks via `markup.rs`, images by zip entry, language from `dc:language` in `crates/lanrurugi-book-worker/src/backend/epub.rs`
- [X] T019 [P] [US2] MOBI text mode: PalmDOC LZ77 decompression of text records, trailing-entry stripping, HTML → blocks, `recindex` images in `crates/lanrurugi-book-worker/src/backend/mobi.rs`
- [X] T020 [P] [US2] FB2 backend: streaming sections/titles/paragraphs/images, binaries decoded on demand in `crates/lanrurugi-book-worker/src/backend/fb2.rs`
- [X] T021 [P] [US2] TXT backend: encoding detection (BOM, UTF-8, chardetng), paragraphs, chapter-heading detection in `crates/lanrurugi-book-worker/src/backend/txt.rs`
- [X] T022 [US2] Tests: reflowable EPUB (CJK + image), text MOBI, FB2, GBK TXT; typeset pages decode and are non-blank in `crates/lanrurugi-book-worker/tests/`

## Phase 5: User Story 3 - Chapters from the book (P2)

- [X] T023 [US3] EPUB outline: EPUB 3 nav (`epub:type="toc"`) else NCX, href → spine item → page (image and flow modes) in `crates/lanrurugi-book-worker/src/backend/epub.rs`
- [X] T024 [P] [US3] Flow outline: heading blocks → page (FB2 sections with depth, TXT chapters, MOBI h1–h3) in `crates/lanrurugi-book-worker/src/flow/typeset.rs`
- [X] T025 [US3] Tests: nav and NCX outlines map to correct pages; TXT chapters detected; user-edited ToC preserved (existing pipeline guard) in `crates/lanrurugi-book-worker/tests/`

## Phase 6: User Story 4 - Clear failures (P3)

- [X] T026 [US4] Encryption detection: EPUB `encryption.xml` (non-font algorithms), MOBI record-0 encryption, PDF password → `Encrypted` in respective backends
- [X] T027 [US4] Tests: DRM EPUB/MOBI and truncated files report errors; pool survives in `crates/lanrurugi-book-worker/tests/`

## Phase 7: User Story 5 - Format conveniences (P3)

- [X] T028 [US5] Split-suggestion guard and frontend button hiding for every book extension in `crates/lanrurugi-api/src/archive_split.rs` and `apps/frontend/src/pages/Reader/ArchiveOverviewOverlay/ArchiveOverviewOverlayPanel.tsx`; inline open stays PDF-only in `crates/lanrurugi-api/src/archives.rs`

## Phase 8: Polish

- [X] T029 Update `scripts/fetch-pdfium.sh`/`.mise.toml` references, CI test step fetches PDFium in `.github/workflows/ci.yml`
- [X] T030 Live verification in the dev container per `quickstart.md` (every format, RSS check, worker idle exit)
- [X] T031 `mise run check`, full affected-crate tests, README "Improvements over LANraragi" entry, CLAUDE.md feature list

## Dependencies

Setup → Foundational → US1 → US2 (reuses EPUB/MOBI backends) → US3 → US4 → US5 → Polish. Within
a phase, `[P]` tasks touch different files.

## Implementation Strategy

US1 is the MVP (image books, the library's core content); US2–US3 add text books and chapters on
the same typesetter; US4–US5 harden and finish.

## Implementation Record (2026-10-18)

Verified live in the dev container with generated and Project Gutenberg samples (`book-test-*`):

- Each of 10 books catalogued exactly once; pagecount and 1-based ToC checked in Redis for every
  format. Fixed-layout EPUB (spine reversed against file order) and comic AZW3 (pictures referenced
  in reverse record order): all 12 pages byte-identical to the stored pictures, in reading order.
- Text pages inspected visually: Japanese/Chinese/Russian (cp1251 FB2), GB18030 TXT (1.78 MB, 120
  回), EPUB, AZW3, MOBI 6; DjVu pages render and its outline imports; 9 PDF samples serve.
- DRM MOBI, AES-encrypted EPUB and truncated EPUB: logged with the reason, the reader shows it,
  the server keeps serving.
- Server RSS 77.8 → 78.5 MB after reading 106 pages; workers retired and reaped ~5 min after their
  last request; a request after that spawns a fresh worker.

Fixed during verification: idle workers that had exited were reused (first page after 5 idle
minutes failed with a broken pipe); exited workers were left as zombies until the next request; the
book cache key had no version; rendered pages were not counted against `tempmaxsize`; EPUB ToC
fragments resolved to the start of the file; MOBI 6 `<font size>` headings, `<mbp:pagebreak>` and
guide `filepos` ToC were ignored; KF8 text included its CSS flows; hard-wrapped TXT and EPUB text
converted from it split every line into a paragraph or glued chapter lines into paragraphs; chapter
detection took prose lines starting with 第N回/Part for headings; a one- or two-entry ToC was not
completed from the text.

Known limits: horizontal typesetting only; KF8 NCX index not read (headings/guide used instead);
HUFF/CDIC-compressed MOBI and KFX unsupported; the archive ToC holds one entry per page, so two
chapters starting on the same page keep the first; a chapter title glued to prose on the same line
(第一零四回 in the sample) is not detected.
