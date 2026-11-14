# Contract: ShareHost

The single interface between `lanrurugi-share` (engine) and `lanrurugi-api` (application). Everything the engine needs
from the application goes through it; nothing else does.

```rust
pub trait ShareHost: Send + Sync + 'static {
    fn archives(&self) -> &dyn ArchiveSource;
    fn downloads(&self) -> &dyn DownloadSource;
    fn tankoubons(&self) -> &dyn TankoubonSource;
    fn activity(&self) -> &dyn ActivitySink;
    fn settings(&self) -> &dyn SettingsSource;
}

pub trait ArchiveSource: Send + Sync {
    fn path(&self, id: &ArchiveId) -> Result<Option<PathBuf>>;
    fn metadata(&self, id: &ArchiveId) -> Result<Option<ArchiveMeta>>;   // title, size, mtime, pages
    fn page_count(&self, id: &ArchiveId) -> Result<Option<u32>>;
    fn open(&self, id: &ArchiveId) -> Result<Option<Box<dyn AsyncRead + Send + Unpin>>>;
}

pub trait DownloadSource: Send + Sync {
    fn enqueue(&self, request: ShareDownloadRequest) -> Result<DownloadHandle>;
    fn progress(&self, handle: &DownloadHandle) -> Result<DownloadProgress>;
}

pub trait TankoubonSource: Send + Sync {
    fn manifest(&self, id: &TankoubonId) -> Result<Option<TankoubonManifest>>; // ordered members
}

pub trait ActivitySink: Send + Sync {
    fn record(&self, event: ShareActivity);
}

pub trait SettingsSource: Send + Sync {
    fn share(&self) -> ShareSettings;
    fn general(&self) -> GeneralSettings;   // direct_download_url, public_url
}
```

## Rules

- **Data only.** No `axum` types, no `Json`, no API error enums, no `AppState`.
- **Static where possible.** Accessors return `&dyn` once per operation, not per chunk.
- **Failures are opaque.** `Result<…>` errors cross as `ShareError::Host { source }`; the API maps them to status codes.
- **No blocking in the engine.** Anything that touches the filesystem or Redis incompatibly is the host's problem.
- **Narrow.** A method is added only when a sharing caller exists; a "do anything" method is rejected in review.
- **Testable.** An in-memory implementation lives next to the engine's tests; it implements only the traits a test needs.

## What must NOT appear in the engine crate

| Symbol | Why |
|---|---|
| `lanrurugi_api::…` | the whole point of the extraction |
| `AppState` | replaced by `ShareHost` |
| `axum::…` / `Router` / `State<…>` | HTTP stays in the API crate |
| `Json<…>` | response shaping belongs to the handler |
| `lanrurugi_plugin::…` | plugins are not a sharing dependency |

## HTTP surface (unchanged)

All share routes remain registered in `lanrurugi-api`:

- `GET/POST /api/shares`, `GET /api/shares/{id}`, `DELETE /api/shares/{id}`
- `GET/POST /api/share/imports`, import progress/cancel
- `GET /api/share/r/{token}/…` (recipient page and download)
- `GET/POST /api/share/settings`, `GET /api/share/status`
- relay and node endpoints under `/api/share/…`

The route-scan documentation gate must produce the same table before and after the move.
