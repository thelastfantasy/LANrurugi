# Data model: native packages

No persisted data changes. Two descriptive models drive the implementation.

## Release package

`lanrurugi-<version>-<platform>.tar.gz`, `<platform>` ∈ `linux-x86_64`, `linux-aarch64`,
`macos-arm64`, `freebsd-x86_64`. One top-level directory of the same name; layout in
[contracts/package-layout.md](contracts/package-layout.md). Published with `SHA256SUMS`.

| Field | Source |
|---|---|
| version | release tag without `v` |
| binaries | `lanrurugi-server`, `lanrurugi-book-worker`, `lanrurugi-embed-worker`, `lanrurugi-gpu-worker` |
| bundled libraries | PDFium, ONNX Runtime — Linux and macOS only |
| static assets | frontend build, plugins, docs site |
| scripts | model download, service definitions |

## Platform capability

| Feature | Needs | Linux | macOS arm64 | FreeBSD |
|---|---|---|---|---|
| Library, reader, search, archives | — | ✓ | ✓ | ✓ |
| PDF | PDFium | bundled | bundled | `pkg install pdfium` |
| DjVu | DjVuLibre CLI | pkg | brew | pkg |
| Text e-books | CJK font | Noto CJK pkg | system fonts | Noto SC/JP/KR pkgs |
| Plugins (metadata/download/subscriptions) | Deno | pkg/binary | brew | pkg |
| Recommendations, OCR, translation | ONNX Runtime + models | bundled + script | bundled + script | `pkg install onnxruntime` + script |
| GPU-accelerated inference | CUDA/OpenVINO | ✓ (as in the image) | — (CPU) | — (CPU) |
| GeoIP for the device list | MaxMind DB | optional | optional | optional |

A missing component disables only its row; the server logs the unavailable rows once at start-up.
