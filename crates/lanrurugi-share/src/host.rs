//! What the engine needs from its host, and nothing else.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_share_proto::node_code::NodeCode;
use lanrurugi_storage::redis::RedisDbs;

use crate::settings::ShareSettings;

/// Everything the sharing engine borrows from the running application.
///
/// Grouped accessors rather than one flat trait: the engine's subsystems each need a narrow slice,
/// and a single trait with fifty methods would force every test host to implement all of them.
///
/// Storage is handed over as the same pooled handles the rest of the process uses — the engine is
/// not the only writer to those keys, so a second connection setup would only add a way for the
/// two to disagree.
pub trait ShareHost: Send + Sync + 'static {
    /// The process's Redis pools; share records, imports and node keys live in `config`.
    fn redis(&self) -> RedisDbs;
    /// The job registry imports and fingerprint preparation report progress to.
    fn jobs(&self) -> JobRegistry;
    /// Where the engine may keep derived files (fingerprint outboards, merged copies).
    fn temp_dir(&self) -> PathBuf;
    /// The archive behind an id, as the engine may serve it.
    fn archive(&self, id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>>;
    /// The current sharing settings, defaults filled in.
    fn settings(&self) -> BoxFuture<'_, ShareSettings>;
    /// An archive's cover as the share serves it: MIME type and bytes.
    fn thumbnail(&self, id: &ArchiveId, page: u32) -> BoxFuture<'_, Option<(String, Vec<u8>)>>;
    /// The zone the sharing schedule is evaluated in (UTC when unset).
    fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz>;
    /// Admits one upload to `peer` under the group's current caps, reading this host's settings
    /// and zone. Defaulted here so both implementations cannot diverge in *which* settings view
    /// they judge against.
    fn admit_upload(
        &self,
        group: crate::limits::Group,
        peer: String,
    ) -> BoxFuture<'_, Result<crate::limits::Caps, crate::limits::Held>> {
        Box::pin(async move {
            let settings = self.settings().await;
            crate::limits::admit(&settings, self.timezone().await, group, &peer)
        })
    }
    /// The names of the categories an archive belongs to, static ones only.
    fn categories(&self, id: &ArchiveId) -> BoxFuture<'_, Vec<String>>;
    /// The tankoubon behind an id, with its members in order.
    fn tankoubon(&self, id: &lanrurugi_core::ids::TankId) -> BoxFuture<'_, Option<ShareTankoubon>>;
    /// This instance's own node code, when the node role is enabled. `None` when it is off or
    /// could not be read: publishing a record without a code is meaningless, so callers skip.
    fn node_code(&self) -> BoxFuture<'_, Option<NodeCode>>;
    /// Adds bytes served for a share to its imported total. The engine keeps these in memory and
    /// flushes them when it gives up its endpoint, so this is the one write that must not be lost.
    fn add_import_bytes(
        &self,
        share: &lanrurugi_core::ids::ShareId,
        bytes: u64,
    ) -> BoxFuture<'_, ()>;

    /// Starts the host's own loops that ride on sharing (the fingerprint preparation worker).
    /// Called once per process, the first time the endpoint comes up.
    fn start_host_services(&self);
    /// Records one automatic activity entry (a transfer finishing).
    fn record_activity(
        &self,
        action: &'static str,
        target: lanrurugi_storage::activity::ActivityTarget,
        after: serde_json::Value,
    ) -> BoxFuture<'_, ()>;

    /// The importing side: everything that writes into the library or reports to the download
    /// queue. Grouped so a test host that never imports can hand over [`EmptyHost`].
    fn imports(&self) -> &dyn ImportHost;
}

/// What an import needs from its host: the library to write into and the queue row to report to.
pub trait ImportHost: Send + Sync {
    /// Catalogues a verified, staged import file under `name`. `blake3_hex` is the file's own
    /// fingerprint, already checked by the transfer. `overwrite` replaces whichever local archive
    /// owns the name; otherwise a taken name is refused.
    fn catalogue_import(
        &self,
        staging: PathBuf,
        name: String,
        blake3_hex: String,
        overwrite: bool,
    ) -> BoxFuture<'_, Result<ArchiveId, CatalogueRefusal>>;
    /// Gives an imported archive the sharer's title, summary, chapters and tags.
    fn apply_shared_metadata(&self, id: ArchiveId, shared: SharedMetadata) -> BoxFuture<'_, ()>;
    /// Replaces an archive's tag string, keeping the search indexes in step.
    fn set_archive_tags(&self, id: ArchiveId, tags: String) -> BoxFuture<'_, ()>;
    /// Adds an archive to a category by id.
    fn add_to_category(&self, id: ArchiveId, category_id: String) -> BoxFuture<'_, ()>;
    /// The id of the static category called `name`, created when it does not exist yet.
    fn static_category_named(&self, name: String) -> BoxFuture<'_, Result<String, String>>;
    /// Whether a tankoubon with this name and exactly these members (in order) already exists.
    fn tankoubon_exists(&self, name: String, members: Vec<ArchiveId>) -> BoxFuture<'_, bool>;
    /// Creates a tankoubon with its members in order.
    fn create_tankoubon(
        &self,
        name: String,
        summary: String,
        tags: String,
        members: Vec<ArchiveId>,
    ) -> BoxFuture<'_, Result<(), String>>;
    /// Moves an import's download-queue row to `to`, with the failure reason when it errored.
    fn set_queue_state(
        &self,
        queue_item_id: String,
        to: lanrurugi_storage::download_queue::DownloadQueueState,
        error: Option<String>,
    ) -> BoxFuture<'_, ()>;
    /// Records the job an import runs under on its queue row.
    fn set_queue_job(&self, queue_item_id: String, job_id: String) -> BoxFuture<'_, ()>;
    /// Whether the queue row was paused (rather than stopped) by the user.
    fn queue_paused(&self, queue_item_id: String) -> BoxFuture<'_, bool>;
    /// Registers (`Some`) or forgets (`None`) the token that the queue's pause/stop cancels.
    fn set_import_cancellation(
        &self,
        queue_item_id: String,
        token: Option<tokio_util::sync::CancellationToken>,
    ) -> BoxFuture<'_, ()>;
    /// Pushes an import's progress to whoever watches the download queue. Already rate-limited by
    /// the engine; a host without listeners ignores it.
    fn import_progress(
        &self,
        queue_item_id: &str,
        job_id: &str,
        record: &lanrurugi_storage::shares::ShareImport,
        done: u64,
        total: u64,
    );
    /// `tempmaxsize` in bytes: the ceiling for the whole temp tree, partial imports included.
    fn temp_quota_bytes(&self) -> BoxFuture<'_, Option<u64>>;
}

/// Why the library would not take a staged import file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueRefusal {
    /// An archive with the same content is already there: the import reuses it.
    SameContent(ArchiveId),
    /// The filename is taken by a different archive.
    NameTaken,
    Failed(String),
}

/// What the sharer said about one archive, applied to the imported copy.
#[derive(Debug, Clone, Default)]
pub struct SharedMetadata {
    pub title: String,
    /// Tags as the sharer stores them, `date_added:` already removed.
    pub tags: String,
    pub summary: String,
    pub toc: Vec<lanrurugi_share_proto::messages::Chapter>,
}

/// The fields a share needs to describe an archive without loading its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareArchive {
    pub id: ArchiveId,
    pub title: String,
    /// The archive's own name (the file's stem), used when the title is empty.
    pub name: String,
    /// Path on disk, as the library records it (the archive file itself, never a derived copy).
    pub file: PathBuf,
    pub tags: String,
    pub summary: String,
    pub pagecount: u32,
    pub arcsize: u64,
    pub toc: Vec<lanrurugi_share_proto::messages::Chapter>,
}

/// A tankoubon as a share sees it: its members in volume order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareTankoubon {
    pub name: String,
    pub summary: String,
    /// The stored tag string, handed over as-is; the wire entry carries the same text.
    pub tags: String,
    pub archives: Vec<ArchiveId>,
}

/// A host that owns nothing: useful for tests that only exercise pure parsing.
pub struct EmptyHost;

impl ShareHost for EmptyHost {
    fn redis(&self) -> RedisDbs {
        panic!("EmptyHost has no Redis")
    }
    fn jobs(&self) -> JobRegistry {
        panic!("EmptyHost has no job registry")
    }
    fn temp_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
    fn archive(&self, _id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>> {
        Box::pin(async { None })
    }
    fn settings(&self) -> BoxFuture<'_, ShareSettings> {
        Box::pin(async { ShareSettings::from_stored(&Default::default()) })
    }
    fn add_import_bytes(
        &self,
        _share: &lanrurugi_core::ids::ShareId,
        _bytes: u64,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn node_code(&self) -> BoxFuture<'_, Option<NodeCode>> {
        Box::pin(async { None })
    }
    fn thumbnail(&self, _id: &ArchiveId, _page: u32) -> BoxFuture<'_, Option<(String, Vec<u8>)>> {
        Box::pin(async { None })
    }
    fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz> {
        Box::pin(async { chrono_tz::UTC })
    }
    fn categories(&self, _id: &ArchiveId) -> BoxFuture<'_, Vec<String>> {
        Box::pin(async { Vec::new() })
    }
    fn tankoubon(
        &self,
        _id: &lanrurugi_core::ids::TankId,
    ) -> BoxFuture<'_, Option<ShareTankoubon>> {
        Box::pin(async { None })
    }
    fn start_host_services(&self) {}
    fn record_activity(
        &self,
        _action: &'static str,
        _target: lanrurugi_storage::activity::ActivityTarget,
        _after: serde_json::Value,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn imports(&self) -> &dyn ImportHost {
        self
    }
}

impl ImportHost for EmptyHost {
    fn catalogue_import(
        &self,
        _staging: PathBuf,
        _name: String,
        _blake3_hex: String,
        _overwrite: bool,
    ) -> BoxFuture<'_, Result<ArchiveId, CatalogueRefusal>> {
        Box::pin(async { Err(CatalogueRefusal::Failed("EmptyHost has no library".into())) })
    }
    fn apply_shared_metadata(&self, _id: ArchiveId, _shared: SharedMetadata) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn set_archive_tags(&self, _id: ArchiveId, _tags: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn add_to_category(&self, _id: ArchiveId, _category_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn static_category_named(&self, _name: String) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("EmptyHost has no categories".into()) })
    }
    fn tankoubon_exists(&self, _name: String, _members: Vec<ArchiveId>) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
    fn create_tankoubon(
        &self,
        _name: String,
        _summary: String,
        _tags: String,
        _members: Vec<ArchiveId>,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("EmptyHost has no tankoubons".into()) })
    }
    fn set_queue_state(
        &self,
        _queue_item_id: String,
        _to: lanrurugi_storage::download_queue::DownloadQueueState,
        _error: Option<String>,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn set_queue_job(&self, _queue_item_id: String, _job_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn queue_paused(&self, _queue_item_id: String) -> BoxFuture<'_, bool> {
        Box::pin(async { false })
    }
    fn set_import_cancellation(
        &self,
        _queue_item_id: String,
        _token: Option<tokio_util::sync::CancellationToken>,
    ) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn import_progress(
        &self,
        _queue_item_id: &str,
        _job_id: &str,
        _record: &lanrurugi_storage::shares::ShareImport,
        _done: u64,
        _total: u64,
    ) {
    }
    fn temp_quota_bytes(&self) -> BoxFuture<'_, Option<u64>> {
        Box::pin(async { None })
    }
}

/// The engine holds its host as [`Host`]; forwarding lets that handle go wherever a host is asked
/// for, without spelling `&**host` at every call.
impl<T: ShareHost + ?Sized> ShareHost for Arc<T> {
    fn redis(&self) -> RedisDbs {
        (**self).redis()
    }
    fn jobs(&self) -> JobRegistry {
        (**self).jobs()
    }
    fn temp_dir(&self) -> PathBuf {
        (**self).temp_dir()
    }
    fn archive(&self, id: &ArchiveId) -> BoxFuture<'_, Option<ShareArchive>> {
        (**self).archive(id)
    }
    fn settings(&self) -> BoxFuture<'_, ShareSettings> {
        (**self).settings()
    }
    fn thumbnail(&self, id: &ArchiveId, page: u32) -> BoxFuture<'_, Option<(String, Vec<u8>)>> {
        (**self).thumbnail(id, page)
    }
    fn timezone(&self) -> BoxFuture<'_, chrono_tz::Tz> {
        (**self).timezone()
    }
    fn admit_upload(
        &self,
        group: crate::limits::Group,
        peer: String,
    ) -> BoxFuture<'_, Result<crate::limits::Caps, crate::limits::Held>> {
        (**self).admit_upload(group, peer)
    }
    fn categories(&self, id: &ArchiveId) -> BoxFuture<'_, Vec<String>> {
        (**self).categories(id)
    }
    fn tankoubon(&self, id: &lanrurugi_core::ids::TankId) -> BoxFuture<'_, Option<ShareTankoubon>> {
        (**self).tankoubon(id)
    }
    fn node_code(&self) -> BoxFuture<'_, Option<NodeCode>> {
        (**self).node_code()
    }
    fn add_import_bytes(
        &self,
        share: &lanrurugi_core::ids::ShareId,
        bytes: u64,
    ) -> BoxFuture<'_, ()> {
        (**self).add_import_bytes(share, bytes)
    }
    fn start_host_services(&self) {
        (**self).start_host_services()
    }
    fn record_activity(
        &self,
        action: &'static str,
        target: lanrurugi_storage::activity::ActivityTarget,
        after: serde_json::Value,
    ) -> BoxFuture<'_, ()> {
        (**self).record_activity(action, target, after)
    }
    fn imports(&self) -> &dyn ImportHost {
        (**self).imports()
    }
}

/// Shared handle to a host, as the engine's spawned tasks hold it.
pub type Host = Arc<dyn ShareHost>;
