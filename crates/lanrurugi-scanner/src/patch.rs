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
    #[error("patch targetCrc32 {expected} does not match target archive's real crc32 {actual} — refusing to apply a patch that may belong to a different file")]
    Crc32Mismatch { expected: String, actual: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
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

/// `metadata.json`'s own shape, inside a `.patch.zip`. `target_crc32` is checked against the real
/// target archive's own crc32 before a patch is ever applied (issue #77: "补丁json里面最好也包括
/// 补丁目标的crc32值...避免打错目标") — a patch is only ever matched to its target by filename
/// convention (same directory, same stem), which breaks silently if the target is later renamed
/// out from under a stale patch or a differently-content file happens to reuse the name; the crc32
/// check turns that into a loud, refused-to-apply error instead of silently splicing in pages that
/// don't belong.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchMetadata {
    pub target_crc32: String,
    /// CRC32 of the source archive whose pages were extracted for this patch — stored so a future
    /// re-download of the same source can be recognized as "already patched" and skip the filename-
    /// conflict menu entirely rather than re-offering the same compare-and-resolve flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_crc32: Option<String>,
    pub insertions: Vec<PatchInsertion>,
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

/// Loads and validates a patch zip's metadata against the real target archive's own crc32 —
/// [`Crc32Mismatch`](PatchError::Crc32Mismatch) if they don't match, which every caller below
/// treats as "there is effectively no valid patch here" (degrades to the unpatched page list)
/// rather than a hard error surfaced to the reader.
pub fn load(patch_path: &Path, target_archive_path: &Path) -> Result<PatchMetadata> {
    let metadata_entry = archive_format::find_entry_by_suffix(patch_path, METADATA_ENTRY_SUFFIX)?
        .ok_or(PatchError::MissingMetadata)?;
    let raw = archive_format::read_entry(patch_path, &metadata_entry)?;
    let metadata: PatchMetadata = serde_json::from_slice(&raw)?;

    let actual = crc32_of_file(target_archive_path)?;
    if !metadata.target_crc32.eq_ignore_ascii_case(&actual) {
        return Err(PatchError::Crc32Mismatch {
            expected: metadata.target_crc32,
            actual,
        });
    }
    Ok(metadata)
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
pub fn effective_pages(archive_path: &Path, original_pages: &[String]) -> Vec<EffectivePage> {
    let patch_path = patch_path_for(archive_path);
    if !patch_path.exists() {
        return original_pages
            .iter()
            .map(|name| EffectivePage::Original(name.clone()))
            .collect();
    }
    match load(&patch_path, archive_path) {
        Ok(metadata) => apply(original_pages, &metadata),
        Err(_) => original_pages
            .iter()
            .map(|name| EffectivePage::Original(name.clone()))
            .collect(),
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
pub fn build_merged_zip(archive_path: &Path) -> Result<Option<Vec<u8>>> {
    let patch_path = patch_path_for(archive_path);
    if !patch_path.exists() {
        return Ok(None);
    }
    let original_pages = archive_format::list_pages(archive_path)?;
    let effective = effective_pages(archive_path, &original_pages);
    // A patch that failed to load (missing metadata, crc32 mismatch, ...) degrades `effective`
    // back down to exactly `original_pages` — nothing left to actually merge, so there's no need
    // to repackage at all; the caller should serve the original file bytes as-is.
    if effective.len() == original_pages.len()
        && effective
            .iter()
            .all(|p| matches!(p, EffectivePage::Original(_)))
    {
        return Ok(None);
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buf);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
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
    writer
        .finish()
        .map_err(|e| PatchError::Io(std::io::Error::other(e.to_string())))?;
    Ok(Some(buf.into_inner()))
}

/// Builds a brand-new `.patch.zip`'s raw bytes from a target archive's crc32 plus the actual page
/// image bytes to bundle (`(entry_name, bytes)` pairs — the export endpoint reads these from
/// wherever the user's chosen pages actually live, e.g. `lanrurugi_imgcompare`'s staged comparison
/// files, before calling this) and the `insertions` describing where each belongs. Pure in-memory
/// assembly (no filesystem write) — the caller decides whether to write this to
/// [`patch_path_for`]'s own path directly or hand the bytes back to a user for manual placement
/// (issue #77's own confirmed design: "补丁不需要应用，只要放在漫画目录内" — a user-managed file,
/// not something this crate installs on the user's behalf).
pub fn build_patch_zip(
    target_crc32: &str,
    source_crc32: Option<&str>,
    pages: &[(String, Vec<u8>)],
    insertions: Vec<PatchInsertion>,
) -> Result<Vec<u8>> {
    let metadata = PatchMetadata {
        source_crc32: source_crc32.map(|s| s.to_string()),
        target_crc32: target_crc32.to_string(),
        insertions,
    };
    let metadata_json = serde_json::to_vec_pretty(&metadata)?;

    let mut buf = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buf);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
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

/// Streaming CRC32 of a whole file — same algorithm/output shape (lowercase hex) as
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

    #[test]
    fn load_rejects_a_crc32_that_does_not_match_the_real_target() {
        let target = write_zip(&[("page1.jpg", b"real page bytes")]);
        let patch_json = br#"{"targetCrc32":"deadbeef","insertions":[]}"#;
        let patch = write_zip(&[("metadata.json", patch_json)]);

        let err = load(patch.path(), target.path()).unwrap_err();
        assert!(matches!(err, PatchError::Crc32Mismatch { .. }), "{err:?}");
    }

    #[test]
    fn load_accepts_a_matching_crc32() {
        let target = write_zip(&[("page1.jpg", b"real page bytes")]);
        let real_crc32 = crc32_of_file(target.path()).unwrap();
        let patch_json = format!(r#"{{"targetCrc32":"{real_crc32}","insertions":[]}}"#);
        let patch = write_zip(&[("metadata.json", patch_json.as_bytes())]);

        let metadata = load(patch.path(), target.path()).unwrap();
        assert_eq!(metadata.target_crc32, real_crc32);
    }

    #[test]
    fn apply_inserts_after_the_given_anchor() {
        let original = vec![
            "a.jpg".to_string(),
            "b.jpg".to_string(),
            "c.jpg".to_string(),
        ];
        let metadata = PatchMetadata {
            source_crc32: None,
            target_crc32: String::new(),
            insertions: vec![PatchInsertion {
                after_filename: Some("a.jpg".to_string()),
                before_filename: None,
                files: vec!["patch1.jpg".to_string(), "patch2.jpg".to_string()],
            }],
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
            source_crc32: None,
            target_crc32: String::new(),
            insertions: vec![PatchInsertion {
                after_filename: None,
                before_filename: Some("b.jpg".to_string()),
                files: vec!["patch1.jpg".to_string()],
            }],
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
            source_crc32: None,
            target_crc32: String::new(),
            insertions: vec![PatchInsertion {
                after_filename: Some("a.jpg".to_string()),
                before_filename: Some("c.jpg".to_string()),
                files: vec!["patch1.jpg".to_string()],
            }],
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
            source_crc32: None,
            target_crc32: String::new(),
            insertions: vec![PatchInsertion {
                after_filename: None,
                before_filename: None,
                files: vec!["patch1.jpg".to_string()],
            }],
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["patch1.jpg", "a.jpg"]);
    }

    #[test]
    fn apply_skips_an_insertion_whose_anchor_does_not_exist() {
        let original = vec!["a.jpg".to_string()];
        let metadata = PatchMetadata {
            source_crc32: None,
            target_crc32: String::new(),
            insertions: vec![PatchInsertion {
                after_filename: Some("does-not-exist.jpg".to_string()),
                before_filename: None,
                files: vec!["patch1.jpg".to_string()],
            }],
        };
        let result = apply(&original, &metadata);
        let names: Vec<&str> = result.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["a.jpg"]);
    }

    #[test]
    fn effective_pages_returns_originals_unchanged_when_no_patch_file_exists() {
        let target = write_zip(&[("a.jpg", b"data")]);
        let original = vec!["a.jpg".to_string()];
        let result = effective_pages(target.path(), &original);
        assert_eq!(result, vec![EffectivePage::Original("a.jpg".to_string())]);
    }

    #[test]
    fn effective_pages_degrades_to_originals_on_crc32_mismatch() {
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
        let result = effective_pages(&target_path, &original);
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
        let real_crc32 = crc32_of_file(&target_path).unwrap();

        let bytes = build_patch_zip(
            &real_crc32,
            None,
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

        let metadata = load(&patch_path, &target_path).unwrap();
        assert_eq!(metadata.target_crc32, real_crc32);
        assert_eq!(metadata.insertions.len(), 1);

        let effective = apply(&["a.jpg".to_string()], &metadata);
        let names: Vec<&str> = effective.iter().map(|p| p.entry_name()).collect();
        assert_eq!(names, ["a.jpg", "extra1.jpg"]);

        let read_back = read_page(&target_path, &effective[1]).unwrap();
        assert_eq!(read_back, b"extra bytes");
    }
}
