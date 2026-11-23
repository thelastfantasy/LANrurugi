//! Where the library directory actually comes from, plus the "records that still have a file"
//! ratio — the two numbers an operator needs to tell "the volume is not mounted" from "the records
//! are wrong" (`specs/028-deletion-safety-net`, research.md D7).
//!
//! Deliberately diagnostic only: nothing here ever refuses an operation, and every failure mode is
//! reported as `unknown` rather than propagated. Container runtimes synthesize mount roots in ways
//! that differ wildly (Docker Desktop, gVisor, a plain native install where the root is always
//! `/`), so using this as a safety judgement would produce both false alarms and silent misses.

use std::path::{Component, Path, PathBuf};

/// What the library path looks like from the host's mount table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibrarySource {
    /// The mount's `root` field (`"/"` becomes `"root-filesystem"`), or `"unknown"`.
    pub value: String,
    /// `false` when the value says nothing useful: a plain root-filesystem path, a non-Linux
    /// platform, or a mount table that could not be read.
    pub known: bool,
}

impl LibrarySource {
    fn unknown() -> Self {
        Self {
            value: "unknown".to_string(),
            known: false,
        }
    }
}

/// Detects the mount backing `library`. Linux only; every other platform reports `unknown`.
pub fn detect(library: &Path) -> LibrarySource {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/self/mountinfo") {
            Ok(contents) => detect_in(&contents, library),
            Err(_) => LibrarySource::unknown(),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = library;
        LibrarySource::unknown()
    }
}

/// [`detect`] against mount table text supplied by the caller — the testable half.
pub fn detect_in(mountinfo: &str, library: &Path) -> LibrarySource {
    let Some(library) = normalize(library) else {
        return LibrarySource::unknown();
    };
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        // `<id> <parent> <major:minor> <root> <mount point> <options> [optional...] - <fstype>
        // <source> <super options>`; only root (3) and mount point (4) matter here.
        let mut fields = line.split_whitespace();
        let (Some(_id), Some(_parent), Some(_dev), Some(root), Some(mount_point)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let mount_point = decode_octal(mount_point);
        let Some(mount_path) = normalize(Path::new(&mount_point)) else {
            continue;
        };
        if !library.starts_with(&mount_path) {
            continue;
        }
        let depth = mount_path.components().count();
        // Longest mount point wins: `/data/manga` and `/data` both match, and only the deeper one
        // says where this particular directory comes from.
        if best
            .as_ref()
            .is_none_or(|(best_depth, _)| depth >= *best_depth)
        {
            best = Some((depth, decode_octal(root)));
        }
    }
    match best {
        None => LibrarySource::unknown(),
        Some((_, root)) if root == "/" => LibrarySource {
            value: "root-filesystem".to_string(),
            known: false,
        },
        Some((_, root)) => LibrarySource {
            value: root,
            known: true,
        },
    }
}

/// Percent of `records` whose `file` exists. Records with no path at all count as missing, so a
/// library full of unlinked records reads as 0% rather than as "nothing to compare".
pub fn overlap_percent<'a>(files: impl IntoIterator<Item = &'a str>) -> u8 {
    let mut total = 0usize;
    let mut present = 0usize;
    for file in files {
        total += 1;
        if !file.is_empty() && Path::new(file).is_file() {
            present += 1;
        }
    }
    if total == 0 {
        return 0;
    }
    u8::try_from(present * 100 / total).unwrap_or(100)
}

/// `/proc/self/mountinfo` escapes space, tab, newline and backslash as octal (`\040` etc.).
fn decode_octal(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let digits = &raw[i + 1..i + 4];
            if let Ok(code) = u8::from_str_radix(digits, 8) {
                out.push(code as char);
                i += 4;
                continue;
            }
        }
        // The input is valid UTF-8 here (it came from a `&str`), so a byte-wise copy is safe for
        // multi-byte characters one byte at a time only if we push a char; copy the char instead.
        let ch = raw[i..].chars().next().expect("i is a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Lexical normalization only (no filesystem access): drops `.` and empty components, resolves
/// `..` against the already-collected prefix. `None` for a relative path — a mount point never is.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Prefix(_) => return None,
        }
    }
    out.is_absolute().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real line shape, with the optional fields mountinfo allows before the `-`.
    const TABLE: &str = "\
25 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
30 25 8:2 /mnt/pool/manga /data rw,relatime shared:2 - zfs tank/manga rw
31 30 0:45 / /data/manga rw,relatime shared:3 - overlay overlay rw
";

    #[test]
    fn longest_matching_mount_wins() {
        // `/data/manga` (root `/`) is deeper than `/data` (root `/mnt/pool/manga`), so the deepest
        // entry decides — the overlay mounted over the pool directory is what is really there.
        let source = detect_in(TABLE, Path::new("/data/manga/books"));
        assert_eq!(
            source,
            LibrarySource {
                value: "root-filesystem".to_string(),
                known: false,
            }
        );

        // A path that only the pool mount covers reports the pool's root.
        let source = detect_in(TABLE, Path::new("/data/other"));
        assert_eq!(source.value, "/mnt/pool/manga");
        assert!(source.known);
    }

    #[test]
    fn the_deepest_mount_is_picked_when_roots_are_real_paths() {
        let table = "\
25 1 8:1 / / rw - ext4 /dev/sda1 rw
40 25 0:50 /dataset/manga /library rw - nfs nas:/export/manga rw
";
        let source = detect_in(table, Path::new("/library/a1"));
        assert_eq!(source.value, "/dataset/manga");
        assert!(source.known);
    }

    #[test]
    fn a_root_filesystem_path_is_flagged_rather_than_reported_as_a_source() {
        let source = detect_in(TABLE, Path::new("/home/jade/library"));
        assert_eq!(source.value, "root-filesystem");
        assert!(!source.known);
    }

    #[test]
    fn escaped_mount_points_and_roots_are_decoded() {
        let table = "25 1 8:1 /mnt/my\\040library /data\\040volume rw - ext4 /dev/sda1 rw\n";
        let source = detect_in(table, Path::new("/data volume/manga"));
        assert_eq!(source.value, "/mnt/my library");
        assert!(source.known);
    }

    #[test]
    fn no_mount_table_means_unknown() {
        assert_eq!(
            detect_in("", Path::new("/library")),
            LibrarySource::unknown()
        );
        // A relative library path can never match an absolute mount point.
        assert_eq!(
            detect_in(TABLE, Path::new("library")),
            LibrarySource::unknown()
        );
        // Malformed lines are skipped, not fatal.
        assert_eq!(
            detect_in("garbage\n\n", Path::new("/library")),
            LibrarySource::unknown()
        );
    }

    #[test]
    fn overlap_counts_missing_and_empty_paths_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let present = tmp.path().join("a.zip");
        std::fs::write(&present, b"x").unwrap();
        let present = present.to_string_lossy().to_string();
        let missing = tmp.path().join("gone.zip").to_string_lossy().to_string();

        assert_eq!(overlap_percent([present.as_str(), present.as_str()]), 100);
        assert_eq!(
            overlap_percent([present.as_str(), missing.as_str(), "", present.as_str()]),
            50
        );
        assert_eq!(overlap_percent([missing.as_str()]), 0);
        assert_eq!(overlap_percent([] as [&str; 0]), 0);
    }
}
