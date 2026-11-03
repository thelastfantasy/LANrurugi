# Feature Specification: E-book Format Support

**Feature Branch**: `013-ebook-formats`

**Created**: 2026-10-18

**Status**: Draft

**Input**: User description: "电子书格式支持：把已经实现（未提交）的 PDF 支持扩展为统一的电子书格式层，一次性支持 PDF、EPUB（固定版式与流式文字）、MOBI/AZW/AZW3（图片型漫画与文字书）、DjVu、FB2、TXT。所有格式都像现有 PDF 一样作为档案入库，按每页一张图在现有阅读器中阅读……"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read image-based e-books like any archive (Priority: P1)

A user drops manga and scanned books distributed as e-books — a scanned PDF, a fixed-layout EPUB
from a digital storefront, a comic MOBI/AZW3 produced by a comic converter, a DjVu scan — into the
library folder. Each appears in the library with a cover thumbnail and the right page count, opens
in the existing reader, and pages through exactly like a zip archive. Pages that are pictures are
shown at their original quality.

**Why this priority**: This is the library's core audience (manga, doujinshi, scans). Today these
files are either ignored or catalogued as empty 0-page books.

**Independent Test**: Put one file of each image-based format into the library; each is catalogued
with a thumbnail and correct page count, and every page displays in the reader in the book's own
order.

**Acceptance Scenarios**:

1. **Given** a fixed-layout EPUB whose image file names do not sort in reading order, **When** it
   is read, **Then** pages follow the book's own reading order, not file-name order.
2. **Given** a scanned PDF or comic MOBI/AZW3 whose pages are stored as JPEG pictures, **When** a
   page is requested as the original, **Then** the bytes served are the picture stored in the book,
   unmodified.
3. **Given** a DjVu scan, **When** it is read, **Then** every page is displayed as an image.
4. **Given** any of these books, **When** thumbnails, page dimensions, bookmarks, OCR translation or
   OPDS are used, **Then** they work the same as for a zip archive.

---

### User Story 2 - Read text e-books in the same reader (Priority: P2)

A user adds novels and text books — a reflowable EPUB, a text MOBI/AZW3, an FB2, a plain-text web
novel — in Chinese, Japanese or English. Each is typeset by the server into pages, so it can be
read page by page in the existing reader, including illustrations placed in the text.

**Why this priority**: Libraries commonly hold light novels and text books next to manga; without
this they cannot be opened at all.

**Independent Test**: Add one text book of each format containing Chinese, Japanese and Latin text
and an illustration; each is catalogued with a page count, and the pages show all text legibly with
the illustration in place.

**Acceptance Scenarios**:

1. **Given** a plain-text novel in a legacy Chinese encoding (e.g. GBK), **When** it is read,
   **Then** the text displays correctly, not as mojibake.
2. **Given** a reflowable EPUB with headings, paragraphs and an inline illustration, **When** it is
   read, **Then** headings stand out, paragraphs flow across pages, and the illustration is shown.
3. **Given** a text page mixing CJK and Latin characters, **When** it is displayed, **Then** no
   characters are missing.

---

### User Story 3 - Chapters come from the book itself (Priority: P2)

When a book carries its own table of contents — PDF bookmarks, an EPUB navigation document, DjVu
bookmarks, FB2 sections — or a plain-text novel has recognisable chapter headings ("第一章",
"Chapter 1"), the book's chapters appear in the reader's chapter list the first time the book is
catalogued, pointing at the right pages.

**Why this priority**: Navigating long books without chapters is painful, and the information is
already in the file.

**Independent Test**: Catalogue a book of each format with a known table of contents; the chapter
list matches it and each chapter opens the correct page.

**Acceptance Scenarios**:

1. **Given** a book with a nested table of contents, **When** it is catalogued, **Then** its entries
   appear in reading order, each at the page where that section begins.
2. **Given** a user has edited a book's chapter list, **When** the book is rescanned, **Then** the
   user's chapters are kept.

---

### User Story 4 - Clear failure instead of empty books (Priority: P3)

A user adds a DRM-protected Kindle book, a password-protected PDF or a damaged file. Instead of an
empty 0-page book with no explanation, the reason the book cannot be read is recorded and visible
to the administrator, and the rest of the library and server keep working.

**Why this priority**: Silent empty books erode trust; a crash caused by one bad file would take
down the whole library.

**Independent Test**: Add a DRM-protected file and a truncated file; neither crashes the server,
both report a clear reason, and other books remain readable.

**Acceptance Scenarios**:

1. **Given** a DRM-protected or password-protected book, **When** it is catalogued, **Then** the
   reason is reported as "protected", not as an empty book.
2. **Given** a malformed file that makes the book engine crash or hang, **When** it is processed,
   **Then** the server stays up and other books continue to load.

---

### User Story 5 - Format-specific conveniences (Priority: P3)

For a PDF, a logged-in user can open the original file in the browser's own viewer (selectable,
searchable text). Archive-split suggestions, which regroup files inside a zip, are not offered for
e-books, and the server refuses to run that analysis for them.

**Independent Test**: The "open original PDF" button appears only for PDFs; the split-suggestion
button never appears for an e-book and a direct request is answered with "nothing to split".

---

### Edge Cases

- A `.txt` file that is a short readme or note sitting next to archives MUST NOT be catalogued as a
  book.
- A file whose extension lies about its format is identified by its content.
- An EPUB whose spine mixes full-page images with text pages is typeset as text with the images
  placed in the flow.
- A very long text novel (thousands of pages) is catalogued without blocking the server.
- A page that cannot be produced (corrupt image inside the book) shows the existing corrupted-page
  placeholder; the rest of the book still reads.
- A book changed on disk is re-inspected; stale cached pages are never served for it.
- A book whose table of contents points outside the book or contains loops is handled without
  hanging; invalid entries are dropped.
- Several table-of-contents entries starting on the same page keep the first one (the chapter list
  holds one entry per page).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST catalogue PDF, EPUB, MOBI, AZW, AZW3, DjVu, FB2 and TXT files found in
  the library like archives, with a page count and cover thumbnail.
- **FR-002**: The system MUST identify a file's format from its content where the extension is
  missing or wrong.
- **FR-003**: Pages that are stored in the book as pictures MUST be served as those original bytes
  without re-encoding.
- **FR-004**: Pages that are not stored as pictures (vector/text PDF pages, DjVu pages) MUST be
  rendered to images.
- **FR-005**: Reflowable text content (EPUB text, MOBI/AZW/AZW3 text, FB2, TXT) MUST be typeset by
  the server into fixed-size pages, supporting Chinese, Japanese, Korean and Latin text, headings,
  paragraphs and inline illustrations.
- **FR-006**: Page order MUST follow each format's own reading order (EPUB spine, PDF/DjVu page
  order, MOBI record order, FB2 document order).
- **FR-007**: Plain-text files MUST be decoded from their actual character encoding (UTF-8/16,
  GBK/GB18030, Big5, Shift-JIS, EUC-JP, EUC-KR).
- **FR-008**: The book's own table of contents MUST become the archive's chapter list on first
  cataloguing, without ever overwriting a chapter list the user has edited; plain-text chapter
  headings MUST be recognised for Chinese, Japanese and English conventions.
- **FR-009**: DRM-protected, password-protected and unreadable books MUST be reported with a clear
  reason instead of being catalogued silently as empty.
- **FR-010**: Parsing and rendering of book files MUST be isolated so that a crash or hang caused by
  one file cannot take down the server; a hung operation MUST be abandoned after a bounded time.
- **FR-011**: The server's own memory use MUST NOT grow with the number or size of books read;
  book engines run in separately managed processes whose number is capped to a share of the CPU.
- **FR-012**: Inspection results and produced pages MUST be cached and invalidated when the file
  changes; previously failed e-books MUST be retried after this feature is deployed.
- **FR-013**: Every existing page consumer (reader, thumbnails, page dimensions, bookmarks, OCR
  translation, OPDS, page-format negotiation) MUST work for e-books without changes to its own
  logic.
- **FR-014**: Logged-in users MUST be able to open an original PDF in the browser's viewer; guests
  MUST NOT.
- **FR-015**: Archive-split suggestions MUST be hidden for e-books and refused by the server.
- **FR-016**: Plain-text files below a minimum size MUST be ignored by the scanner.
- **FR-017**: The production and development images MUST include every native engine and the CJK
  fonts the formats need, with third-party license notices.

### Key Entities

- **Book**: an e-book file in the library, catalogued as an archive; has a format, pages, an
  outline, and optional title/author metadata.
- **Page**: one image of a book; either a picture stored in the book (served as-is) or a rendered
  page (vector page or typeset text), with known pixel size.
- **Outline entry**: a title, the page where it begins, and its nesting depth.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For every supported format, a sample book is catalogued with the correct page count
  and every page displays in the reader.
- **SC-002**: Picture pages served as originals are byte-identical to the pictures stored in the
  book.
- **SC-003**: A typical page of a text book appears in under 2 seconds the first time and instantly
  afterwards.
- **SC-004**: A malformed or DRM-protected book never stops the server; 100% of such files report a
  reason.
- **SC-005**: The server process's resident memory does not increase by more than 10 MB after
  reading books of every format.
- **SC-006**: Mixed CJK/Latin text pages render with no missing characters.
- **SC-007**: Table-of-contents entries open the page where their section begins.

## Assumptions

- Reading stays image-based in the existing reader; a separate reflowable text reader (adjustable
  font size, text selection) is out of scope. The PDF "open original" button covers text selection
  for PDFs.
- Text books are typeset horizontally at one fixed page size and font size; vertical CJK typesetting
  of reflowable text is out of scope (vertical text already inside PDF/DjVu pages is unaffected).
- KFX (Kindle's newest container) and DRM removal are out of scope; DRM-protected files are reported,
  not read.
- MOBI/AZW3 tables of contents are derived from headings in the text; the Kindle-internal NCX index
  is not parsed.
- The minimum size for `.txt` files is 8 KB.
- Existing tag/metadata plugins keep working on e-books through the archive record; embedded
  title/author metadata is captured but not written over the file-name title.
