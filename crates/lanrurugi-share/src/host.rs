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
    fn redis(&self) -> RedisDbs;
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
}

/// Shared handle to a host, as the engine's spawned tasks hold it.
pub type Host = Arc<dyn ShareHost>;
