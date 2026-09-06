//! Rayon-parallel batch hashing for bulk scans (constitution Principle III — CPU-bound hashing
//! work is parallelized across cores, bridged into async via `spawn_blocking` so it can't stall
//! concurrent request handling).

use std::path::PathBuf;

use lanrurugi_core::concurrency::parallel_map;
use lanrurugi_storage::id::{self, IdError};

#[derive(Debug, Clone)]
pub struct HashedFile {
    pub path: PathBuf,
    pub id: Result<String, String>,
}

/// Hashes every path in `paths` in parallel (rayon, off the async reactor), using the size-aware
/// algorithm — the default for freshly-scanned content (research.md §1).
pub async fn hash_batch(paths: Vec<PathBuf>) -> Vec<HashedFile> {
    parallel_map(paths, |path| {
        let id = id::size_aware_id(&path).map_err(|e: IdError| e.to_string());
        HashedFile { path, id }
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn hashes_many_files_in_parallel_and_preserves_order() {
        let mut files = Vec::new();
        let mut paths = Vec::new();
        for i in 0..50 {
            let mut f = tempfile::NamedTempFile::new().unwrap();
            write!(f, "content for file {i}").unwrap();
            paths.push(f.path().to_path_buf());
            files.push(f); // keep alive until after hashing
        }

        let results = hash_batch(paths.clone()).await;
        assert_eq!(results.len(), paths.len());
        for (result, path) in results.iter().zip(paths.iter()) {
            assert_eq!(&result.path, path);
            assert!(result.id.is_ok());
            assert_eq!(result.id.as_ref().unwrap().len(), 40);
        }
    }
}
