//! Putting files into library directories that another task may be pruning.
//!
//! The library's bucket directories are removed as soon as they become empty (relocation and the
//! recycle bin both prune after a move). A writer that created the directory a moment earlier can
//! find it gone by the time it writes, which surfaces as a `NotFound` from the write itself — the
//! same race the sidecar write hit (#302). Every write or move into a library directory goes through
//! [`in_dir`], which recreates the directory and tries again.

use std::future::Future;
use std::io::ErrorKind;
use std::path::Path;

/// Attempts at most: a prune would have to win the race this many times in a row.
const ATTEMPTS: usize = 3;

/// Creates `target`'s parent directory and runs `op` (the write or move that puts `target` there),
/// recreating the directory and running `op` again when it fails with `NotFound`. `op` must be safe
/// to repeat after a `NotFound` — a rename or a fresh write is.
pub async fn in_dir<T, F, Fut>(target: &Path, mut op: F) -> std::io::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::io::Result<T>>,
{
    let mut attempt = 1;
    loop {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match op().await {
            Err(e) if e.kind() == ErrorKind::NotFound && attempt < ATTEMPTS => attempt += 1,
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::in_dir;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_directory_pruned_between_create_and_write_is_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("ab").join("file.zip");
        let calls = AtomicUsize::new(0);
        in_dir(&target, || {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let target = target.clone();
            async move {
                if call == 0 {
                    // The prune that races this write.
                    tokio::fs::remove_dir(target.parent().unwrap()).await?;
                }
                tokio::fs::write(&target, b"bytes").await
            }
        })
        .await
        .expect("the second attempt recreates the directory");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(target.is_file());
    }

    #[tokio::test]
    async fn a_missing_source_still_fails_after_the_retries() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("ab").join("file.zip");
        let missing = tmp.path().join("gone.zip");
        let err = in_dir(&target, || tokio::fs::rename(&missing, &target))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
