//! Library-directory diagnostics (`specs/028-deletion-safety-net`, research.md D7).
//!
//! The real mount-table parsing lives in `lanrurugi_core::library_diagnostics` because the scanner
//! publishes the same five fields from `full_scan` and cannot depend on this crate. This module is
//! the API-side face of it: the source of a library path, and the record/file agreement ratio.
//!
//! Diagnostic only. Nothing here is ever a reason to refuse an operation.

use std::path::Path;

use lanrurugi_core::entities::Archive;

/// `(library_source, library_source_known)` for a library directory.
pub fn library_source(library: &Path) -> (String, bool) {
    let source = lanrurugi_core::library_diagnostics::detect(library);
    (source.value, source.known)
}

/// Percent of `records` whose `file` still exists on disk.
pub fn overlap_percent(records: &[Archive]) -> u8 {
    lanrurugi_core::library_diagnostics::overlap_percent(
        records.iter().map(|record| record.file.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::ids::ArchiveId;

    fn record(file: &str) -> Archive {
        Archive {
            id: ArchiveId("a".repeat(40)),
            name: "n".to_string(),
            title: "t".to_string(),
            file: file.to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
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
            primary_source: None,
            progress_pages: None,
        }
    }

    #[test]
    fn overlap_counts_records_whose_file_is_still_there() {
        let tmp = tempfile::tempdir().unwrap();
        let present = tmp.path().join("present.zip");
        std::fs::write(&present, b"x").unwrap();
        let records = vec![
            record(&present.to_string_lossy()),
            record(&tmp.path().join("gone.zip").to_string_lossy()),
        ];
        assert_eq!(overlap_percent(&records), 50);
        assert_eq!(overlap_percent(&[]), 0);
        // An unlinked record (empty path) is never counted as present.
        assert_eq!(overlap_percent(&[record("")]), 0);
    }

    #[test]
    fn a_relative_or_unknown_path_reports_unknown_rather_than_failing() {
        let (value, known) = library_source(Path::new("relative/library"));
        assert_eq!((value.as_str(), known), ("unknown", false));
    }
}
