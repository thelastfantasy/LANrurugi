# Contract: book worker protocol

Transport: a Unix socketpair is the worker's stdin and stdout. Every frame is a big-endian `u32`
length followed by that many bytes (max 128 MiB). A request is one JSON frame; a response is one
JSON frame, followed by one raw payload frame only for `page`.

## Requests

```json
{ "op": "inspect", "path": "/library/book.epub", "format": "epub" }
{ "op": "page", "path": "/library/book.epub", "format": "epub", "index": 3,
  "source": "render", "target_width": 1800 }
```

`source` is the value `inspect` reported for that page (`"jpeg"`, `{"image":{"kind":"png"}}`,
`"render"`), so the entry name's extension and the bytes always agree.

## Responses

```json
{ "status": "info", "info": { "format": "epub", "pages": [...], "sizes": [[1200,1700]],
  "outline": [{ "title": "第一章", "page": 0, "depth": 0 }], "title": null, "author": null } }
{ "status": "page", "extension": "webp" }            // then one payload frame
{ "status": "error", "kind": "encrypted", "message": "…" }
```

`kind`: `encrypted` | `unreadable` | `page_out_of_range` | `internal`.

## Lifecycle

- Spawned by the client on demand, at most `max_workers` (30% of cores, ≥ 1) alive.
- Exits after 300 s without a request, on EOF, or on a malformed frame.
- Client timeouts: inspect 300 s, page 60 s; a worker that times out or breaks framing is killed.
- A panic inside a request is caught, reported as `internal`, and the open document is dropped.

## Environment

- `LANRURUGI_PDFIUM_LIB` — PDFium library file or directory (else system search path).
- `LANRURUGI_DJVU_BIN_DIR` — directory holding `djvused`/`ddjvu` (else `PATH`).
