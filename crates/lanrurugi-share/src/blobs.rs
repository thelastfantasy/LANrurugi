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

use crate::host::ShareHost;

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

fn share_dir(state: &dyn ShareHost) -> PathBuf {
    state.temp_dir().join("share")
}

fn outboard_path(state: &dyn ShareHost, blake3: &[u8; 32]) -> PathBuf {
    share_dir(state)
        .join("outboards")
        .join(format!("{}.obao", hex(blake3)))
}

fn merged_path(state: &dyn ShareHost, archive_id: &ArchiveId) -> PathBuf {
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
pub async fn ready(state: &dyn ShareHost, archive_id: &ArchiveId) -> Option<BlobInfo> {
    let archive = state.archive(archive_id).await?;
    let mut conn = state.redis().config.get().await.ok()?;
    let fields: std::collections::HashMap<String, String> = conn
        .hgetall(keys::share_blob_key(archive_id.as_str()))
        .await
        .ok()?;
    let blake3 = unhex32(fields.get("blake3")?)?;
    let size: u64 = fields.get("size")?.parse().ok()?;
    let (mtime, source_size) = stamp(&archive.file)?;
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
    // A per-writer temp name: the background prepare worker and an importer's `Prepare` can hash
    // the same archive at once, and a shared `.part` let one writer truncate the other's outboard
    // (which then got renamed into place and served forever, since the source file never changed).
    let tmp = target.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
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
    let tmp = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
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
pub async fn prepare(state: &dyn ShareHost, archive_ids: &[ArchiveId]) -> usize {
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
        let Some(archive) = state.archive(id).await else {
            continue;
        };
        let Some(stamp) = stamp(&archive.file) else {
            continue;
        };
        let has_patch = lanrurugi_scanner::patch::patch_path_for(&archive.file).is_file();
        jobs.push(Job {
            data_path: if has_patch {
                merged_path(state, id)
            } else {
                PathBuf::from(&archive.file)
            },
            archive_id: id.clone(),
            source: archive.file.display().to_string(),
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
    if let Ok(mut conn) = state.redis().config.get().await {
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
pub async fn ensure_ready(state: &dyn ShareHost, archive_id: &ArchiveId) -> Option<BlobInfo> {
    if let Some(info) = ready(state, archive_id).await {
        return Some(info);
    }
    prepare(state, std::slice::from_ref(archive_id)).await;
    ready(state, archive_id).await
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

/// The most one `Range` request may ask for, in bytes.
/// The protocol counts chunks in 1 KiB units, independent of the tree's block size (see
/// `share_net::import`'s note): the ranges a `Range` request names are in these units.
pub const CHUNK_BYTES: u64 = 1024;
pub const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;
/// The most spans one `Range` request may name.
pub const MAX_RANGE_SPANS: usize = 64;

/// The chunk ranges of `spans`, refused when they exceed the per-request cap.
pub fn requested_ranges(
    spans: &[lanrurugi_share_proto::messages::ChunkSpan],
    size: u64,
    chunk_bytes: u64,
) -> Option<bao_tree::ChunkRanges> {
    use bao_tree::ChunkNum;
    if spans.len() > MAX_RANGE_SPANS {
        return None;
    }
    let total_chunks = size.div_ceil(chunk_bytes);
    let mut ranges = bao_tree::ChunkRanges::empty();
    let mut requested = 0u64;
    for span in spans {
        let start = span.start.min(total_chunks);
        let end = span.end.min(total_chunks);
        if end <= start {
            continue;
        }
        requested = requested.saturating_add((end - start).saturating_mul(chunk_bytes));
        ranges |= bao_tree::ChunkRanges::from(ChunkNum(start)..ChunkNum(end));
    }
    (requested <= MAX_RANGE_BYTES).then_some(ranges)
}

/// The verified bao encoding of `spans` of a prepared blob.
pub async fn encode_range(
    info: BlobInfo,
    spans: &[lanrurugi_share_proto::messages::ChunkSpan],
) -> Option<Vec<u8>> {
    use bao_tree::io::outboard::PreOrderOutboard;
    let ranges = requested_ranges(spans, info.size, CHUNK_BYTES)?;
    tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let data = std::fs::File::open(&info.data_path)?;
        let outboard = PreOrderOutboard {
            root: blake3::Hash::from_bytes(info.blake3),
            tree: bao_tree::BaoTree::new(info.size, BLOCK_SIZE),
            data: std::fs::File::open(&info.outboard_path)?,
        };
        let mut out = Vec::new();
        bao_tree::io::sync::encode_ranges_validated(&data, &outboard, &ranges, &mut out)?;
        Ok(out)
    })
    .await
    .ok()?
    .ok()
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use bao_tree::{ChunkNum, ChunkRanges};
    use lanrurugi_share_proto::messages::ChunkSpan;

    fn chunk_bytes() -> u64 {
        CHUNK_BYTES
    }

    #[test]
    fn a_span_that_would_wrap_the_cap_is_refused() {
        // `(end - start) * chunk_bytes` for this pair is 2^64, i.e. 0 in a wrapping u64.
        let spans = [ChunkSpan {
            start: 0,
            end: 1 << 54,
        }];
        assert!(requested_ranges(&spans, 1 << 30, chunk_bytes()).is_none());
    }

    #[test]
    fn spans_are_clamped_to_the_blob_before_being_counted() {
        let size = 1024 * 1024;
        let ranges = requested_ranges(
            &[ChunkSpan {
                start: 0,
                end: u64::MAX,
            }],
            size,
            chunk_bytes(),
        )
        .expect("a whole small blob fits the cap");
        assert_eq!(ranges, ChunkRanges::from(ChunkNum(0)..ChunkNum(1024)));
    }

    #[test]
    fn too_many_spans_are_refused() {
        let spans: Vec<ChunkSpan> = (0..=MAX_RANGE_SPANS as u64)
            .map(|i| ChunkSpan {
                start: i,
                end: i + 1,
            })
            .collect();
        assert!(requested_ranges(&spans, 1 << 30, chunk_bytes()).is_none());
    }

    #[test]
    fn a_normal_piece_is_accepted() {
        let ranges = requested_ranges(
            &[ChunkSpan { start: 0, end: 64 }],
            8 * 1024 * 1024,
            chunk_bytes(),
        )
        .unwrap();
        assert_eq!(ranges, ChunkRanges::from(ChunkNum(0)..ChunkNum(64)));
    }

    #[test]
    fn a_cap_sized_request_is_accepted_and_one_chunk_more_is_not() {
        let size = 16 * 1024 * 1024;
        let cap_chunks = MAX_RANGE_BYTES / chunk_bytes();
        assert!(requested_ranges(
            &[ChunkSpan {
                start: 0,
                end: cap_chunks
            }],
            size,
            chunk_bytes()
        )
        .is_some());
        assert!(requested_ranges(
            &[ChunkSpan {
                start: 0,
                end: cap_chunks + 1
            }],
            size,
            chunk_bytes()
        )
        .is_none());
    }
}
