//! Recycle-bin inventory and payload storage (`specs/019-recycle-bin-safety-password`).
//!
//! The Redis hash is authoritative; each entry also carries a `meta.json` next to its payload so a
//! crashed delete can be reconciled from disk. Payloads live inside the library under
//! [`RECYCLE_DIR_NAME`], which keeps delete/restore a same-filesystem rename and is excluded from
//! every library walker by `lanrurugi_scanner::watcher::is_watched_archive_path`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::entities::{Stamp, TocEntry};
use lanrurugi_core::ids::{ArchiveId, CategoryId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::keys::{RECYCLE_BIN_KEY, RECYCLE_TANKS_KEY};

/// Reserved directory name inside the library. Dot-prefixed so it stays out of casual browsing.
pub const RECYCLE_DIR_NAME: &str = ".lanrurugi-recycle";

/// Current on-disk/Redis entry format. An entry with an unknown version is reported as
/// un-restorable rather than parsed on a guess.
pub const RECYCLE_SCHEMA: u32 = 1;

#[derive(Debug, Error)]
pub enum RecycleError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed recycle entry {id:?}: {source}")]
    Json {
        id: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, RecycleError>;

/// Who deleted the archive. Mirrors the activity log's actor shape without depending on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecycleActor {
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkSnapshot {
    pub page: u32,
    pub bookmarked_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Everything deletion would otherwise destroy, kept so restore can put it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecycleSnapshot {
    pub tags: String,
    pub summary: String,
    pub isnew: bool,
    pub lastreadpage: u32,
    pub lastreadtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbhash: Option<String>,
    #[serde(default)]
    pub toc: Vec<TocEntry>,
    #[serde(default)]
    pub has_patch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    #[serde(default)]
    pub corrupted_pages: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heal_failed_at: Option<u64>,
    #[serde(default)]
    pub version_history: Vec<(String, String)>,
    #[serde(default)]
    pub categories: Vec<CategoryId>,
    #[serde(default)]
    pub bookmarks: Vec<BookmarkSnapshot>,
    #[serde(default)]
    pub stamps: Vec<Stamp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecycleEntry {
    pub schema: u32,
    pub id: ArchiveId,
    pub title: String,
    pub name: String,
    /// Absolute path the file was deleted from; the restore target.
    pub original_path: String,
    /// Basename including extension; the payload lives at `payload/<file_name>`.
    pub file_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_file_name: Option<String>,
    pub size: u64,
    pub page_count: u32,
    /// Unix seconds.
    pub deleted_at: i64,
    pub deleted_by: RecycleActor,
    pub snapshot: RecycleSnapshot,
}

/// A whole Tankoubon in the bin: its grouping metadata plus the ids of the member archives, each
/// stored as an ordinary [`RecycleEntry`] in the same bin. The list shows this record as one row;
/// the members are hidden from the top level and restored/purged with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecycleTank {
    pub schema: u32,
    pub tank_id: String,
    /// The whole grouping record, so restore re-creates the Tankoubon exactly (name, summary,
    /// tags, volume order, reading progress, cover choice).
    pub grouping: lanrurugi_core::entities::Grouping,
    pub size: u64,
    pub page_count: u32,
    pub deleted_at: i64,
    pub deleted_by: RecycleActor,
}

/// Why an entry cannot be restored right now. `None` in [`RecycleEntryView::conflict`] means it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecycleConflict {
    IdenticalArchiveExists,
    PayloadMissing,
    PayloadSizeMismatch,
    UnsupportedSchema,
}

/// What a bin row represents: a single archive, or a whole Tankoubon whose members are hidden
/// inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecycleKind {
    Archive,
    Tankoubon,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecycleMemberView {
    pub id: ArchiveId,
    pub title: String,
    pub size: u64,
    pub page_count: u32,
    pub restorable: bool,
    pub conflict: Option<RecycleConflict>,
}

/// Read-time projection handed to the API layer. Never stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecycleEntryView {
    pub kind: RecycleKind,
    pub id: ArchiveId,
    pub title: String,
    pub name: String,
    pub original_path: String,
    pub file_name: String,
    pub size: u64,
    pub page_count: u32,
    pub deleted_at: i64,
    pub deleted_by: RecycleActor,
    pub expires_at: Option<i64>,
    pub remaining_seconds: Option<i64>,
    pub restorable: bool,
    pub conflict: Option<RecycleConflict>,
    pub has_thumbnail: bool,
    /// Tankoubon only: how many archives it holds, and their read-only summaries for the bin's
    /// expandable row. Always empty for an archive entry.
    pub member_count: u32,
    pub members: Vec<RecycleMemberView>,
}

#[derive(Clone)]
pub struct RecycleRepository {
    pool: Pool,
}

impl RecycleRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, id: &ArchiveId) -> Result<Option<RecycleEntry>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(RECYCLE_BIN_KEY, id.as_str()).await?;
        match raw {
            Some(raw) => Ok(Some(parse_entry(id, &raw)?)),
            None => Ok(None),
        }
    }

    pub async fn put(&self, entry: &RecycleEntry) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let raw = serde_json::to_string(entry).map_err(|source| RecycleError::Json {
            id: entry.id.to_string(),
            source,
        })?;
        let _: () = conn.hset(RECYCLE_BIN_KEY, entry.id.as_str(), raw).await?;
        Ok(())
    }

    /// Every entry, newest deletion first. A single malformed field is logged and skipped rather
    /// than failing the whole bin.
    pub async fn list(&self) -> Result<Vec<RecycleEntry>> {
        let mut conn = self.pool.get().await?;
        let raw: std::collections::HashMap<String, String> = conn.hgetall(RECYCLE_BIN_KEY).await?;
        let mut entries = Vec::with_capacity(raw.len());
        for (id, value) in raw {
            match parse_entry(&ArchiveId(id.clone()), &value) {
                Ok(entry) => entries.push(entry),
                Err(e) => tracing::warn!(%id, error = %e, "skipping malformed recycle entry"),
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.deleted_at));
        Ok(entries)
    }

    pub async fn remove(&self, id: &ArchiveId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(RECYCLE_BIN_KEY, id.as_str()).await?;
        Ok(())
    }

    /// `(count, total_bytes)` from the hash alone — no directory walk.
    pub async fn usage(&self) -> Result<(u64, u64)> {
        let entries = self.list().await?;
        let count = entries.len() as u64;
        let bytes = entries.iter().map(|e| e.size).sum();
        Ok((count, bytes))
    }

    pub async fn contains(&self, id: &ArchiveId) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        let exists: bool = conn.hexists(RECYCLE_BIN_KEY, id.as_str()).await?;
        Ok(exists)
    }

    pub async fn get_tank(&self, tank_id: &str) -> Result<Option<RecycleTank>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(RECYCLE_TANKS_KEY, tank_id).await?;
        match raw {
            Some(raw) => {
                serde_json::from_str(&raw)
                    .map(Some)
                    .map_err(|source| RecycleError::Json {
                        id: tank_id.to_string(),
                        source,
                    })
            }
            None => Ok(None),
        }
    }

    pub async fn put_tank(&self, tank: &RecycleTank) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let raw = serde_json::to_string(tank).map_err(|source| RecycleError::Json {
            id: tank.tank_id.clone(),
            source,
        })?;
        let _: () = conn.hset(RECYCLE_TANKS_KEY, &tank.tank_id, raw).await?;
        Ok(())
    }

    pub async fn remove_tank(&self, tank_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(RECYCLE_TANKS_KEY, tank_id).await?;
        Ok(())
    }

    /// Every recycled Tankoubon, newest first. Same per-entry skip-on-malformed posture as `list`.
    pub async fn list_tanks(&self) -> Result<Vec<RecycleTank>> {
        let mut conn = self.pool.get().await?;
        let raw: std::collections::HashMap<String, String> =
            conn.hgetall(RECYCLE_TANKS_KEY).await?;
        let mut tanks = Vec::with_capacity(raw.len());
        for (tank_id, value) in raw {
            match serde_json::from_str::<RecycleTank>(&value) {
                Ok(tank) => tanks.push(tank),
                Err(e) => tracing::warn!(%tank_id, error = %e, "skipping malformed recycled tank"),
            }
        }
        tanks.sort_by_key(|t| std::cmp::Reverse(t.deleted_at));
        Ok(tanks)
    }
}

fn parse_entry(id: &ArchiveId, raw: &str) -> Result<RecycleEntry> {
    serde_json::from_str(raw).map_err(|source| RecycleError::Json {
        id: id.to_string(),
        source,
    })
}

// -------------------------------------------------------------------------------------------
// Filesystem layout
// -------------------------------------------------------------------------------------------

pub fn bin_root(library: &Path) -> PathBuf {
    library.join(RECYCLE_DIR_NAME)
}

pub fn entry_dir(library: &Path, id: &ArchiveId) -> PathBuf {
    bin_root(library).join(id.as_str())
}

pub fn payload_dir(library: &Path, id: &ArchiveId) -> PathBuf {
    entry_dir(library, id).join("payload")
}

pub fn meta_path(entry_dir: &Path) -> PathBuf {
    entry_dir.join("meta.json")
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Writes `meta.json` through a temp file + rename so a crash never leaves half a journal.
pub async fn write_meta(entry_dir: &Path, entry: &RecycleEntry) -> Result<()> {
    let raw = serde_json::to_vec_pretty(entry).map_err(|source| RecycleError::Json {
        id: entry.id.to_string(),
        source,
    })?;
    let tmp = entry_dir.join("meta.json.tmp");
    tokio::fs::write(&tmp, &raw).await?;
    tokio::fs::rename(&tmp, meta_path(entry_dir)).await?;
    Ok(())
}

pub async fn read_meta(entry_dir: &Path) -> Option<RecycleEntry> {
    let raw = tokio::fs::read(meta_path(entry_dir)).await.ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Moves the payload (and its sidecar patch) into the bin. Returns the patch basename when one was
/// moved. `rename` only: a cross-device fallback would silently double the bytes and is refused.
pub async fn move_into_bin(
    library: &Path,
    id: &ArchiveId,
    source_path: &str,
    patch_source: Option<&Path>,
) -> Result<Option<String>> {
    let payload = payload_dir(library, id);
    tokio::fs::create_dir_all(&payload).await?;
    let file_name = Path::new(source_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("archive path {source_path:?} has no file name"),
            )
        })?;
    tokio::fs::rename(source_path, payload.join(&file_name)).await?;
    let mut patch_name = None;
    if let Some(patch) = patch_source {
        if let Some(name) = patch.file_name().map(|n| n.to_string_lossy().into_owned()) {
            if patch.exists() {
                tokio::fs::rename(patch, payload.join(&name)).await?;
                patch_name = Some(name);
            }
        }
    }
    Ok(patch_name)
}

/// Moves the payload out to `destination` (and the patch beside it). Returns the patch's new path.
pub async fn move_out_of_bin(
    library: &Path,
    id: &ArchiveId,
    file_name: &str,
    patch_file_name: Option<&str>,
    destination: &Path,
) -> Result<Option<PathBuf>> {
    let payload = payload_dir(library, id);
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(payload.join(file_name), destination).await?;
    let mut patch_dest = None;
    if let Some(name) = patch_file_name {
        let dest = destination.with_file_name(name);
        tokio::fs::rename(payload.join(name), &dest).await?;
        patch_dest = Some(dest);
    }
    Ok(patch_dest)
}

pub async fn remove_entry_dir(library: &Path, id: &ArchiveId) -> Result<()> {
    match tokio::fs::remove_dir_all(entry_dir(library, id)).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// `(payload_exists, payload_len)` for one entry — the read-time restorable check.
pub async fn payload_size(library: &Path, id: &ArchiveId, file_name: &str) -> Option<u64> {
    let meta = tokio::fs::metadata(payload_dir(library, id).join(file_name))
        .await
        .ok()?;
    Some(meta.len())
}

/// Directories present under the bin root with no matching inventory entry — leftovers from a
/// crash before the hash write, or foreign files.
pub async fn scan_entry_dirs(library: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dir = match tokio::fs::read_dir(bin_root(library)).await {
        Ok(d) => d,
        Err(_) => return out,
    };
    while let Ok(Some(entry)) = dir.next_entry().await {
        if entry.path().is_dir() {
            out.push(entry.path());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> RecycleActor {
        RecycleActor {
            kind: "session".into(),
            label: "admin".into(),
        }
    }

    fn entry(id: &str, size: u64, at: i64) -> RecycleEntry {
        RecycleEntry {
            schema: RECYCLE_SCHEMA,
            id: ArchiveId(id.into()),
            title: "Title".into(),
            name: "book".into(),
            original_path: "/library/book.zip".into(),
            file_name: "book.zip".into(),
            patch_file_name: None,
            size,
            page_count: 10,
            deleted_at: at,
            deleted_by: actor(),
            snapshot: RecycleSnapshot {
                tags: "artist:x".into(),
                summary: String::new(),
                isnew: false,
                lastreadpage: 3,
                lastreadtime: 42,
                thumbhash: None,
                toc: vec![],
                has_patch: false,
                blake3: None,
                corrupted_pages: vec![],
                heal_failed_at: None,
                version_history: vec![],
                categories: vec![CategoryId("SET_1".into())],
                bookmarks: vec![BookmarkSnapshot {
                    page: 3,
                    bookmarked_at: 7,
                    name: Some("here".into()),
                }],
                stamps: vec![],
            },
        }
    }

    #[tokio::test]
    async fn move_into_and_out_of_bin_roundtrip() {
        let library = tempfile::tempdir().unwrap();
        let id = ArchiveId("a".repeat(40));
        let source = library.path().join("book.zip");
        tokio::fs::write(&source, b"payload-bytes").await.unwrap();

        #[cfg(unix)]
        let inode_before = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&source).unwrap().ino()
        };
        let patch = move_into_bin(library.path(), &id, source.to_str().unwrap(), None)
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // SC-006: the move is a rename, not a copy — a copy would allocate a new inode.
            let moved = payload_dir(library.path(), &id).join("book.zip");
            assert_eq!(std::fs::metadata(&moved).unwrap().ino(), inode_before);
        }
        assert!(patch.is_none());
        assert!(!source.exists());
        assert_eq!(
            payload_size(library.path(), &id, "book.zip").await,
            Some(13)
        );

        let dest = library.path().join("restored.zip");
        move_out_of_bin(library.path(), &id, "book.zip", None, &dest)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"payload-bytes");
        remove_entry_dir(library.path(), &id).await.unwrap();
        assert!(!entry_dir(library.path(), &id).exists());
    }

    #[test]
    fn entry_json_roundtrips_without_losing_the_snapshot() {
        let e = entry("b".repeat(40).as_str(), 1024, 100);
        let raw = serde_json::to_string(&e).unwrap();
        let back: RecycleEntry = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, e);
    }
}
