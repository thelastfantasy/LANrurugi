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
    pub failed: u64,
}

impl RelocationReport {
    pub fn is_empty(&self) -> bool {
        self.moved == 0 && self.skipped_occupied == 0 && self.failed == 0
    }
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

/// Moves `path` and the `.patch.zip` sidecar that belongs to it (if any), returning the new path.
///
/// `Ok(None)` means the destination already holds a file. The pass never overwrites: a collision
/// means the user has two different files that used to be in different directories, and either one
/// winning would be a silent data loss.
/// A file that changed in the last minute may still be arriving (`cp`, rsync, an SMB upload).
/// Moving it mid-copy would leave the writer appending to a path that no longer exists while
/// `_byname/` holds a truncated copy — and, worse, a record pointing at those partial bytes. The
/// next pass moves it once it has settled.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(60);

pub async fn relocate_file(library: &Path, path: &Path) -> std::io::Result<Option<PathBuf>> {
    let Some(dest) = destination_for(library, path) else {
        return Ok(None);
    };
    if let Ok(meta) = tokio::fs::metadata(path).await {
        let settling = meta
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < SETTLE);
        if settling {
            return Ok(None);
        }
    }
    if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        return Ok(None);
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    move_file(path, &dest).await?;

    // A patch is addressed by its archive's own path, so it has to travel with it.
    let old_patch = crate::patch::patch_path_for(path);
    if tokio::fs::try_exists(&old_patch).await.unwrap_or(false) {
        let new_patch = crate::patch::patch_path_for(&dest);
        if let Err(e) = move_file(&old_patch, &new_patch).await {
            // The archive moved; leaving its patch behind is recoverable by hand and must not
            // undo (or fail) the move.
            tracing::warn!(
                patch = %old_patch.display(),
                error = %e,
                "archive moved into _byname but its patch did not"
            );
        }
    }
    Ok(Some(dest))
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
        // A record without bytes is the sweeper's problem, not this pass's.
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            continue;
        }
        if paths::is_hash_layout_path(library, &path, archive.id.as_str())
            || paths::is_in_byname(library, &path)
        {
            continue;
        }
        match relocate_file(library, &path).await {
            Ok(Some(dest)) => {
                let old = std::mem::replace(&mut archive.file, dest.to_string_lossy().to_string());
                if let Err(e) = archives.save(&archive).await {
                    tracing::warn!(
                        id = %archive.id,
                        error = %e,
                        "moved an archive into _byname but could not update its record"
                    );
                    report.failed += 1;
                    continue;
                }
                let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &old).await;
                let _: Result<(), _> = conn
                    .hset(FILEMAP_KEY, &archive.file, archive.id.as_str())
                    .await;
                report.moved += 1;
            }
            Ok(None) => {
                tracing::warn!(
                    path = %path.display(),
                    "not relocating: _byname already holds a file at that relative path"
                );
                report.skipped_occupied += 1;
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

/// `rename`, falling back to copy + `fsync` + unlink across filesystems (`_byname/` is normally on
/// the same one, but a library can be assembled from several mounts).
async fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices || e.raw_os_error() == Some(18) => {
            tokio::fs::copy(from, to).await?;
            // `copy` is not durable; the rename path is, so match it here.
            let file = tokio::fs::OpenOptions::new().write(true).open(to).await?;
            file.sync_all().await?;
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

        let moved = relocate_file(library, &original).await.unwrap().unwrap();
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

        assert_eq!(relocate_file(library, &original).await.unwrap(), None);
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

        assert_eq!(relocate_file(library, &original).await.unwrap(), None);
        assert!(original.exists(), "an unfinished copy must not move");
        assert!(!library.join("_byname").exists());
    }
}
