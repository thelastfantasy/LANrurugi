# Contract: ShareHost

The single interface between `lanrurugi-share` (engine) and `lanrurugi-api` (application). Everything the engine needs
from the application goes through it; nothing else does.

The trait as implemented (`crates/lanrurugi-share/src/host.rs`; the API's `impl` is
`crates/lanrurugi-api/src/share_host.rs`). Every method returns owned data or a boxed future, so the
trait stays object-safe and the engine holds it as `Host = Arc<dyn ShareHost>`:

```rust
pub trait ShareHost: Send + Sync + 'static {
    // Storage the engine shares with the rest of the process (share records, imports, node keys).
    fn redis(&self) -> RedisDbs;
    fn jobs(&self) -> JobRegistry;
    fn temp_dir(&self) -> PathBuf;

    // Reading the library for a share.
    fn archive(&self, id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>>;
    fn thumbnail(&self, id: &ArchiveId, page: u32) -> BoxFuture<'_, Option<(String, Vec<u8>)>>;
    fn categories(&self, id: &ArchiveId) -> BoxFuture<'_, Vec<String>>;
    fn tankoubon(&self, id: &TankId) -> BoxFuture<'_, Option<ShareTankoubon>>;

    // Settings and identity.
    fn settings(&self) -> BoxFuture<'_, ShareSettings>;
    fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz>;
    fn admit_upload(&self, group: Group, peer: String) -> BoxFuture<'_, Result<Caps, Held>>; // defaulted
    fn node_code(&self) -> BoxFuture<'_, Option<NodeCode>>;
    fn add_import_bytes(&self, share: &ShareId, bytes: u64) -> BoxFuture<'_, ()>;

    // Host-side loops and activity.
    fn start_host_services(&self);
    fn record_activity(&self, action: &'static str, target: ActivityTarget, after: Value) -> BoxFuture<'_, ()>;

    // The importing side, grouped so a host that never imports hands over `EmptyHost`.
    fn imports(&self) -> &dyn ImportHost;
}

/// Writing into the library and reporting to the download queue.
pub trait ImportHost: Send + Sync {
    fn catalogue_import(&self, staging: PathBuf, name: String, blake3_hex: String, overwrite: bool)
        -> BoxFuture<'_, Result<ArchiveId, CatalogueRefusal>>;
    fn apply_shared_metadata(&self, id: ArchiveId, shared: SharedMetadata) -> BoxFuture<'_, ()>;
    fn set_archive_tags(&self, id: ArchiveId, tags: String) -> BoxFuture<'_, ()>;
    fn add_to_category(&self, id: ArchiveId, category_id: String) -> BoxFuture<'_, ()>;
    fn static_category_named(&self, name: String) -> BoxFuture<'_, Result<String, String>>;
    fn tankoubon_exists(&self, name: String, members: Vec<ArchiveId>) -> BoxFuture<'_, bool>;
    fn create_tankoubon(&self, name: String, summary: String, tags: String, members: Vec<ArchiveId>)
        -> BoxFuture<'_, Result<(), String>>;
    fn set_queue_state(&self, queue_item_id: String, to: DownloadQueueState, error: Option<String>) -> BoxFuture<'_, ()>;
    fn set_queue_job(&self, queue_item_id: String, job_id: String) -> BoxFuture<'_, ()>;
    fn queue_paused(&self, queue_item_id: String) -> BoxFuture<'_, bool>;
    fn set_import_cancellation(&self, queue_item_id: String, token: Option<CancellationToken>) -> BoxFuture<'_, ()>;
    fn import_progress(&self, queue_item_id: &str, job_id: &str, record: &ShareImport, done: u64, total: u64);
    fn temp_quota_bytes(&self) -> BoxFuture<'_, Option<u64>>;
}
```

**Departure from the planned shape.** The plan grouped the methods behind five accessors
(`archives()`, `downloads()`, `tankoubons()`, `activity()`, `settings()`). The implementation is one
trait plus a single `imports()` group instead, for two reasons found while moving the code: the engine reads and writes its own
records (shares, imports, node secret) in the same Redis pools as the rest of the process, so it takes
the pool handles rather than a wrapper per key family; and the import side needed library *writes*
(cataloguing, metadata, categories, tankoubons, queue state) that the planned `DownloadSource`
(`enqueue`/`progress`) did not describe. `Arc<T>` forwards the trait, so a `Host` goes wherever a host
is asked for.

## Rules

- **Data only.** No `axum` types, no `Json`, no API error enums, no `AppState`.
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
