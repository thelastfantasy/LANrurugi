//! Natural page sizes for the infinite-scroll reader (`GET /archives/{id}/page-dimensions`).
//!
//! Sizes are kept by entry name — the archive's own and its `.patch.zip`'s separately, each with the
//! size and mtime of the file it was read from — so the reader's page list (patch pages spliced in)
//! maps onto them however a patch shifts the numbering. Redis answers first; the sidecar keeps the
//! same data next to the bytes, like the title, and refills Redis when it has been lost. Only pages
//! neither has are measured.

use std::path::{Path, PathBuf};

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_scanner::patch::EffectivePage;
use lanrurugi_storage::sidecar::FileDimensions;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// What `LANRURUGI_PAGE_DIMENSIONS:{id}` holds: the sidecar's two fields.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Cached {
    archive: Option<FileDimensions>,
    patch: Option<FileDimensions>,
}

impl Cached {
    /// Drops a part whose file has changed since it was measured.
    fn keep_current(&mut self, archive: &Path, patch: &Path) {
        self.archive = self.archive.take().filter(|d| d.describes(archive));
        self.patch = self.patch.take().filter(|d| d.describes(patch));
    }

    fn merge(&mut self, other: &Cached) {
        for (mine, theirs) in [
            (&mut self.archive, &other.archive),
            (&mut self.patch, &other.patch),
        ] {
            match (mine.as_mut(), theirs) {
                (Some(mine), Some(theirs)) => mine.merge(theirs),
                (None, Some(theirs)) => *mine = Some(theirs.clone()),
                _ => {}
            }
        }
    }

    fn part(&self, page: &EffectivePage) -> Option<&FileDimensions> {
        match page {
            EffectivePage::Original(_) => self.archive.as_ref(),
            EffectivePage::Patched(_) => self.patch.as_ref(),
        }
    }

    fn has(&self, page: &EffectivePage) -> bool {
        self.part(page)
            .is_some_and(|d| d.pages.contains_key(page.entry_name()))
    }
}

/// Sizes of the first `count` pages of the reader's page list for `id`, whose file is `file` and
/// whose record's BLAKE3 is `blake3`.
pub async fn first_pages(
    state: &AppState,
    id: &str,
    file: &str,
    blake3: Option<&str>,
    count: usize,
) -> Result<Vec<Option<(u32, u32)>>, String> {
    let file = PathBuf::from(file);
    let patch = lanrurugi_scanner::patch::patch_path_for(&file);
    let listed = {
        let file = file.clone();
        let blake3 = blake3.map(str::to_string);
        lanrurugi_core::concurrency::run_blocking(move || {
            let originals = lanrurugi_scanner::archive_format::list_pages(&file)?;
            let mut pages =
                lanrurugi_scanner::patch::effective_pages(&file, &originals, blake3.as_deref());
            pages.truncate(count);
            Ok::<_, lanrurugi_scanner::archive_format::ArchiveFormatError>(pages)
        })
        .await
    };
    let pages = listed
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let key = lanrurugi_storage::keys::page_dimensions_key(id);
    let mut cached = read_redis(state, &key).await.unwrap_or_default();
    cached.keep_current(&file, &patch);
    let mut changed = false;

    if !pages.iter().all(|p| cached.has(p)) {
        let library = state.library.archive_dir.clone();
        let id_owned = id.to_string();
        if let Ok(Some(sidecar)) = tokio::task::spawn_blocking(move || {
            lanrurugi_storage::sidecar::read(&library, &id_owned)
        })
        .await
        {
            let mut stored = Cached {
                archive: sidecar.page_dimensions,
                patch: sidecar.patch_page_dimensions,
            };
            stored.keep_current(&file, &patch);
            let before = cached.clone();
            cached.merge(&stored);
            changed |= cached != before;
        }
    }

    let missing = |patched: bool| -> Vec<String> {
        let mut names: Vec<String> = pages
            .iter()
            .filter(|p| matches!(p, EffectivePage::Patched(_)) == patched && !cached.has(p))
            .map(|p| p.entry_name().to_string())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let (missing_archive, missing_patch) = (missing(false), missing(true));
    let mut measured = Cached::default();
    for (source, names, slot) in [
        (&file, missing_archive, &mut measured.archive),
        (&patch, missing_patch, &mut measured.patch),
    ] {
        if names.is_empty() {
            continue;
        }
        let source = source.clone();
        let result = lanrurugi_core::concurrency::run_blocking(move || {
            let mut dims = FileDimensions::for_file(&source).map_err(|e| e.to_string())?;
            let sizes = lanrurugi_scanner::archive_format::read_page_dimensions(&source, &names)
                .map_err(|e| e.to_string())?;
            for (name, size) in names.into_iter().zip(sizes) {
                dims.pages.insert(name, size.map(|(w, h)| [w, h]));
            }
            Ok::<_, String>(dims)
        })
        .await;
        *slot = Some(result.map_err(|e| e.to_string())??);
    }
    if measured.archive.is_some() || measured.patch.is_some() {
        cached.merge(&measured);
        changed = true;
        let mut current = cached.clone();
        let written = lanrurugi_storage::sidecar::amend(&state.library.archive_dir, id, move |s| {
            // Ours describe the files as they are now, so they win where the two disagree on that.
            current.merge(&Cached {
                archive: s.page_dimensions.take(),
                patch: s.patch_page_dimensions.take(),
            });
            s.page_dimensions = current.archive;
            s.patch_page_dimensions = current.patch;
        })
        .await;
        if let Err(e) = written {
            tracing::warn!(%id, error = %e, "failed to record page sizes in the sidecar");
        }
    }
    if changed {
        write_redis(state, &key, &cached).await;
    }

    Ok(pages
        .iter()
        .map(|p| {
            cached
                .part(p)
                .and_then(|d| d.pages.get(p.entry_name()).copied().flatten())
                .map(|[w, h]| (w, h))
        })
        .collect())
}

async fn read_redis(state: &AppState, key: &str) -> Option<Cached> {
    let mut conn = state.redis.config.get().await.ok()?;
    let raw: Option<String> = conn.get(key).await.ok()?;
    serde_json::from_str(&raw?).ok()
}

/// Best effort: a lost write only means the next request reads the sidecar again.
async fn write_redis(state: &AppState, key: &str, entry: &Cached) {
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let Ok(raw) = serde_json::to_string(entry) else {
        return;
    };
    let _: Result<(), _> = conn
        .set_ex(key, raw, lanrurugi_storage::keys::PAGE_DIMENSIONS_TTL_SECS)
        .await;
}
