//! Where a `.patch.zip`'s pages came from, where they sit in the merged page list, and whether the
//! patch has already been reported as unreadable.
//!
//! Deliberately kept outside the patch zip. The zip's own `metadata.json` only records CRC32s, and
//! a CRC32 cannot tell a user which archive a page was taken from, nor where that archive came
//! from — and when the patch is the broken thing, `metadata.json` is exactly what cannot be read.
//! So the provenance (source archive id, name, title, `source:` sites, CRC32) and the merged-list
//! positions of the patch's pages live here, written when the patch is created.
//!
//! When the patch stops being readable ([`report_damage`]) the archive's own reading progress and
//! chapter markers are shifted down past the vanished pages — once, guarded by `damaged_at`, since
//! this runs from a read path. Bookmarks and stamps are left alone in storage (they become valid
//! again the moment the patch is restored) and are hidden by the reader for the affected pages,
//! which is why the damaged page numbers are handed back to it.

use deadpool_redis::redis::AsyncCommands;
use serde::{Deserialize, Serialize};

use lanrurugi_core::entities::{Archive, Stamp};
use lanrurugi_core::ids::StampId;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

use crate::AppState;

/// `archive_id -> PatchRecord` JSON. Additive, LANrurugi-only.
pub const KEY: &str = "LANRURUGI_PATCH_PROVENANCE";

/// One archive whose pages ended up inside a patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchSource {
    /// The source archive's id — its content hash, so this still identifies the file after the
    /// record is deleted or renamed.
    pub archive_id: String,
    /// Title and filename as they were at patch-creation time (a snapshot: the source may be gone
    /// by the time anyone reads this).
    pub name: String,
    pub title: String,
    /// CRC32 of the source file, matching the patch's own `sources`/`sourceCrc32`.
    pub crc32: String,
    /// `source:` tags of the source archive (prefix stripped) plus its primary source, which is
    /// where that source itself came from.
    pub sites: Vec<String>,
}

/// Everything remembered about one archive's patch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchRecord {
    #[serde(default)]
    pub sources: Vec<PatchSource>,
    /// 1-based positions the patch's own pages occupy in the merged page list, ascending.
    #[serde(default)]
    pub pages: Vec<u32>,
    /// When the patch was first found unreadable, or `None` while it reads fine. Cleared whenever
    /// the patch is rewritten or removed, so a repaired patch is reported again if it breaks anew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damaged_at: Option<i64>,
}

async fn read(state: &AppState, archive_id: &str) -> Option<PatchRecord> {
    let mut conn = state.redis.archive.get().await.ok()?;
    let raw: String = conn.hget(KEY, archive_id).await.ok()?;
    serde_json::from_str(&raw).ok()
}

pub async fn load(state: &AppState, archive_id: &str) -> Option<PatchRecord> {
    read(state, archive_id).await
}

pub async fn store(state: &AppState, archive_id: &str, record: &PatchRecord) {
    let Ok(mut conn) = state.redis.archive.get().await else {
        return;
    };
    let Ok(raw) = serde_json::to_string(record) else {
        return;
    };
    let _: Result<(), _> = conn.hset(KEY, archive_id, raw).await;
}

/// Drops the record — the patch is gone, so nothing about it is true any more.
pub async fn forget(state: &AppState, archive_id: &str) {
    let Ok(mut conn) = state.redis.archive.get().await else {
        return;
    };
    let _: Result<(), _> = conn.hdel(KEY, archive_id).await;
}

/// Remembers that `source` contributed pages to `archive_id`'s patch, and re-records where those
/// pages now sit. Called after every successful merge; a later merge from another archive adds to
/// the list instead of replacing it, and any earlier damage report stops applying (the file the
/// report was about has just been rewritten).
pub async fn record_merge(
    state: &AppState,
    archive_id: &str,
    source: PatchSource,
    pages: Vec<u32>,
) -> PatchRecord {
    let mut record = read(state, archive_id).await.unwrap_or_default();
    record.sources.retain(|s| s.archive_id != source.archive_id);
    record.sources.push(source);
    record.pages = pages;
    record.damaged_at = None;
    store(state, archive_id, &record).await;
    record
}

/// The `source:` sites an archive is known to come from, plus its primary source — the answer to
/// "where did B itself come from?", which is what the patch report has to carry.
pub fn source_sites(archive: &Archive) -> Vec<String> {
    let mut sites: Vec<String> = archive
        .tags
        .split(',')
        .filter_map(|tag| tag.trim().strip_prefix("source:"))
        .map(|site| site.trim().to_string())
        .filter(|site| !site.is_empty())
        .collect();
    if let Some(primary) = archive.primary_source.as_deref() {
        let primary = primary.trim();
        if !primary.is_empty() {
            sites.push(primary.to_string());
        }
    }
    sites.sort();
    sites.dedup();
    sites
}

/// Where `page` moves to once a patch's own pages are gone; `None` when `page` itself was one of
/// them (the page no longer exists, and the caller decides what "closest surviving page" means).
pub fn surviving_page(page: u32, patch_pages: &[u32]) -> Option<u32> {
    if patch_pages.contains(&page) {
        return None;
    }
    let removed_before = patch_pages.iter().filter(|p| **p < page).count() as u32;
    Some(page.saturating_sub(removed_before).max(1))
}

/// The page to fall back to when the stored one was itself a patch page: the last surviving page
/// before it, or page 1 when the patch opened the archive.
fn fallback_page(page: u32, patch_pages: &[u32]) -> u32 {
    let removed_up_to = patch_pages.iter().filter(|p| **p <= page).count() as u32;
    page.saturating_sub(removed_up_to).max(1)
}

/// What the reader is told about a patch it cannot use.
#[derive(Debug, Clone, Serialize)]
pub struct PatchDamage {
    /// Why the patch could not be read (the patch error's own `Display`).
    pub reason: String,
    /// 1-based merged-list positions the patch's pages used to occupy — the reader hides the
    /// bookmarks and stamps sitting on them.
    pub pages: Vec<u32>,
    pub sources: Vec<PatchSource>,
}

/// Reports a patch that cannot be read, once per patch file: records the damage, shifts the
/// archive's progress and chapter markers down past the vanished pages, and writes the activity
/// entry that carries the provenance (it is the only place that survives, since the zip's own
/// `metadata.json` is what broke). Returns what the reader should be told, or `None` when there is
/// nothing to say (no record: the patch was never created here, or it was already reported).
pub async fn report_damage(
    state: &AppState,
    archive: &Archive,
    reason: &str,
) -> Option<PatchDamage> {
    let archive_id = archive.id.as_str();
    let mut record = read(state, archive_id).await?;
    if record.damaged_at.is_some() {
        return None;
    }
    record.damaged_at = Some(crate::integrity_scan::now_secs());
    store(state, archive_id, &record).await;

    shift_progress_and_toc(state, archive, &record.pages).await;
    shift_bookmarks_and_stamps(state, archive, &record.pages).await;

    crate::activity::record_automatic(
        state,
        "reader",
        action_types::ARCHIVE_PATCH_UNREADABLE,
        ActivityTarget {
            id: Some(archive_id.to_string()),
            label: Some(archive.name.clone()),
            kind: Some("archive".to_string()),
        },
        // A detection, not a failed operation: what happened is that the archive kept working
        // while the patch's pages dropped out of it.
        Outcome::Success,
        Some(serde_json::json!({
            "reason": reason,
            "archive": {
                "id": archive_id,
                "name": archive.name,
                "title": archive.title,
                "file": archive.file,
            },
            "pages": record.pages,
            "sources": record.sources,
        })),
        None,
    )
    .await;

    Some(PatchDamage {
        reason: reason.to_string(),
        pages: record.pages,
        sources: record.sources,
    })
}

/// Moves the archive's own reading position and chapter markers off the pages the patch used to
/// contribute. Progress lands on the last surviving page before it (page 1 when the patch opened
/// the archive); a chapter marker that pointed at a patch page is dropped rather than moved, since
/// which surviving page it "meant" is unknowable.
async fn shift_progress_and_toc(state: &AppState, archive: &Archive, patch_pages: &[u32]) {
    if patch_pages.is_empty() {
        return;
    }
    let pages = patch_pages.to_vec();
    let updated = state
        .repos
        .archives
        .update(&archive.id, move |record| {
            if record.lastreadpage > 0 {
                record.lastreadpage = surviving_page(record.lastreadpage, &pages)
                    .unwrap_or_else(|| fallback_page(record.lastreadpage, &pages));
            }
            record.toc = record
                .toc
                .iter()
                .filter_map(|entry| {
                    surviving_page(entry.page, &pages).map(|page| {
                        lanrurugi_core::entities::TocEntry {
                            page,
                            name: entry.name.clone(),
                        }
                    })
                })
                .collect();
        })
        .await;
    if let Err(e) = updated {
        tracing::warn!(
            archive = %archive.id,
            error = %e,
            "could not shift reading progress past an unreadable patch"
        );
    }
}

/// Keeps an archive's bookmarks and stamps pointing at the pictures they marked after a patch's own
/// pages vanish: the ones further down move down with those pictures, and the ones that sat *on* a
/// vanished page are removed, since the page they marked no longer exists (and after the shift the
/// number they held belongs to something else).
async fn shift_bookmarks_and_stamps(state: &AppState, archive: &Archive, patch_pages: &[u32]) {
    let archive_id = archive.id.as_str();

    match state.bookmarks.list_for_archive(archive_id).await {
        Ok(bookmarks) => {
            for bookmark in bookmarks {
                let Some(page) = surviving_page(bookmark.page, patch_pages) else {
                    let _ = state
                        .bookmarks
                        .remove(archive_id, bookmark.page, bookmark.bookmarked_at)
                        .await;
                    continue;
                };
                if page == bookmark.page {
                    continue;
                }
                if let Err(error) = state
                    .bookmarks
                    .add(archive_id, page, bookmark.bookmarked_at)
                    .await
                {
                    tracing::warn!(archive = %archive.id, error = %error, "could not move a bookmark past an unreadable patch");
                    continue;
                }
                if let Some(name) = bookmark.name.as_deref() {
                    let _ = state.bookmarks.set_name(archive_id, page, Some(name)).await;
                }
                let _ = state
                    .bookmarks
                    .remove(archive_id, bookmark.page, bookmark.bookmarked_at)
                    .await;
            }
        }
        Err(error) => tracing::warn!(
            archive = %archive.id,
            error = %error,
            "could not list bookmarks to move past an unreadable patch"
        ),
    }

    // A stamp carries its page inside its own id (`STAMPS_<page>_<millis>`), so moving one means
    // writing it under a new key and letting the archive's own `stamp_ids` index follow.
    let mut moved: Vec<(StampId, StampId)> = Vec::new();
    let mut vanished: Vec<StampId> = Vec::new();
    for stamp_id in &archive.stamp_ids {
        let Some(page) = crate::stamps::page_of(stamp_id.as_str()) else {
            continue;
        };
        let Some(target) = surviving_page(page, patch_pages) else {
            vanished.push(stamp_id.clone());
            continue;
        };
        if target == page {
            continue;
        }
        let prefix = format!("STAMPS_{page}_");
        let Some(suffix) = stamp_id.as_str().strip_prefix(&prefix) else {
            continue;
        };
        let new_id = StampId(format!("STAMPS_{target}_{suffix}"));
        let Ok(Some(stamp)) = state.repos.stamps.get(stamp_id).await else {
            continue;
        };
        let relocated = Stamp {
            stamp_id: new_id.clone(),
            ..stamp
        };
        if state.repos.stamps.restore_raw(&relocated).await.is_err() {
            continue;
        }
        moved.push((stamp_id.clone(), new_id));
    }
    if moved.is_empty() && vanished.is_empty() {
        return;
    }

    let mapping: std::collections::HashMap<StampId, StampId> = moved.iter().cloned().collect();
    let dropping = vanished.clone();
    let updated = state
        .repos
        .archives
        .update(&archive.id, move |record| {
            record
                .stamp_ids
                .retain(|stamp_id| !dropping.contains(stamp_id));
            for stamp_id in record.stamp_ids.iter_mut() {
                if let Some(new_id) = mapping.get(stamp_id) {
                    *stamp_id = new_id.clone();
                }
            }
        })
        .await;
    if let Err(error) = updated {
        tracing::warn!(
            archive = %archive.id,
            error = %error,
            "could not reindex stamps moved past an unreadable patch"
        );
        return;
    }
    // Old keys go only once the index already points at the new ones.
    let mut stale: Vec<StampId> = moved.into_iter().map(|(old, _)| old).collect();
    stale.extend(vanished);
    if let Err(error) = state.repos.stamps.delete_many_raw(&stale).await {
        tracing::warn!(
            archive = %archive.id,
            error = %error,
            "could not remove the superseded stamp records"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_named(name: &str, title: &str) -> Archive {
        Archive {
            id: lanrurugi_core::ids::ArchiveId("a".repeat(40)),
            name: name.to_string(),
            title: title.to_string(),
            file: String::new(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 0,
            pagecount: 0,
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
        }
    }

    #[test]
    fn surviving_page_shifts_pages_after_the_patch_down() {
        // Patch pages 3 and 4: page 5 becomes 3, page 2 stays 2, pages 3 and 4 are gone.
        assert_eq!(surviving_page(5, &[3, 4]), Some(3));
        assert_eq!(surviving_page(2, &[3, 4]), Some(2));
        assert_eq!(surviving_page(3, &[3, 4]), None);
        assert_eq!(surviving_page(4, &[3, 4]), None);
        // Nothing removed: identity.
        assert_eq!(surviving_page(9, &[]), Some(9));
    }

    #[test]
    fn fallback_page_lands_on_the_last_surviving_page_before() {
        assert_eq!(fallback_page(3, &[3, 4]), 2);
        assert_eq!(fallback_page(4, &[3, 4]), 2);
        // A patch at the very start leaves nothing before it: page 1.
        assert_eq!(fallback_page(1, &[1, 2]), 1);
        assert_eq!(fallback_page(2, &[1, 2]), 1);
    }

    #[test]
    fn source_sites_reads_source_tags_and_the_primary_source() {
        let mut archive = archive_named("B", "B");
        archive.tags =
            "artist:someone,source:https://example.org/b,source:https://mirror.example/b,other:x"
                .to_string();
        archive.primary_source = Some("https://example.org/b".to_string());
        assert_eq!(
            source_sites(&archive),
            vec![
                "https://example.org/b".to_string(),
                "https://mirror.example/b".to_string()
            ]
        );
    }

    #[test]
    fn a_record_survives_a_round_trip_through_json() {
        let record = PatchRecord {
            sources: vec![PatchSource {
                archive_id: "b".repeat(40),
                name: "B.zip".to_string(),
                title: "B".to_string(),
                crc32: "deadbeef".to_string(),
                sites: vec!["https://example.org/b".to_string()],
            }],
            pages: vec![3, 4],
            damaged_at: None,
        };
        let raw = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<PatchRecord>(&raw).unwrap(), record);
        // No damage field at all on the wire until there is damage.
        assert!(!raw.contains("damaged_at"));
    }
}
