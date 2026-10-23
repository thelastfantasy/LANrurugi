# Quickstart: validating e-book format support

## Prerequisites

- Dev container rebuilt (`mise run dev-rebuild && mise run dev-down && mise run dev-up`) — it ships
  PDFium, DjVuLibre tools and Noto CJK fonts.
- For host-side tests: `mise run fetch-pdfium` (PDFium); DjVu tests skip when `djvused` is absent.

## Automated

```sh
mise run test-crate -- lanrurugi-book lanrurugi-book-worker lanrurugi-scanner
```

Covers every backend with generated fixtures: fixed-layout and reflowable EPUB (spine order, nav and
NCX outlines, encryption.xml), MOBI image and text books (PalmDOC), FB2 (sections, binaries), TXT
(GBK, chapter detection, size threshold), PDF (scan passthrough, render, outline), DjVu (when tools
exist), typesetting of mixed CJK/Latin text.

## Manual (dev container)

1. Copy one sample of each format into `data/manga/` (fixed-layout EPUB, reflowable EPUB, comic
   AZW3, text MOBI, DjVu, FB2, GBK TXT ≥ 8 KB, scanned and text PDF).
2. Logs show `Catalogued` once per file (not once per write event).
3. Each archive's `pagecount` equals the book's pages; `toc` holds its chapters (1-based pages).
4. In the reader, every page displays; picture pages report their original format and size in the
   file-info bar; text pages show CJK and Latin text without gaps.
5. A DRM-protected or truncated file logs `cannot list pages` with a reason; the server keeps
   serving other books.
6. `ps` in the container shows the server's RSS unchanged after reading every sample;
   `lanrurugi-book-worker` processes exit after 5 idle minutes.
