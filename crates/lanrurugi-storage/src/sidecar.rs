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
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths;

/// The sidecar format this build writes. Nothing else has ever shipped, so there is no older
/// shape to read: a file claiming a higher version is left alone (a newer LANrurugi owns it).
pub const SIDECAR_VERSION: u32 = 1;

/// Formats `t` as an RFC 3339 timestamp in UTC with millisecond precision
/// (`2026-11-11T01:06:03.123Z`).
fn stamp(t: SystemTime) -> String {
    let at: chrono::DateTime<chrono::Utc> = t.into();
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

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
    /// The file's modification time, RFC 3339 UTC with milliseconds.
    pub mtime: String,
    /// When this sidecar was written, RFC 3339 UTC with milliseconds.
    pub written: String,
    /// The whole-file BLAKE3 (lowercase hex), as the record holds it. A rebuild uses it instead of
    /// reading the file again while the file still matches ([`Self::matches_file`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    /// BLAKE3 of the file's first and last 64 KiB ([`sample_of`]). Size and mtime alone do not
    /// prove the bytes are the same: a restore with `rsync -a`/`cp -p` keeps the mtime, and some
    /// mounts store it to the second only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// The record's `primary_source`: where the archive was downloaded from, or what its metadata
    /// plugin vouched for. Nothing in the file itself can give it back after Redis is lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_source: Option<String>,
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
            .map(stamp)
            .unwrap_or_else(|_| stamp(SystemTime::UNIX_EPOCH));
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
            written: stamp(SystemTime::now()),
            blake3: None,
            sample: sample_of(path).ok(),
            primary_source: None,
        })
    }

    /// Whether the file at `path` still has the size, modification time and sampled bytes this
    /// sidecar recorded — the condition for trusting its `blake3` without reading the whole file.
    /// A sidecar without a sample never matches.
    pub fn matches_file(&self, path: &Path) -> bool {
        let Ok(meta) = std::fs::metadata(path) else {
            return false;
        };
        let Some(sample) = &self.sample else {
            return false;
        };
        meta.len() == self.size
            && meta.modified().map(stamp).ok().as_deref() == Some(&*self.mtime)
            && sample_of(path).ok().as_ref() == Some(sample)
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// The bytes [`Sidecar::sample`] covers from each end of the file.
const SAMPLE_BYTES: u64 = 64 * 1024;

/// BLAKE3 (lowercase hex) of `path`'s first and last [`SAMPLE_BYTES`] (the whole file when it is
/// shorter than both together).
pub fn sample_of(path: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    let mut buf = Vec::new();
    (&mut file).take(SAMPLE_BYTES).read_to_end(&mut buf)?;
    hasher.update(&buf);
    let tail_start = len.saturating_sub(SAMPLE_BYTES).max(SAMPLE_BYTES.min(len));
    if tail_start < len {
        buf.clear();
        file.seek(SeekFrom::Start(tail_start))?;
        file.read_to_end(&mut buf)?;
        hasher.update(&buf);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// The lock every write of `id`'s sidecar holds: a read-modify-write ([`amend`], [`rewrite`]) and
/// a whole write ([`write`]) that interleave would otherwise drop each other's fields.
async fn lock(id: &str) -> impl Send + 'static {
    crate::record_lock::lock(&format!("sidecar:{id}")).await
}

/// Builds `id`'s sidecar from its current one (`None` when there is none) and writes the result,
/// under the sidecar's lock.
pub async fn rewrite(
    library: &Path,
    id: &str,
    build: impl FnOnce(Option<Sidecar>) -> std::io::Result<Sidecar>,
) -> std::io::Result<()> {
    let _guard = lock(id).await;
    let sidecar = build(read(library, id))?;
    write_unlocked(library, &sidecar)
}

/// Changes fields of an existing sidecar (a value its record gained after cataloguing), under the
/// sidecar's own lock so two such changes cannot drop each other. `Ok(false)` when there is no
/// sidecar to change (a by-name archive, or one not written yet).
pub async fn amend(
    library: &Path,
    id: &str,
    change: impl FnOnce(&mut Sidecar),
) -> std::io::Result<bool> {
    let _guard = lock(id).await;
    let Some(mut sidecar) = read(library, id) else {
        return Ok(false);
    };
    let before = sidecar.clone();
    change(&mut sidecar);
    if sidecar == before {
        return Ok(true);
    }
    sidecar.written = stamp(SystemTime::now());
    write_unlocked(library, &sidecar)?;
    Ok(true)
}

/// Writes (or replaces) the sidecar for `id` next to its archive. Atomic: a `.tmp` file is renamed
/// over the destination, so a crash leaves either the old snapshot or the new one — never half of
/// one, which the rebuild path would otherwise have to defend against.
pub async fn write(library: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    let _guard = lock(&sidecar.id).await;
    write_unlocked(library, sidecar)
}

fn write_unlocked(library: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    let dest = paths::sidecar_path(library, &sidecar.id);
    // Retried once on `NotFound`: a flat-library conversion prunes a bucket directory the moment
    // its last archive leaves, which can happen between `create_dir_all` below and the temp file
    // being created. Recreating the directory is the whole recovery — the bytes were not the
    // problem (issue seen four times on 2026-11-10 during a relocation pass).
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match write_inner(&dest, sidecar) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_inner(&dest, sidecar)
        }
        other => other,
    }
}

fn write_inner(dest: &Path, sidecar: &Sidecar) -> std::io::Result<()> {
    let tmp = dest.with_extension("json.tmp");
    // Always stamp the version we actually emit: a sidecar read back from an older file would
    // otherwise be rewritten with the current field shapes while still claiming to be version 1.
    let current = Sidecar {
        version: SIDECAR_VERSION,
        ..sidecar.clone()
    };
    let bytes = serde_json::to_vec_pretty(&current)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Removes the sidecar for `id` (a no-op when there is none). Callers use this when the archive
/// leaves the library — a permanent delete or a move into the recycle bin — so the snapshot does
/// not outlive the bytes it describes.
pub fn remove(library: &Path, id: &str) -> std::io::Result<()> {
    match std::fs::remove_file(paths::sidecar_path(library, id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Reads and parses the sidecar without insisting that the `id` inside matches `id`. The rebuild
/// audit uses this to tell "a sidecar for a different archive is sitting here" apart from "there is
/// no readable sidecar at all"; nothing else should.
pub fn read_raw(library: &Path, id: &str) -> Option<Sidecar> {
    let raw = std::fs::read(paths::sidecar_path(library, id)).ok()?;
    let parsed: Sidecar = serde_json::from_slice(&raw).ok()?;
    (parsed.version <= SIDECAR_VERSION).then_some(parsed)
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

    #[tokio::test]
    async fn a_write_recreates_a_bucket_directory_that_a_conversion_just_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let id = "a".repeat(40);
        let archive = library.join(format!("{id}.zip"));
        std::fs::write(
            &archive,
            b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        )
        .unwrap();
        let sidecar = Sidecar::for_archive(&id, &archive, "x.zip", "X").expect("build");
        let bucket = paths::sidecar_path(library, &id)
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(&bucket).unwrap();
        // The prune that races this write removes the directory it was about to write into.
        std::fs::remove_dir(&bucket).unwrap();
        write(library, &sidecar)
            .await
            .expect("write recreates the directory");
        assert!(paths::sidecar_path(library, &id).is_file());
    }

    /// The fingerprint and primary source added after cataloguing reach the sidecar, a sidecar from
    /// before these fields still reads, and the fingerprint is trusted only for an unchanged file.
    #[tokio::test]
    async fn amend_adds_the_fingerprint_and_it_is_trusted_only_for_an_unchanged_file() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let id = "b".repeat(40);
        let archive = library.join(format!("{id}.zip"));
        std::fs::write(&archive, b"first bytes").unwrap();
        let sidecar = Sidecar::for_archive(&id, &archive, "x.zip", "X").unwrap();
        write(library, &sidecar).await.unwrap();
        // Written before the fields existed: no `blake3`/`primary_source` on disk at all.
        let raw = std::fs::read_to_string(paths::sidecar_path(library, &id)).unwrap();
        assert!(!raw.contains("blake3") && !raw.contains("primary_source"));
        assert_eq!(read(library, &id).unwrap().blake3, None);

        assert!(amend(library, &id, |s| s.blake3 = Some("ab".repeat(32)))
            .await
            .unwrap());
        assert!(amend(library, &id, |s| s.primary_source =
            Some("example.org/a".into()))
        .await
        .unwrap());
        let read_back = read(library, &id).unwrap();
        assert_eq!(read_back.blake3, Some("ab".repeat(32)));
        assert_eq!(read_back.primary_source.as_deref(), Some("example.org/a"));
        assert!(read_back.matches_file(&archive));

        // Same size, mtime put back (what `rsync -a` from a backup does), different bytes: only
        // the sample tells.
        let mtime = std::fs::metadata(&archive).unwrap().modified().unwrap();
        std::fs::write(&archive, b"FIRST BYTES").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&archive)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert!(!read_back.matches_file(&archive));

        // Different bytes (and size): the recorded fingerprint no longer describes the file.
        std::fs::write(&archive, b"other bytes, longer").unwrap();
        assert!(!read_back.matches_file(&archive));
        // No sidecar to amend.
        assert!(!amend(library, &"c".repeat(40), |s| s.blake3 = None)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_sidecar_round_trips_and_lands_next_to_its_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path();
        let id = "5fff1a46220b45cf8db24c1721062b7e20fdb8dc";
        let archive = paths::hash_layout_path(library, id, "zip");
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(&archive, b"bytes").unwrap();

        let sidecar = Sidecar::for_archive(id, &archive, "[Circle] Book.zip", "Book").unwrap();
        write(library, &sidecar).await.unwrap();

        assert!(paths::sidecar_path(library, id).is_file());
        let read_back = read(library, id).unwrap();
        assert_eq!(read_back.original_filename, "[Circle] Book.zip");
        assert_eq!(read_back.title, "Book");
        assert_eq!(read_back.size, 5);
        // Timestamps are RFC 3339 UTC with milliseconds, not epoch seconds.
        assert!(read_back.written.ends_with('Z'), "{}", read_back.written);
        assert!(read_back.written.contains('.'), "{}", read_back.written);
        assert!(chrono::DateTime::parse_from_rfc3339(&read_back.written).is_ok());
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths::sidecar_path(library, id)).unwrap())
                .unwrap();
        assert!(raw["mtime"].is_string(), "{raw}");
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
