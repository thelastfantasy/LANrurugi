//! Page patches (issue #77's own follow-on design) — a `<archive stem>.patch.zip` sitting next to
//! its target archive on disk, holding extra pages (typically pulled from a *different* version of
//! the same work during an AI quality comparison — see `lanrurugi_imgcompare`) that the reader
//! should splice into the target's own page list at read time, without ever modifying the target
//! archive file itself.
//!
//! Deliberately NOT merged at the `archive_format::list_pages`/`read_entry` level — those are the
//! shared primitives every consumer (OPDS, thumbnail generation, `pagecount` accounting, the scan
//! health-check) reads from, and none of those need or want patch-awareness (confirmed design:
//! only the reader-facing endpoints — page list, single page, the overview thumbnail grid, and the
//! whole-archive download — merge patches in; everything else keeps seeing the archive exactly as
//! it is on disk). Callers that do want the merged view use [`effective_pages`]/[`read_page`]
//! below instead of calling `archive_format` directly.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::archive_format::{self, ArchiveFormatError};

#[derive(Debug, Error)]
pub enum PatchError {
    #[error(transparent)]
    Archive(#[from] ArchiveFormatError),
    #[error("patch has no metadata.json")]
    MissingMetadata,
    #[error("invalid patch metadata.json: {0}")]
    InvalidMetadata(#[from] serde_json::Error),
    #[error("patch target {expected} does not match the target archive's {actual} — refusing to apply a patch that may belong to a different file")]
    TargetMismatch { expected: String, actual: String },
    #[error("patch metadata names no target archive")]
    NoTarget,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl PatchError {
    /// Whether the patch stays unusable however often it is read again: its container or metadata is
    /// broken, or it belongs to other bytes. A plain I/O failure says nothing about the file.
    pub fn is_permanent(&self) -> bool {
        match self {
            PatchError::Archive(e) => e.is_permanent(),
            PatchError::MissingMetadata
            | PatchError::InvalidMetadata(_)
            | PatchError::TargetMismatch { .. }
            | PatchError::NoTarget => true,
            PatchError::Io(_) => false,
        }
    }
}

type Result<T> = std::result::Result<T, PatchError>;

/// One group of pages to splice in at a single anchor point — `files` are inserted, in array
/// order, immediately after `afterFilename` (or before `beforeFilename` when `afterFilename` is
/// absent; both absent means "at the very start"). When both are present, `afterFilename` wins and
/// `beforeFilename` is ignored entirely (confirmed design — not validated as a consistency pair,
/// simply a documented precedence rule).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchInsertion {
    #[serde(default)]
    pub after_filename: Option<String>,
    #[serde(default)]
    pub before_filename: Option<String>,
    pub files: Vec<String>,
}

/// `metadata.json`'s own shape, inside a `.patch.zip`. `target_blake3` is checked against the real
/// target archive before a patch is ever applied (issue #77: "补丁json里面最好也包括补丁目标的crc32值
/// ...避免打错目标") — a patch is only matched to its target by filename convention (same directory,
/// same stem), which breaks silently if the target is later renamed out from under a stale patch or
/// a differently-content file happens to reuse the name; the check turns that into a loud,
/// refused-to-apply error instead of silently splicing in pages that don't belong.
///
/// BLAKE3 rather than the CRC32 patches first carried: the library already records every archive's
/// BLAKE3, so checking costs a comparison instead of reading the whole archive on every page-list
/// request. The CRC32 fields are only read, to migrate such a patch once (see [`load`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_blake3: Option<String>,
    /// BLAKE3 of every source archive whose pages were merged into this patch (lowercase hex), so a
    /// re-download of the same source is recognised as "already patched".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources_blake3: Vec<String>,
    /// Legacy: the target's CRC32, in patches written before `target_blake3`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_crc32: String,
    /// Legacy: the first source's CRC32.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_crc32: Option<String>,
    /// Legacy: every source's CRC32, until the duplicates scan converts them to `sources_blake3`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    pub insertions: Vec<PatchInsertion>,
}

impl PatchMetadata {
    /// The legacy `sources` plus `source_crc32`, lowercased and deduplicated.
    pub fn legacy_sources(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .sources
            .iter()
            .chain(self.source_crc32.iter())
            .map(|s| s.to_ascii_lowercase())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// One page in an [`effective_pages`] result — either a plain passthrough to the target archive's
/// own entry, or a patch-sourced page that must be read via [`read_page`] (which knows to look
/// inside the patch zip instead) rather than `archive_format::read_entry` directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectivePage {
    Original(String),
    Patched(String),
}

impl EffectivePage {
    pub fn entry_name(&self) -> &str {
        match self {
            EffectivePage::Original(name) | EffectivePage::Patched(name) => name,
        }
    }
}

const PATCH_SUFFIX: &str = ".patch.zip";

/// The sidecar patch path for a given archive path — `<dir>/<stem>.patch.zip`, e.g.
/// `foo.zip` → `foo.patch.zip`, `foo.cbz` → `foo.patch.zip`. Doesn't check the path actually
/// exists; callers combine this with [`load`] (which does the existence/read check) or a plain
/// `Path::exists`.
pub fn patch_path_for(archive_path: &Path) -> PathBuf {
    let stem = archive_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    archive_path.with_file_name(format!("{stem}{PATCH_SUFFIX}"))
}

/// True for a path that is itself a patch file (`*.patch.zip`) — used by
/// [`crate::watcher::is_watched_archive_path`] to exclude a patch from being catalogued as its own
/// independent archive (it shares the `zip` extension its target uses, so without this check it
/// would otherwise pass the plain extension-allowlist check).
pub fn is_patch_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(PATCH_SUFFIX))
        .unwrap_or(false)
}

const METADATA_ENTRY_SUFFIX: &str = "metadata.json";

/// Loads and validates a patch zip's metadata against its target archive —
/// [`TargetMismatch`](PatchError::TargetMismatch) if it belongs to other bytes, which every caller
/// below treats as "there is effectively no valid patch here" (degrades to the unpatched page list)
/// rather than a hard error surfaced to the reader. `target_blake3` is the target's BLAKE3 when the
/// caller has its record (see [`target_blake3`]).
///
/// A patch from before `targetBlake3` is checked by its CRC32 once — a full read of the target —
/// and then rewritten with the BLAKE3, so that read never repeats.
pub fn load(
    patch_path: &Path,
    target_archive_path: &Path,
    target_blake3: Option<&str>,
) -> Result<PatchMetadata> {
    let metadata = read_metadata(patch_path)?;
    if let Some(expected) = &metadata.target_blake3 {
        let actual = self::target_blake3(target_archive_path, target_blake3)?;
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(PatchError::TargetMismatch {
                expected: expected.clone(),
                actual,
            });
        }
        return Ok(metadata);
    }
    if metadata.target_crc32.is_empty() {
        return Err(PatchError::NoTarget);
    }
    let actual = crc32_of_file(target_archive_path)?;
    if !metadata.target_crc32.eq_ignore_ascii_case(&actual) {
        return Err(PatchError::TargetMismatch {
            expected: metadata.target_crc32,
            actual,
        });
    }
    let mut upgraded = metadata;
    upgraded.target_blake3 = Some(self::target_blake3(target_archive_path, target_blake3)?);
    upgraded.target_crc32.clear();
    if let Err(e) = rewrite_metadata(patch_path, &upgraded) {
        tracing::warn!(patch = %patch_path.display(), error = %e, "could not record the target's BLAKE3 in a CRC32 patch; it will be checked by CRC32 again");
    }
    Ok(upgraded)
}

fn read_metadata(patch_path: &Path) -> Result<PatchMetadata> {
    let metadata_entry = archive_format::find_entry_by_suffix(patch_path, METADATA_ENTRY_SUFFIX)?
        .ok_or(PatchError::MissingMetadata)?;
    let raw = archive_format::read_entry(patch_path, &metadata_entry)?;
    Ok(serde_json::from_slice(&raw)?)
}

/// Replaces a patch's `metadata.json`, keeping its pages. Atomic, like every patch write.
pub fn rewrite_metadata(patch_path: &Path, metadata: &PatchMetadata) -> Result<()> {
    use std::io::Read;
    let to_io = |e: zip::result::ZipError| PatchError::Io(std::io::Error::other(e.to_string()));
    let mut zip = zip::ZipArchive::new(std::fs::File::open(patch_path)?).map_err(to_io)?;
    let mut pages = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(to_io)?;
        let name = entry.name().map_err(to_io)?.to_string();
        if entry.is_dir() || name.ends_with(METADATA_ENTRY_SUFFIX) {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        pages.push((name, bytes));
    }
    let bytes = build_patch_zip_from(metadata, &pages)?;
    let tmp = patch_path.with_extension("zip.tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, patch_path)?;
    Ok(())
}

type TargetKey = (PathBuf, u64, Option<std::time::SystemTime>);

/// BLAKE3s already known per state of a file, so a target the caller had no record for is hashed
/// at most once per process.
static TARGET_HASHES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<TargetKey, String>>,
> = std::sync::LazyLock::new(Default::default);

/// The target archive's BLAKE3 (lowercase hex): `known` (its record's) when the caller has it, else
/// one learned earlier for the same state of the file, else the sidecar's while it still matches
/// the file, else the file is hashed.
pub fn target_blake3(path: &Path, known: Option<&str>) -> std::io::Result<String> {
    let meta = std::fs::metadata(path)?;
    let key: TargetKey = (path.to_path_buf(), meta.len(), meta.modified().ok());
    let remember = |hash: &str| {
        if let Ok(mut map) = TARGET_HASHES.lock() {
            map.insert(key.clone(), hash.to_string());
        }
    };
    if let Some(known) = known {
        let hash = known.to_ascii_lowercase();
        remember(&hash);
        return Ok(hash);
    }
    if let Some(hash) = TARGET_HASHES
        .lock()
        .ok()
        .and_then(|map| map.get(&key).cloned())
    {
        return Ok(hash);
    }
    let hash = match sidecar_blake3(path) {
        Some(hash) => hash,
        None => lanrurugi_core::io::blake3_file(path)?.to_hex().to_string(),
    };
    remember(&hash);
    Ok(hash)
}

fn sidecar_blake3(path: &Path) -> Option<String> {
    let id = path.file_stem()?.to_str()?;
    // Only a hash-layout archive (named by its id) has a sidecar; a by-name one is named by title.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let library = path.parent()?.parent()?;
    let sidecar = lanrurugi_storage::sidecar::read(library, id)?;
    let hash = sidecar.blake3.clone()?;
    sidecar.matches_file(path).then_some(hash)
}

/// Splices `metadata`'s insertions into `original_pages` (the target archive's own
/// `archive_format::list_pages` result, already natural-sorted) to produce the reader-facing page
/// list. `afterFilename`/`beforeFilename` anchors that don't match any entry in `original_pages`
/// are silently skipped (that insertion's pages are dropped) rather than erroring or falling back
/// to appending at the end — a stale/malformed anchor shouldn't corrupt the position of every
/// *other*, valid insertion in the same patch, and a page landing in a silently-wrong position
/// would be worse than it simply not appearing.
pub fn apply(original_pages: &[String], metadata: &PatchMetadata) -> Vec<EffectivePage> {
    let mut result: Vec<EffectivePage> = original_pages
        .iter()
        .map(|name| EffectivePage::Original(name.clone()))
        .collect();

    for insertion in &metadata.insertions {
        let patched: Vec<EffectivePage> = insertion
            .files
            .iter()
            .map(|f| EffectivePage::Patched(f.clone()))
            .collect();
        if patched.is_empty() {
            continue;
        }

        let insert_at = if let Some(after) = &insertion.after_filename {
            match result
                .iter()
                .position(|p| matches!(p, EffectivePage::Original(n) if n == after))
            {
                Some(idx) => idx + 1,
                None => continue,
            }
        } else if let Some(before) = &insertion.before_filename {
            match result
                .iter()
                .position(|p| matches!(p, EffectivePage::Original(n) if n == before))
            {
                Some(idx) => idx,
                None => continue,
            }
        } else {
            0
        };

        result.splice(insert_at..insert_at, patched);
    }

    result
}

/// Convenience wrapper over [`load`] + [`apply`] for the common case (a reader-facing endpoint
/// just wants the effective page list, doesn't care about the metadata itself). Returns
/// `original_pages` unchanged, as plain [`EffectivePage::Original`]s, whenever there's no patch
/// file, the patch fails to load, or its crc32 doesn't match — a missing/invalid/stale patch must
/// never break ordinary reading of the archive it targets.
pub fn effective_pages(
    archive_path: &Path,
    original_pages: &[String],
    target_blake3: Option<&str>,
) -> Vec<EffectivePage> {
    effective_pages_checked(archive_path, original_pages, target_blake3).0
}

/// [`effective_pages`] for a caller that also needs to know the patch was tried and failed — a
/// patch nobody can read is something the reader must be told about, and the silent degradation
/// [`effective_pages`] performs leaves nothing to report from.
pub fn effective_pages_checked(
    archive_path: &Path,
    original_pages: &[String],
    target_blake3: Option<&str>,
) -> (Vec<EffectivePage>, Option<PatchError>) {
    let originals = || {
        original_pages
            .iter()
            .map(|name| EffectivePage::Original(name.clone()))
            .collect::<Vec<_>>()
    };
    let patch_path = patch_path_for(archive_path);
    if !patch_path.exists() {
        return (originals(), None);
    }
    match load(&patch_path, archive_path, target_blake3) {
        Ok(metadata) => (apply(original_pages, &metadata), None),
        Err(error) => (originals(), Some(error)),
    }
}

/// Reads one [`EffectivePage`]'s raw bytes — an `Original` page reads from `archive_path` itself
/// (plain `archive_format::read_entry`), a `Patched` page reads the same-named entry from the
/// sidecar patch zip instead.
pub fn read_page(archive_path: &Path, page: &EffectivePage) -> Result<Vec<u8>> {
    match page {
        EffectivePage::Original(name) => Ok(archive_format::read_entry(archive_path, name)?),
        EffectivePage::Patched(name) => {
            let patch_path = patch_path_for(archive_path);
            Ok(archive_format::read_entry(&patch_path, name)?)
        }
    }
}

/// Builds a fresh, self-contained zip combining `archive_path`'s own pages with any patched-in
/// ones from its sidecar `.patch.zip`, in effective (merged) reading order — what
/// `GET /archives/{id}/download` serves when a patch exists, so the downloaded file matches what
/// the reader itself shows rather than silently missing the patched pages (issue #77's own
/// follow-on design: "档案下载功能...要兼顾一下"). Returns `None` when `archive_path` has no
/// patch at all — the caller serves the original file bytes directly instead in that case, same
/// as before this feature existed (no repackaging cost for the common, unpatched case).
///
/// Entries are written in effective order under sequential, natural-sort-stable names
/// (`page_0001.<ext>`, `page_0002.<ext>`, ...) rather than each page's own original entry name —
/// original and patched pages can otherwise collide on name (they come from two different zips
/// that don't know about each other's contents), and this also guarantees the merged zip's own
/// natural sort order matches the effective order even though the two source archives' own
/// internal naming schemes may differ.
pub fn build_merged_zip(
    archive_path: &Path,
    target_blake3: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    Ok(write_merged_zip(archive_path, target_blake3, &mut buf)?.then(|| buf.into_inner()))
}

/// [`build_merged_zip`] written to `out` a page at a time, so memory stays at one page whatever
/// the archive's size. `false` (nothing written) when there is nothing to merge.
pub fn write_merged_zip<W: std::io::Write + std::io::Seek>(
    archive_path: &Path,
    target_blake3: Option<&str>,
    out: W,
) -> Result<bool> {
    let patch_path = patch_path_for(archive_path);
    if !patch_path.exists() {
        return Ok(false);
    }
    let original_pages = archive_format::list_pages(archive_path)?;
    let effective = effective_pages(archive_path, &original_pages, target_blake3);
    // A patch that failed to load (missing metadata, target mismatch, ...) degrades `effective`
    // back down to exactly `original_pages` — nothing left to actually merge, so there's no need
    // to repackage at all; the caller should serve the original file bytes as-is.
    if effective.len() == original_pages.len()
        && effective
            .iter()
            .all(|p| matches!(p, EffectivePage::Original(_)))
    {
        return Ok(false);
    }

    let mut writer = zip::ZipWriter::new(out);
    // Stored, not deflated: the pages being copied in are already-compressed images, so deflate
    // would spend CPU recompressing data with no entropy left to remove. The merged archive is
    // served/downloaded as-is, so the smaller output also isn't worth the CPU here.
    let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    for (i, page) in effective.iter().enumerate() {
        let bytes = read_page(archive_path, page)?;
        let ext = Path::new(page.entry_name())
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg");
        writer
            .start_file::<_, ()>(format!("page_{:04}.{ext}", i + 1), options)
            .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
        std::io::Write::write_all(&mut writer, &bytes)?;
    }
    let mut out = writer
        .finish()
        .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
    // A buffered `out` would otherwise flush on drop and swallow a full disk.
    out.flush()?;
    Ok(true)
}

/// Builds a brand-new `.patch.zip`'s raw bytes from a target archive's BLAKE3 plus the actual page
/// image bytes to bundle (`(entry_name, bytes)` pairs) and the `insertions` describing where each
/// belongs. Pure in-memory assembly (no filesystem write) — the caller decides whether to write this
/// to [`patch_path_for`]'s own path directly or hand the bytes back to a user for manual placement
/// (issue #77's own confirmed design: "补丁不需要应用，只要放在漫画目录内" — a user-managed file,
/// not something this crate installs on the user's behalf).
pub fn build_patch_zip(
    target_blake3: &str,
    sources_blake3: Vec<String>,
    pages: &[(String, Vec<u8>)],
    insertions: Vec<PatchInsertion>,
) -> Result<Vec<u8>> {
    let metadata = PatchMetadata {
        target_blake3: Some(target_blake3.to_string()),
        sources_blake3,
        insertions,
        ..Default::default()
    };
    build_patch_zip_from(&metadata, pages)
}

/// [`build_patch_zip`] for a caller that assembled the whole [`PatchMetadata`] itself (a merge
/// keeps `sources` from the existing patch).
pub fn build_patch_zip_from(
    metadata: &PatchMetadata,
    pages: &[(String, Vec<u8>)],
) -> Result<Vec<u8>> {
    let metadata_json = serde_json::to_vec_pretty(metadata)?;

    let mut buf = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buf);
    let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
    writer
        .start_file::<_, ()>(METADATA_ENTRY_SUFFIX, options)
        .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
    std::io::Write::write_all(&mut writer, &metadata_json)?;
    for (name, bytes) in pages {
        writer
            .start_file::<_, ()>(name.as_str(), options)
            .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
        std::io::Write::write_all(&mut writer, bytes)?;
    }
    writer
        .finish()
        .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
    Ok(buf.into_inner())
}

/// Streaming CRC32 of a whole file, for checking and migrating patches that predate
/// `targetBlake3` — same algorithm/output shape (lowercase hex) as
/// `lanrurugi-api`'s own `hex_crc32_of_file` (download-staging's content-derived temp filename),
/// duplicated here rather than shared because that one lives in a crate `lanrurugi-scanner` sits
/// *below* in the dependency graph (`lanrurugi-api` depends on `lanrurugi-scanner`, not the other
/// way around) — this crate needs its own copy to check a patch's `targetCrc32` against the real
/// target archive without introducing a dependency cycle.
pub fn crc32_of_file(path: &Path) -> std::result::Result<String, std::io::Error> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:08x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn only_a_failure_that_rereading_cannot_fix_is_permanent() {
        assert!(!PatchError::Io(std::io::Error::other("busy")).is_permanent());
        assert!(PatchError::MissingMetadata.is_permanent());
        assert!(PatchError::TargetMismatch {
            expected: "a".into(),
            actual: "b".into()
        }
        .is_permanent());
        assert!(
            PatchError::Archive(ArchiveFormatError::Libarchive("truncated".into())).is_permanent()
        );
        assert!(
            !PatchError::Archive(ArchiveFormatError::Io(std::io::Error::other("busy")))
                .is_permanent()
        );
    }

    fn write_zip(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::with_suffix(".zip").unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, data) in entries {
            writer
                .start_file::<_, ()>(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[test]
    fn patch_path_for_swaps_extension_for_patch_zip() {
        assert_eq!(
            patch_path_for(Path::new("/lib/foo.cbz")),
            Path::new("/lib/foo.patch.zip")
        );
        assert_eq!(
            patch_path_for(Path::new("/lib/foo.zip")),
            Path::new("/lib/foo.patch.zip")
        );
    }

    #[test]
    fn is_patch_path_matches_only_the_patch_suffix() {
        assert!(is_patch_path(Path::new("/lib/foo.patch.zip")));
        assert!(!is_patch_path(Path::new("/lib/foo.zip")));
        assert!(!is_patch_path(Path::new("/lib/foo.patch.rar")));
    }

    fn blake3_of(path: &Path) -> String {
        lanrurugi_core::io::blake3_file(path)
            .unwrap()
            .to_hex()
            .to_string()
    }

    #[test]
    fn load_rejects_a_target_that_does_not_match() {
        let target = write_zip(&[("page1.jpg", b"real page bytes")]);
        let patch_json = format!(r#"{{"targetBlake3":"{}","insertions":[]}}"#, "0".repeat(64));
        let patch = write_zip(&[("metadata.json", patch_json.as_bytes())]);

        let err = load(patch.path(), target.path(), None).unwrap_err();
        assert!(matches!(err, PatchError::TargetMismatch { .. }), "{err:?}");
        // The caller's record says the same thing without reading the file.
        let err = load(patch.path(), target.path(), Some(&"1".repeat(64))).unwrap_err();
        assert!(matches!(err, PatchError::TargetMismatch { .. }), "{err:?}");
    }

    #[test]
    fn load_accepts_a_matching_blake3() {
        let target = write_zip(&[("page1.jpg", b"real page bytes")]);
        let real = blake3_of(target.path());
        let patch_json = format!(r#"{{"targetBlake3":"{real}","insertions":[]}}"#);
        let patch = write_zip(&[("metadata.json", patch_json.as_bytes())]);

        assert_eq!(
            load(patch.path(), target.path(), None)
                .unwrap()
                .target_blake3,
            Some(real.clone())
        );
        assert!(load(patch.path(), target.path(), Some(&real.to_uppercase())).is_ok());
    }

    #[test]
    fn a_by_name_target_without_a_record_is_hashed() {
        // A by-name archive is named by its title, which is no id: there is no sidecar to read.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("_byname").join("無垢な少女 [DL版].zip");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, zip_bytes(&[("a.jpg", b"data")])).unwrap();
        let real = blake3_of(&target);
        let patch_json = format!(r#"{{"targetBlake3":"{real}","insertions":[]}}"#);
        let patch = write_zip(&[("metadata.json", patch_json.as_bytes())]);

        assert!(load(patch.path(), &target, None).is_ok());
    }

    #[test]
    fn a_crc32_patch_is_checked_once_and_then_carries_the_blake3() {
        let target = write_zip(&[("page1.jpg", b"real page bytes")]);
        let crc = crc32_of_file(target.path()).unwrap();
        let patch_json = format!(
            r#"{{"targetCrc32":"{crc}","sources":["aaaa0000"],"insertions":[{{"files":["p.jpg"]}}]}}"#
        );
        let patch = write_zip(&[
            ("metadata.json", patch_json.as_bytes()),
            ("p.jpg", b"patch page"),
        ]);

        let metadata = load(patch.path(), target.path(), None).unwrap();
        let real = blake3_of(target.path());
        assert_eq!(metadata.target_blake3.as_deref(), Some(real.as_str()));

        let stored = read_metadata(patch.path()).unwrap();
        assert_eq!(stored.target_blake3.as_deref(), Some(real.as_str()));
        assert!(stored.target_crc32.is_empty());
        // Sources wait for the duplicates scan, which knows the archives they were.
        assert_eq!(stored.legacy_sources(), ["aaaa0000"]);
        assert_eq!(
            archive_format::read_entry(patch.path(), "p.jpg").unwrap(),
            b"patch page"
        );

        let wrong = write_zip(&[(
            "metadata.json",
            br#"{"targetCrc32":"deadbeef","insertions":[]}"#,
        )]);
        assert!(matches!(
            load(wrong.path(), target.path(), None).unwrap_err(),
            PatchError::TargetMismatch { .. }
        ));
    }

    #[test]
    fn apply_inserts_after_the_given_anchor() {
        let original = vec![
            "a.jpg".to_string(),
            "b.jpg".to_string(),
            "c.jpg".to_string(),
        ];
        let metadata = PatchMetadata {
            insertions: vec![PatchInsertion {
                after_filename: Some("a.jpg".to_string()),
                before_filename: None,
                files: vec!["patch1.jpg".to_string(), "patch2.jpg".to_string()],
            }],
            ..Default::default()
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(
            names,
            ["a.jpg", "patch1.jpg", "patch2.jpg", "b.jpg", "c.jpg"]
        );
        assert!(matches!(result[1], EffectivePage::Patched(_)));
    }

    #[test]
    fn apply_inserts_before_the_given_anchor_when_after_is_absent() {
        let original = vec!["a.jpg".to_string(), "b.jpg".to_string()];
        let metadata = PatchMetadata {
            insertions: vec![PatchInsertion {
                after_filename: None,
                before_filename: Some("b.jpg".to_string()),
                files: vec!["patch1.jpg".to_string()],
            }],
            ..Default::default()
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["a.jpg", "patch1.jpg", "b.jpg"]);
    }

    #[test]
    fn apply_prefers_after_when_both_anchors_are_given() {
        let original = vec![
            "a.jpg".to_string(),
            "b.jpg".to_string(),
            "c.jpg".to_string(),
        ];
        let metadata = PatchMetadata {
            insertions: vec![PatchInsertion {
                after_filename: Some("a.jpg".to_string()),
                before_filename: Some("c.jpg".to_string()),
                files: vec!["patch1.jpg".to_string()],
            }],
            ..Default::default()
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        // If `before_filename` had won instead, patch1 would land at index 2 (right before c.jpg)
        // rather than index 1 (right after a.jpg) — this distinguishes the two.
        assert_eq!(names, ["a.jpg", "patch1.jpg", "b.jpg", "c.jpg"]);
    }

    #[test]
    fn apply_inserts_at_the_start_when_both_anchors_are_absent() {
        let original = vec!["a.jpg".to_string()];
        let metadata = PatchMetadata {
            insertions: vec![PatchInsertion {
                after_filename: None,
                before_filename: None,
                files: vec!["patch1.jpg".to_string()],
            }],
            ..Default::default()
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["patch1.jpg", "a.jpg"]);
    }

    #[test]
    fn apply_skips_an_insertion_whose_anchor_does_not_exist() {
        let original = vec!["a.jpg".to_string()];
        let metadata = PatchMetadata {
            insertions: vec![PatchInsertion {
                after_filename: Some("does-not-exist.jpg".to_string()),
                before_filename: None,
                files: vec!["patch1.jpg".to_string()],
            }],
            ..Default::default()
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["a.jpg"]);
    }

    #[test]
    fn effective_pages_returns_originals_unchanged_when_no_patch_file_exists() {
        let target = write_zip(&[("a.jpg", b"data")]);
        let original = vec!["a.jpg".to_string()];
        let result = effective_pages(target.path(), &original, None);
        assert_eq!(result, vec![EffectivePage::Original("a.jpg".to_string())]);
    }

    #[test]
    fn effective_pages_degrades_to_originals_on_a_target_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("foo.zip");
        std::fs::write(&target_path, zip_bytes(&[("a.jpg", b"data")])).unwrap();
        let patch_path = dir.path().join("foo.patch.zip");
        std::fs::write(
            &patch_path,
            zip_bytes(&[(
                "metadata.json",
                br#"{"targetCrc32":"deadbeef","insertions":[]}"#,
            )]),
        )
        .unwrap();

        let original = vec!["a.jpg".to_string()];
        let result = effective_pages(&target_path, &original, None);
        assert_eq!(result, vec![EffectivePage::Original("a.jpg".to_string())]);
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            writer
                .start_file::<_, ()>(*name, zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn read_page_reads_original_from_the_target_and_patched_from_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("foo.zip");
        std::fs::write(&target_path, zip_bytes(&[("a.jpg", b"original bytes")])).unwrap();
        let patch_path = dir.path().join("foo.patch.zip");
        std::fs::write(&patch_path, zip_bytes(&[("patch1.jpg", b"patched bytes")])).unwrap();

        let original =
            read_page(&target_path, &EffectivePage::Original("a.jpg".to_string())).unwrap();
        assert_eq!(original, b"original bytes");
        let patched = read_page(
            &target_path,
            &EffectivePage::Patched("patch1.jpg".to_string()),
        )
        .unwrap();
        assert_eq!(patched, b"patched bytes");
    }

    #[test]
    fn build_patch_zip_round_trips_through_load_and_read_page() {
        let dir = tempfile::tempdir().unwrap();
        let target_path = dir.path().join("foo.zip");
        std::fs::write(&target_path, zip_bytes(&[("a.jpg", b"original")])).unwrap();
        let real = blake3_of(&target_path);

        let bytes = build_patch_zip(
            &real,
            vec!["b".repeat(64)],
            &[("extra1.jpg".to_string(), b"extra bytes".to_vec())],
            vec![PatchInsertion {
                after_filename: Some("a.jpg".to_string()),
                before_filename: None,
                files: vec!["extra1.jpg".to_string()],
            }],
        )
        .unwrap();
        let patch_path = dir.path().join("foo.patch.zip");
        std::fs::write(&patch_path, &bytes).unwrap();

        let metadata = load(&patch_path, &target_path, None).unwrap();
        assert_eq!(metadata.target_blake3, Some(real));
        assert_eq!(metadata.sources_blake3, ["b".repeat(64)]);
        assert_eq!(metadata.insertions.len(), 1);

        let effective = apply(&["a.jpg".to_string()], &metadata);
        let names: Vec<&str> = effective.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["a.jpg", "extra1.jpg"]);

        let read_back = read_page(&target_path, &effective[1]).unwrap();
        assert_eq!(read_back, b"extra bytes");
    }
}
