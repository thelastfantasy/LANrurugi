//! Where an archive lives on disk under the hash-named layout (020).
//!
//! Two layout families coexist forever:
//!
//! - **hash layout** — `<library>/<first two id chars>/<id>.<ext>`, plus a sibling `<id>.json`
//!   sidecar. The name carries no meaning; identity is the content-derived id.
//! - **by-name layout** — anything under `<library>/_byname/`, keeping the filename and relative
//!   directory the user gave it (LANraragi's own historical layout). Catalogued by filename, no
//!   sidecar.
//!
//! Everything else at the library root is a candidate for relocation into `_byname/` (see
//! `lanrurugi_scanner::relocate`); these helpers are the single place that decides what "hash
//! layout" means, so the scanner, the ingest path and the rebuild path cannot disagree.

use std::path::{Path, PathBuf};

/// Directory keeping the by-name layout. A leading underscore sorts before the hex bucket
/// directories in Explorer/Finder (which sort punctuation first) and stays visible, unlike a dot
/// directory — the point is that a user browsing over SMB finds their own files first.
pub const BYNAME_DIR: &str = "_byname";

/// Bucket directory for an id: its first two hex characters. 256 buckets keeps the root readable
/// (100k archives ≈ 390 per bucket) and needs no fan-out below it; growing to 4,096+ buckets moves
/// the problem to the root, where every `ls`, backup walk and SMB listing pays for it.
pub fn bucket_name(id: &str) -> &str {
    if id.len() < 2 {
        "00"
    } else {
        // Ids are ASCII hex by construction, so this never splits a multi-byte character.
        &id[..2]
    }
}

/// `<library>/<bucket>/<id>.<ext>`; an empty extension yields `<id>`.
pub fn hash_layout_path(library: &Path, id: &str, extension: &str) -> PathBuf {
    let name = if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    };
    library.join(bucket_name(id)).join(name)
}

/// `<library>/<bucket>/<id>.json` — the sidecar sits in the same bucket as its archive, so moving
/// or deleting one is a two-file operation inside one directory.
pub fn sidecar_path(library: &Path, id: &str) -> PathBuf {
    library.join(bucket_name(id)).join(format!("{id}.json"))
}

/// `<library>/_byname`.
pub fn byname_dir(library: &Path) -> PathBuf {
    library.join(BYNAME_DIR)
}

/// Whether `path` is exactly where this id's own bytes belong in the hash layout. Only the
/// directory and the file stem are checked, not the extension: an archive re-downloaded under a
/// different extension keeps its id and stays put.
pub fn is_hash_layout_path(library: &Path, path: &Path, id: &str) -> bool {
    path.parent() == Some(library.join(bucket_name(id)).as_path())
        && path.file_stem().and_then(|s| s.to_str()) == Some(id)
}

/// Whether `path` sits anywhere under `_byname/`.
pub fn is_in_byname(library: &Path, path: &Path) -> bool {
    path.starts_with(byname_dir(library))
}

/// The path relative to `_byname/`, if `path` is under it — what a manual `mv` back to the library
/// root has to preserve. `None` for anything outside `_byname/`.
pub fn byname_relative_path<'a>(library: &Path, path: &'a Path) -> Option<&'a Path> {
    path.strip_prefix(byname_dir(library)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "5fff1a46220b45cf8db24c1721062b7e20fdb8dc";
    const LIB: &str = "/library";

    #[test]
    fn bucket_is_the_first_two_characters() {
        assert_eq!(bucket_name(ID), "5f");
        assert_eq!(bucket_name("ab"), "ab");
        assert_eq!(bucket_name("a"), "00");
        assert_eq!(bucket_name(""), "00");
    }

    #[test]
    fn hash_layout_path_keeps_the_extension() {
        assert_eq!(
            hash_layout_path(Path::new(LIB), ID, "zip"),
            PathBuf::from(format!("/library/5f/{ID}.zip"))
        );
        assert_eq!(
            hash_layout_path(Path::new(LIB), ID, ""),
            PathBuf::from(format!("/library/5f/{ID}"))
        );
    }

    #[test]
    fn sidecar_sits_next_to_its_archive() {
        assert_eq!(
            sidecar_path(Path::new(LIB), ID),
            PathBuf::from(format!("/library/5f/{ID}.json"))
        );
    }

    #[test]
    fn hash_layout_detection_ignores_the_extension_but_not_the_directory() {
        let library = Path::new(LIB);
        assert!(is_hash_layout_path(
            library,
            &hash_layout_path(library, ID, "zip"),
            ID
        ));
        // Same stem, wrong bucket.
        assert!(!is_hash_layout_path(
            library,
            &PathBuf::from(format!("/library/00/{ID}.zip")),
            ID
        ));
        // Right bucket, someone else's name.
        assert!(!is_hash_layout_path(
            library,
            &PathBuf::from("/library/5f/another-name.zip"),
            ID
        ));
    }

    #[test]
    fn byname_relative_path_round_trips() {
        let library = Path::new(LIB);
        let stored = library.join(BYNAME_DIR).join("Series/Book 01.zip");
        assert!(is_in_byname(library, &stored));
        assert_eq!(
            byname_relative_path(library, &stored),
            Some(Path::new("Series/Book 01.zip"))
        );
        assert_eq!(byname_relative_path(library, &library.join("a.zip")), None);
    }
}
