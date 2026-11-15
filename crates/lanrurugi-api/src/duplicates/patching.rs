//! Page patches from the duplicate review: copy the pages one member has and the kept member lacks
//! into the kept archive's sidecar `.patch.zip` (`lanrurugi_scanner::patch`), and recognise on the
//! next scan that a patched archive already contains another member.
//!
//! Merging into an existing patch dedupes by BLAKE3 of the page bytes: an identical page is
//! skipped, and a same-named but different page renames the one already in the patch. The result is
//! re-derived from the merged reading order, so the stored insertions never depend on how the old
//! patch happened to group them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use lanrurugi_scanner::archive_format;
use lanrurugi_scanner::patch::{self, EffectivePage, PatchInsertion, PatchMetadata};
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::AppState;

/// What a member's patch adds, as seen by the scan.
#[derive(Debug, Clone, Default)]
pub struct PatchState {
    /// Patch pages that actually apply (an insertion whose anchor no longer exists is dropped).
    pub pages: u32,
    /// CRC32s of every archive pages were taken from.
    pub sources: Vec<String>,
}

/// Reads `archive_path`'s patch, if any and if it still matches the archive. Blocking: the CRC32
/// check reads the whole archive.
pub fn patch_state(archive_path: &Path) -> Option<PatchState> {
    let patch_path = patch::patch_path_for(archive_path);
    if !patch_path.is_file() {
        return None;
    }
    let metadata = patch::load(&patch_path, archive_path).ok()?;
    let original = archive_format::list_pages(archive_path).ok()?;
    let pages = patch::apply(&original, &metadata)
        .iter()
        .filter(|p| matches!(p, EffectivePage::Patched(_)))
        .count() as u32;
    Some(PatchState {
        pages,
        sources: metadata.all_sources(),
    })
}

#[derive(Debug, Deserialize)]
pub struct PatchRequest {
    pub target: String,
    pub source: String,
    pub pages: Vec<PatchPick>,
}

/// One source page and where it goes. Both indexes are into the patch-merged page lists the
/// comparison reports, so a page picked from an already patched source works too.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchPick {
    pub source_page: usize,
    /// Position in the target's current merged list the page lands before; `>= len` appends.
    pub insert_before: usize,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct PatchOutcome {
    pub added: usize,
    /// Picks whose bytes were already in the patch (same BLAKE3).
    pub skipped: usize,
    /// Existing patch files renamed because a new page took their name.
    pub renamed: usize,
    pub patch_pages: usize,
}

#[derive(Debug)]
enum PatchFailure {
    BadRequest(String),
    /// The existing patch no longer matches the target; merging would discard or misplace it.
    StalePatch(String),
    Io(String),
}

impl From<archive_format::ArchiveFormatError> for PatchFailure {
    fn from(e: archive_format::ArchiveFormatError) -> Self {
        PatchFailure::Io(e.to_string())
    }
}

impl From<patch::PatchError> for PatchFailure {
    fn from(e: patch::PatchError) -> Self {
        PatchFailure::Io(e.to_string())
    }
}

impl From<std::io::Error> for PatchFailure {
    fn from(e: std::io::Error) -> Self {
        PatchFailure::Io(e.to_string())
    }
}

/// `POST /database/duplicates/patch`.
pub async fn create_patch(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    axum::Json(body): axum::Json<PatchRequest>,
) -> Response {
    if body.target == body.source {
        return error(
            StatusCode::BAD_REQUEST,
            "duplicates_patch",
            "target and source must differ",
        );
    }
    if body.pages.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "duplicates_patch",
            "no pages selected",
        );
    }
    let target = match super::review::load_archive(&state, &body.target).await {
        Ok(a) => a,
        Err((status, message)) => return error(status, "duplicates_patch", message),
    };
    let source = match super::review::load_archive(&state, &body.source).await {
        Ok(a) => a,
        Err((status, message)) => return error(status, "duplicates_patch", message),
    };
    // A patch shown half-written to a reader would splice in pages that are not there yet.
    for id in [&body.target, &body.source] {
        if crate::visibility::is_pending(&state, id).await {
            return error(
                StatusCode::CONFLICT,
                "duplicates_patch",
                "archive is still being ingested",
            );
        }
    }

    let target_path = PathBuf::from(&target.file);
    let source_path = PathBuf::from(&source.file);
    let picks = body.pages.clone();
    let result = lanrurugi_core::concurrency::run_blocking(move || {
        merge_into_patch(&target_path, &source_path, &picks)
    })
    .await;

    let activity_target = ActivityTarget {
        id: Some(body.target.clone()),
        label: Some(target.name.clone()),
        kind: Some("archive".to_string()),
    };
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(failure)) => {
            let (status, message) = match failure {
                PatchFailure::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
                PatchFailure::StalePatch(m) => (StatusCode::CONFLICT, m),
                PatchFailure::Io(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            };
            crate::activity::record_manual(
                &state,
                auth.as_deref(),
                action_types::ARCHIVE_PATCH_CREATE,
                activity_target,
                Outcome::Failure {
                    reason: message.clone(),
                },
                None,
                None,
            )
            .await;
            return error(status, "duplicates_patch", message);
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "duplicates_patch",
                e.to_string(),
            )
        }
    };

    if outcome.added > 0 {
        let mut target = target;
        if !target.has_patch {
            target.has_patch = true;
            if let Err(e) = state.repos.archives.save(&target).await {
                tracing::warn!(id = %body.target, error = %e, "patch written but has_patch flag not saved");
            }
        }
        crate::archives::clear_page_thumbnails(&state, &body.target).await;
        crate::activity::record_manual(
            &state,
            auth.as_deref(),
            action_types::ARCHIVE_PATCH_CREATE,
            activity_target,
            Outcome::Success,
            None,
            Some(json!({ "source": body.source, "added": outcome.added, "skipped": outcome.skipped })),
        )
        .await;
    }
    axum::Json(json!({ "operation": "duplicates_patch", "success": 1, "result": outcome }))
        .into_response()
}

fn merge_into_patch(
    target: &Path,
    source: &Path,
    picks: &[PatchPick],
) -> Result<PatchOutcome, PatchFailure> {
    let patch_path = patch::patch_path_for(target);
    let original = archive_format::list_pages(target)?;
    let existing = if patch_path.is_file() {
        Some(patch::load(&patch_path, target).map_err(|e| {
            PatchFailure::StalePatch(format!("the existing patch cannot be merged: {e}"))
        })?)
    } else {
        None
    };
    let mut effective: Vec<EffectivePage> = match &existing {
        Some(metadata) => patch::apply(&original, metadata),
        None => original
            .iter()
            .cloned()
            .map(EffectivePage::Original)
            .collect(),
    };

    // Bytes and BLAKE3 of every patch page that currently applies.
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    let mut by_hash: HashMap<blake3::Hash, String> = HashMap::new();
    for page in &effective {
        if let EffectivePage::Patched(name) = page {
            let bytes = archive_format::read_entry(&patch_path, name)?;
            by_hash.insert(blake3::hash(&bytes), name.clone());
            files.insert(name.clone(), bytes);
        }
    }

    let source_pages = patch::effective_pages(source, &archive_format::list_pages(source)?);
    let mut outcome = PatchOutcome::default();
    let mut planned: Vec<(usize, String)> = Vec::new();
    for pick in picks {
        let page = source_pages.get(pick.source_page).ok_or_else(|| {
            PatchFailure::BadRequest(format!("source page {} does not exist", pick.source_page))
        })?;
        let bytes = patch::read_page(source, page)?;
        let hash = blake3::hash(&bytes);
        if by_hash.contains_key(&hash) {
            outcome.skipped += 1;
            continue;
        }
        let name = base_name(page.entry_name());
        if files.contains_key(&name) {
            let renamed = unique_name(&name, &files);
            let old = files.remove(&name).expect("checked above");
            files.insert(renamed.clone(), old);
            for entry in effective.iter_mut() {
                if matches!(entry, EffectivePage::Patched(n) if *n == name) {
                    *entry = EffectivePage::Patched(renamed.clone());
                }
            }
            for value in by_hash.values_mut() {
                if *value == name {
                    *value = renamed.clone();
                }
            }
            for (_, planned_name) in planned.iter_mut() {
                if *planned_name == name {
                    *planned_name = renamed.clone();
                }
            }
            outcome.renamed += 1;
        }
        by_hash.insert(hash, name.clone());
        files.insert(name.clone(), bytes);
        planned.push((pick.insert_before, name));
        outcome.added += 1;
    }

    let sequence = splice(&effective, &planned);
    outcome.patch_pages = sequence
        .iter()
        .filter(|p| matches!(p, EffectivePage::Patched(_)))
        .count();
    if outcome.added == 0 {
        return Ok(outcome);
    }

    let source_crc = patch::crc32_of_file(source)?;
    let mut sources = existing
        .as_ref()
        .map(PatchMetadata::all_sources)
        .unwrap_or_default();
    if !sources.contains(&source_crc) {
        sources.push(source_crc.clone());
    }
    let metadata = PatchMetadata {
        target_crc32: patch::crc32_of_file(target)?,
        source_crc32: existing
            .as_ref()
            .and_then(|m| m.source_crc32.clone())
            .or(Some(source_crc)),
        sources,
        insertions: insertions_for(&sequence),
    };
    let bundled: Vec<(String, Vec<u8>)> = sequence
        .iter()
        .filter_map(|p| match p {
            EffectivePage::Patched(name) => files.remove(name).map(|b| (name.clone(), b)),
            EffectivePage::Original(_) => None,
        })
        .collect();
    let bytes = patch::build_patch_zip_from(&metadata, &bundled)?;
    write_durably(&patch_path, &bytes)?;
    Ok(outcome)
}

/// `effective` with each planned page inserted before its position (in request order for equal
/// positions; anything past the end is appended).
fn splice(effective: &[EffectivePage], planned: &[(usize, String)]) -> Vec<EffectivePage> {
    let mut out = Vec::with_capacity(effective.len() + planned.len());
    for i in 0..=effective.len() {
        for (position, name) in planned {
            let lands_here = if i == effective.len() {
                *position >= effective.len()
            } else {
                *position == i
            };
            if lands_here {
                out.push(EffectivePage::Patched(name.clone()));
            }
        }
        if let Some(page) = effective.get(i) {
            out.push(page.clone());
        }
    }
    out
}

/// Re-derives insertions from a merged reading order: each run of patch pages anchors after the
/// original page before it, or before the first original page when it leads.
fn insertions_for(sequence: &[EffectivePage]) -> Vec<PatchInsertion> {
    let mut insertions = Vec::new();
    let mut last_original: Option<String> = None;
    let mut run: Vec<String> = Vec::new();
    for page in sequence {
        match page {
            EffectivePage::Patched(name) => run.push(name.clone()),
            EffectivePage::Original(name) => {
                if !run.is_empty() {
                    insertions.push(PatchInsertion {
                        after_filename: last_original.clone(),
                        before_filename: last_original.is_none().then(|| name.clone()),
                        files: std::mem::take(&mut run),
                    });
                }
                last_original = Some(name.clone());
            }
        }
    }
    if !run.is_empty() {
        insertions.push(PatchInsertion {
            after_filename: last_original,
            before_filename: None,
            files: run,
        });
    }
    insertions
}

fn base_name(entry: &str) -> String {
    entry
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(entry)
        .to_string()
}

/// `name` with a `~N` before its extension, the first one not taken.
fn unique_name(name: &str, taken: &HashMap<String, Vec<u8>>) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem}~{n}{ext}"))
        .find(|candidate| !taken.contains_key(candidate))
        .expect("unbounded range")
}

/// Temp file in the same directory, fsync, rename, fsync the directory: a crash leaves either the
/// old patch or the new one, never a torn file. The temp name ends in `.tmp`, which the library
/// watcher does not treat as an archive.
fn write_durably(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        if let Some(dir) = path.parent() {
            #[cfg(unix)]
            std::fs::File::open(dir)?.sync_all()?;
            #[cfg(not(unix))]
            let _ = dir;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn o(n: &str) -> EffectivePage {
        EffectivePage::Original(n.to_string())
    }
    fn p(n: &str) -> EffectivePage {
        EffectivePage::Patched(n.to_string())
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, bytes) in entries {
            zip.start_file::<_, ()>(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    /// Distinct, decodable-enough bytes are not needed: the merge never decodes pages.
    fn page(tag: u8) -> Vec<u8> {
        vec![tag; 64]
    }

    #[test]
    fn insertions_round_trip_through_apply() {
        let sequence = vec![p("x"), o("1"), o("2"), p("y"), p("z"), o("3"), p("w")];
        let metadata = PatchMetadata {
            target_crc32: String::new(),
            source_crc32: None,
            sources: Vec::new(),
            insertions: insertions_for(&sequence),
        };
        let originals = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        assert_eq!(patch::apply(&originals, &metadata), sequence);
    }

    #[test]
    fn splice_places_pages_before_their_position_and_appends_past_the_end() {
        let effective = vec![o("1"), o("2")];
        let planned = vec![
            (1, "a".to_string()),
            (9, "b".to_string()),
            (1, "c".to_string()),
        ];
        assert_eq!(
            splice(&effective, &planned),
            vec![o("1"), p("a"), p("c"), o("2"), p("b")]
        );
    }

    #[test]
    fn unique_name_suffixes_before_the_extension() {
        let taken: HashMap<String, Vec<u8>> = [
            ("010.jpg".to_string(), vec![]),
            ("010~2.jpg".to_string(), vec![]),
        ]
        .into();
        assert_eq!(unique_name("010.jpg", &taken), "010~3.jpg");
    }

    #[test]
    fn merging_skips_identical_pages_and_renames_a_clashing_name() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.zip");
        let first = dir.path().join("first.zip");
        let second = dir.path().join("second.zip");
        write_zip(&target, &[("001.jpg", &page(1)), ("002.jpg", &page(2))]);
        write_zip(
            &first,
            &[
                ("001.jpg", &page(1)),
                ("002.jpg", &page(2)),
                ("003.jpg", &page(3)),
            ],
        );
        // Same name as the page already patched in, different bytes; plus a byte-identical copy.
        write_zip(&second, &[("003.jpg", &page(30)), ("x.jpg", &page(3))]);

        let outcome = merge_into_patch(
            &target,
            &first,
            &[PatchPick {
                source_page: 2,
                insert_before: 9,
            }],
        )
        .unwrap();
        assert_eq!(
            outcome,
            PatchOutcome {
                added: 1,
                skipped: 0,
                renamed: 0,
                patch_pages: 1
            }
        );

        let outcome = merge_into_patch(
            &target,
            &second,
            &[
                PatchPick {
                    source_page: 0,
                    insert_before: 0,
                },
                PatchPick {
                    source_page: 1,
                    insert_before: 0,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            outcome,
            PatchOutcome {
                added: 1,
                skipped: 1,
                renamed: 1,
                patch_pages: 2
            }
        );

        let originals = archive_format::list_pages(&target).unwrap();
        let pages = patch::effective_pages(&target, &originals);
        assert_eq!(
            pages,
            vec![p("003.jpg"), o("001.jpg"), o("002.jpg"), p("003~2.jpg")]
        );
        assert_eq!(patch::read_page(&target, &pages[0]).unwrap(), page(30));
        assert_eq!(patch::read_page(&target, &pages[3]).unwrap(), page(3));

        let state = patch_state(&target).unwrap();
        assert_eq!(state.pages, 2);
        assert_eq!(state.sources.len(), 2);
        assert!(state
            .sources
            .contains(&patch::crc32_of_file(&first).unwrap()));
    }
}
