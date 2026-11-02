//! The importing side (017 US4, FR-019–FR-022, research §5, §13).
//!
//! Each archive moves as 1 MiB pieces of a bao stream, verified against the root fingerprint from
//! the sharer's manifest before any byte is written. Verified hash nodes are kept in a local
//! outboard next to the partial file, so a restart recomputes exactly which pieces are already
//! good (`valid_ranges`) and fetches only the rest. An import never uses the per-plugin download
//! pools, so it cannot hold slots other downloads wait on (FR-019a).
//!
//! Pieces are spread over the sharer and the helpers it lists (US7, FR-034), at most four in
//! flight per source. A helper whose piece fails verification is dropped for the rest of the
//! import and its piece goes back to the queue; helpers can never make a bad byte stick, because
//! every piece is checked against the sharer's root.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{FuturesUnordered, StreamExt};

use bao_tree::io::outboard::PreOrderOutboard;
use bao_tree::{BaoTree, ChunkNum, ChunkRanges};
use iroh::endpoint::Connection;
use lanrurugi_core::ids::{ArchiveId, ShareImportId};
use lanrurugi_share_proto::code::ShareRef;
use lanrurugi_share_proto::derive::access_hash;
use lanrurugi_share_proto::messages::{
    ChunkSpan, ClaimKind, HelperAddr, ManifestItem, RefusalReason, Request, Response, ShareManifest,
};
use lanrurugi_share_proto::ticket::{SignedTicket, RENEW_INTERVAL_SECS};
use lanrurugi_storage::download_queue::DownloadQueueState;
use lanrurugi_storage::shares::{
    FilenamePolicy, ImportArchive, ImportArchiveState, ImportState, ShareImport,
    ShareImportRepository,
};
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{blobs, call, connect, publish, read_body, ShareNet, ShareNetError};
use crate::AppState;

/// One bao chunk: 1 KiB, the unit `ChunkNum`, `ChunkRanges` and therefore `BaoTree::chunks()` count
/// in. `blobs::BLOCK_SIZE` (16 KiB) only groups chunks for hashing — using it here made every piece
/// and every progress figure off by 16×.
const CHUNK_BYTES: u64 = 1024;
/// A failure writing on *this* instance's disk — as opposed to anything the peer did. The leading
/// code is what the queue row localizes; the OS text after it stays for support.
fn storage_error(e: &std::io::Error) -> String {
    let code = match e.raw_os_error() {
        Some(28) => "disk_full",       // ENOSPC
        Some(30) => "read_only",       // EROFS
        Some(122) => "quota_exceeded", // EDQUOT
        _ => match e.kind() {
            std::io::ErrorKind::PermissionDenied => "permission_denied",
            _ => "storage_error",
        },
    };
    format!("{code}: {e}")
}

/// One piece: the 1 MiB the protocol's contract names, in those chunks.
const PIECE_CHUNKS: u64 = (1024 * 1024) / CHUNK_BYTES;
const MAX_PIECE_ATTEMPTS: u32 = 3;
/// One piece plus its bao path, generously bounded — the peer's announced length is not trusted.
const MAX_PIECE_BODY_BYTES: u64 = 4 * 1024 * 1024;
/// A card thumbnail is a few hundred KiB at worst.
const MAX_THUMBNAIL_BYTES: u64 = 16 * 1024 * 1024;
/// Per-archive cap on what a sharer may announce. Well past any real archive; a hostile manifest
/// otherwise asks for a sparse file (and an outboard) of whatever size it likes.
const MAX_ANNOUNCED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Free filenames tried when a same-named different archive already exists.
const MAX_RENAME_ATTEMPTS: u32 = 32;
const RECONNECT_CAP: Duration = Duration::from_secs(5 * 60);
/// Pieces in flight per source (FR-034).
const IN_FLIGHT_PER_SOURCE: usize = 4;
const HELPER_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

pub fn repo(state: &AppState) -> ShareImportRepository {
    ShareImportRepository::new(state.redis.config.clone())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    let bytes = crate::shares::unhex(text)?;
    bytes.try_into().ok()
}

/// A live connection to the sharer with an open session.
pub struct Opened {
    pub conn: Connection,
    pub session: String,
    pub manifest: ShareManifest,
    pub public_url: Option<String>,
    pub allow_helpers: bool,
}

/// Resolves a code, dials the sharer and opens a session.
pub async fn open(state: &AppState, net: &ShareNet, code: &str) -> Result<Opened, ShareNetError> {
    let reference =
        ShareRef::parse(code).map_err(|_| ShareNetError::Refused(RefusalReason::NotFound))?;
    let (_, conn) = publish::dial(state, net, &reference).await?;
    let (response, _) = call(
        &conn,
        Request::Open {
            access_hash: access_hash(&reference.code),
        },
    )
    .await?;
    match response {
        Response::Opened {
            session,
            manifest,
            public_url,
            allow_helpers,
        } => Ok(Opened {
            conn,
            session,
            manifest,
            public_url,
            allow_helpers,
        }),
        _ => Err(ShareNetError::Protocol("unexpected answer to Open".into())),
    }
}

/// A thumbnail fetched over an open session, for the import preview.
pub async fn thumbnail(
    opened: &Opened,
    archive_id: &str,
) -> Result<(Option<String>, Vec<u8>), ShareNetError> {
    let (response, mut recv) = call(
        &opened.conn,
        Request::Thumbnail {
            session: opened.session.clone(),
            archive_id: archive_id.to_string(),
        },
    )
    .await?;
    match response {
        Response::Bytes { content_type, len } => Ok((
            content_type,
            read_body(&mut recv, len, MAX_THUMBNAIL_BYTES).await?,
        )),
        _ => Err(ShareNetError::Protocol(
            "unexpected answer to Thumbnail".into(),
        )),
    }
}

/// The claim: counts one download, and returns the manifest with fingerprints plus a ticket.
pub async fn claim(
    opened: &Opened,
) -> Result<(String, ShareManifest, SignedTicket), ShareNetError> {
    let (response, _) = call(
        &opened.conn,
        Request::Claim {
            session: opened.session.clone(),
            kind: ClaimKind::Import,
        },
    )
    .await?;
    match response {
        Response::Claimed {
            claim_id,
            manifest,
            ticket,
            ..
        } => Ok((claim_id, manifest, ticket)),
        _ => Err(ShareNetError::Protocol("unexpected answer to Claim".into())),
    }
}

/// The import's archive list from a claim manifest; an archive listed twice (on its own and in a
/// tankoubon) is transferred once.
pub fn archives_from(manifest: &ShareManifest) -> Vec<ImportArchive> {
    let mut seen = std::collections::HashSet::new();
    manifest
        .archives()
        .filter(|a| seen.insert(a.archive_id.clone()))
        .map(|a| ImportArchive {
            sharer_archive_id: a.archive_id.clone(),
            title: a.title.clone(),
            blake3: a.blake3.map(|h| hex(&h)),
            size: a.size,
            state: ImportArchiveState::Pending,
            bytes_done: 0,
            error: None,
            note: None,
            local_archive_id: None,
        })
        .collect()
}

/// A helper the import fetches pieces from.
struct HelperSource {
    id: [u8; 32],
    conn: Connection,
}

/// Where one piece fetch goes: the sharer, or one of the helpers.
#[derive(Clone)]
enum Source {
    Sharer,
    Helper(Arc<HelperSource>),
}

impl Source {
    fn key(&self) -> Option<[u8; 32]> {
        match self {
            Self::Sharer => None,
            Self::Helper(h) => Some(h.id),
        }
    }
}

/// The import's and each archive's state plus the pause, as last pushed; any change is sent at once.
type PushedStates = (ImportState, Option<i64>, Vec<ImportArchiveState>);

/// Everything one running import shares between its archive transfers.
struct Run {
    state: AppState,
    import_id: ShareImportId,
    net: Arc<ShareNet>,
    conn: Connection,
    helper_list: Arc<RwLock<Vec<Arc<HelperSource>>>>,
    /// Helpers dialled already (live or dropped), so none is dialled twice.
    tried_helpers: std::sync::Mutex<HashSet<[u8; 32]>>,
    session: String,
    claim_id: String,
    ticket: RwLock<SignedTicket>,
    /// Set when the sharer refuses a renewal; every transfer stops with this reason.
    stopped: RwLock<Option<RefusalReason>>,
    cancel: CancellationToken,
    record: tokio::sync::Mutex<ShareImport>,
    job_id: String,
    queue_item_id: String,
    /// Bytes this import may still write into `<temp>/share` before `tempmaxsize` is reached.
    budget: u64,
    /// What to do when an archive's filename belongs to a different local archive.
    filename_policy: FilenamePolicy,
    /// When progress was last pushed to the Upload page, and the states it showed.
    pushed: std::sync::Mutex<(Option<std::time::Instant>, PushedStates)>,
    dir: PathBuf,
}

impl Run {
    async fn save(&self) {
        let record = self.record.lock().await.clone();
        let _ = repo(&self.state).save(&record).await;
        let (done, total): (u64, u64) = record
            .archives
            .iter()
            .fold((0, 0), |(d, t), a| (d + a.bytes_done, t + a.size));
        self.state
            .jobs
            .set_download_progress(&self.job_id, done, Some(total))
            .await;
        self.push(&record, done, total);
    }

    /// Pushes progress over the download-queue stream: at once when a state changes, otherwise at
    /// most every 250 ms, since a fast transfer saves after every piece.
    fn push(&self, record: &ShareImport, done: u64, total: u64) {
        let Some(tx) = &self.state.download_queue_tx else {
            return;
        };
        let states: PushedStates = (
            record.state,
            record.paused_until,
            record.archives.iter().map(|a| a.state).collect(),
        );
        {
            let mut pushed = self.pushed.lock().expect("push lock");
            let recent = pushed
                .0
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(250));
            if recent && pushed.1 == states {
                return;
            }
            *pushed = (Some(std::time::Instant::now()), states);
        }
        let _ = tx.send(serde_json::json!({
            "kind": "progress",
            "id": self.queue_item_id,
            "job_id": self.job_id,
            "downloaded_bytes": done,
            "total_bytes": total,
        }));
        let _ = tx.send(serde_json::json!({
            "kind": "share_import",
            "id": self.queue_item_id,
            "import": crate::share_imports::detail(record, false),
        }));
    }

    /// Whether this import has written past its share of `tempmaxsize`.
    async fn over_budget(&self) -> bool {
        let written: u64 = self
            .record
            .lock()
            .await
            .archives
            .iter()
            .map(|a| a.bytes_done)
            .sum();
        written > self.budget
    }

    async fn update(&self, sharer_archive_id: &str, f: impl FnOnce(&mut ImportArchive)) {
        {
            let mut record = self.record.lock().await;
            if let Some(a) = record
                .archives
                .iter_mut()
                .find(|a| a.sharer_archive_id == sharer_archive_id)
            {
                f(a);
            }
        }
        self.save().await;
    }

    async fn renew(&self) -> Result<(), ShareNetError> {
        let (response, _) = call(
            &self.conn,
            Request::Renew {
                claim_id: self.claim_id.clone(),
            },
        )
        .await?;
        match response {
            Response::Renewed { ticket, helpers } => {
                *self.ticket.write().await = ticket;
                self.add_helpers(helpers);
                Ok(())
            }
            _ => Err(ShareNetError::Protocol("unexpected answer to Renew".into())),
        }
    }

    /// Dials helpers not tried yet, in the background; each joins the sources once connected.
    fn add_helpers(&self, helpers: Vec<HelperAddr>) {
        let fresh: Vec<HelperAddr> = {
            let mut tried = self.tried_helpers.lock().expect("helpers lock");
            helpers
                .into_iter()
                .filter(|h| tried.insert(h.endpoint_id))
                .collect()
        };
        for helper in fresh {
            let Some(addr) = super::helper::helper_endpoint_addr(&helper) else {
                continue;
            };
            let net = self.net.clone();
            let list = self.helpers_handle();
            tokio::spawn(async move {
                let dialled = tokio::time::timeout(HELPER_DIAL_TIMEOUT, connect(&net, addr)).await;
                if let Ok(Ok(conn)) = dialled {
                    list.write().await.push(Arc::new(HelperSource {
                        id: helper.endpoint_id,
                        conn,
                    }));
                }
            });
        }
    }

    fn helpers_handle(&self) -> Arc<RwLock<Vec<Arc<HelperSource>>>> {
        self.helper_list.clone()
    }

    async fn drop_helper(&self, id: [u8; 32]) {
        tracing::info!(helper = %hex(&id[..6]), "dropping a helper whose piece failed");
        self.helper_list.write().await.retain(|h| h.id != id);
    }

    /// Keeps the ticket fresh; a refusal stops the whole import (SC-009).
    fn spawn_renewer(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let run = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = run.cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(RENEW_INTERVAL_SECS)) => {}
                }
                match run.renew().await {
                    Ok(()) => {}
                    Err(ShareNetError::Refused(reason)) => {
                        *run.stopped.write().await = Some(reason);
                        run.cancel.cancel();
                        return;
                    }
                    // A transient failure: the next attempt may succeed before the ticket lapses.
                    Err(e) => tracing::debug!(error = %e, "ticket renewal failed"),
                }
            }
        })
    }

    async fn fetch_piece(
        &self,
        source: &Source,
        blake3: [u8; 32],
        span: ChunkSpan,
    ) -> Result<Vec<u8>, ShareNetError> {
        let ticket = self.ticket.read().await.clone();
        let conn = match source {
            Source::Sharer => &self.conn,
            Source::Helper(h) => &h.conn,
        };
        let (response, mut recv) = call(
            conn,
            Request::Range {
                ticket,
                blake3,
                spans: vec![span],
            },
        )
        .await?;
        match response {
            Response::Bytes { len, .. } => read_body(&mut recv, len, MAX_PIECE_BODY_BYTES).await,
            _ => Err(ShareNetError::Protocol("unexpected answer to Range".into())),
        }
    }

    /// Waits out a sharer's upload pause, keeping every verified piece (FR-044): until the time it
    /// gave, or five minutes for a pause with no end. `false` when the import was stopped meanwhile.
    async fn wait_out_pause(&self, until: Option<u64>) -> bool {
        let now = crate::shares::now_secs();
        let wake = until
            .map(|u| u as i64)
            .filter(|&u| u > now)
            .unwrap_or(now + 300);
        {
            let mut record = self.record.lock().await;
            record.paused_until = Some(until.map(|u| u as i64).unwrap_or(0));
        }
        self.save().await;
        let wait = Duration::from_secs((wake - now).max(1) as u64);
        let resumed = tokio::select! {
            _ = self.cancel.cancelled() => false,
            _ = tokio::time::sleep(wait) => true,
        };
        self.record.lock().await.paused_until = None;
        self.save().await;
        resumed
    }

    /// Fingerprint of an archive the manifest listed as not yet prepared (research §13).
    async fn prepare(&self, sharer_archive_id: &str) -> Result<([u8; 32], u64), ShareNetError> {
        let (response, _) = call(
            &self.conn,
            Request::Prepare {
                session: self.session.clone(),
                claim_id: self.claim_id.clone(),
                archive_id: sharer_archive_id.to_string(),
            },
        )
        .await?;
        match response {
            Response::Prepared { blake3, size } => {
                // The current ticket predates this fingerprint.
                self.renew().await?;
                Ok((blake3, size))
            }
            _ => Err(ShareNetError::Protocol(
                "unexpected answer to Prepare".into(),
            )),
        }
    }
}

/// Opens the partial file and its outboard, both sized for the whole archive.
fn open_partial(
    dir: &Path,
    root: [u8; 32],
    size: u64,
) -> std::io::Result<(std::fs::File, PreOrderOutboard<std::fs::File>)> {
    std::fs::create_dir_all(dir)?;
    let name = hex(&root);
    let open = |path: PathBuf| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    };
    let data = open(dir.join(format!("{name}.part")))?;
    data.set_len(size)?;
    let tree = BaoTree::new(size, blobs::BLOCK_SIZE);
    let outboard_file = open(dir.join(format!("{name}.obao")))?;
    outboard_file.set_len(tree.outboard_size())?;
    Ok((
        data,
        PreOrderOutboard {
            root: blake3::Hash::from_bytes(root),
            tree,
            data: outboard_file,
        },
    ))
}

/// Chunk ranges that are already verified on disk.
fn verified_ranges(
    data: &std::fs::File,
    outboard: &PreOrderOutboard<std::fs::File>,
) -> ChunkRanges {
    let mut valid = ChunkRanges::empty();
    for range in bao_tree::io::sync::valid_ranges(outboard, data, &ChunkRanges::all()) {
        match range {
            Ok(r) => valid |= ChunkRanges::from(r),
            Err(_) => break,
        }
    }
    valid
}

/// Spans of `PIECE_CHUNKS` chunks that are not yet verified.
fn missing_pieces(total_chunks: u64, valid: &ChunkRanges) -> Vec<ChunkSpan> {
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < total_chunks {
        let end = (start + PIECE_CHUNKS).min(total_chunks);
        let piece = ChunkRanges::from(ChunkNum(start)..ChunkNum(end));
        if !valid.is_superset(&piece) {
            pieces.push(ChunkSpan { start, end });
        }
        start = end;
    }
    pieces
}

/// The whole file's BLAKE3, checked once more before cataloguing (FR-022).
fn hash_file(path: &Path) -> std::io::Result<blake3::Hash> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize())
}

/// Bytes under `<temp>/share` now: partial imports, merged patch copies and outboards. All three
/// live in the temp tree, so all three count against `tempmaxsize`.
async fn share_temp_bytes(state: &AppState) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .filter_map(|entry| entry.ok())
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path()),
                Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    let dir = state.library.temp_dir.join("share");
    tokio::task::spawn_blocking(move || walk(&dir))
        .await
        .unwrap_or(0)
}

/// `tempmaxsize` in bytes (the setting is in MB): the ceiling for the whole temp tree.
async fn temp_quota_bytes(state: &AppState) -> Option<u64> {
    let mut conn = state.redis.config.get().await.ok()?;
    let megabytes: Option<String> = deadpool_redis::redis::AsyncCommands::hget(
        &mut conn,
        lanrurugi_storage::keys::CONFIG_KEY,
        "tempmaxsize",
    )
    .await
    .ok()?;
    let megabytes = megabytes
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(crate::download_manager::ingest::DEFAULT_TEMP_MAX_SIZE_MB)
        .max(0) as u64;
    Some(megabytes * 1024 * 1024)
}

/// `book.zip`, `book (2).zip`, … for the n-th free-name attempt.
fn numbered_name(name: &str, n: u32) -> String {
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("archive");
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem} ({n}).{ext}"),
        None => format!("{stem} ({n})"),
    }
}

enum ArchiveOutcome {
    Done(String),
    Reused(String),
    /// The name was taken locally and the import's policy is to skip such archives.
    Skipped,
    Failed(String),
    /// The import was stopped or refused; the archive keeps its verified pieces.
    Interrupted,
}

async fn transfer_archive(run: Arc<Run>, entry: ImportArchive) -> ArchiveOutcome {
    let id = entry.sharer_archive_id.clone();
    // Archive ids are content-derived, so the same id here means the same file (FR-020).
    if let Ok(Some(_)) = run.state.repos.archives.get(&ArchiveId(id.clone())).await {
        return ArchiveOutcome::Reused(id);
    }
    let (root, size) = match entry.blake3.as_deref().and_then(unhex32) {
        Some(root) => (root, entry.size),
        None => match run.prepare(&id).await {
            Ok(found) => found,
            Err(ShareNetError::Refused(RefusalReason::Paused { until })) => {
                if run.wait_out_pause(until).await {
                    return Box::pin(transfer_archive(run.clone(), entry)).await;
                }
                return ArchiveOutcome::Interrupted;
            }
            Err(ShareNetError::Refused(_)) if run.cancel.is_cancelled() => {
                return ArchiveOutcome::Interrupted
            }
            Err(e) => return ArchiveOutcome::Failed(e.to_string()),
        },
    };
    if size > MAX_ANNOUNCED_BYTES {
        return ArchiveOutcome::Failed(format!(
            "the sharer announced {size} bytes for this archive, over the {MAX_ANNOUNCED_BYTES}-byte cap"
        ));
    }
    run.update(&id, |a| {
        a.blake3 = Some(hex(&root));
        a.size = size;
        a.state = ImportArchiveState::Transferring;
    })
    .await;

    let dir = run.dir.clone();
    let opened = tokio::task::spawn_blocking(move || {
        let (data, outboard) = open_partial(&dir, root, size)?;
        let valid = verified_ranges(&data, &outboard);
        Ok::<_, std::io::Error>((data, outboard, valid))
    })
    .await;
    let (data, outboard, valid) = match opened {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return ArchiveOutcome::Failed(storage_error(&e)),
        Err(e) => return ArchiveOutcome::Failed(format!("storage_error: {e}")),
    };
    let total_chunks = outboard.tree.chunks().0;
    let pieces = missing_pieces(total_chunks, &valid);
    let already = size.saturating_sub(
        pieces
            .iter()
            .map(|p| (p.end - p.start) * CHUNK_BYTES)
            .sum::<u64>(),
    );
    run.update(&id, |a| a.bytes_done = already.min(size)).await;

    let shared = Arc::new(std::sync::Mutex::new((data, outboard)));
    if let Some(outcome) = fetch_pieces(&run, &id, root, size, pieces, shared.clone()).await {
        return outcome;
    }
    drop(shared);

    run.update(&id, |a| a.state = ImportArchiveState::Ingesting)
        .await;
    let part = run.dir.join(format!("{}.part", hex(&root)));
    let check = part.clone();
    match tokio::task::spawn_blocking(move || hash_file(&check)).await {
        Ok(Ok(hash)) if *hash.as_bytes() == root => {}
        Ok(Ok(_)) => {
            let _ = tokio::fs::remove_file(&part).await;
            return ArchiveOutcome::Failed(
                "the received file does not match its fingerprint".into(),
            );
        }
        Ok(Err(e)) => return ArchiveOutcome::Failed(e.to_string()),
        Err(e) => return ArchiveOutcome::Failed(e.to_string()),
    }
    ingest(&run, &id, &part, root).await
}

/// One finished piece fetch: which source, which piece and how many times it already failed.
type Fetched = (Source, ChunkSpan, u32, Result<Vec<u8>, ShareNetError>);

/// Moves `pieces` into the partial file from every live source. `None` once all are verified,
/// else how the archive ended.
async fn fetch_pieces(
    run: &Arc<Run>,
    id: &str,
    root: [u8; 32],
    size: u64,
    pieces: Vec<ChunkSpan>,
    shared: Arc<std::sync::Mutex<(std::fs::File, PreOrderOutboard<std::fs::File>)>>,
) -> Option<ArchiveOutcome> {
    let mut queue: VecDeque<(ChunkSpan, u32)> = pieces.into_iter().map(|p| (p, 0)).collect();
    let mut in_flight = FuturesUnordered::new();
    let mut busy: std::collections::HashMap<Option<[u8; 32]>, usize> = Default::default();
    // When the sharer asked us to wait; helpers keep going meanwhile.
    let mut sharer_paused: Option<Option<u64>> = None;
    loop {
        if run.cancel.is_cancelled() {
            return Some(ArchiveOutcome::Interrupted);
        }
        let mut sources = Vec::new();
        if sharer_paused.is_none() {
            sources.push(Source::Sharer);
        }
        sources.extend(
            run.helper_list
                .read()
                .await
                .iter()
                .cloned()
                .map(Source::Helper),
        );
        for source in &sources {
            while busy.get(&source.key()).copied().unwrap_or(0) < IN_FLIGHT_PER_SOURCE {
                let Some((piece, attempts)) = queue.pop_front() else {
                    break;
                };
                *busy.entry(source.key()).or_default() += 1;
                let run = run.clone();
                let source = source.clone();
                in_flight.push(async move {
                    let result = run.fetch_piece(&source, root, piece).await;
                    (source, piece, attempts, result)
                });
            }
        }
        if in_flight.is_empty() {
            if queue.is_empty() {
                return None;
            }
            // Only the paused sharer is left: wait it out, keeping every verified piece (FR-044).
            let until = sharer_paused.take().flatten();
            if !run.wait_out_pause(until).await {
                return Some(ArchiveOutcome::Interrupted);
            }
            continue;
        }
        let Some((source, piece, attempts, fetched)): Option<Fetched> = in_flight.next().await
        else {
            continue;
        };
        if let Some(n) = busy.get_mut(&source.key()) {
            *n = n.saturating_sub(1);
        }
        let verified = match fetched {
            Ok(body) => {
                let shared = shared.clone();
                // Verified against the manifest's root before anything reaches the file; a failure
                // is split into "our disk refused the write" (final) and "the bytes did not verify"
                // (retry, or another source).
                let decoded = tokio::task::spawn_blocking(move || {
                    let mut guard = shared.lock().expect("partial lock");
                    let (data, outboard) = &mut *guard;
                    let ranges = ChunkRanges::from(ChunkNum(piece.start)..ChunkNum(piece.end));
                    bao_tree::io::sync::decode_ranges(
                        std::io::Cursor::new(body),
                        &ranges,
                        &mut *data,
                        &mut *outboard,
                    )
                })
                .await;
                match decoded {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(bao_tree::io::DecodeError::Io(e))) => {
                        return Some(ArchiveOutcome::Failed(storage_error(&e)))
                    }
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) => return Some(ArchiveOutcome::Failed(format!("storage_error: {e}"))),
                }
            }
            Err(ShareNetError::Refused(RefusalReason::Paused { until }))
                if matches!(source, Source::Sharer) =>
            {
                sharer_paused = Some(until);
                queue.push_front((piece, attempts));
                continue;
            }
            Err(e) => Err(e.to_string()),
        };
        match (verified, &source) {
            (Ok(()), _) => {
                let piece_bytes = ((piece.end - piece.start) * CHUNK_BYTES).min(size);
                run.update(id, |a| {
                    a.bytes_done = (a.bytes_done + piece_bytes).min(size)
                })
                .await;
                if run.over_budget().await {
                    return Some(ArchiveOutcome::Failed(
                        "temp space ran out: the import reached tempmaxsize".into(),
                    ));
                }
            }
            (Err(_), _) if run.cancel.is_cancelled() => return Some(ArchiveOutcome::Interrupted),
            // A helper that fails once is not asked again; the piece goes to another source.
            (Err(_), Source::Helper(h)) => {
                run.drop_helper(h.id).await;
                queue.push_front((piece, attempts));
            }
            (Err(e), Source::Sharer) if attempts + 1 >= MAX_PIECE_ATTEMPTS => {
                return Some(ArchiveOutcome::Failed(e))
            }
            (Err(_), Source::Sharer) => {
                tokio::time::sleep(Duration::from_secs(u64::from(attempts) + 1)).await;
                queue.push_back((piece, attempts + 1));
            }
        }
    }
}

/// Catalogues a completed archive and applies the shared title, tags and summary.
async fn ingest(run: &Run, sharer_archive_id: &str, part: &Path, root: [u8; 32]) -> ArchiveOutcome {
    let manifest: ShareManifest = {
        let record = run.record.lock().await;
        match serde_json::from_value(record.manifest.clone()) {
            Ok(m) => m,
            Err(e) => return ArchiveOutcome::Failed(e.to_string()),
        }
    };
    let Some(entry) = manifest
        .archives()
        .find(|a| a.archive_id == sharer_archive_id)
        .cloned()
    else {
        return ArchiveOutcome::Failed("archive missing from the manifest".into());
    };
    let filename = if entry.filename.is_empty() {
        format!("{}.zip", hex(&root))
    } else {
        entry.filename.clone()
    };
    let staging = run.dir.join(format!("{}.staged", hex(&root)));
    if let Err(e) = tokio::fs::rename(part, &staging).await {
        return ArchiveOutcome::Failed(storage_error(&e));
    }
    let _ = tokio::fs::remove_file(run.dir.join(format!("{}.obao", hex(&root)))).await;
    // A matching content hash really is the same archive, so reusing it is right. A *filename*
    // collision is not: two libraries both holding `001.zip` is ordinary, and treating that as
    // "already present" would point this import at a different, unrelated archive — so it gets a
    // free name and is catalogued like any other import.
    let mut attempt = 0;
    let local_id = loop {
        let name = if attempt == 0 {
            filename.clone()
        } else {
            numbered_name(&filename, attempt)
        };
        // `Overwrite` applies to the first attempt only: it deletes whichever local archive owns
        // the name, which a retry under a different name must never repeat.
        let policy = if attempt == 0 && run.filename_policy == FilenamePolicy::Overwrite {
            lanrurugi_scanner::pipeline::DuplicatePolicy::Overwrite
        } else {
            lanrurugi_scanner::pipeline::DuplicatePolicy::Reject
        };
        match crate::download_manager::ingest::catalogue_staged_file(
            &run.state, &staging, &name, None, policy,
        )
        .await
        {
            Ok(done) => break done.archive_id,
            Err(crate::download_manager::ingest::IngestDownloadError::Duplicate {
                existing_id,
                reason: lanrurugi_scanner::pipeline::DuplicateReason::ContentHash,
            }) => {
                let _ = tokio::fs::remove_file(&staging).await;
                return ArchiveOutcome::Reused(existing_id);
            }
            Err(crate::download_manager::ingest::IngestDownloadError::Duplicate {
                reason: lanrurugi_scanner::pipeline::DuplicateReason::Filename,
                ..
            }) if run.filename_policy == FilenamePolicy::Skip => {
                let _ = tokio::fs::remove_file(&staging).await;
                return ArchiveOutcome::Skipped;
            }
            Err(crate::download_manager::ingest::IngestDownloadError::Duplicate {
                reason: lanrurugi_scanner::pipeline::DuplicateReason::Filename,
                ..
            }) if attempt < MAX_RENAME_ATTEMPTS => {
                attempt += 1;
                let renamed = numbered_name(&filename, attempt);
                run.update(sharer_archive_id, |a| {
                    a.note = Some(format!("renamed to {renamed}"))
                })
                .await;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&staging).await;
                return ArchiveOutcome::Failed(e.to_string());
            }
        }
    };
    apply_shared_metadata(
        &run.state,
        &local_id,
        &entry.title,
        &entry.tags,
        &entry.summary,
        &entry.toc,
    )
    .await;
    let (category, inherit) = {
        let record = run.record.lock().await;
        (record.category.clone(), record.inherit_categories)
    };
    if inherit {
        for name in entry
            .categories
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
        {
            match crate::categories::static_category_named(&run.state, name).await {
                Ok(catid) => {
                    let _ =
                        crate::categories::add_archive_to_category(&run.state, &catid.0, &local_id)
                            .await;
                }
                Err(e) => tracing::warn!(error = %e, "could not keep a shared category"),
            }
        }
    }
    if let Some(category) = category.filter(|c| !c.is_empty()) {
        let _ = crate::categories::add_archive_to_category(&run.state, &category, &local_id).await;
    }
    ArchiveOutcome::Done(local_id)
}

/// Title and summary replace the scanner's guesses; shared tags merge into the archive's own; the
/// sharer's chapters fill an empty table of contents (one the book itself gave is kept), as a
/// book's own outline does. Bookmarks and stamps are the sharer's own and are not carried.
async fn apply_shared_metadata(
    state: &AppState,
    archive_id: &str,
    title: &str,
    tags: &str,
    summary: &str,
    toc: &[lanrurugi_share_proto::messages::Chapter],
) {
    if let Ok(Some(mut archive)) = state
        .repos
        .archives
        .get(&ArchiveId(archive_id.to_string()))
        .await
    {
        let old_title = archive.title.clone();
        if !title.is_empty() {
            archive.title = title.to_string();
        }
        if !summary.is_empty() {
            archive.summary = summary.to_string();
        }
        if archive.toc.is_empty() {
            archive.toc = toc
                .iter()
                .filter(|c| c.page >= 1 && c.page <= archive.pagecount.max(1))
                .map(|c| lanrurugi_core::entities::TocEntry {
                    page: c.page,
                    name: c.name.clone(),
                })
                .collect();
        }
        if state.repos.archives.save(&archive).await.is_ok() && old_title != archive.title {
            let _ = lanrurugi_search::indexer::update_title_index(
                &state.redis.search,
                &state.equivalence,
                archive_id,
                &old_title,
                &archive.title,
            )
            .await;
        }
    }
    crate::plugins::apply_metadata_tags(state, archive_id, &shared_tags(tags)).await;
}

/// The sharer's tags minus `date_added:`, which records when the *sharer* got the archive: the
/// import has its own, and merging both would leave the archive with two.
fn shared_tags(tags: &str) -> String {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with("date_added:"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `date_added:` value in a tag string, if any.
fn date_added_of(tags: &str) -> Option<u64> {
    tags.split(',')
        .find_map(|t| t.trim().strip_prefix("date_added:")?.trim().parse().ok())
}

/// `tags` with its `date_added:` set to `stamp`; `None` when it has none (the setting is off).
fn with_date_added(tags: &str, stamp: u64) -> Option<String> {
    let mut found = false;
    let out: Vec<String> = tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t.starts_with("date_added:") {
                found = true;
                format!("date_added:{stamp}")
            } else {
                t.to_string()
            }
        })
        .collect();
    found.then(|| out.join(", "))
}

/// Stamps for `(local id, sharer's date_added)` in manifest order: sorted by the sharer's date
/// (undated last, ties in manifest order), one second apart and ending at `now`.
fn import_stamps(entries: &[(String, Option<u64>)], now: u64) -> Vec<(String, u64)> {
    let mut order: Vec<_> = entries.iter().enumerate().collect();
    order.sort_by_key(|(i, (_, added))| (added.unwrap_or(u64::MAX), *i));
    let last = order.len().saturating_sub(1) as u64;
    order
        .into_iter()
        .enumerate()
        .map(|(rank, (_, (id, _)))| (id.clone(), now.saturating_sub(last - rank as u64)))
        .collect()
}

/// Re-dates every archive this import catalogued so the import lands as the newest in the
/// library while keeping the sharer's order: transfers run in parallel, so each archive's own
/// ingest time is arbitrary. Reused archives keep their date.
async fn restamp_date_added(run: &Run) {
    let record = run.record.lock().await.clone();
    let Ok(manifest) = serde_json::from_value::<ShareManifest>(record.manifest.clone()) else {
        return;
    };
    let mut seen = HashSet::new();
    let entries: Vec<(String, Option<u64>)> = manifest
        .archives()
        .filter(|a| seen.insert(a.archive_id.clone()))
        .filter_map(|a| {
            let local = record
                .archives
                .iter()
                .find(|r| {
                    r.sharer_archive_id == a.archive_id && r.state == ImportArchiveState::Done
                })?
                .local_archive_id
                .clone()?;
            Some((local, date_added_of(&a.tags)))
        })
        .collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for (local, stamp) in import_stamps(&entries, now) {
        let id = ArchiveId(local.clone());
        let Ok(Some(mut archive)) = run.state.repos.archives.get(&id).await else {
            continue;
        };
        let Some(tags) = with_date_added(&archive.tags, stamp) else {
            continue;
        };
        let old_tags = std::mem::replace(&mut archive.tags, tags);
        if run.state.repos.archives.save(&archive).await.is_ok() {
            let _ = lanrurugi_search::indexer::update_tag_indexes(
                &run.state.redis.search,
                &run.state.equivalence,
                &local,
                &old_tags,
                &archive.tags,
            )
            .await;
        }
    }
}

/// Recreates each shared tankoubon from the imported archives, members in order (FR-020).
async fn rebuild_tankoubons(run: &Run) {
    let record = run.record.lock().await.clone();
    let Ok(manifest) = serde_json::from_value::<ShareManifest>(record.manifest.clone()) else {
        return;
    };
    for item in &manifest.items {
        let ManifestItem::Tankoubon {
            name,
            tags,
            summary,
            members,
            ..
        } = item
        else {
            continue;
        };
        let local: Vec<ArchiveId> = members
            .iter()
            .filter_map(|m| {
                record
                    .archives
                    .iter()
                    .find(|a| a.sharer_archive_id == m.archive_id)
                    .and_then(|a| a.local_archive_id.clone())
            })
            .map(ArchiveId)
            .collect();
        if local.is_empty() {
            continue;
        }
        let existing = run
            .state
            .repos
            .groupings
            .list_all()
            .await
            .unwrap_or_default();
        if existing
            .iter()
            .any(|g| g.name == *name && g.archives == local)
        {
            run.record.lock().await.warnings.push(format!(
                "tankoubon \"{name}\" already exists with the same members; left as it was"
            ));
            continue;
        }
        if let Err(e) =
            crate::tankoubons::create_tankoubon_with(&run.state, name, summary, tags, local).await
        {
            tracing::warn!(error = %e, "could not recreate a shared tankoubon");
        }
    }
}

async fn set_queue_state(
    state: &AppState,
    queue_item_id: &str,
    to: DownloadQueueState,
    error: Option<String>,
) {
    if let Ok(Some(mut item)) = state.download_queue.get(queue_item_id).await {
        item.state = to;
        if matches!(
            to,
            DownloadQueueState::Done | DownloadQueueState::Error | DownloadQueueState::Cancelled
        ) {
            item.finished_at = Some(crate::shares::now_secs() * 1000);
        }
        item.error =
            error.map(|reason| lanrurugi_core::queue_error::QueueError::ShareImport { reason });
        if state.download_queue.update(&item).await.is_ok() {
            crate::download_queue::broadcast_item_update(state, &item);
        }
    }
}

/// Starts (or resumes) an import for its queue item. Returns the job id.
pub async fn start(state: &AppState, queue_item_id: &str, import_id: ShareImportId) -> String {
    let job_id = state.jobs.create("share_import").await;
    state.jobs.mark_active(&job_id).await;
    // Recorded before the task starts, so its own state updates never race this write.
    if let Ok(Some(mut item)) = state.download_queue.get(queue_item_id).await {
        item.job_id = Some(job_id.clone());
        if state.download_queue.update(&item).await.is_ok() {
            crate::download_queue::broadcast_item_update(state, &item);
        }
    }
    let cancel = CancellationToken::new();
    state
        .download_cancellations
        .lock()
        .await
        .insert(queue_item_id.to_string(), cancel.clone());
    let state = state.clone();
    let queue_item_id = queue_item_id.to_string();
    let job = job_id.clone();
    tokio::spawn(async move {
        let outcome = run_import(&state, &queue_item_id, &import_id, &job, cancel.clone()).await;
        state
            .download_cancellations
            .lock()
            .await
            .remove(&queue_item_id);
        match outcome {
            Ok(()) => {
                set_queue_state(&state, &queue_item_id, DownloadQueueState::Done, None).await;
                state
                    .jobs
                    .finish(&job, serde_json::json!({ "import_id": import_id }))
                    .await;
            }
            Err(message) if cancel.is_cancelled() && message.is_empty() => {
                set_queue_state(&state, &queue_item_id, DownloadQueueState::Cancelled, None).await;
                state.jobs.fail(&job, "stopped").await;
            }
            Err(message) => {
                set_queue_state(
                    &state,
                    &queue_item_id,
                    DownloadQueueState::Error,
                    Some(message.clone()),
                )
                .await;
                state.jobs.fail(&job, message).await;
            }
        }
    });
    job_id
}

/// Connects to the sharer, retrying with backoff while the import is active (FR-029).
async fn reconnect(
    state: &AppState,
    code: &str,
    cancel: &CancellationToken,
) -> Result<Opened, String> {
    let mut delay = Duration::from_secs(5);
    loop {
        let net = super::handle()
            .await
            .ok_or("sharing is not running on this instance")?;
        match open(state, &net, code).await {
            Ok(opened) => return Ok(opened),
            Err(ShareNetError::Refused(reason)) => return Err(reason.code().to_string()),
            Err(e) => tracing::info!(error = %e, "sharer unreachable; retrying"),
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(String::new()),
            _ = tokio::time::sleep(delay) => {}
        }
        delay = (delay * 2).min(RECONNECT_CAP);
    }
}

async fn run_import(
    state: &AppState,
    queue_item_id: &str,
    import_id: &ShareImportId,
    job_id: &str,
    cancel: CancellationToken,
) -> Result<(), String> {
    let mut record = repo(state)
        .get(import_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("the import record is gone")?;
    set_queue_state(state, queue_item_id, DownloadQueueState::Downloading, None).await;
    let code = record
        .code
        .clone()
        .ok_or("the import has no code left to reconnect with")?;
    let opened = reconnect(state, &code, &cancel).await?;

    // A claim lives 24 h after its last use; an import resumed later claims again.
    let ticket = match call(
        &opened.conn,
        Request::Renew {
            claim_id: record.claim_id.clone(),
        },
    )
    .await
    {
        Ok((Response::Renewed { ticket, .. }, _)) => ticket,
        Ok(_) => return Err("unexpected answer to Renew".into()),
        Err(ShareNetError::Refused(RefusalReason::NotFound)) => {
            let (claim_id, manifest, ticket) =
                claim(&opened).await.map_err(|e| e.code().to_string())?;
            record.claim_id = claim_id;
            record.manifest = serde_json::to_value(&manifest).map_err(|e| e.to_string())?;
            ticket
        }
        Err(e) => return Err(e.code().to_string()),
    };
    record.state = ImportState::Transferring;
    let dir = state
        .library
        .temp_dir
        .join("share")
        .join("import")
        .join(import_id.as_str());
    let concurrency = record.concurrency.max(1) as usize;
    // Partial files, merged patch copies and outboards all live under `<temp>/share`, so the import
    // is bounded by `tempmaxsize` like every other temp consumer — announced sizes first (so a
    // share that obviously cannot fit is refused before it touches the disk), then the bytes
    // actually written, below.
    let used = share_temp_bytes(state).await;
    let announced = record.archives.iter().map(|a| a.size).sum::<u64>();
    let budget = match temp_quota_bytes(state).await {
        Some(quota) if used.saturating_add(announced) > quota => {
            return Err(format!(
                "not enough temp space: {used} bytes in use, {announced} more announced, tempmaxsize {quota}"
            ))
        }
        Some(quota) => quota.saturating_sub(used),
        None => u64::MAX,
    };
    let net = super::handle()
        .await
        .ok_or("sharing is not running on this instance")?;
    let filename_policy = record.filename_policy;
    let run = Arc::new(Run {
        state: state.clone(),
        import_id: import_id.clone(),
        net,
        conn: opened.conn.clone(),
        helper_list: Arc::new(RwLock::new(Vec::new())),
        tried_helpers: std::sync::Mutex::new(HashSet::new()),
        session: opened.session.clone(),
        claim_id: record.claim_id.clone(),
        ticket: RwLock::new(ticket),
        stopped: RwLock::new(None),
        cancel: cancel.child_token(),
        record: tokio::sync::Mutex::new(record),
        job_id: job_id.to_string(),
        queue_item_id: queue_item_id.to_string(),
        budget,
        filename_policy,
        pushed: std::sync::Mutex::new((None, (ImportState::Transferring, None, Vec::new()))),
        dir,
    });
    run.save().await;
    // The first renewal also brings the sharer's current helper list.
    let _ = run.renew().await;
    let renewer = run.spawn_renewer();

    let pending: Vec<ImportArchive> = run
        .record
        .lock()
        .await
        .archives
        .iter()
        .filter(|a| !a.state.is_settled())
        .cloned()
        .collect();
    let permits = Arc::new(Semaphore::new(concurrency));
    let tasks = pending.into_iter().map(|entry| {
        let run = run.clone();
        let permits = permits.clone();
        async move {
            let _permit = permits.acquire().await.expect("semaphore open");
            let id = entry.sharer_archive_id.clone();
            let outcome = transfer_archive(run.clone(), entry).await;
            run.update(&id, |a| match outcome {
                ArchiveOutcome::Done(local) => {
                    a.state = ImportArchiveState::Done;
                    a.local_archive_id = Some(local);
                    a.bytes_done = a.size;
                    a.error = None;
                }
                ArchiveOutcome::Reused(local) => {
                    a.state = ImportArchiveState::Reused;
                    a.local_archive_id = Some(local);
                    // Counted as complete, so "4 / 4 archives" never shows short bytes.
                    a.bytes_done = a.size;
                    a.error = None;
                }
                ArchiveOutcome::Skipped => {
                    a.state = ImportArchiveState::Skipped;
                    a.note = Some("skipped: the filename is already in use locally".into());
                }
                ArchiveOutcome::Failed(e) => {
                    a.state = ImportArchiveState::Failed;
                    a.error = Some(e);
                }
                ArchiveOutcome::Interrupted => a.state = ImportArchiveState::Pending,
            })
            .await;
        }
    });
    futures_util::future::join_all(tasks).await;
    renewer.abort();

    if let Some(reason) = run.stopped.read().await.clone() {
        let mut record = run.record.lock().await;
        record.state = ImportState::Failed;
        record.error = Some(reason.code().to_string());
        drop(record);
        run.save().await;
        return Err(reason.code().to_string());
    }
    if cancel.is_cancelled() {
        return Err(String::new());
    }
    {
        let mut record = run.record.lock().await;
        record.state = ImportState::Rebuilding;
    }
    run.save().await;
    restamp_date_added(&run).await;
    rebuild_tankoubons(&run).await;
    let failed = {
        let mut record = run.record.lock().await;
        let failed = record
            .archives
            .iter()
            .filter(|a| a.state == ImportArchiveState::Failed)
            .count();
        record.state = if failed == 0 {
            ImportState::Done
        } else {
            ImportState::Failed
        };
        failed
    };
    run.save().await;
    let _ = tokio::fs::remove_dir_all(&run.dir).await;
    let _ = &run.import_id;
    if failed == 0 {
        if run.record.lock().await.help_deliver {
            super::helper::wake();
        }
        Ok(())
    } else {
        Err(format!("{failed} archive(s) failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Error, ErrorKind};

    #[test]
    fn local_write_failures_carry_a_stable_code() {
        // The queue row localizes the code; the OS text after it is for support.
        assert!(storage_error(&Error::from_raw_os_error(28)).starts_with("disk_full: "));
        assert!(storage_error(&Error::from_raw_os_error(30)).starts_with("read_only: "));
        assert!(storage_error(&Error::from_raw_os_error(122)).starts_with("quota_exceeded: "));
        assert!(
            storage_error(&Error::new(ErrorKind::PermissionDenied, "denied"))
                .starts_with("permission_denied: ")
        );
        assert!(storage_error(&Error::from_raw_os_error(5)).starts_with("storage_error: "));
    }

    #[test]
    fn shared_tags_keep_everything_but_the_sharers_date_added() {
        assert_eq!(
            shared_tags("artist:a, date_added:1700000000, rating:⭐⭐⭐, ,female:glasses"),
            "artist:a, rating:⭐⭐⭐, female:glasses"
        );
        assert_eq!(shared_tags(""), "");
    }

    #[test]
    fn import_stamps_follow_the_sharers_order_and_end_now() {
        let entries = vec![
            ("b".to_string(), Some(300)),
            ("x".to_string(), None),
            ("a".to_string(), Some(100)),
            ("c".to_string(), Some(300)),
        ];
        assert_eq!(
            import_stamps(&entries, 1000),
            vec![
                ("a".to_string(), 997),
                ("b".to_string(), 998),
                ("c".to_string(), 999),
                ("x".to_string(), 1000),
            ]
        );
        assert!(import_stamps(&[], 1000).is_empty());
    }

    #[test]
    fn with_date_added_replaces_only_an_existing_stamp() {
        assert_eq!(
            with_date_added("artist:a, date_added:5,female:b", 9).as_deref(),
            Some("artist:a, date_added:9, female:b")
        );
        assert_eq!(with_date_added("artist:a", 9), None);
        assert_eq!(date_added_of("artist:a, date_added: 42"), Some(42));
    }
}
