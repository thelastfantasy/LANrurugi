//! Where a `.patch.zip`'s pages came from, where they sit in the merged page list, and whether the
//! patch has already been reported as unreadable.
//!
//! Deliberately kept outside the patch zip. The zip's own `metadata.json` only records CRC32s, and
//! a CRC32 cannot tell a user which archive a page was taken from, nor where that archive came
//! from — and when the patch is the broken thing, `metadata.json` is exactly what cannot be read.
//! So the provenance (source archive id, name, title, `source:` sites, CRC32) and the merged-list
//! positions of the patch's pages live here, written when the patch is created.
//!
//! When the patch is found permanently unreadable ([`report_damage`]) everything that numbers pages
//! follows the pictures past the vanished ones: reading progress, chapter markers, bookmarks and
//! stamps move down, and those that sat on a vanished page are dropped. That rewrites user data from
//! a read path, so it happens only for damage that re-reading cannot fix (a transient I/O error is
//! ignored), and only once per patch file — decided under the archive's record lock, so two reads
//! arriving together cannot shift everything twice. The damaged page numbers are handed back to the
//! reader as well.

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
    /// BLAKE3 of the source file, matching the patch's own `sourcesBlake3`.
    #[serde(default)]
    pub blake3: String,
    /// Legacy: the CRC32 records made before patches named their sources by BLAKE3 carry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub crc32: String,
    /// `source:` tags of the source archive (prefix stripped) plus its primary source, which is
    /// where that source itself came from.
    pub sites: Vec<String>,
}

/// Everything remembered about one archive's patch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Whether the two shifts that follow a damage report have run: progress and chapters, then
    /// bookmarks and stamps. Each is set only once its step completed, so a report interrupted
    /// between recording the damage and finishing the shifts is resumed by the next read. `None`
    /// on a record written before these existed, whose shifts always ran with the report: done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_shifted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks_shifted: Option<bool>,
    /// The progress/chapter shift, decided before it is applied: what the record held and what it
    /// becomes. Applying it is then a compare-and-set, so a retry after a lost completion flag is
    /// a no-op rather than a second shift (and a reader who moved on meanwhile is left alone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_shift: Option<ProgressShift>,
}

/// See [`PatchRecord::progress_shift`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressShift {
    pub from_page: u32,
    pub to_page: u32,
    /// The page list's length with the patch's pages, and without them: a progress saved with one
    /// of them says which numbering it is in (`Archive::progress_pages`). `0` when not known.
    #[serde(default)]
    pub pages_before: u32,
    #[serde(default)]
    pub pages_after: u32,
    pub from_toc: Vec<lanrurugi_core::entities::TocEntry>,
    pub to_toc: Vec<lanrurugi_core::entities::TocEntry>,
}

impl PatchRecord {
    fn shifts_pending(&self) -> (bool, bool) {
        if self.damaged_at.is_none() {
            return (true, true);
        }
        (
            self.progress_shifted == Some(false),
            self.marks_shifted == Some(false),
        )
    }
}

async fn read(state: &AppState, archive_id: &str) -> Option<PatchRecord> {
    let mut conn = state.redis.archive.get().await.ok()?;
    let raw: String = conn.hget(KEY, archive_id).await.ok()?;
    serde_json::from_str(&raw).ok()
}

pub async fn load(state: &AppState, archive_id: &str) -> Option<PatchRecord> {
    read(state, archive_id).await
}

/// `false` when the record could not be written — the damage report depends on that knowledge.
pub async fn store(state: &AppState, archive_id: &str, record: &PatchRecord) -> bool {
    let Ok(mut conn) = state.redis.archive.get().await else {
        return false;
    };
    let Ok(raw) = serde_json::to_string(record) else {
        return false;
    };
    let written: Result<(), _> = conn.hset(KEY, archive_id, raw).await;
    written.is_ok()
}

/// Drops the record — the patch is gone, so nothing about it is true any more.
pub async fn forget(state: &AppState, archive_id: &str) {
    let mut conn = match state.redis.archive.get().await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!(archive = %archive_id, error = %e, "could not reach Redis to forget a patch record");
            return;
        }
    };
    let dropped: Result<(), _> = conn.hdel(KEY, archive_id).await;
    if let Err(e) = dropped {
        // A record left behind outlives the patch it describes and keeps being served to the
        // damage report as if those pages still carried it.
        tracing::warn!(archive = %archive_id, error = %e, "failed to forget a patch record");
    }
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
    record.progress_shifted = None;
    record.marks_shifted = None;
    record.progress_shift = None;
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
    /// The progress/chapter shift did not complete on this read: it is retried by the next one, so
    /// the notice must not say it already happened.
    pub pending: bool,
}

/// Reports a patch that cannot be read, once per patch file: records the damage, shifts the
/// archive's progress and chapter markers down past the vanished pages, and writes the activity
/// entry that carries the provenance (it is the only place that survives, since the zip's own
/// `metadata.json` is what broke). Returns what the reader should be told, or `None` when there is
/// nothing to say (no record: the patch was never created here, or it was already reported).
///
/// Lock order: `patch:<id>` is taken first and `archive:<id>` (through `archives.update`) inside it.
/// Never call this while holding `archive:<id>`.
///
/// `listed` is the length of the page list this read serves (the patch's pages already left out).
pub async fn report_damage(
    state: &AppState,
    archive: &Archive,
    error: &lanrurugi_scanner::patch::PatchError,
    listed: u32,
) -> Option<PatchDamage> {
    // A read that failed for a reason that may pass must not rewrite progress, chapters, bookmarks
    // and stamps for good.
    if !error.is_permanent() {
        // Info, not a warning: this runs on every read, and a flaky mount would otherwise file a
        // tracker event per page list.
        tracing::info!(archive = %archive.id, %error, "patch could not be read this time; not treating it as damaged");
        return None;
    }
    let reason = error.to_string();
    let reason = reason.as_str();
    let archive_id = archive.id.as_str();
    // Held through the shifts: the reader and the overview ask for the page list together, and only
    // one of them may run them. Each step is recorded as it completes, so one interrupted midway
    // is finished by a later read instead of being skipped for good.
    let _guard = lanrurugi_storage::record_lock::lock(&format!("patch:{archive_id}")).await;
    let mut record = read(state, archive_id).await?;
    let (progress_pending, marks_pending) = record.shifts_pending();
    if !progress_pending && !marks_pending {
        // Long done — but a reader still holding the list from before the damage (another tab)
        // keeps saving pages in the old numbering. It says so with the list length it saved, and
        // such a page is moved like the original was. While the patch stays unreadable the server
        // only hands out lists of `pages_after`, so a `pages_before` here can only come from a list
        // fetched before the damage.
        if let Some(shift) = record.progress_shift.clone() {
            if shift.pages_before > 0 && archive.progress_pages == Some(shift.pages_before) {
                apply_progress_shift(state, archive, shift, &record.pages).await;
            }
        }
        return None;
    }
    let first_report = record.damaged_at.is_none();
    if first_report {
        record.damaged_at = Some(crate::integrity_scan::now_secs());
        record.progress_shifted = Some(false);
        record.marks_shifted = Some(false);
        // Nothing moves before the damage is on record: a shift whose bookkeeping could not be
        // written would run again on the next read.
        if !store(state, archive_id, &record).await {
            return None;
        }
    }
    let mut progress_done = !progress_pending;
    if progress_pending {
        // Decide the shift once and keep it; applying it is a compare-and-set, safe to repeat.
        if record.progress_shift.is_none() {
            record.progress_shift =
                plan_progress_shift(state, archive, &record.pages, listed).await;
            if record.progress_shift.is_some() && !store(state, archive_id, &record).await {
                record.progress_shift = None;
            }
        }
        if let Some(shift) = record.progress_shift.clone() {
            if apply_progress_shift(state, archive, shift, &record.pages).await {
                record.progress_shifted = Some(true);
                progress_done = store(state, archive_id, &record).await;
            }
        }
    }
    if marks_pending {
        // Bookmarks and stamps move relative to where they are, so a second run would move them
        // twice: the step is recorded as done *before* it runs, and runs only if that stuck. A
        // crash in between loses the move, which is the lesser harm.
        record.marks_shifted = Some(true);
        if store(state, archive_id, &record).await {
            shift_bookmarks_and_stamps(state, archive, &record.pages).await;
        }
    }
    if !first_report {
        return None;
    }

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
        pending: !progress_done,
    })
}

/// Where the archive's own reading position and chapter markers go once the patch's pages are
/// gone. Progress lands on the last surviving page before it (page 1 when the patch opened the
/// archive); a chapter marker that pointed at a patch page is dropped rather than moved, since which
/// surviving page it "meant" is unknowable. `None` when the record cannot be read.
async fn plan_progress_shift(
    state: &AppState,
    archive: &Archive,
    patch_pages: &[u32],
    listed: u32,
) -> Option<ProgressShift> {
    let current = state.repos.archives.get(&archive.id).await.ok()??;
    let to_page = progress_after(current.lastreadpage, patch_pages);
    let to_toc = current
        .toc
        .iter()
        .filter_map(|entry| {
            surviving_page(entry.page, patch_pages).map(|page| lanrurugi_core::entities::TocEntry {
                page,
                name: entry.name.clone(),
            })
        })
        .collect();
    Some(ProgressShift {
        from_page: current.lastreadpage,
        to_page,
        pages_before: listed + patch_pages.len() as u32,
        pages_after: listed,
        from_toc: current.toc,
        to_toc,
    })
}

/// A reading position once the patch's pages are gone: the same picture, or the last surviving
/// one before it (page 1 when the patch opened the archive). `0` (never read) stays.
fn progress_after(page: u32, patch_pages: &[u32]) -> u32 {
    if page == 0 || patch_pages.is_empty() {
        return page;
    }
    surviving_page(page, patch_pages).unwrap_or_else(|| fallback_page(page, patch_pages))
}

/// Applies a planned shift. `true` once the write went through (including when there was nothing
/// left to change).
///
/// Which numbering the saved page is in decides it, and the list length saved with it
/// (`Archive::progress_pages`) says so: the old one is moved, whatever page it is; the new one
/// (the reader got this read's list, or this shift already ran) is left alone. Without a length
/// (an older client), the page is moved only while it is still the one the shift was planned
/// from — a different one was most likely saved by the reader holding the new list, and moving it
/// would shift it twice. A moved page is stamped with the new length, which is what makes a repeat
/// a no-op.
///
/// Chapter markers carry no such length, so they keep the compare-and-set: they change only while
/// they are still what the plan saw.
async fn apply_progress_shift(
    state: &AppState,
    archive: &Archive,
    shift: ProgressShift,
    patch_pages: &[u32],
) -> bool {
    let patch_pages = patch_pages.to_vec();
    let updated = state
        .repos
        .archives
        .update(&archive.id, move |record| {
            let known = shift.pages_before > 0 && shift.pages_before != shift.pages_after;
            let saved_in_old = known && record.progress_pages == Some(shift.pages_before);
            let saved_in_new = known && record.progress_pages == Some(shift.pages_after);
            if saved_in_old {
                record.lastreadpage = progress_after(record.lastreadpage, &patch_pages);
                record.progress_pages = Some(shift.pages_after);
            } else if !saved_in_new && record.lastreadpage == shift.from_page {
                record.lastreadpage = shift.to_page;
                if known {
                    record.progress_pages = Some(shift.pages_after);
                }
            }
            if record.toc == shift.from_toc {
                record.toc = shift.to_toc;
            }
        })
        .await;
    if let Err(e) = updated {
        tracing::warn!(
            archive = %archive.id,
            error = %e,
            "could not shift reading progress past an unreadable patch"
        );
        return false;
    }
    true
}

/// Runs at most once per report (see `report_damage`): every move here is relative to where the
/// mark is now, so a retry would move the ones that already moved a second time. Because nothing retries
/// it, it is not part of `PatchDamage::pending` either. A bookmark or stamp
/// that fails to move is logged and left on its old page rather than retried. `false` when the
/// stamp index could not be rewritten.
async fn shift_bookmarks_and_stamps(
    state: &AppState,
    archive: &Archive,
    patch_pages: &[u32],
) -> bool {
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
        return true;
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
        return false;
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
    true
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
            progress_pages: None,
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
                blake3: "deadbeef".to_string(),
                crc32: String::new(),
                sites: vec!["https://example.org/b".to_string()],
            }],
            pages: vec![3, 4],
            ..PatchRecord::default()
        };
        let raw = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<PatchRecord>(&raw).unwrap(), record);
        // No damage field at all on the wire until there is damage.
        assert!(!raw.contains("damaged_at"));
    }

    #[test]
    fn a_report_from_before_the_shift_flags_counts_as_shifted() {
        let legacy = PatchRecord {
            damaged_at: Some(1),
            ..PatchRecord::default()
        };
        assert_eq!(legacy.shifts_pending(), (false, false));
        let interrupted = PatchRecord {
            damaged_at: Some(1),
            progress_shifted: Some(true),
            marks_shifted: Some(false),
            ..PatchRecord::default()
        };
        assert_eq!(interrupted.shifts_pending(), (false, true));
        assert_eq!(PatchRecord::default().shifts_pending(), (true, true));
    }

    /// A transient read failure changes nothing; a permanent one shifts progress once, however
    /// many reads report it.
    #[tokio::test]
    async fn damage_shifts_progress_once_and_only_for_a_permanent_failure() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        let mut archive = archive_named("A.zip", "A");
        archive.id = lanrurugi_core::ids::ArchiveId("3025".repeat(10));
        archive.pagecount = 10;
        state
            .repos
            .archives
            .create_or_replace(&archive)
            .await
            .unwrap();
        state
            .repos
            .archives
            .set_progress(&archive.id, 6, None, 1)
            .await
            .unwrap();
        assert!(
            store(
                &state,
                archive.id.as_str(),
                &PatchRecord {
                    pages: vec![2, 3],
                    ..PatchRecord::default()
                },
            )
            .await
        );
        let progress = |state: AppState, id: lanrurugi_core::ids::ArchiveId| async move {
            state
                .repos
                .archives
                .get(&id)
                .await
                .unwrap()
                .unwrap()
                .lastreadpage
        };

        let transient = lanrurugi_scanner::patch::PatchError::Io(std::io::Error::other("busy"));
        assert!(report_damage(&state, &archive, &transient, 8)
            .await
            .is_none());
        assert_eq!(progress(state.clone(), archive.id.clone()).await, 6);

        let broken = lanrurugi_scanner::patch::PatchError::MissingMetadata;
        assert!(report_damage(&state, &archive, &broken, 8).await.is_some());
        assert_eq!(progress(state.clone(), archive.id.clone()).await, 4);
        assert!(report_damage(&state, &archive, &broken, 8).await.is_none());
        assert_eq!(progress(state.clone(), archive.id.clone()).await, 4);

        // The shift was applied but its completion flag never got written: the next read finds it
        // pending and runs it again, which must change nothing.
        let mut record = read(&state, archive.id.as_str()).await.unwrap();
        record.progress_shifted = Some(false);
        assert!(store(&state, archive.id.as_str(), &record).await);
        assert!(report_damage(&state, &archive, &broken, 8).await.is_none());
        assert_eq!(progress(state.clone(), archive.id.clone()).await, 4);
        assert_eq!(
            read(&state, archive.id.as_str())
                .await
                .unwrap()
                .progress_shifted,
            Some(true)
        );

        forget(&state, archive.id.as_str()).await;
        let _ = state.repos.archives.delete(&archive.id).await;
    }

    /// The list length saved with a page says which numbering it is in: a page counted in the list
    /// from before the damage is moved, even after the shift (another tab still holding the old
    /// list); one counted in the new list stays, even when it equals the page the shift planned from.
    #[tokio::test]
    async fn the_saved_list_length_decides_whether_a_page_moves() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        let mut archive = archive_named("B.zip", "B");
        archive.id = lanrurugi_core::ids::ArchiveId("3026".repeat(10));
        archive.pagecount = 10;
        let archives = &state.repos.archives;
        archives.create_or_replace(&archive).await.unwrap();
        async fn reload(state: &AppState, id: &lanrurugi_core::ids::ArchiveId) -> Archive {
            state.repos.archives.get(id).await.unwrap().unwrap()
        }
        let broken = lanrurugi_scanner::patch::PatchError::MissingMetadata;
        // Patch pages 2 and 3: the list goes from 10 pages to 8.
        let patch = PatchRecord {
            pages: vec![2, 3],
            ..PatchRecord::default()
        };

        // Counted in the new list already: left alone although it is the planned-from page.
        archives
            .set_progress(&archive.id, 6, Some(8), 1)
            .await
            .unwrap();
        assert!(store(&state, archive.id.as_str(), &patch).await);
        let current = reload(&state, &archive.id).await;
        assert!(report_damage(&state, &current, &broken, 8).await.is_some());
        assert_eq!(reload(&state, &archive.id).await.lastreadpage, 6);

        // Afterwards, a tab with the old list saves page 7 of 10: moved to 5 of 8, once.
        archives
            .set_progress(&archive.id, 7, Some(10), 2)
            .await
            .unwrap();
        let current = reload(&state, &archive.id).await;
        assert!(report_damage(&state, &current, &broken, 8).await.is_none());
        let moved = reload(&state, &archive.id).await;
        assert_eq!((moved.lastreadpage, moved.progress_pages), (5, Some(8)));
        assert!(report_damage(&state, &moved, &broken, 8).await.is_none());
        assert_eq!(reload(&state, &archive.id).await.lastreadpage, 5);

        forget(&state, archive.id.as_str()).await;
        let _ = state.repos.archives.delete(&archive.id).await;
    }
}
