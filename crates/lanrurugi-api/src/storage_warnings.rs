//! Storage problems found at start-up, shown to an administrator as a homepage toast next to the
//! default-password one (`GET /login/status`'s `storage_warnings`).

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageWarning {
    /// The library directory is not a mounted volume in this container.
    LibraryUnmounted,
    /// The thumbnail directory is not a mounted volume in this container.
    ThumbnailsUnmounted,
    /// The bundled Redis's data directory is not a mounted volume in this container.
    DatabaseUnmounted,
    /// No archive file the database knows of exists; the records were kept.
    LibraryMissing,
}

static WARNINGS: Mutex<Vec<StorageWarning>> = Mutex::new(Vec::new());

/// Records `warning` for the rest of this process's life.
pub fn report(warning: StorageWarning) {
    let mut warnings = WARNINGS.lock().unwrap_or_else(|e| e.into_inner());
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

pub fn current() -> Vec<StorageWarning> {
    WARNINGS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_warning_is_reported_once() {
        report(StorageWarning::LibraryMissing);
        report(StorageWarning::LibraryMissing);
        let count = current()
            .iter()
            .filter(|w| **w == StorageWarning::LibraryMissing)
            .count();
        assert_eq!(count, 1);
        assert_eq!(
            serde_json::to_value(StorageWarning::ThumbnailsUnmounted).unwrap(),
            "thumbnails_unmounted"
        );
    }
}
