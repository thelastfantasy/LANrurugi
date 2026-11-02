# Contract: install-relative resource discovery

At start-up, before argument parsing, the server resolves its **install prefix** as the parent of the
directory holding the real (symlink-resolved) executable. For each variable below that is **unset**,
it is set to the install-relative path **only if that path exists**. Set variables are never changed.
Child workers inherit the result.

| Variable | Install-relative path |
|---|---|
| `LANRURUGI_STATIC_DIR` | `share/lanrurugi/frontend` |
| `LANRURUGI_PLUGINS_DIR` | `share/lanrurugi/plugins` |
| `LANRURUGI_DOCS_DIR` | `share/lanrurugi/docs` |
| `LANRURUGI_PDFIUM_LIB` | `lib/libpdfium.so` (Linux, FreeBSD) / `lib/libpdfium.dylib` (macOS) |
| `ORT_DYLIB_PATH` | `lib/libonnxruntime.so` / `lib/libonnxruntime.dylib` |
| `LANRURUGI_MANGA_OCR_MODEL_DIR` | `share/lanrurugi/models/manga-ocr` |
| `LANRURUGI_INPAINT_MODEL_DIR` | `share/lanrurugi/models/inpaint` |
| `LANRURUGI_BUBBLE_SEG_MODEL_DIR` | `share/lanrurugi/models/bubble-seg` |

Additionally, when `ORT_DYLIB_PATH` is still unset, the platform's system location is tried for the
CPU runtime (`/usr/local/lib/libonnxruntime.so` on FreeBSD, where `pkg install onnxruntime` puts it).

Precedence, highest first: command-line flag → environment variable (incl. container image) →
install-relative path → the code's existing built-in default.

Inside the container image every variable is already set, so discovery changes nothing there.
