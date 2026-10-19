# Contract: release package layout

`lanrurugi-<version>-<platform>.tar.gz` unpacks to one directory, the **install prefix**:

```text
lanrurugi-<version>-<platform>/
├── bin/
│   ├── lanrurugi-server
│   ├── lanrurugi-book-worker
│   ├── lanrurugi-embed-worker
│   └── lanrurugi-gpu-worker
├── lib/                          # Linux and macOS only
│   ├── libpdfium.so | libpdfium.dylib
│   └── libonnxruntime.so* | libonnxruntime*.dylib
├── share/lanrurugi/
│   ├── frontend/                 # built web interface
│   ├── plugins/                  # bundled plugins
│   ├── docs/                     # documentation site
│   └── models/                   # created by fetch-models.sh
│       ├── manga-ocr/
│       ├── inpaint/
│       └── bubble-seg/
├── share/licenses/               # LICENSE files of bundled third-party libraries
├── service/
│   ├── lanrurugi.service         # systemd (Linux)
│   ├── org.lanrurugi.server.plist  # launchd (macOS)
│   ├── lanrurugi.rc              # rc.d (FreeBSD)
│   └── lanrurugi.env.example     # data locations used by the service files
├── fetch-models.sh
├── LICENSE
└── README.md                     # per-platform quick start, prerequisites, feature availability
```

Rules:

- Workers live next to the server (they are found through the server's own location).
- Nothing in the package is written to at run time except `share/lanrurugi/models/` by the script.
- User data (library, thumbnails, temp, logs, downloaded embedding models) lives outside the prefix,
  at the locations the user or the service file sets.
