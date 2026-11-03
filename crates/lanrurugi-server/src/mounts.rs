//! Container start-up check: warns when a directory that must outlive the container (library,
//! thumbnails, the bundled Redis's data) is not on a mount of its own, so a forgotten volume shows
//! up in the first log lines instead of after a recreate.

use std::path::{Path, PathBuf};

/// Whether this process runs in a Docker or Podman container.
pub fn in_container() -> bool {
    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

/// One warning per directory in `dirs` (`(path, consequence, tag)`) that sits on the container's
/// own root filesystem; returns those directories' tags. Does nothing outside a container or
/// without `/proc/self/mountinfo`.
pub fn warn_unmounted<T: Copy>(dirs: &[(&Path, &str, T)]) -> Vec<T> {
    if !in_container() {
        return Vec::new();
    }
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let mut unmounted = Vec::new();
    for (dir, consequence, tag) in dirs {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !on_own_mount(&mountinfo, &dir) {
            tracing::warn!(
                dir = %dir.display(),
                "directory is not a mounted volume — {consequence}"
            );
            unmounted.push(*tag);
        }
    }
    unmounted
}

/// Whether `path` lies under a mount other than the root filesystem.
fn on_own_mount(mountinfo: &str, path: &Path) -> bool {
    covering_mount(mountinfo, path).is_some_and(|mount| mount != Path::new("/"))
}

/// The longest mount point in `mountinfo` (the `/proc/self/mountinfo` format) containing `path`.
fn covering_mount(mountinfo: &str, path: &Path) -> Option<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| line.split(' ').nth(4))
        .map(|field| PathBuf::from(unescape(field)))
        .filter(|mount| path.starts_with(mount))
        .max_by_key(|mount| mount.components().count())
}

/// Undoes mountinfo's octal escapes (`\040` for a space).
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let digits = std::str::from_utf8(&bytes[i + 1..i + 4]).ok();
            if let Some(value) = digits.and_then(|d| u8::from_str_radix(d, 8).ok()) {
                out.push(value);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
612 540 0:52 / / rw,relatime - overlay overlay rw,lowerdir=/a
613 612 0:55 / /proc rw,nosuid - proc proc rw
640 612 259:2 /home/u/data/manga /home/koyomi/lanrurugi/content rw,relatime - ext4 /dev/nvme0n1p2 rw
641 612 0:60 / /temp rw,relatime - ext4 /dev/vdb rw
642 612 259:2 /home/u/my\\040books /srv/my\\040books rw - ext4 /dev/nvme0n1p2 rw
";

    #[test]
    fn a_bind_mounted_directory_and_its_children_are_on_their_own_mount() {
        assert!(on_own_mount(
            MOUNTINFO,
            Path::new("/home/koyomi/lanrurugi/content")
        ));
        assert!(on_own_mount(
            MOUNTINFO,
            Path::new("/home/koyomi/lanrurugi/content/sub")
        ));
        assert!(on_own_mount(MOUNTINFO, Path::new("/temp/book")));
    }

    #[test]
    fn a_directory_on_the_container_root_is_not() {
        assert!(!on_own_mount(
            MOUNTINFO,
            Path::new("/home/koyomi/lanrurugi/thumb")
        ));
        // A sibling sharing a name prefix is not under the mount.
        assert!(!on_own_mount(
            MOUNTINFO,
            Path::new("/home/koyomi/lanrurugi/content2")
        ));
    }

    #[test]
    fn escaped_mount_points_are_decoded() {
        assert!(on_own_mount(MOUNTINFO, Path::new("/srv/my books")));
        assert_eq!(unescape("a\\040b\\134c"), "a b\\c");
        assert_eq!(unescape("trailing\\04"), "trailing\\04");
    }

    #[test]
    fn no_mountinfo_means_no_mount() {
        assert!(!on_own_mount("", Path::new("/data")));
    }
}
