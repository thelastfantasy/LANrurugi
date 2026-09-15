//! Scanner → host ingestion events, sent over the long-lived `new_archive_tx` channel every
//! watcher start / full scan hands a clone of (`AppState::new_archive_tx`). The scanner crate has
//! no access to `AppState`/activity recording (avoiding a circular dependency on `lanrurugi-api`),
//! so this is the smallest shape that lets the host record the unified `archive.ingest` activity
//! for both outcomes — previously only a brand-new archive id crossed this channel, which meant a
//! file that failed to catalog was visible only in tracing logs and the scan summary.

/// One outcome of a scanner-side ingest attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestEvent {
    /// A genuinely new archive was catalogued — the host records a success and runs every enabled
    /// metadata plugin on it. (Only `IngestOutcome::Catalogued`, never `Rekeyed`/`Unchanged`/
    /// `Rejected`, matching the pre-existing channel semantics: a rekey preserves the old archive's
    /// metadata and intentionally does not trigger auto-plugins.)
    Catalogued { id: String },
    /// A file/directory entry could not be catalogued (unreadable archive, pipeline error,
    /// ingest timeout, or a panicked scan task). `path` is `None` only for a panicked scan task,
    /// where the path was lost with the panic.
    Failed {
        path: Option<String>,
        reason: String,
    },
}
