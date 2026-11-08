//! What the engine needs from its host, and nothing else.

use std::sync::Arc;

use lanrurugi_core::ids::ArchiveId;

/// Everything the sharing engine borrows from the running application.
///
/// Grouped accessors rather than one flat trait: the engine's subsystems each need a narrow slice,
/// and a single trait with fifty methods would force every test host to implement all of them.
pub trait ShareHost: Send + Sync + 'static {
    fn archives(&self) -> Arc<dyn ArchiveSource>;
}

/// Archives the engine may offer and serve.
pub trait ArchiveSource: Send + Sync {
    fn get(&self, id: &ArchiveId) -> Option<ArchiveSummary>;
}

/// The fields a share needs to describe an archive without loading its bytes.
#[derive(Debug, Clone)]
pub struct ArchiveSummary {
    pub id: ArchiveId,
    pub title: String,
    pub filename: String,
}
