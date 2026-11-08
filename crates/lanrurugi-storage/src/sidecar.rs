//! Per-archive sidecar (`<library>/<ab>/<id>.json`), 020 FR-009.
//!
//! Redis stays the source of truth; the sidecar is a derived snapshot written next to the bytes so
//! a library whose Redis is lost can be rebuilt with its titles and original filenames intact.
//! Two rules keep it from ever *becoming* a second source of truth: it is written after the record
//! exists and after the bytes are at their final path, and the rebuild path only reads it for
//! archives the records do not know. Nothing ever reads it back into a live record.
//!
//! Files under `_byname/` have no sidecar: they are addressed by filename, so their name already
//! carries everything a sidecar would.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths;

/// The sidecar format this build writes. A file with a higher version is left alone (a newer
/// LANrurugi owns it); a lower one is upgraded on the next write.
pub const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    pub id: String,
    /// The filename the archive arrived under, verbatim (extension included). This is the only
    /// copy of the user's own name once the file is hash-named.
    #[serde(default)]
    pub original_filename: String,
    /// Relative path the archive was moved out of, when it came from the by-name layout; empty for
    /// anything a download/upload created directly under the hash layout.
    #[serde(default)]
    pub original_path: String,
    pub title: String,
    #[serde(default)]
    pub extension: String,
    pub size: u64,
    pub mtime: u64,
    /// Seconds since the Unix epoch.
    pub written: u64,
}

impl Sidecar {
    /// Builds the snapshot for an archive whose bytes are already at `path` (a hash-layout path).
    pub fn for_archive(
        id: &str,
        path: &Path,
        original_filename: &str,
        title: &str,
    ) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(Self {
            version: SIDECAR_VERSION,
            id: id.to_string(),
            original_filename: original_filename.to_string(),
            original_path: String::new(),
            title: title.to_string(),
            extension: path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_string(),
            size: meta.len(),
            mtime,
            written: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        })
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Writes (or replaces) the sidecar for `id` next to its archive. Atomic: a `.tmp` file is renamed
/// over the destination, so a crash leaves either the old snapshot or the new one — never half of
/// one, which the rebuild path would otherwise have to defend against.
pub fn write(library: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    let dest = paths::sidecar_path(library, &sidecar.id);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(sidecar)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(())
}

/// Reads the sidecar for `id`. Any failure (missing, unparsable, newer version, a different id
/// inside) is `None`: the caller treats it as "no snapshot", never as an error worth stopping a
/// rebuild for.
pub fn read(library: &Path, id: &str) -> Option<Sidecar> {
    let raw = std::fs::read(paths::sidecar_path(library, id)).ok()?;
    let parsed: Sidecar = serde_json::from_slice(&raw).ok()?;
    (parsed.version <= SIDECAR_VERSION && parsed.id == id).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_round_trips_and_lands_next_to_its_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let id = "5fff1a46220b45cf8db24c1721062b7e20fdb8dc";
        let archive = paths::hash_layout_path(library, id, "zip");
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(&archive, b"bytes").unwrap();

        let sidecar = Sidecar::for_archive(id, &archive, "[Circle] Book.zip", "Book").unwrap();
        write(library, &sidecar).unwrap();

        assert!(paths::sidecar_path(library, id).is_file());
        let read_back = read(library, id).unwrap();
        assert_eq!(read_back.original_filename, "[Circle] Book.zip");
        assert_eq!(read_back.title, "Book");
        assert_eq!(read_back.size, 5);
    }

    #[test]
    fn a_missing_or_foreign_sidecar_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read(tmp.path(), "00").is_none());

        let id = "0000000000000000000000000000000000000000";
        let dest = paths::sidecar_path(tmp.path(), id);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(
            &dest,
            r#"{"version":1,"id":"someone-else","title":"x","size":0,"mtime":0,"written":0}"#,
        )
        .unwrap();
        assert!(read(tmp.path(), id).is_none());
    }
}
