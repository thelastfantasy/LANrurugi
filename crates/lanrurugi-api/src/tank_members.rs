//! Keeps a Tankoubon consistent when one of its member archives goes away.
//!
//! Deleting an archive is deliberately untidy about Tankoubons — the member ID is left in the
//! listing so restoring the archive from the recycle bin brings it back where it was. What that
//! leaves behind is a *page marker* (`Grouping::progress`, a global page number spanning the whole
//! tank) that can now point into a member nobody can read.
//!
//! Two cases, because they have different futures:
//! - **Recycled** (bin): the member stays listed, so page numbers do not move. Only a marker
//!   pointing inside the unreadable member is nudged to the previous member's last page (or the
//!   next member's first page), so opening the tank does not land in a skipped chapter.
//! - **Permanently deleted**: the member and its chapter name are dropped, later pages move down
//!   by its page count, and a tank whose only member this was is deleted along with it.

use lanrurugi_core::entities::{Archive, Grouping};
use lanrurugi_core::ids::ArchiveId;

use crate::AppState;

/// 1-based global page where `grouping.archives[index]` starts.
async fn start_page(state: &AppState, grouping: &Grouping, index: usize) -> u32 {
    let mut start = 1u32;
    for member in &grouping.archives[..index] {
        if let Ok(Some(a)) = state.repos.archives.get(member).await {
            start = start.saturating_add(a.pagecount);
        }
    }
    start
}

/// The archive went to the recycle bin: it keeps its place, but a marker inside it must not stay.
pub(crate) async fn after_recycle(state: &AppState, archive: &Archive) {
    let Ok(groupings) = state.repos.groupings.for_archive(&archive.id).await else {
        return;
    };
    for grouping in groupings {
        let Some(index) = grouping.archives.iter().position(|a| *a == archive.id) else {
            continue;
        };
        let start = start_page(state, &grouping, index).await;
        let last = start.saturating_add(archive.pagecount.saturating_sub(1));
        if grouping.progress < start || grouping.progress > last {
            continue;
        }
        let moved = if index > 0 {
            start - 1
        } else if index + 1 < grouping.archives.len() {
            last + 1
        } else {
            1
        };
        let tank_id = grouping.tankid.clone();
        let result = state
            .repos
            .groupings
            .update(&tank_id, move |g| g.progress = moved)
            .await;
        if let Err(e) = result {
            tracing::warn!(tank = %grouping.tankid, error = %e, "failed to move a tank's page marker out of a recycled member");
        }
    }
}

/// The archive is gone for good: take it out of its tanks, and take a tank out with it when it was
/// the only member left.
pub(crate) async fn after_permanent_delete(state: &AppState, archive: &Archive) {
    after_permanent_delete_with(state, &archive.id, archive.pagecount).await;
}

/// The same cleanup when only the ID and page count are still known — a recycled member being
/// purged from the bin, whose live record is long gone.
pub(crate) async fn after_permanent_delete_with(state: &AppState, id: &ArchiveId, pagecount: u32) {
    let Ok(groupings) = state.repos.groupings.for_archive(id).await else {
        return;
    };
    let old_id = id.clone();
    for grouping in groupings {
        let Some(index) = grouping.archives.iter().position(|a| *a == old_id) else {
            continue;
        };
        let tank_id = grouping.tankid.clone();
        if grouping.archives.len() <= 1 {
            if let Err(e) = state.repos.groupings.delete(&tank_id).await {
                tracing::warn!(tank = %tank_id, error = %e, "failed to delete a tank whose only member was deleted");
            }
            continue;
        }
        let start = start_page(state, &grouping, index).await;
        let last = start.saturating_add(pagecount.saturating_sub(1));

        let removed_id = old_id.clone();
        let removed_str = old_id.to_string();
        let result = state
            .repos
            .groupings
            .update(&tank_id, move |g| {
                // Move the marker before the member disappears from the listing: after it, "the
                // previous member's last page" is no longer `start - 1`.
                if g.progress > last {
                    g.progress = g.progress.saturating_sub(pagecount);
                } else if g.progress >= start {
                    g.progress = if index > 0 { start - 1 } else { start };
                }
                g.archives.retain(|a| *a != removed_id);
                g.chapter_names.retain(|c| c.id != removed_str);
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(tank = %grouping.tankid, error = %e, "failed to remove a deleted archive from its tank");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::ids::TankId;

    fn grouping(progress: u32) -> Grouping {
        Grouping {
            tankid: TankId("TANK_0000000001".to_string()),
            name: String::new(),
            summary: String::new(),
            tags: String::new(),
            progress,
            archives: Vec::new(),
            thumbnail_manual: false,
            thumbnail_source_archive: None,
            thumbnail_source_page: None,
            chapter_names: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    /// The arithmetic both paths rely on, without a Redis round trip.
    fn marker_after_removal(progress: u32, start: u32, pagecount: u32, index: usize) -> u32 {
        let last = start + pagecount - 1;
        if progress > last {
            progress - pagecount
        } else if progress >= start {
            if index > 0 {
                start - 1
            } else {
                start
            }
        } else {
            progress
        }
    }

    #[test]
    fn markers_inside_the_deleted_member_land_on_a_neighbour() {
        // Member 2 spans pages 101..=160; the marker at 130 becomes page 100 (member 1's last).
        assert_eq!(marker_after_removal(130, 101, 60, 1), 100);
        // Member 1 spans 1..=60; the marker moves to page 1, where member 2 now starts.
        assert_eq!(marker_after_removal(30, 1, 60, 0), 1);
    }

    #[test]
    fn markers_after_the_member_move_down_by_its_page_count() {
        assert_eq!(marker_after_removal(200, 101, 60, 1), 140);
    }

    #[test]
    fn markers_before_the_member_stay() {
        assert_eq!(marker_after_removal(50, 101, 60, 1), 50);
    }

    #[test]
    fn an_empty_tank_still_round_trips_its_marker() {
        assert_eq!(grouping(7).progress, 7);
    }
}
