# Data Model: E-book Format Support

No Redis schema changes. A book is an ordinary `Archive` record (`pagecount`, `toc`, `arcsize`,
`file`, …). Everything below is in-memory / on-disk cache state of `lanrurugi-book`.

## BookFormat (enum)

`Pdf | Epub | Mobi | Djvu | Fb2 | Txt` — detected from extension, confirmed by content
(research §9). Extensions: `pdf`; `epub`; `mobi`, `azw`, `azw3`, `prc`; `djvu`, `djv`; `fb2`; `txt`.

## BookInfo (cached per file identity)

| Field | Type | Notes |
|---|---|---|
| `format` | `BookFormat` | |
| `pages` | `Vec<PageSource>` | one per page, reading order |
| `sizes` | `Vec<(u32, u32)>` | pixel size of each page as served |
| `outline` | `Vec<OutlineItem>` | document order |
| `title`, `author` | `Option<String>` | embedded metadata, informational |

Identity key: `sha1(path, size, mtime)`. Stored in memory (LRU 512) and as
`<temp>/book/<key>/info.json`. A changed file gets a new key, so stale data is never served.

## PageSource (enum, serialized snake_case)

| Variant | Entry extension | Bytes |
|---|---|---|
| `jpeg` | `jpg` | PDF full-page JPEG, stored bytes (kept for cache compatibility with the PDF work) |
| `image{kind}` | `jpg`/`png`/`gif`/`webp`/`bmp` | picture stored in an EPUB/MOBI/FB2, stored bytes |
| `render` | `webp` | rendered (PDF/DjVu page) or typeset (flow book) page |

Entry name: `{index+1:0≥4}.{ext}`; parsing accepts exactly these names.

## OutlineItem

`title: String`, `page: u32` (0-based), `depth: u32`. Converted to `TocEntry { page: page + 1, name }`
on import; entries whose page is out of range are dropped; first entry per page wins.

## Flow document (worker-internal)

- `FlowBook { lang: Lang, blocks: Vec<Block>, anchors: Vec<(String /*title*/, usize /*block*/ , u32 /*depth*/)> }`
- `Block = Heading { level, text } | Para { text } | Image { data: ImageRef, size } | PageBreak`
- `ImageRef` = how to fetch the picture again on render (zip entry name, MOBI record index, FB2
  binary id) — never the decoded pixels.
- `Layout { pages: Vec<PageStart { block, line }> }` — produced at inspection, kept in the worker's
  LRU(2) and recomputed on a miss.

## Errors (`ErrorKind`)

`Encrypted` (DRM/password) · `Unreadable` (corrupt/unsupported variant) · `PageOutOfRange` ·
`Internal` (panic caught, engine failure). Client maps to `BookError`; the archive layer maps
`PageOutOfRange` to `EntryNotFound`.
