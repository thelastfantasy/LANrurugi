//! What makes a repeated "smart compare" fast and a first one visible: page fingerprints kept on
//! disk between comparisons, and how far a running comparison has decoded each side.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use lanrurugi_imgcompare::{FingerprintCache, Side};

/// The fingerprint files together are kept under this; the least recently used go first.
const CACHE_LIMIT_BYTES: u64 = 256 << 20;

/// Page fingerprints under `<temp>/compare_fingerprints/`, one file per archive state (the key
/// `lanrurugi-imgcompare` builds already changes with the file, its patch and its page list).
pub struct DiskFingerprintCache {
    dir: PathBuf,
}

impl DiskFingerprintCache {
    pub fn new(temp_dir: &Path) -> Self {
        Self {
            dir: temp_dir.join("compare_fingerprints"),
        }
    }

    fn file_for(&self, key: &str) -> PathBuf {
        self.dir
            .join(format!("{}.bin", blake3::hash(key.as_bytes()).to_hex()))
    }

    /// Deletes the least recently used files until the directory fits [`CACHE_LIMIT_BYTES`].
    fn trim(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some((meta.modified().ok()?, meta.len(), entry.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        if total <= CACHE_LIMIT_BYTES {
            return;
        }
        files.sort_by_key(|(modified, _, _)| *modified);
        for (_, len, path) in files {
            if total <= CACHE_LIMIT_BYTES {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }
}

impl FingerprintCache for DiskFingerprintCache {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.file_for(key);
        let bytes = std::fs::read(&path).ok()?;
        // A read counts as a use, so `trim` keeps what is still being compared.
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(bytes)
    }

    fn put(&self, key: &str, bytes: Vec<u8>) {
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.file_for(key);
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&temp, &bytes).is_ok() && std::fs::rename(&temp, &path).is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        self.trim();
    }
}

/// `done`/`total` pages for one side of a comparison.
type SideProgress = (usize, usize);

/// `done`/`total` pages per side of each comparison in flight, keyed by [`progress_key`].
static PROGRESS: Mutex<Option<HashMap<String, [SideProgress; 2]>>> = Mutex::new(None);

pub fn progress_key(a: &str, b: &str) -> String {
    format!("{a}|{b}")
}

pub fn report(key: &str, side: Side, done: usize, total: usize) {
    let mut guard = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = guard
        .get_or_insert_with(HashMap::new)
        .entry(key.to_string())
        .or_insert([(0, 0); 2]);
    entry[side_index(side)] = (done, total);
}

pub fn finish(key: &str) {
    if let Some(map) = PROGRESS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        map.remove(key);
    }
}

/// `[(done, total) for A, for B]`, or `None` when that comparison is not running.
pub fn current(key: &str) -> Option<[(usize, usize); 2]> {
    PROGRESS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(key)
        .copied()
}

fn side_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_reads_without_leaving_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskFingerprintCache::new(dir.path());
        cache.put("a", vec![1, 2, 3]);
        assert_eq!(cache.get("a"), Some(vec![1, 2, 3]));
        assert_eq!(cache.get("missing"), None);
        assert!(!std::fs::read_dir(&cache.dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn progress_is_tracked_per_comparison_until_finished() {
        let key = progress_key("x-test", "y-test");
        assert_eq!(current(&key), None);
        report(&key, Side::A, 10, 223);
        report(&key, Side::B, 0, 223);
        assert_eq!(current(&key), Some([(10, 223), (0, 223)]));
        finish(&key);
        assert_eq!(current(&key), None);
    }
}
