# Contract: HTTP surface

No new endpoints; no existing response shape changes (constitution Principle II).

| Endpoint | Change |
|---|---|
| `GET /api/archives/{id}/download?inline=true` | Already added by the PDF work: for a PDF, streams `application/pdf` with `Content-Disposition: inline`. Ignored for every other format (normal attachment download). |
| `GET /api/archives/{id}/files`, `/page`, `/page-dimensions`, `/thumbnail`, OPDS | Unchanged; books list synthetic page names (`0001.jpg`, `0002.webp`). |
| `POST /api/archives/{id}/split-suggestion`, auto-analysis | Returns `"suggestion": null` ("does not need splitting") for any e-book format without reading it. |

Frontend: the reader's "open original PDF" button shows only when `extension === "pdf"`; the
split-suggestion button is hidden for every e-book extension (`pdf`, `epub`, `mobi`, `azw`, `azw3`,
`prc`, `djvu`, `djv`, `fb2`, `txt`).
