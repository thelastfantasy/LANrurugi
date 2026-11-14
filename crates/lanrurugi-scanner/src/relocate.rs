//! Moves every archive still addressed by its filename into `<library>/_byname/` (020 FR-012).
//!
//! The hash layout (`<library>/<ab>/<id>.<ext>`) is the only place a *new* archive is written.
//! Anything catalogued by filename — a library from before 020, or a file a user dropped into the
//! mount over SMB — keeps its own name and relative directory, but lives under `_byname/` so the
//! library root has exactly one rule: hash-named archives in buckets, everything else in
//! `_byname/`. Moving preserves the id (the file's bytes do not change), so categories, tankoubon
//! membership and reading progress are untouched; only the recorded path and the `LRR_FILEMAP`
//! entry move.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::Archive;
use lanrurugi_storage::keys::FILEMAP_KEY;
use lanrurugi_storage::paths;
use lanrurugi_storage::repository::ArchiveRepository;
use lanrurugi_storage::repository::RepositoryError;

/// What one relocation pass did. Only ever counts — there is no preview or confirmation step:
/// the rule is unconditional, and a file that cannot move stays where it is and is reported.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RelocationReport {
    pub moved: u64,
    pub skipped_occupied: u64,
    /// Files that changed too recently to be safe to move yet; the next pass takes them.
    pub settling: u64,
    /// Records whose path is outside the library (a legacy import pointing at another mount).
    pub outside: u64,
    /// Records whose path was put right after an interrupted pass (see `relocate_library`).
    pub repaired: u64,
    pub failed: u64,
}

impl RelocationReport {
    pub fn is_empty(&self) -> bool {
        self.moved == 0
            && self.skipped_occupied == 0
            && self.settling == 0
            && self.outside == 0
            && self.repaired == 0
            && self.failed == 0
    }
}

/// What one `relocate_file` call decided. The three "did not move" cases are distinct on purpose:
/// folding them together made every pass log "`_byname` already holds a file" for a file that was
/// merely still being copied, and counted paths outside the library as collisions forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocationOutcome {
    Moved,
    Settling,
    Occupied,
    /// The record already points inside `_byname/`; `relocate_library` never asks about these.
    AlreadyPlaced,
    /// The record's path is not under the library at all (a legacy import pointing elsewhere).
    Outside,
}

/// Where `path` should live, or `None` when it is already in one of the two layouts.
pub fn destination_for(library: &Path, path: &Path) -> Option<PathBuf> {
    if paths::is_in_byname(library, path) {
        return None;
    }
    // A hash-layout path is checked by stem only, so this needs the id; callers that know it skip
    // earlier. Without an id we fall back to "does it sit in a bucket-looking directory" via the
    // record's own id.
    let relative = path.strip_prefix(library).ok()?;
    Some(paths::byname_dir(library).join(relative))
}

/// A file that changed in the last minute may still be arriving (`cp`, rsync, an SMB upload).
/// Moving it mid-copy would leave the writer appending to a path that no longer exists while
/// `_byname/` holds a truncated copy — and, worse, a record pointing at those partial bytes. The
/// next pass moves it once it has settled.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(60);

/// Moves `path` and the `.patch.zip` sidecar that belongs to it (if any).
///
/// The outcome says why nothing moved: a destination that already holds a file is never
/// overwritten (two different files that used to live in different directories must not silently
/// collapse into one), a file still arriving reports `Settling`, and a record pointing outside the
/// library reports `Outside`. The archive is only left at its destination when the caller then
/// manages to save the new path.
pub async fn relocate_file(
    library: &Path,
    path: &Path,
) -> std::io::Result<(RelocationOutcome, Option<PathBuf>)> {
    if paths::is_in_byname(library, path) {
        return Ok((RelocationOutcome::AlreadyPlaced, None));
    }
    let Some(dest) = destination_for(library, path) else {
        // `relocate_library` already skips `_byname/`, so the only way here is a record whose path
        // is not under the library at all.
        return Ok((RelocationOutcome::Outside, None));
    };
    if let Ok(meta) = tokio::fs::metadata(path).await {
        let settling = meta
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < SETTLE);
        if settling {
            return Ok((RelocationOutcome::Settling, None));
        }
    }
    if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        return Ok((RelocationOutcome::Occupied, None));
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    move_file(path, &dest).await?;

    // A patch is addressed by its archive's own path, so it has to travel with it. Either half of
    // a pair that cannot move together leaves the archive in place: relocation never revisits a
    // file already in `_byname/`, so a split pair is permanent (`has_patch` stays true while the
    // patch can no longer be found).
    let old_patch = crate::patch::patch_path_for(path);
    if tokio::fs::try_exists(&old_patch).await.unwrap_or(false) {
        let new_patch = crate::patch::patch_path_for(&dest);
        if tokio::fs::try_exists(&new_patch).await.unwrap_or(false) {
            // Two different patches for the same relative name: keep both, and keep the archive
            // with its own patch instead of letting it adopt the destination's one.
            tracing::warn!(
                patch = %new_patch.display(),
                "not moving the archive: _byname already has a patch at its path"
            );
            if let Err(back) = move_file(&dest, path).await {
                tracing::warn!(
                    path = %path.display(),
                    error = %back,
                    "could not move the archive back after its patch collided"
                );
            }
            return Ok((RelocationOutcome::Occupied, None));
        }
        if let Err(e) = move_file(&old_patch, &new_patch).await {
            tracing::warn!(
                patch = %old_patch.display(),
                error = %e,
                "could not move the patch with its archive; moving the archive back"
            );
            if let Err(back) = move_file(&dest, path).await {
                tracing::warn!(
                    path = %path.display(),
                    error = %back,
                    "could not move the archive back after its patch failed to move"
                );
            }
            // A patch that cannot be written is an I/O failure, not a collision: the caller must
            // count it as `failed`, or a broken mount looks like "someone else took the name".
            return Err(e);
        }
    }
    Ok((RelocationOutcome::Moved, Some(dest)))
}

/// Removes the directories a moved archive left empty, up to (but never including) the library
/// root. The recycle bin does the same; without it a flat-library conversion leaves a skeleton of
/// empty folders behind.
async fn prune_empty_parents(library: &Path, moved_from: &Path) {
    let Some(mut dir) = moved_from.parent() else {
        return;
    };
    while dir.starts_with(library) && dir != library {
        match tokio::fs::remove_dir(dir).await {
            Ok(()) => {}
            // Not empty (another file still lives there) or gone: either way stop climbing.
            Err(_) => return,
        }
        let Some(parent) = dir.parent() else {
            return;
        };
        dir = parent;
    }
}

/// One pass over every catalogued archive, moving those outside both layouts into `_byname/`.
pub async fn relocate_library(
    archives: &ArchiveRepository,
    filemap: &deadpool_redis::Pool,
    library: &Path,
) -> Result<RelocationReport, RepositoryError> {
    let all = archives.list_all().await?;
    let mut report = RelocationReport::default();
    let mut conn = match filemap.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "cannot relocate: no Redis connection for LRR_FILEMAP");
            return Ok(report);
        }
    };

    for mut archive in all {
        let path = PathBuf::from(&archive.file);
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            // A pass killed between its rename and its save leaves exactly this: the record still
            // names the original path, the bytes are already in `_byname/`, and nothing else would
            // ever put them back together (the scanner sees the file at its new place and the
            // record already exists). Derive the counterpart and fix the record in place.
            if let Some(dest) = destination_for(library, &path) {
                let same_bytes = tokio::fs::try_exists(&dest).await.unwrap_or(false)
                    && lanrurugi_storage::id::size_aware_id(&dest)
                        .map(|id| id == archive.id.as_str())
                        .unwrap_or(false);
                if same_bytes {
                    archive.file = dest.to_string_lossy().to_string();
                    match save_record(archives, &archive).await {
                        Ok(()) => {
                            let _: Result<(), _> = conn
                                .hdel(FILEMAP_KEY, path.to_string_lossy().to_string())
                                .await;
                            let _: Result<(), _> = conn
                                .hset(FILEMAP_KEY, archive.file.as_str(), archive.id.as_str())
                                .await;
                            tracing::info!(
                                id = %archive.id,
                                from = %path.display(),
                                to = %archive.file,
                                "repaired a record left behind by an interrupted relocation"
                            );
                            report.repaired += 1;
                        }
                        Err(e) => {
                            tracing::warn!(
                                id = %archive.id,
                                error = %e,
                                "found an interrupted relocation but could not repair its record"
                            );
                            report.failed += 1;
                        }
                    }
                }
            }
            // A record without bytes is the sweeper's problem, not this pass's.
            continue;
        }
        if paths::is_hash_layout_path(library, &path, archive.id.as_str())
            || paths::is_in_byname(library, &path)
        {
            continue;
        }
        match relocate_file(library, &path).await {
            Ok((RelocationOutcome::Moved, Some(dest))) => {
                let moved_from = path.clone();
                let old = std::mem::replace(&mut archive.file, dest.to_string_lossy().to_string());
                // The list was taken before this pass, and a first run over a large library can take
                // hours; saving the whole struct would write back every field as it was then and
                // silently drop tags, a title or reading progress changed in between. Re-read the
                // record and change only `file`.
                // A record that vanished while the file was moving must not be resurrected from
                // this pass's stale snapshot: the file goes back where it came from and the next
                // scan decides what that path is.
                let (saved, record_vanished) = match archives.get(&archive.id).await {
                    Ok(Some(fresh)) => {
                        let mut fresh = fresh;
                        fresh.file = archive.file.clone();
                        (save_record(archives, &fresh).await, false)
                    }
                    Ok(None) => (Ok(()), true),
                    Err(e) => (Err(e), false),
                };
                let saved = if record_vanished {
                    Err(RepositoryError::NotFound("archive", archive.id.to_string()))
                } else {
                    saved
                };
                if let Err(e) = saved {
                    tracing::warn!(
                        id = %archive.id,
                        error = %e,
                        "moved an archive into _byname but could not update its record; moving it back"
                    );
                    // Leaving the file at its destination with a record that still names the old
                    // path is the one state nothing later repairs: the scan sees the bytes in
                    // `_byname/` and the record already exists, so it neither re-catalogues nor
                    // fixes the path. Put it back where the record still points.
                    // `relocate_file` pruned nothing, but a rollback still has to tolerate a
                    // missing parent (a flat library where the source directory held one file),
                    // a cross-filesystem move, and a patch that already travelled.
                    if let Err(back) = move_back(&dest, &path).await {
                        tracing::warn!(
                            path = %dest.display(),
                            error = %back,
                            "could not move the archive back after the record update failed"
                        );
                    }
                    report.failed += 1;
                    continue;
                }
                let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &old).await;
                let _: Result<(), _> = conn
                    .hset(FILEMAP_KEY, &archive.file, archive.id.as_str())
                    .await;
                // Only now: pruning before the save is what made a failed save unrecoverable.
                prune_empty_parents(library, &moved_from).await;
                tracing::info!(
                    id = %archive.id,
                    from = %old,
                    to = %archive.file,
                    "moved an archive into _byname"
                );
                report.moved += 1;
            }
            Ok((RelocationOutcome::Occupied, _)) => {
                tracing::warn!(
                    path = %path.display(),
                    "not relocating: _byname already holds a file at that relative path"
                );
                report.skipped_occupied += 1;
            }
            Ok((RelocationOutcome::Settling, _)) => {
                report.settling += 1;
            }
            Ok((RelocationOutcome::AlreadyPlaced, _)) => {}
            Ok((RelocationOutcome::Outside, _)) => {
                tracing::info!(
                    path = %path.display(),
                    "not relocating: the record points outside the library"
                );
                report.outside += 1;
            }
            Ok((RelocationOutcome::Moved, None)) => {
                report.failed += 1;
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to relocate an archive into _byname"
                );
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Puts a moved archive (and a patch that already travelled with it) back where the record still
/// points, recreating directories the move may have emptied.
/// Test seam: a save that can be made to fail for one archive id, so the rollback path is
/// reachable without a Redis fault. Keyed by id rather than a global flag: `cargo test` runs the
/// relocation tests in parallel and a process-wide switch made unrelated tests fail at random.
#[cfg(test)]
static FAIL_SAVE_FOR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) fn fail_saves_for(id: &str) {
    *FAIL_SAVE_FOR.lock().unwrap() = Some(id.to_string());
}

#[cfg(test)]
pub(crate) fn allow_saves() {
    *FAIL_SAVE_FOR.lock().unwrap() = None;
}

async fn save_record(
    archives: &ArchiveRepository,
    archive: &Archive,
) -> Result<(), RepositoryError> {
    #[cfg(test)]
    if FAIL_SAVE_FOR
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|id| id == archive.id.as_str())
    {
        return Err(RepositoryError::NotFound("archive", archive.id.to_string()));
    }
    archives.save(archive).await
}

async fn move_back(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    move_file(from, to).await?;
    let moved_patch = crate::patch::patch_path_for(from);
    if tokio::fs::try_exists(&moved_patch).await.unwrap_or(false) {
        let original_patch = crate::patch::patch_path_for(to);
        move_file(&moved_patch, &original_patch).await?;
    }
    Ok(())
}

/// Durable `rename` (027 FR-005: flushed before and after), falling back across filesystems to
/// the checked copy (`lanrurugi_core::io::copy_file`: length check, flush, buffered) under a
/// temporary name, renamed into place and flushed before the source is removed. `_byname/` is
/// normally on the same filesystem, but a library can be assembled from several mounts.
async fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match lanrurugi_core::io::durable_rename(from, to).await {
        Ok(()) => Ok(()),
        Err(e)
            if e.kind() == ErrorKind::CrossesDevices || lanrurugi_core::io::is_cross_device(&e) =>
        {
            // Copy under a temporary name and rename it into place: a copy straight to `to` that is
            // interrupted leaves a truncated file at the destination, which then looks "occupied"
            // forever and gets catalogued as an archive of its own.
            let staging = to.with_file_name(format!(
                ".{}.part-{}",
                to.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "relocate".to_string()),
                uuid::Uuid::new_v4().simple()
            ));
            let copied = async {
                lanrurugi_core::io::copy_file(
                    from,
                    &staging,
                    &lanrurugi_core::io::CopyOptions::default(),
                )
                .await
                .map_err(std::io::Error::other)?;
                tokio::fs::rename(&staging, to).await?;
                if let Some(parent) = to.parent() {
                    lanrurugi_core::io::sync_dir(parent).await?;
                }
                Ok::<(), std::io::Error>(())
            }
            .await;
            if let Err(e) = copied {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(e);
            }
            tokio::fs::remove_file(from).await?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settle window keeps a file that is still being copied in place, so the tests that
    /// expect a move have to age the file first.
    async fn settle(path: &Path) {
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .await
            .unwrap()
            .into_std()
            .await;
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        file.set_modified(old).unwrap();
    }

    /// Two scratch pools: one for records, one for `LRR_FILEMAP`. Each test uses its own db offset.
    async fn test_repo(db: u8) -> Option<(ArchiveRepository, deadpool_redis::Pool)> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = |d: u8| format!("{}/{d}", base.trim_end_matches('/'));
        let archive = lanrurugi_storage::test_support::test_pool_for_url(&url(db)).await?;
        let filemap = lanrurugi_storage::test_support::test_pool_for_url(&url(db + 1)).await?;
        let archives = ArchiveRepository::new(archive);
        // These slices belong to this test alone; start from a clean one.
        for record in archives.list_all().await.unwrap_or_default() {
            let _ = archives.delete(&record.id).await;
        }
        let mut conn = filemap.get().await.unwrap();
        let _: Result<(), _> = deadpool_redis::redis::cmd("DEL")
            .arg(FILEMAP_KEY)
            .query_async(&mut conn)
            .await;
        Some((archives, filemap))
    }

    #[test]
    fn only_paths_outside_both_layouts_are_destinations() {
        let library = Path::new("/library");
        assert_eq!(
            destination_for(library, Path::new("/library/Series/Book.zip")),
            Some(PathBuf::from("/library/_byname/Series/Book.zip"))
        );
        assert_eq!(
            destination_for(library, Path::new("/library/_byname/Series/Book.zip")),
            None
        );
    }

    #[tokio::test]
    async fn relocating_keeps_the_relative_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Series One/Book 01.zip");
        tokio::fs::create_dir_all(original.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"bytes").await.unwrap();
        settle(&original).await;

        let (outcome, moved) = relocate_file(library, &original).await.unwrap();
        assert_eq!(outcome, RelocationOutcome::Moved);
        let moved = moved.unwrap();
        assert_eq!(moved, library.join("_byname/Series One/Book 01.zip"));
        assert!(!original.exists());
        assert_eq!(tokio::fs::read(&moved).await.unwrap(), b"bytes");
    }

    #[tokio::test]
    async fn an_occupied_destination_is_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Book.zip");
        let occupied = library.join("_byname/Book.zip");
        tokio::fs::create_dir_all(occupied.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"new").await.unwrap();
        tokio::fs::write(&occupied, b"old").await.unwrap();
        settle(&original).await;

        assert_eq!(
            relocate_file(library, &original).await.unwrap().0,
            RelocationOutcome::Occupied
        );
        assert_eq!(tokio::fs::read(&original).await.unwrap(), b"new");
        assert_eq!(tokio::fs::read(&occupied).await.unwrap(), b"old");
    }

    #[tokio::test]
    async fn a_file_that_is_still_being_copied_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Series/Arriving.zip");
        tokio::fs::create_dir_all(original.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"half").await.unwrap();

        assert_eq!(
            relocate_file(library, &original).await.unwrap().0,
            RelocationOutcome::Settling
        );
        assert!(original.exists(), "an unfinished copy must not move");
        assert!(!library.join("_byname").exists());
    }

    #[tokio::test]
    async fn the_patch_moves_with_its_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Series/Book.zip");
        tokio::fs::create_dir_all(original.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"archive").await.unwrap();
        let patch = crate::patch::patch_path_for(&original);
        tokio::fs::write(&patch, b"patch").await.unwrap();
        settle(&original).await;
        settle(&patch).await;

        let moved = relocate_file(library, &original).await.unwrap().1.unwrap();
        assert_eq!(moved, library.join("_byname/Series/Book.zip"));
        let moved_patch = crate::patch::patch_path_for(&moved);
        assert!(
            moved_patch.exists(),
            "the patch moved alongside its archive"
        );
        assert_eq!(tokio::fs::read(&moved_patch).await.unwrap(), b"patch");
        assert!(!patch.exists());
    }

    fn archive_at(path: &Path, id: &str) -> Archive {
        Archive {
            id: lanrurugi_core::ids::ArchiveId(id.to_string()),
            name: "Book".into(),
            title: "Book".into(),
            file: path.to_string_lossy().to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 6,
            pagecount: 1,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
            blake3: None,
        }
    }

    /// A relocation pass killed between the rename and the save leaves the record naming the old
    /// path with the bytes already in `_byname/`. The next pass must put them back together instead
    /// of leaving a record that points at nothing.
    #[tokio::test]
    async fn an_interrupted_relocation_is_repaired_by_the_next_pass() {
        let Some((archives, filemap)) = test_repo(44).await else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Series/Book.zip");
        tokio::fs::create_dir_all(original.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"archive bytes").await.unwrap();
        settle(&original).await;
        let id = lanrurugi_storage::id::size_aware_id(&original).unwrap();

        // Simulate the kill: the file moved, the record did not.
        let moved = library.join("_byname/Series/Book.zip");
        tokio::fs::create_dir_all(moved.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::rename(&original, &moved).await.unwrap();
        archives.save(&archive_at(&original, &id)).await.unwrap();

        let report = relocate_library(&archives, &filemap, library)
            .await
            .unwrap();

        assert_eq!(report.repaired, 1, "the pass repairs the stale path");
        assert_eq!(report.moved, 0);
        let record = archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
            .unwrap()
            .expect("the record still exists");
        assert_eq!(
            PathBuf::from(&record.file),
            moved,
            "the record now names where the bytes actually are"
        );
        assert!(moved.exists());
        assert!(!original.exists());
        archives
            .delete(&lanrurugi_core::ids::ArchiveId(id))
            .await
            .unwrap();
    }

    /// When the record cannot be saved after the move, the file and the patch have to come back —
    /// otherwise the record points at a path nothing exists at and no later pass repairs it.
    #[tokio::test]
    async fn a_failed_save_puts_the_archive_and_its_patch_back() {
        let Some((archives, filemap)) = test_repo(46).await else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Series/Book.zip");
        tokio::fs::create_dir_all(original.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&original, b"archive bytes").await.unwrap();
        let patch = crate::patch::patch_path_for(&original);
        tokio::fs::write(&patch, b"patch").await.unwrap();
        settle(&original).await;
        settle(&patch).await;
        let id = lanrurugi_storage::id::size_aware_id(&original).unwrap();
        archives.save(&archive_at(&original, &id)).await.unwrap();

        fail_saves_for(id.as_str());
        let report = relocate_library(&archives, &filemap, library).await;
        allow_saves();
        let report = report.unwrap();

        assert_eq!(report.failed, 1);
        assert_eq!(report.moved, 0);
        assert!(
            original.exists(),
            "the archive is back where the record says"
        );
        assert!(patch.exists(), "the patch came back with it");
        assert!(!library.join("_byname/Series/Book.zip").exists());
        archives
            .delete(&lanrurugi_core::ids::ArchiveId(id))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_second_pass_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let original = library.join("Book.zip");
        tokio::fs::write(&original, b"bytes").await.unwrap();
        settle(&original).await;

        let moved = relocate_file(library, &original).await.unwrap().1.unwrap();
        settle(&moved).await;
        assert_eq!(
            relocate_file(library, &moved).await.unwrap().0,
            RelocationOutcome::AlreadyPlaced
        );
        assert!(moved.exists());
    }
}
