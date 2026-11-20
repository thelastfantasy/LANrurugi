//! Puts a "delete and download again" archive back where it was.
//!
//! Deleting an archive is deliberately untidy about Tankoubons: the member ID stays in the listing
//! so restoring the archive from the recycle bin brings it back where it was. A re-download instead
//! produces a *new* archive record, so the membership, the chapter name and the reading progress
//! would all be left pointing at the old one.
//!
//! This module records what that archive looked like at the moment the re-download deleted it
//! (keyed by the queue item that does the re-downloading), and applies it once that same queue item
//! finishes. Keying by queue item is the point: a later manual download of the same URL is a
//! different item, produces the same content-addressed ID, and must NOT inherit the plan — otherwise
//! an archive the user deleted on purpose would silently rejoin its old Tankoubon.

use std::time::{SystemTime, UNIX_EPOCH};

use lanrurugi_core::entities::ChapterNameEntry;
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::{Deserialize, Serialize};

use crate::AppState;

/// One Tankoubon that holds the archive: where, under what chapter name, and where that member's
/// pages start in the tank's global numbering (needed to shift progress when page counts differ).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TankSlot {
    pub tank_id: String,
    pub chapter_name: Option<String>,
    pub start_page: u32,
    /// The tank's own page marker at the time of the delete. Recycling moves it out of the member
    /// that just became unreadable, so this is what puts it back once the archive returns.
    pub progress: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub old_id: String,
    pub progress: u32,
    pub pagecount: u32,
    pub tanks: Vec<TankSlot>,
    /// The archive had a sidecar `.patch.zip`. Recycling takes it into the bin with the archive; the
    /// replacement gets it back, because a damaged main file is exactly the case where the patch is
    /// the part still worth keeping.
    #[serde(default)]
    pub patch: bool,
    pub created_at: u64,
}

/// Hash of `queue item id -> Plan`. Entries are dropped as they are applied, and stale ones are
/// pruned on every write: a re-download that never finishes must not linger forever.
const KEY: &str = "LANRURUGI_REDOWNLOAD_RESTORE";
const TTL_SECS: u64 = 7 * 24 * 60 * 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Everything the plan needs, captured while the archive record still exists.
pub(crate) async fn capture(state: &AppState, archive: &lanrurugi_core::entities::Archive) -> Plan {
    let mut tanks = Vec::new();
    if let Ok(groupings) = state.repos.groupings.for_archive(&archive.id).await {
        for grouping in groupings {
            let Some(index) = grouping.archives.iter().position(|a| a == &archive.id) else {
                continue;
            };
            // Global page where this member starts: 1 + the pages of every member before it.
            let mut start_page = 1u32;
            for member in &grouping.archives[..index] {
                if let Ok(Some(a)) = state.repos.archives.get(member).await {
                    start_page = start_page.saturating_add(a.pagecount);
                }
            }
            tanks.push(TankSlot {
                tank_id: grouping.tankid.to_string(),
                progress: grouping.progress,
                chapter_name: grouping
                    .chapter_names
                    .iter()
                    .find(|c| c.id == archive.id.as_str())
                    .map(|c| c.name.clone()),
                start_page,
            });
        }
    }
    Plan {
        old_id: archive.id.to_string(),
        progress: archive.lastreadpage,
        pagecount: archive.pagecount,
        tanks,
        patch: lanrurugi_scanner::patch::patch_path_for(std::path::Path::new(&archive.file))
            .is_file(),
        created_at: now_secs(),
    }
}

pub(crate) async fn store(state: &AppState, queue_item_id: &str, plan: &Plan) {
    let Ok(mut conn) = state.redis.config.get().await else {
        tracing::warn!(%queue_item_id, "could not store the re-download restore plan");
        return;
    };
    let Ok(raw) = serde_json::to_string(plan) else {
        return;
    };
    let _: Result<(), _> = redis_hset(&mut conn, KEY, queue_item_id, &raw).await;
    prune(&mut conn).await;
}

async fn prune(conn: &mut deadpool_redis::Connection) {
    use deadpool_redis::redis::AsyncCommands;
    let Ok(all) = conn
        .hgetall::<_, std::collections::HashMap<String, String>>(KEY)
        .await
    else {
        return;
    };
    let cutoff = now_secs().saturating_sub(TTL_SECS);
    let stale: Vec<String> = all
        .into_iter()
        .filter(|(_, raw)| {
            serde_json::from_str::<Plan>(raw)
                .map(|p| p.created_at < cutoff)
                .unwrap_or(true)
        })
        .map(|(id, _)| id)
        .collect();
    if !stale.is_empty() {
        let _: Result<(), _> = conn.hdel(KEY, stale).await;
    }
}

async fn redis_hset(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    field: &str,
    value: &str,
) -> Result<(), deadpool_redis::redis::RedisError> {
    use deadpool_redis::redis::AsyncCommands;
    conn.hset(key, field, value).await
}

/// Applies (and forgets) the plan recorded for `queue_item_id`, if there is one. Called once the
/// download has been catalogued as `new_id`.
pub(crate) async fn apply(state: &AppState, queue_item_id: &str, new_id: &ArchiveId) {
    let Some(plan) = take(state, queue_item_id).await else {
        return;
    };
    let new_archive = match state.repos.archives.get(new_id).await {
        Ok(Some(a)) => a,
        _ => return,
    };

    // The archive's own place in the book, which the fresh record has at zero.
    if plan.progress > 0 {
        let page = plan.progress.min(new_archive.pagecount);
        if page > 0 {
            if let Err(e) = state
                .repos
                .archives
                // No list length: the patch is reattached after this, which changes the list.
                .set_progress(new_id, page, None, now_secs())
                .await
            {
                tracing::warn!(%new_id, error = %e, "could not restore the re-downloaded archive's progress");
            }
        }
    }

    // The patch came from the file that was just replaced, so it belongs beside the new one. Taken
    // out of the bin rather than copied: the still-recycled archive keeps everything else, and a
    // second copy of the same pages would only drift.
    if plan.patch && !new_archive.file.trim().is_empty() {
        let destination =
            lanrurugi_scanner::patch::patch_path_for(std::path::Path::new(&new_archive.file));
        crate::recycle::take_patch_out_of_bin(state, &ArchiveId(plan.old_id.clone()), &destination)
            .await;
    }

    // Tanks whose member list still names the old ID get the new one in the same slot; a member
    // deleted from the tank in the meantime is left deleted.
    let delta = i64::from(new_archive.pagecount) - i64::from(plan.pagecount);
    for slot in &plan.tanks {
        let tank_id = TankId(slot.tank_id.clone());
        let mut chapters: Vec<ChapterNameEntry> = Vec::new();
        if let Some(name) = &slot.chapter_name {
            chapters.push(ChapterNameEntry {
                id: new_id.to_string(),
                name: name.clone(),
            });
        }
        let before = plan.pagecount;
        let start = slot.start_page;
        let new_pagecount = new_archive.pagecount;
        let old_id = plan.old_id.clone();
        let new_id_owned = new_id.to_string();
        let result = state
            .repos
            .groupings
            .update(&tank_id, move |g| {
                // A member taken out of the tank meanwhile stays out, and its page marker is no
                // longer this archive's business.
                let Some(pos) = g.archives.iter().position(|a| a.as_str() == old_id) else {
                    return;
                };
                g.archives[pos] = ArchiveId(new_id_owned.clone());
                if let Some(entry) = chapters.first() {
                    match g.chapter_names.iter_mut().find(|c| c.id == old_id) {
                        Some(existing) => {
                            existing.id = entry.id.clone();
                            existing.name = entry.name.clone();
                        }
                        None => g.chapter_names.push(entry.clone()),
                    }
                }
                // Recycling moved a marker that sat inside this archive to a neighbour. Put it back
                // only while it is still exactly there: anything else is reading done since the
                // delete, which a snapshot from back then must not undo.
                let was_inside =
                    slot.progress >= start && slot.progress < start.saturating_add(before);
                let parked = crate::tank_members::recycled_marker(start, pos);
                if was_inside && g.progress == parked {
                    g.progress = slot
                        .progress
                        .min(start.saturating_add(new_pagecount).saturating_sub(1));
                } else {
                    g.progress = shifted_progress(g.progress, start, before, delta);
                }
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(tank = %slot.tank_id, error = %e, "could not put the re-downloaded archive back into its Tankoubon");
        }
    }
}

/// Moves a tank's own page marker by `delta` when the replaced member's page count changed. Pages
/// up to the member's last page keep their number; everything from the next member on moves.
fn shifted_progress(progress: u32, start_page: u32, old_pagecount: u32, delta: i64) -> u32 {
    // The first page of the next member.
    let after = start_page.saturating_add(old_pagecount);
    if progress >= after {
        (i64::from(progress) + delta).max(0) as u32
    } else {
        progress
    }
}

async fn take(state: &AppState, queue_item_id: &str) -> Option<Plan> {
    let mut conn = state.redis.config.get().await.ok()?;
    let raw: Option<String> = redis_hget(&mut conn, KEY, queue_item_id).await.ok()?;
    let raw = raw?;
    let _: Result<(), _> = redis_hdel(&mut conn, KEY, queue_item_id).await;
    serde_json::from_str(&raw).ok()
}

async fn redis_hget(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    field: &str,
) -> Result<Option<String>, deadpool_redis::redis::RedisError> {
    use deadpool_redis::redis::AsyncCommands;
    conn.hget(key, field).await
}

async fn redis_hdel(
    conn: &mut deadpool_redis::Connection,
    key: &str,
    field: &str,
) -> Result<(), deadpool_redis::redis::RedisError> {
    use deadpool_redis::redis::AsyncCommands;
    conn.hdel(key, field).await
}

#[cfg(test)]
mod tests {
    use super::shifted_progress;

    /// A member taken out of the tank while it was being downloaded again stays out, and the tank's
    /// marker is not touched.
    #[tokio::test]
    async fn a_member_removed_meanwhile_is_not_put_back() {
        use lanrurugi_core::entities::Grouping;
        use lanrurugi_core::ids::{ArchiveId, TankId};
        let Some(state) = crate::plugins::tests::test_state().await else {
            return;
        };
        let old_id = "3032".repeat(10);
        let other = ArchiveId("3033".repeat(10));
        let fresh = lanrurugi_core::entities::Archive {
            id: ArchiveId("3034".repeat(10)),
            name: "n".into(),
            title: "t".into(),
            file: String::new(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 12,
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
        };
        state
            .repos
            .archives
            .create_or_replace(&fresh)
            .await
            .unwrap();
        let tank_id = TankId("TANK_3020000005".to_string());
        state
            .repos
            .groupings
            .create_or_replace(&Grouping {
                tankid: tank_id.clone(),
                name: String::new(),
                summary: String::new(),
                tags: String::new(),
                progress: 9,
                lastreadtime: None,
                archives: vec![other.clone()],
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Vec::new(),
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();
        super::store(
            &state,
            "queue-3020",
            &super::Plan {
                old_id: old_id.clone(),
                progress: 0,
                pagecount: 10,
                tanks: vec![super::TankSlot {
                    tank_id: tank_id.to_string(),
                    chapter_name: None,
                    start_page: 1,
                    progress: 3,
                }],
                patch: false,
                created_at: super::now_secs(),
            },
        )
        .await;
        super::apply(&state, "queue-3020", &fresh.id).await;
        let tank = state.repos.groupings.get(&tank_id).await.unwrap().unwrap();
        assert_eq!((tank.progress, tank.archives), (9, vec![other]));
        let _ = state.repos.groupings.delete(&tank_id).await;
        let _ = state.repos.archives.delete(&fresh.id).await;
    }

    #[test]
    fn the_next_members_first_page_moves_too() {
        // Member spans 100..=119; page 120 is the next member's first page.
        assert_eq!(shifted_progress(120, 100, 20, -5), 115);
        assert_eq!(shifted_progress(119, 100, 20, -5), 119);
    }

    #[test]
    fn pages_after_the_member_move_by_the_difference() {
        // Member starts at page 100 and used to be 20 pages long; it is now 5 pages shorter.
        assert_eq!(shifted_progress(150, 100, 20, -5), 145);
    }

    #[test]
    fn pages_inside_or_before_the_member_stay() {
        // The member spans 100..=119.
        assert_eq!(shifted_progress(100, 100, 20, -5), 100);
        assert_eq!(shifted_progress(119, 100, 20, -5), 119);
    }

    #[test]
    fn a_longer_archive_pushes_later_pages_forward() {
        assert_eq!(shifted_progress(150, 100, 20, 7), 157);
    }
}
