---
title: Supported formats
description: The archive and e-book formats the library accepts, how each is displayed, and where its table of contents comes from.
---

Every file in the library is an "archive" that you read page by page in the same reader. Besides
archives of images, e-books such as PDF and EPUB can be dropped into the library directory or added
from the upload page as they are, with no conversion step.

## Archives

| Format | Extensions |
| --- | --- |
| ZIP | `.zip` `.cbz` |
| RAR (including RAR5) | `.rar` `.cbr` |
| 7-Zip | `.7z` `.cb7` |
| TAR | `.tar` `.cbt`, and tar compressed with gzip / bzip2 / xz / lzma (`.gz` `.bz2` `.xz` `.lzma`) |
| LZH | `.lzh` `.lha` |

Images that can be pages inside an archive: PNG, JPEG (`.jpg` `.jpeg` `.jfif`), GIF, BMP, WebP, AVIF,
JPEG XL and HEIF/HEIC. Formats the browser cannot display (such as AVIF, JPEG XL or animated images on
some browsers) are converted by the server first; HEIF/HEIC cannot be converted yet and only shows in
browsers that support it natively (such as Safari).

zstd-compressed files (`.zst`) are not supported yet and are not picked up.

## E-books

| Format | Extensions | How pages are shown | Where the table of contents comes from |
| --- | --- | --- | --- |
| PDF | `.pdf` | Scanned pages are the picture stored in the PDF; text and vector pages are rendered | PDF bookmarks |
| EPUB (fixed layout) | `.epub` | Each page is the picture stored in the book | The book's own contents (nav / NCX) |
| EPUB (reflowable) | `.epub` | Typeset into pages on the server, illustrations in place | The book's own contents, down to where each chapter starts |
| MOBI / AZW / AZW3 | `.mobi` `.azw` `.azw3` `.prc` | Comics: the stored picture per page; text books: typeset on the server | The book's contents page or chapter headings |
| DjVu | `.djvu` `.djv` | Rendered | DjVu bookmarks |
| FB2 | `.fb2` | Typeset on the server, illustrations in place | The book's sections |
| Plain text | `.txt` | Typeset on the server | Chapter lines detected automatically ("Chapter 3", "Prologue", 第一章, 第十二回, …) |

Notes:

- **Pictures are never re-compressed.** Where a page is a picture stored in the book (scanned PDFs,
  fixed-layout EPUB, comic MOBI/AZW3, FB2 illustrations), the reader gets that exact picture.
- **Text books are typeset by the server**, with a Simplified Chinese, Japanese or Korean font chosen by
  the book's language; headings, paragraphs and illustrations all land on the page. Only horizontal
  text is supported for now.
- **The table of contents is imported once, on first import**, and only into an archive that has no
  chapters yet, so chapters you edited are never overwritten. When the book's own contents are missing
  or have just one or two entries (a cover or a licence page, say), they are completed from the
  headings and chapter lines in the text.
- **Only `.txt` files of 8 KB or more are picked up**, so readme files and notes next to your archives
  are not mistaken for books. The encoding is detected automatically (UTF-8, UTF-16, GBK/GB18030,
  Big5, Shift_JIS, EUC-JP, EUC-KR and others), and text hard-wrapped at a fixed width is joined back
  into paragraphs.
- **The library marks the format.** E-books show a small `PDF`, `EPUB`, … label beside their title,
  and a PDF can also be opened as-is in the browser from the reader.
- Split suggestions only make sense for archives and are not offered for e-books.
- Rendered pages of text books and text PDF pages are cached, and count towards the same
  "Settings → Archive Files → Maximum Reader Cache Size" limit as the reader's other caches.

## What is not supported

- **DRM- or password-protected books** (for example store-bought Kindle books whose protection was not
  removed, or a PDF with an open password) cannot be read. They still appear in the library (with 0
  pages); opening one shows why it cannot be read (protected by DRM or a password), and the server log
  records it too.
- **Damaged or incomplete files** likewise report the reason and do not affect the rest of the library.
- Kindle's KFX format and HUFF/CDIC-compressed MOBI files (mostly Kindle dictionaries) are not
  supported yet.
