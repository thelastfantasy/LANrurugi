//! BLAKE3 fingerprints and bao outboards of shared archives (research §5, §13).
//!
//! An archive is identified on the wire by the BLAKE3 hash of the bytes a download delivers: the
//! file itself, or the patch-merged zip when a `.patch.zip` sidecar exists. The outboard (the hash
//! tree, about 0.4% of the size at 16 KiB groups) is cached on disk so ranges can be served without
//! rehashing; it is regenerated whenever the source file changes or the cache was cleaned.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use bao_tree::io::outboard::PreOrderOutboard;
use bao_tree::io::sync::CreateOutboard;
use bao_tree::BlockSize;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_storage::keys;

use crate::AppState;

/// 16 KiB chunk groups.
pub const BLOCK_SIZE: BlockSize = BlockSize::from_chunk_log(4);

/// A fingerprinted archive, ready to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    pub blake3: [u8; 32],
    pub size: u64,
    /// The bytes served: the archive file, or its merged copy.
    pub data_path: PathBuf,
    pub outboard_path: PathBuf,
}

fn share_dir(state: &AppState) -> PathBuf {
    state.library.temp_dir.join("share")
}

fn outboard_path(state: &AppState, blake3: &[u8; 32]) -> PathBuf {
    share_dir(state)
        .join("outboards")
        .join(format!("{}.obao", hex(blake3)))
}

fn merged_path(state: &AppState, archive_id: &ArchiveId) -> PathBuf {
    share_dir(state)
        .join("merged")
        .join(format!("{archive_id}.zip"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// `(mtime secs, size)` of a file, used to notice that an archive changed after it was hashed.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((mtime, meta.len()))
}

/// The fingerprint of `archive_id` if it is current: the source file is unchanged since hashing
/// and the outboard is still on disk. `None` means it needs (re)preparing.
pub async fn ready(state: &AppState, archive_id: &ArchiveId) -> Option<BlobInfo> {
    let archive = state.repos.archives.get(archive_id).await.ok().flatten()?;
    let mut conn = state.redis.config.get().await.ok()?;
    let fields: std::collections::HashMap<String, String> = conn
        .hgetall(keys::share_blob_key(archive_id.as_str()))
        .await
        .ok()?;
    let blake3 = unhex32(fields.get("blake3")?)?;
    let size: u64 = fields.get("size")?.parse().ok()?;
    let (mtime, source_size) = stamp(Path::new(&archive.file))?;
    if fields.get("source_mtime")?.parse::<u64>().ok()? != mtime
        || fields.get("source_size")?.parse::<u64>().ok()? != source_size
        || fields.get("source_path")? != &archive.file
    {
        return None;
    }
    let data_path = PathBuf::from(fields.get("data_path")?);
    let outboard_path = outboard_path(state, &blake3);
    (data_path.is_file() && outboard_path.is_file()).then_some(BlobInfo {
        blake3,
        size,
        data_path,
        outboard_path,
    })
}

/// Hashes one file and writes its outboard. Runs on a blocking thread.
fn fingerprint_file(data_path: &Path, outboards: &Path) -> std::io::Result<(blake3::Hash, u64)> {
    let file = std::fs::File::open(data_path)?;
    let outboard = PreOrderOutboard::<Vec<u8>>::create(file, BLOCK_SIZE)?;
    std::fs::create_dir_all(outboards)?;
    let target = outboards.join(format!("{}.obao", outboard.root.to_hex()));
    let tmp = target.with_extension("obao.part");
    std::fs::write(&tmp, &outboard.data)?;
    std::fs::rename(&tmp, &target)?;
    Ok((outboard.root, outboard.tree.size()))
}

/// Writes `source` merged with its patch to `target` through a temporary file, a page at a time.
/// `false` when there was nothing to merge.
fn write_merged(source: &Path, target: &Path) -> std::io::Result<bool> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("zip.tmp");
    let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    match lanrurugi_scanner::patch::write_merged_zip(source, file) {
        Ok(true) => std::fs::rename(&tmp, target).map(|()| true),
        Ok(false) => std::fs::remove_file(&tmp).map(|()| false),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(std::io::Error::other(e.to_string()))
        }
    }
}

/// Fingerprints the given archives (skipping ones already current), in parallel within the
/// process-wide CPU budget. Returns how many were (re)hashed.
pub async fn prepare(state: &AppState, archive_ids: &[ArchiveId]) -> usize {
    struct Job {
        archive_id: ArchiveId,
        source: String,
        data_path: PathBuf,
        merge: bool,
        stamp: (u64, u64),
    }
    let mut jobs = Vec::new();
    for id in archive_ids {
        if ready(state, id).await.is_some() {
            continue;
        }
        let Ok(Some(archive)) = state.repos.archives.get(id).await else {
            continue;
        };
        let Some(stamp) = stamp(Path::new(&archive.file)) else {
            continue;
        };
        let has_patch =
            lanrurugi_scanner::patch::patch_path_for(Path::new(&archive.file)).is_file();
        jobs.push(Job {
            data_path: if has_patch {
                merged_path(state, id)
            } else {
                PathBuf::from(&archive.file)
            },
            archive_id: id.clone(),
            source: archive.file,
            merge: has_patch,
            stamp,
        });
    }
    if jobs.is_empty() {
        return 0;
    }
    let outboards = share_dir(state).join("outboards");
    let results = lanrurugi_core::concurrency::parallel_map(jobs, move |job| {
        if job.merge {
            // The merged zip is what a download delivers, so it is what gets fingerprinted.
            match write_merged(Path::new(&job.source), &job.data_path) {
                Ok(true) => {}
                Ok(false) => {
                    let data_path = PathBuf::from(&job.source);
                    let result = fingerprint_file(&data_path, &outboards).ok();
                    return (Job { data_path, ..job }, result);
                }
                Err(_) => return (job, None),
            }
        }
        let result = fingerprint_file(&job.data_path, &outboards).ok();
        (job, result)
    })
    .await
    .unwrap_or_default();

    let mut done = 0;
    if let Ok(mut conn) = state.redis.config.get().await {
        for (job, result) in results {
            let Some((root, size)) = result else {
                tracing::warn!(archive = %job.archive_id, "could not fingerprint a shared archive");
                continue;
            };
            let fields = [
                ("blake3", root.to_hex().to_string()),
                ("size", size.to_string()),
                ("source_path", job.source.clone()),
                ("source_mtime", job.stamp.0.to_string()),
                ("source_size", job.stamp.1.to_string()),
                ("data_path", job.data_path.to_string_lossy().into_owned()),
                ("merged", if job.merge { "1" } else { "0" }.to_string()),
            ];
            let key = keys::share_blob_key(job.archive_id.as_str());
            let _: Result<(), _> = conn.hset_multiple(&key, &fields).await;
            done += 1;
        }
    }
    done
}

/// Ensures one archive is ready, hashing it now if needed (an importer's `Prepare`).
pub async fn ensure_ready(state: &AppState, archive_id: &ArchiveId) -> Option<BlobInfo> {
    if let Some(info) = ready(state, archive_id).await {
        return Some(info);
    }
    prepare(state, std::slice::from_ref(archive_id)).await;
    ready(state, archive_id).await
}

/// Archives fingerprinted per batch of the background job.
const PREPARE_BATCH: isize = 16;
const PREPARE_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// Drains the preparation queue in the background, one `share_prepare` job per busy period, with
/// the same load backoff as the other background jobs (research §13). An importer that needs an
/// archive first does not wait for this: its `Prepare` request hashes that archive directly.
pub fn spawn_prepare_worker(state: AppState) {
    tokio::spawn(async move {
        let mut throttle = crate::recommend_precompute::LoadThrottle::new();
        let mut job: Option<String> = None;
        loop {
            let batch: Vec<String> = match state.redis.config.get().await {
                Ok(mut conn) => conn
                    .zrange(keys::SHARE_PREPARE_KEY, 0, PREPARE_BATCH - 1)
                    .await
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            if batch.is_empty() {
                if let Some(id) = job.take() {
                    state.jobs.finish(&id, serde_json::json!({})).await;
                }
                tokio::time::sleep(PREPARE_IDLE).await;
                continue;
            }
            if job.is_none() {
                let id = state.jobs.create("share_prepare").await;
                state.jobs.mark_active(&id).await;
                job = Some(id);
            }
            let started = std::time::Instant::now();
            let ids: Vec<ArchiveId> = batch.iter().cloned().map(ArchiveId).collect();
            prepare(&state, &ids).await;
            if let Ok(mut conn) = state.redis.config.get().await {
                let _: Result<(), _> = conn.zrem(keys::SHARE_PREPARE_KEY, &batch).await;
            }
            tokio::time::sleep(throttle.sample_and_backoff(started.elapsed())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_match_blake3_and_write_an_outboard() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let path = dir.path().join("a.zip");
        std::fs::write(&path, &data).unwrap();
        let outboards = dir.path().join("ob");
        let (root, size) = fingerprint_file(&path, &outboards).unwrap();
        assert_eq!(root, blake3::hash(&data));
        assert_eq!(size, data.len() as u64);
        let ob = std::fs::read(outboards.join(format!("{}.obao", root.to_hex()))).unwrap();
        assert!(!ob.is_empty() && ob.len() < data.len() / 50);
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [7u8; 32];
        assert_eq!(unhex32(&hex(&bytes)), Some(bytes));
        assert_eq!(unhex32("zz"), None);
    }
}
