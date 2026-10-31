# Research: E-book Format Support

All decisions were taken autonomously (the user asked for the feature to be carried through
implementation without check-ins). Each records what was chosen, why, and what was rejected.

## 1. Where format support plugs in

- **Decision**: Keep the archive layer's seven page functions (`list_pages`, `read_entry`,
  `read_entries`, `list_all_entries`, `read_page_dimensions`, `find_entry_by_suffix`, plus format
  detection) as the only integration point. A book is a container whose entries are synthetic page
  names (`0001.jpg`, `0002.webp`, …).
- **Rationale**: Every consumer (reader, thumbnails, page dimensions, bookmarks, OCR translation,
  OPDS, split analysis, patches) already funnels through these functions; the PDF work proved the
  approach end to end with zero downstream changes.
- **Alternatives**: A separate reader/route per format (what Kavita does for PDF/EPUB) — rejected:
  duplicates every downstream feature.

## 2. Process boundary

- **Decision**: All book parsing and rendering runs in one worker binary, `lanrurugi-book-worker`,
  driven by a synchronous, pooled client in `lanrurugi-book` (the PDF client generalised). The main
  server only holds the pool, inspection cache handles and page bytes in flight.
- **Rationale**: PDFium (C++) and DjVuLibre (C++) parse untrusted files and must be isolated. The
  pure-Rust parsers could run in-process safely, but the text typesetter needs a font database and
  glyph caches (tens of MB) that would break the main process's 256 MB peak budget; one worker
  binary keeps one code path, one protocol and one pool.
- **Alternatives**: Separate worker per engine — rejected (duplicated pool/protocol, more processes);
  pure-Rust formats in-process — rejected for the memory reason above.

## 3. Crate layout and rename

- **Decision**: Rename `lanrurugi-pdf` → `lanrurugi-book` and `lanrurugi-pdf-worker` →
  `lanrurugi-book-worker` (never committed under the old names, so no migration). Worker modules:
  `backend/{pdf,djvu,epub,mobi,fb2,txt}.rs`, shared `flow/` (document model + typesetter),
  `markup.rs` (tolerant HTML/XHTML → flow blocks, shared by EPUB and MOBI).
- **Rationale**: one name for one concept; shared code lives once.

## 4. Engines and libraries

| Format | Engine | Notes |
|---|---|---|
| PDF | PDFium via `pdfium-render` 0.9.4 (`pdfium_7543`) | unchanged from the PDF work |
| DjVu | DjVuLibre command-line tools (`djvused`, `ddjvu`) run by the worker | DjVuLibre is GPL-2; invoking its programs keeps them separate works, so the project stays MIT. Linking `libdjvulibre` would make the worker GPL. |
| EPUB | `zip` (central directory, random access, streaming entries) + `quick-xml` | no full unzip, no DOM |
| MOBI/AZW/AZW3 | own reader over `memmap2` | PalmDB/MOBI/EXTH headers, PalmDOC LZ77; images are byte slices of the mapping |
| FB2 | `quick-xml` streaming + `base64` | binaries decoded only when their page is requested |
| TXT | `chardetng` + `encoding_rs` | both already in the workspace |
| Typesetting | `cosmic-text` 0.14 (shaping, line breaking incl. CJK, font fallback) + its swash glyph cache | rasterised straight into an RGB buffer, encoded with `webpx` |

- **Rejected**: MuPDF/Poppler (AGPL/GPL); `hayro` (pure-Rust PDF, still incomplete); `mobi` crate
  (reads whole file into memory, no AZW3 image handling we need); `html5ever`/`scraper` (full DOM for
  what is a block-level text extraction); Typst/HTML engines for EPUB layout (far beyond the
  block-level fidelity this feature promises).

## 5. Image books vs text books

- **Decision**: Each backend decides per book:
  - EPUB: *image book* when the package is `rendition:layout pre-paginated`, or every spine item is a
    single image with no text; otherwise *flow book* (images inside the flow).
  - MOBI/AZW3: *image book* when EXTH 122 (`fixed-layout`) is `true`, or the text holds less than 32
    visible characters per image; otherwise *flow book*.
  - FB2, TXT: always flow books.
- Image pages are served as their stored bytes (`PageSource::Image(kind)`), flow pages are typeset
  and rendered (`PageSource::Render`).

## 6. Typesetting

- Page: 1200 × 1700 px, margins 96 px, body 36 px / line height 1.6, headings 1.5–1.25× scale, image
  blocks scaled to the text width (never upscaled more than 2×) and moved to the next page when they
  do not fit; an image taller than the page is scaled to fit it.
- Font: `Noto Serif CJK {SC|JP|KR}` chosen from the book language (EPUB `dc:language`, FB2
  `lang`, otherwise kana → JP, hangul → KR, else SC); cosmic-text's fallback covers anything missing.
  The family's Latin glyphs are used for Latin text, so mixed text never loses characters.
- Pagination runs once per book at inspection and records page starts as `(block, line)`; rendering
  a page re-shapes only the blocks on that page. Shaped buffers are dropped as soon as a block is
  measured, so memory is bounded by one block, not by the book.
- Horizontal only (see spec assumptions).

## 7. Tables of contents

| Format | Source | Mapping to page |
|---|---|---|
| PDF | outline (bookmarks) | destination page |
| DjVu | `djvused -e print-outline` | `#<page>` / `#+<n>` targets |
| EPUB | EPUB 3 `nav` (`epub:type="toc"`), else EPUB 2 NCX | target file → first page of that spine item (fragment ignored) |
| FB2 | `section/title` nesting | page of the title block |
| TXT | chapter-heading lines (`第…章/回/节/卷/部/篇/話`, `序章`, `楔子`, `番外`, `后记/後記/あとがき`, `Chapter N`, `Prologue`, `Epilogue`) | page of the heading block |
| MOBI/AZW3 (text) | `h1`–`h3` headings | page of the heading block |

Imported once when the archive is first catalogued (or healed) and its chapter list is empty; one
entry per page, first wins; page numbers 1-based (the reader's numbering).

## 8. DRM / encryption

- PDF: PDFium password error. EPUB: `META-INF/encryption.xml` encrypting anything other than fonts
  (IDPF/Adobe font obfuscation is not DRM). MOBI: record-0 encryption field ≠ 0. DjVu has no DRM.
- All map to `ErrorKind::Encrypted`; the scanner logs the reason and catalogues 0 pages; heal keeps
  retrying e-books (cheap, fails fast).

## 9. Format detection

- Extension first; content sniffing confirms or overrides: `%PDF`, `AT&TFORM` (DjVu), `BOOKMOBI` at
  offset 60 (MOBI/AZW/AZW3), zip + `mimetype` = `application/epub+zip`, `<FictionBook` in the first
  4 KB. `.txt` is accepted by extension only and only at ≥ 8 KB.

## 10. Memory and resources

- Main process: unchanged by format count — a `BookInfo` per open book (`Arc`, shared, memory LRU
  of 512) and pooled sockets.
- Worker: one open document per worker (PDF/DjVu/EPUB/MOBI handles), one laid-out flow book in an
  LRU of 2, `memmap2` for MOBI, streaming XML everywhere, `bytes` for payloads. Workers exit after 5
  idle minutes; the pool is capped at 30% of cores (project rule).
- Timeouts: inspect 300 s (long novels are typeset during inspection), page 60 s.
