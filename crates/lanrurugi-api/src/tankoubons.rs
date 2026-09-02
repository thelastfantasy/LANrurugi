//! `tankoubons` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Tankoubon.pm` (constitution Principle II). As with `archives.rs`, `GET
//! /tankoubons/{id}/progress/{page}` doesn't exist in the legacy contract (PUT-only,
//! `update_tank_progress`) — implemented as PUT only.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use futures_util::future::join_all;
use lanrurugi_core::entities::Grouping;
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::archives::ArchiveMetadataJson;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found, ok};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

/// Matches legacy's default `archives_per_page` (verified: `ServerInfo` example in
/// `tools/openapi.yaml`).
const DEFAULT_PAGE_SIZE: usize = 100;

/// Computes the `sync_tank_membership` deltas for a tank's archive-list change from `old` to
/// `new`, filtering `left` down to archives not still a member of some *other* Tankoubon (see
/// `sync_tank_membership`'s own docs for why — an archive can belong to more than one tank at
/// once). `other_groupings` is every Tankoubon except the one being changed (caller fetches this
/// via `GroupingRepository::list_all`, same O(n) scan `list_tankoubons` already does — this only
/// runs on the much rarer archive-list-edit path, not a page-load path).
fn tank_membership_delta(
    old: &[ArchiveId],
    new: &[ArchiveId],
    other_groupings: &[Grouping],
) -> (Vec<String>, Vec<String>) {
    let old_set: HashSet<&ArchiveId> = old.iter().collect();
    let new_set: HashSet<&ArchiveId> = new.iter().collect();
    let joined: Vec<String> = new_set.difference(&old_set).map(|a| a.0.clone()).collect();
    let left: Vec<String> = old_set
        .difference(&new_set)
        .filter(|a| !other_groupings.iter().any(|g| g.archives.contains(a)))
        .map(|a| a.0.clone())
        .collect();
    (joined, left)
}

async fn other_groupings(state: &AppState, exclude: &TankId) -> Vec<Grouping> {
    state
        .repos
        .groupings
        .list_all()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|g| &g.tankid != exclude)
        .collect()
}

fn tankoubon_summary_json(g: &Grouping) -> serde_json::Value {
    json!({
        "id": g.tankid,
        "name": g.name,
        "summary": g.summary,
        "tags": g.tags,
        "archives": g.archives,
        "progress": g.progress,
        "chapter_names": g.chapter_names,
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/tankoubons",
            get(list_tankoubons).put(create_or_rename_tankoubon),
        )
        .route(
            "/tankoubons/{id}",
            get(get_tankoubon)
                .put(update_tankoubon)
                .delete(delete_tankoubon),
        )
        .route("/tankoubons/{id}/full", get(get_tankoubon_full))
        .route(
            "/tankoubons/{id}/thumbnail",
            get(get_tankoubon_thumbnail).put(update_tankoubon_thumbnail),
        )
        .route(
            "/tankoubons/{id}/progress/{page}",
            put(update_tankoubon_progress),
        )
        .route(
            "/tankoubons/{id}/{archive}",
            put(add_to_tankoubon).delete(remove_from_tankoubon),
        )
        .route(
            "/tankoubons/{id}/ai/rename-suggestions",
            axum::routing::post(ai_rename_suggestions),
        )
        .route(
            "/tankoubons/{id}/ai/rename-chapter",
            axum::routing::post(ai_rename_chapter),
        )
}

#[derive(Debug, Deserialize)]
pub struct PageParam {
    page: Option<i64>,
}

async fn list_tankoubons(State(state): State<AppState>, Query(q): Query<PageParam>) -> Response {
    match state.repos.groupings.list_all().await {
        Ok(mut groupings) => {
            groupings.sort_by(|a, b| a.tankid.cmp(&b.tankid));
            let total = groupings.len();
            let page = q.page.unwrap_or(0);
            let items: Vec<_> = if page < 0 {
                groupings.iter().map(tankoubon_summary_json).collect()
            } else {
                let start = (page as usize) * DEFAULT_PAGE_SIZE;
                groupings
                    .iter()
                    .skip(start)
                    .take(DEFAULT_PAGE_SIZE)
                    .map(tankoubon_summary_json)
                    .collect()
            };
            let filtered = items.len();
            axum::Json(json!({ "result": items, "total": total, "filtered": filtered }))
                .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_tankoubon_list",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTankoubonParams {
    name: String,
    tankid: Option<String>,
}

async fn create_or_rename_tankoubon(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Form(params): axum::Form<CreateTankoubonParams>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (tankid, mut grouping, is_new) = match params.tankid {
        Some(id) => {
            let id = TankId(id);
            match state.repos.groupings.get(&id).await {
                Ok(Some(g)) => (id, g, false),
                Ok(None) => {
                    return not_found(
                        "create_tankoubon",
                        format!("{id} doesn't exist in the database!"),
                    )
                }
                Err(e) => {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "create_tankoubon",
                        e.to_string(),
                    )
                }
            }
        }
        None => {
            let mut candidate = TankId(format!("TANK_{now}"));
            let mut attempt = now;
            while state
                .repos
                .groupings
                .get(&candidate)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                attempt += 1;
                candidate = TankId(format!("TANK_{attempt}"));
            }
            (
                candidate.clone(),
                Grouping {
                    tankid: candidate,
                    name: String::new(),
                    summary: String::new(),
                    tags: String::new(),
                    progress: 0,
                    archives: Vec::new(),
                    thumbnail_manual: false,
                    thumbnail_source_archive: None,
                    thumbnail_source_page: None,
                    chapter_names: Default::default(),
                    created_at: Some(now),
                    updated_at: Some(now),
                },
                true,
            )
        }
    };

    let old_name = grouping.name.clone();
    grouping.name = params.name;
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            // Legacy's own `create_tankoubon` (`Model/Tankoubon.pm`) unconditionally writes the
            // tank's (possibly brand-new) name into `LRR_TITLES` here — without it, a Tankoubon
            // never survives the default `sortby=title` search path's own `LRR_TITLES ∩ filtered`
            // step, staying invisible even after `TANKGROUPED_KEY` correctly lists it as a
            // candidate.
            if old_name != grouping.name {
                if let Err(e) = lanrurugi_search::indexer::update_title_index(
                    &state.redis.search,
                    &tankid,
                    &old_name,
                    &grouping.name,
                )
                .await
                {
                    tracing::warn!(%tankid, error = %e, "failed to update tank title search index");
                }
            }
            // Added once, unconditionally of member count, only on genuine creation (not a rename
            // of an existing tank, which already has its own id indexed) — see
            // `sync_tank_membership`'s own docs for why this must not depend on `archives` ever
            // becoming non-empty.
            if is_new {
                if let Err(e) =
                    lanrurugi_search::indexer::add_tank_to_index(&state.redis.search, &tankid).await
                {
                    tracing::warn!(%tankid, error = %e, "failed to add new tank to search index");
                }
            }
            let target = ActivityTarget {
                id: Some(tankid.0.clone()),
                label: Some(grouping.name.clone()),
                kind: Some("tankoubon".to_string()),
            };
            if is_new {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_CREATE,
                    target,
                    Outcome::Success,
                    None,
                    None,
                )
                .await;
            } else if old_name != grouping.name {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_RENAME,
                    target,
                    Outcome::Success,
                    Some(json!({ "name": old_name })),
                    Some(json!({ "name": grouping.name })),
                )
                .await;
            }
            axum::Json(json!({
                "operation": "create_tankoubon",
                "tankoubon_id": tankid,
                "success": 1,
            }))
            .into_response()
        }
        Err(e) => {
            // A real Redis write failure on the actual create/rename attempt — pick the same
            // action type the success branch above would have used, so a failed create and a
            // failed rename aren't indistinguishable in the log.
            let action_type = if is_new {
                action_types::TANKOUBON_CREATE
            } else {
                action_types::TANKOUBON_RENAME
            };
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_type,
                ActivityTarget {
                    id: Some(tankid.0.clone()),
                    label: Some(grouping.name.clone()),
                    kind: Some("tankoubon".to_string()),
                },
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "create_tankoubon",
                e.to_string(),
            )
        }
    }
}

async fn get_tankoubon(State(state): State<AppState>, Path(id): Path<TankId>) -> Response {
    match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => axum::Json(tankoubon_summary_json(&g)).into_response(),
        Ok(None) => not_found(
            "get_tankoubon",
            format!("{id} doesn't exist in the database!"),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_tankoubon",
            e.to_string(),
        ),
    }
}

async fn get_tankoubon_full(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    Query(q): Query<PageParam>,
) -> Response {
    let grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "get_tankoubon_full",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_tankoubon_full",
                e.to_string(),
            )
        }
    };

    let total = grouping.archives.len();
    let page = q.page.unwrap_or(-1);
    let page_ids: Vec<ArchiveId> = if page < 0 {
        grouping.archives.clone()
    } else {
        let start = (page as usize) * DEFAULT_PAGE_SIZE;
        grouping
            .archives
            .iter()
            .skip(start)
            .take(DEFAULT_PAGE_SIZE)
            .cloned()
            .collect()
    };

    let mut full_data = Vec::with_capacity(page_ids.len());
    for id in &page_ids {
        if let Ok(Some(a)) = state.repos.archives.get(id).await {
            full_data.push(ArchiveMetadataJson::from(a));
        }
    }
    let filtered = page_ids.len();

    axum::Json(json!({
        "result": {
            "id": grouping.tankid,
            "name": grouping.name,
            "summary": grouping.summary,
            "tags": grouping.tags,
            "progress": grouping.progress,
            "archives": page_ids,
            "chapter_names": grouping.chapter_names,
            "full_data": full_data,
        },
        "total": total,
        "filtered": filtered,
    }))
    .into_response()
}

const PLACEHOLDER_THUMBNAIL: &[u8] = include_bytes!("../assets/no_thumb.png");

/// A legitimate tankoubon ID is always `TANK_` followed by exactly 10 ASCII digits
/// (`lanrurugi_storage::repository::GroupingRepository`, verified via its own `TANK_??????????`
/// Redis key-scan glob) — anything else must never be used to build a filesystem path. `pub(crate)`
/// (not private) since `categories.rs` needs the exact same shape check to tell a Tankoubon id
/// apart from an archive id when validating category membership.
pub(crate) fn is_valid_tankoubon_id(id: &str) -> bool {
    id.strip_prefix("TANK_")
        .is_some_and(|rest| rest.len() == 10 && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Serves the tank cover thumbnail (legacy path `<thumb_dir>/TA/<id>.<ext>`), regenerating it
/// on-demand from the tank's first member archive on a cache miss (a brand-new Tankoubon has never
/// had one written, since nothing calls [`update_tankoubon_thumbnail`] automatically) rather than
/// falling straight back to the placeholder.
///
/// Legacy's own `serve_tankoubon_thumbnail` (`Model/Tankoubon.pm`) handles this same cache-miss
/// case by enqueueing a Minion background job and returning 202 — the caller has to notice the
/// job finished and re-request (in practice: reload the page) before a real thumbnail ever shows
/// up, so a freshly-created Tankoubon visibly shows the gray placeholder until someone happens to
/// reload. This port's single-archive thumbnail endpoint (`archives::get_archive_thumbnail`)
/// already solved the equivalent problem by generating synchronously, in-request, the first time
/// it's needed (bounded via [`AppState::thumbnail_singleflight`]) — mirroring that here is a
/// strictly better user experience than porting legacy's own job-queue-and-202 flow, not just a
/// simplification, since the thumbnail is already correct on the very first paint.
async fn get_tankoubon_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
) -> Response {
    // See `is_valid_tankoubon_id`'s docs: this endpoint (like `archives::get_archive_thumbnail`)
    // builds a filesystem path directly from `id` without a repository lookup first, so an
    // unvalidated value containing `/` or `..` would otherwise escape `thumb_dir`.
    if !is_valid_tankoubon_id(id.as_str()) {
        return ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response();
    }
    if let Some((content_type, bytes)) =
        read_tankoubon_thumbnail_from_disk(&state, id.as_str()).await
    {
        return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    if let Some((content_type, bytes)) = regenerate_tankoubon_thumbnail_on_demand(&state, &id).await
    {
        return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response()
}

async fn read_tankoubon_thumbnail_from_disk(
    state: &AppState,
    id: &str,
) -> Option<(&'static str, bytes::Bytes)> {
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state
            .library
            .thumb_dir
            .join("TA")
            .join(format!("{id}.{}", format.extension()));
        if let Ok(contents) = tokio::fs::read(&path).await {
            return Some((format.content_type(), bytes::Bytes::from(contents)));
        }
    }
    None
}

async fn remove_tankoubon_thumbnail_files(state: &AppState, id: &str) {
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state
            .library
            .thumb_dir
            .join("TA")
            .join(format!("{id}.{}", format.extension()));
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// Extracts `local_page` of `archive` and writes it straight to the tank's own cover thumbnail
/// path (`<thumb_dir>/TA/<id>.<ext>>`), without touching that archive's own thumbnail — the one
/// piece of work every tank-cover write path (`regenerate_tankoubon_thumbnail_on_demand`,
/// `update_tankoubon_thumbnail`, `sync_tankoubon_thumbnail_with_first_archive`) needs, so it's
/// factored out rather than repeated three times.
async fn generate_tankoubon_cover(
    state: &AppState,
    id: &str,
    archive: &lanrurugi_core::entities::Archive,
    local_page: u32,
) -> Result<std::path::PathBuf, String> {
    let thumb_settings = match state.redis.config.get().await {
        Ok(mut conn) => lanrurugi_scanner::thumbnail::read_settings(&mut conn).await,
        Err(e) => return Err(e.to_string()),
    };
    let dir = state.library.thumb_dir.join("TA");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let output = dir.join(format!("{id}.{}", thumb_settings.format.extension()));
    lanrurugi_scanner::thumbnail::generate(
        std::path::PathBuf::from(&archive.file),
        local_page as usize,
        output.clone(),
        thumb_settings.format,
        thumb_settings.quality,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(output)
}

/// Picks the tank's first member archive (reading order — matches legacy's own
/// `zrangebyscore($tank_id, 1, '+inf', 'LIMIT', 0, 1)` in its `tank_thumbnail_task` Minion job) and
/// extracts *its* cover as the tank's own cover. `None` (falls through to the placeholder) for a
/// not-found or empty Tankoubon, or if the picked archive itself can't be read/decoded.
async fn regenerate_tankoubon_thumbnail_on_demand(
    state: &AppState,
    id: &TankId,
) -> Option<(&'static str, bytes::Bytes)> {
    let grouping = state.repos.groupings.get(id).await.ok().flatten()?;
    let first_arc_id = grouping.archives.first()?;
    let archive = state
        .repos
        .archives
        .get(first_arc_id)
        .await
        .ok()
        .flatten()?;

    state
        .thumbnail_singleflight
        .run(format!("tank:{id}"), {
            let state = state.clone();
            let id = id.clone();
            move || async move {
                match generate_tankoubon_cover(&state, id.as_str(), &archive, 1).await {
                    Ok(_) => read_tankoubon_thumbnail_from_disk(&state, id.as_str()).await,
                    Err(e) => {
                        tracing::warn!(%id, error = %e, "on-demand tank thumbnail generation failed");
                        None
                    }
                }
            }
        })
        .await
}

/// Keeps a tank's cover following its first member archive whenever the archive list changes —
/// but only while [`Grouping::thumbnail_manual`] is still `false`, *and* first validates a manual
/// cover's own source is still real (see [`validate_manual_thumbnail_source`]): a cover someone
/// explicitly picked is sticky across ordinary archive-list edits, but if the specific archive
/// (and page) it was extracted from has itself left the tank — or shrunk past that page on a
/// rescan, or been deleted from the library outright — it isn't a valid cover for *this* tank
/// anymore regardless of the sticky rule, and gets reset the same as if no manual pick had ever
/// been made. An empty Tankoubon (no members left, whether from ordinary edits or from a reset
/// above) has nothing to follow, so any stale cached cover is removed instead, falling back to the
/// placeholder like a brand-new tank would.
///
/// Callers pass the *previous* first archive id so the ordinary (non-reset) path only does work
/// when it actually changed — removing archive #3 out of a 5-member tank doesn't need a cover
/// regen, only a change to `archives[0]` does. A reset always regenerates/clears unconditionally —
/// via [`force_apply_first_archive_cover`], not the `old_first`-gated helper, since a stale-manual
/// reset landing on a tank that's *still* empty (its last member was removed at the same time the
/// stale source was noticed) must not be skipped just because "no first archive" didn't change.
async fn sync_tankoubon_thumbnail_with_first_archive(
    state: &AppState,
    grouping: &Grouping,
    old_first: Option<&ArchiveId>,
) {
    if grouping.thumbnail_manual {
        if validate_manual_thumbnail_source(state, grouping).await {
            return;
        }
        let mut reset = grouping.clone();
        reset.thumbnail_manual = false;
        reset.thumbnail_source_archive = None;
        reset.thumbnail_source_page = None;
        if let Err(e) = state.repos.groupings.save(&reset).await {
            tracing::warn!(id = %grouping.tankid, error = %e, "failed to reset stale manual tank thumbnail source");
            return;
        }
        force_apply_first_archive_cover(state, &reset, reset.archives.first()).await;
        return;
    }
    apply_first_archive_cover(state, grouping, old_first).await;
}

/// `true` if a manually-picked cover's recorded source (archive + page) still makes sense for
/// `grouping`'s *current* member list — the archive is still a member, it still exists at all, and
/// its own current page count still covers the recorded page (checked live, not against a stored
/// snapshot, since a rescan can shrink a page count out from under an old pick). Only ever called
/// while `grouping.thumbnail_manual` is already `true` (the caller's own gate) — a *missing*
/// `thumbnail_source_archive` in that state means "manual, but not verifiable" rather than
/// "nothing to check": a real, real data state (not just theoretical) for any tank that had a
/// cover set manually before this field existed, which must resolve to `false` here so it
/// self-heals into the fully-tracked shape on its next archive-list edit, not stay permanently
/// un-resettable because there's nothing recorded to invalidate.
async fn validate_manual_thumbnail_source(state: &AppState, grouping: &Grouping) -> bool {
    let Some(source_id) = &grouping.thumbnail_source_archive else {
        return false;
    };
    if !grouping.archives.contains(source_id) {
        return false;
    }
    let Some(archive) = state.repos.archives.get(source_id).await.ok().flatten() else {
        return false;
    };
    grouping
        .thumbnail_source_page
        .is_some_and(|p| p >= 1 && p <= archive.pagecount)
}

/// Skips the work entirely if the first archive didn't actually change — the common case for most
/// archive-list edits (removing archive #3 out of a 5-member tank doesn't touch `archives[0]`).
/// See [`force_apply_first_archive_cover`] for the actual work and for the unconditional variant
/// the stale-manual-reset path needs instead.
async fn apply_first_archive_cover(
    state: &AppState,
    grouping: &Grouping,
    old_first: Option<&ArchiveId>,
) {
    let new_first = grouping.archives.first();
    if old_first == new_first {
        return;
    }
    force_apply_first_archive_cover(state, grouping, new_first).await;
}

/// The actual "generate from the current first member, or clear if there is none" work, with no
/// "did it change" gate of its own — always runs. Shared by [`apply_first_archive_cover`] (which
/// adds that gate back for the ordinary auto-follow path) and the just-reset-a-stale-manual-cover
/// path in [`sync_tankoubon_thumbnail_with_first_archive`], which calls this directly: a reset
/// landing on a tank that's still empty (its last member left at the same moment the stale source
/// was noticed) must still clear the cached file even though "no first archive" technically didn't
/// change from the reset snapshot's own perspective.
async fn force_apply_first_archive_cover(
    state: &AppState,
    grouping: &Grouping,
    new_first: Option<&ArchiveId>,
) {
    match new_first {
        Some(first_arc_id) => {
            let Some(archive) = state.repos.archives.get(first_arc_id).await.ok().flatten() else {
                return;
            };
            if let Err(e) =
                generate_tankoubon_cover(state, grouping.tankid.as_str(), &archive, 1).await
            {
                tracing::warn!(id = %grouping.tankid, error = %e, "failed to sync tank thumbnail with new first archive");
            }
        }
        None => remove_tankoubon_thumbnail_files(state, grouping.tankid.as_str()).await,
    }
}

/// Maps a Tankoubon-wide global page number to the real member archive and that archive's own
/// local page number it falls in — matches legacy's `Model::Tankoubon::translate_global_page`.
/// Thin wrapper over `GroupingRepository::resolve_global_page` (the actual walk-and-sum logic now
/// lives there, shared with `bookmarks.rs`/`stamps.rs`'s own Tankoubon routes) — drops the
/// `archive_index` that method also returns (no caller here needs it) and collapses a storage
/// error into `None`, same "a lookup failure just means no match" tolerance the original private
/// version of this function had via its own `.ok().flatten()`.
async fn resolve_global_page(
    state: &AppState,
    archives: &[ArchiveId],
    global_page: u32,
) -> Option<(ArchiveId, u32)> {
    state
        .repos
        .groupings
        .resolve_global_page(&state.repos.archives, archives, global_page)
        .await
        .ok()
        .flatten()
        .map(|(id, page, _index)| (id, page))
}

/// Resolves a Tankoubon id + Tankoubon-global page number down to the real member archive id and
/// that archive's own local page number, looking the Tankoubon itself up first (unlike
/// [`resolve_global_page`] above, which takes an already-fetched `archives` list) — shared by
/// `bookmarks.rs`/`stamps.rs`'s own Tankoubon-page-addressed routes (`/tankoubons/{id}/bookmarks/
/// {page}`, `/tankoubons/{id}/stamps/{page}`), which need this exact "look up the grouping, then
/// resolve the page" sequence and have no `Grouping` of their own to hand in. Returns the
/// `Response` to send back directly (a `404`/`400`/`500`) on any resolution failure, so call sites
/// can `let (archive_id, local_page) = match resolve_tank_page(...).await { Ok(v) => v, Err(r) =>
/// return r };` without duplicating the same three failure branches at every call site.
pub(crate) async fn resolve_tank_page(
    state: &AppState,
    tank_id: &TankId,
    op: &str,
    global_page: u32,
) -> Result<(ArchiveId, u32), Response> {
    let grouping = match state.repos.groupings.get(tank_id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return Err(not_found(
                op,
                format!("{tank_id} doesn't exist in the database!"),
            ))
        }
        Err(e) => return Err(error(StatusCode::INTERNAL_SERVER_ERROR, op, e.to_string())),
    };
    match state
        .repos
        .groupings
        .resolve_global_page(&state.repos.archives, &grouping.archives, global_page)
        .await
    {
        Ok(Some((archive_id, local_page, _index))) => Ok((archive_id, local_page)),
        Ok(None) => Err(error(
            StatusCode::BAD_REQUEST,
            op,
            format!("Page {global_page} is out of range for this tankoubon."),
        )),
        Err(e) => Err(error(StatusCode::INTERNAL_SERVER_ERROR, op, e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateTankoubonThumbnailParams {
    page: Option<u32>,
}

/// Sets the Tankoubon's own cover thumbnail from a page within one of its member archives,
/// addressed by a *global* (Tankoubon-wide, not per-archive) page number — matches legacy's
/// `Model::Tankoubon::update_tankoubon_thumbnail` exactly: resolve the global page via
/// [`resolve_global_page`] and generate that member archive's own page as the cover ([shared with
/// the on-demand/auto-follow paths via [`generate_tankoubon_cover`]).
///
/// Also marks the tank's cover as manually chosen (`thumbnail_manual = true`) — additive beyond
/// legacy, which has no such flag: from this point on, [`sync_tankoubon_thumbnail_with_first_archive`]
/// leaves this cover alone even if the archive list changes later, since the user picked this
/// specific page on purpose.
async fn update_tankoubon_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    Query(params): Query<UpdateTankoubonThumbnailParams>,
) -> Response {
    let page = params.page.unwrap_or(1);
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "update_tankoubon_thumbnail",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tankoubon_thumbnail",
                e.to_string(),
            )
        }
    };

    let Some((arc_id, local_page)) = resolve_global_page(&state, &grouping.archives, page).await
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "update_tankoubon_thumbnail",
            format!("Page {page} is out of range for this tankoubon."),
        );
    };

    let archive = match state.repos.archives.get(&arc_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return not_found(
                "update_tankoubon_thumbnail",
                format!("{arc_id} does not exist."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tankoubon_thumbnail",
                e.to_string(),
            )
        }
    };

    let output = match generate_tankoubon_cover(&state, id.as_str(), &archive, local_page).await {
        Ok(path) => path,
        Err(e) => return error(StatusCode::BAD_REQUEST, "update_tankoubon_thumbnail", e),
    };

    grouping.thumbnail_manual = true;
    grouping.thumbnail_source_archive = Some(arc_id);
    grouping.thumbnail_source_page = Some(local_page);
    if let Err(e) = state.repos.groupings.save(&grouping).await {
        tracing::warn!(%id, error = %e, "failed to persist thumbnail_manual flag");
    }

    ok(
        "update_tankoubon_thumbnail",
        [("new_thumbnail", json!(output.display().to_string()))],
    )
}

async fn update_tankoubon_progress(
    State(state): State<AppState>,
    Path((id, page)): Path<(TankId, u32)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "update_tank_progress",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tank_progress",
                e.to_string(),
            )
        }
    };
    grouping.progress = page;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => axum::Json(json!({
            "id": id,
            "operation": "update_tank_progress",
            "page": page,
            "lastreadtime": now,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_tank_progress",
            e.to_string(),
        ),
    }
}

async fn update_tankoubon(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<UpdateTankoubonBody>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "update_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tankoubon",
                e.to_string(),
            )
        }
    };

    // Captured before `body.metadata` is moved out of below.
    let tags_set_explicitly = body.metadata.as_ref().is_some_and(|m| m.tags.is_some());
    let old_name = grouping.name.clone();
    let old_summary = grouping.summary.clone();
    // `updated_at` is bumped below, but only if something other than a bare rating change
    // actually happened — see that check's own comment for why a rating alone is excluded.
    let old_tags = grouping.tags.clone();
    let mut non_rating_change = false;

    if let Some(metadata) = body.metadata {
        if let Some(name) = metadata.name {
            non_rating_change = true;
            grouping.name = name;
        }
        if let Some(summary) = metadata.summary {
            non_rating_change = true;
            grouping.summary = summary;
        }
        if let Some(tags) = metadata.tags {
            grouping.tags = if metadata.append.unwrap_or(false) && !grouping.tags.is_empty() {
                format!("{},{}", grouping.tags, tags)
            } else {
                tags
            };
        }
        if let Some(chapter_names) = metadata.chapter_names {
            non_rating_change = true;
            grouping.chapter_names = chapter_names;
        }
    }
    let old_archives = grouping.archives.clone();
    let old_first = old_archives.first().cloned();
    if let Some(archives) = body.archives {
        non_rating_change = true;
        let archive_ids: Vec<ArchiveId> = archives.into_iter().map(ArchiveId).collect();
        // Auto-fills the Tankoubon's own tags with whatever's common across all the member
        // archives being set here (e.g. the same `artist:` tag on every volume) — but only when
        // tags haven't been given a real value yet (`grouping.tags.is_empty()`) and this same
        // request isn't itself setting them (`tags_set_explicitly`), so this fires exactly once,
        // right after the frontend's own create-Tankoubon flow (`PUT /tankoubons` then `PUT
        // /tankoubons/{id}` with `archives`) first populates the member list — not on every
        // later archive-list edit of an already-tagged Tankoubon.
        if grouping.tags.is_empty() && !tags_set_explicitly && !archive_ids.is_empty() {
            grouping.tags = common_member_tags(&state, &archive_ids).await;
        }
        grouping.archives = archive_ids;
    }

    // A rating-only change (the reader's star widget, right-click-to-clear included) must not
    // bump `updated_at` — rating a Tankoubon you've already read/sorted shouldn't kick it back to
    // the top of a date-sorted library view the way an actual content edit should. Every OTHER
    // tags change (category/artist/anything but `rating:`) still counts as a real edit. Compares
    // the tags with their `rating:` entries stripped rather than special-casing "only a rating
    // field was in the request body" — a request that changes both rating AND another tag in the
    // same call must still count as an edit.
    if !non_rating_change && strip_rating_tag(&old_tags) != strip_rating_tag(&grouping.tags) {
        non_rating_change = true;
    }
    if non_rating_change {
        grouping.updated_at = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
    }

    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            let others = other_groupings(&state, &id).await;
            let (joined, left) = tank_membership_delta(&old_archives, &grouping.archives, &others);
            if let Err(e) =
                lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &joined, &left)
                    .await
            {
                tracing::warn!(%id, error = %e, "failed to sync tank search index");
            }
            // A Tankoubon's own `tags` (a real, independent field — either auto-filled from common
            // member tags at creation or explicitly set/appended here) were never written into
            // `INDEX_<tag>` under the tank's own id — only member archives' individual `tags` are.
            // Confirmed live, 2026-08-27: a tag genuinely visible on the Tankoubon itself (e.g.
            // `female:high heels`, shown in its own metadata) returned zero search results for that
            // tag, the Tankoubon never among them — "what you see is what you can find" broke
            // silently. This makes tag search actually reach what the tank's own metadata already
            // displays, independent of whether any individual member also happens to carry it.
            if old_tags != grouping.tags {
                if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
                    &state.redis.search,
                    id.as_str(),
                    &old_tags,
                    &grouping.tags,
                )
                .await
                {
                    tracing::warn!(%id, error = %e, "failed to update tank tag search index");
                }
            }
            if old_name != grouping.name {
                if let Err(e) = lanrurugi_search::indexer::update_title_index(
                    &state.redis.search,
                    &id,
                    &old_name,
                    &grouping.name,
                )
                .await
                {
                    tracing::warn!(%id, error = %e, "failed to update tank title search index");
                }
            }
            sync_tankoubon_thumbnail_with_first_archive(&state, &grouping, old_first.as_ref())
                .await;

            // Split the same way `archives.rs::update_archive_metadata` splits
            // `ARCHIVE_RATING_UPDATE` from `ARCHIVE_METADATA_UPDATE` — `non_rating_change` above
            // already carries the exact "was this genuinely more than a rating tweak" signal
            // (computed for the unrelated `updated_at` sort-bump suppression, reused here), so no
            // second detection pass is needed. This endpoint recorded no activity at all before —
            // see `TANKOUBON_METADATA_UPDATE`'s own docs.
            let old_tag_set: std::collections::HashSet<&str> = old_tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            let new_tag_set: std::collections::HashSet<&str> = grouping
                .tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            let tags_added: Vec<&str> = new_tag_set.difference(&old_tag_set).copied().collect();
            let tags_removed: Vec<&str> = old_tag_set.difference(&new_tag_set).copied().collect();
            let target = ActivityTarget {
                id: Some(id.0.clone()),
                label: Some(grouping.name.clone()),
                kind: Some("tankoubon".to_string()),
            };
            if !non_rating_change {
                if !tags_added.is_empty() || !tags_removed.is_empty() {
                    record_manual(
                        &state,
                        auth.as_ref().map(|e| &e.0),
                        action_types::TANKOUBON_RATING_UPDATE,
                        target,
                        Outcome::Success,
                        Some(json!({ "rating": tags_removed.first() })),
                        Some(json!({ "rating": tags_added.first() })),
                    )
                    .await;
                }
            } else {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_METADATA_UPDATE,
                    target,
                    Outcome::Success,
                    Some(json!({ "name": old_name, "summary": old_summary })),
                    Some(json!({
                        "name": grouping.name,
                        "summary": grouping.summary,
                        "tags_added": tags_added,
                        "tags_removed": tags_removed,
                    })),
                )
                .await;
            }

            axum::Json(json!({ "operation": "update_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => {
            // A real Redis save failure on the actual update attempt — reuse the same
            // rating-vs-metadata split the success branch uses so a failed rating tweak and a
            // failed real edit aren't indistinguishable in the log.
            let action_type = if non_rating_change {
                action_types::TANKOUBON_METADATA_UPDATE
            } else {
                action_types::TANKOUBON_RATING_UPDATE
            };
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_type,
                ActivityTarget {
                    id: Some(id.0.clone()),
                    label: Some(grouping.name.clone()),
                    kind: Some("tankoubon".to_string()),
                },
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tankoubon",
                e.to_string(),
            )
        }
    }
}

/// The tag set every one of `archive_ids` shares — e.g. all volumes of the same series sharing
/// the same `artist:` tag. Fetches are parallelized (`join_all`), not a sequential loop — see
/// issue #66 for what that anti-pattern costs when it's on a hot page-load path; this one only
/// runs once per Tankoubon (right after its member list is first populated), but there's no
/// reason to write the same mistake twice. An archive that fails to load contributes nothing (its
/// absence doesn't collapse the whole intersection).
///
/// `date_added:`/`timestamp:` tags are filtered out of each archive's own set before the
/// intersection runs — they're auto-generated bookkeeping (scan time / the source site's own
/// upload time), a fresh, real value per archive, so they could never legitimately be "common"
/// tags anyway. That filtering matters beyond just keeping them out of the result: an archive
/// whose only tags happen to be these (e.g. one the metadata-fetch plugin never ran on) would
/// otherwise contribute an *empty* working set to the intersection and collapse the whole
/// computation to nothing, even when every other member archive shares real tags with each other
/// — a real, observed case (a Tankoubon of otherwise-tagged archives plus one bare "just added"
/// one came back with zero common tags). Such an archive is excluded from the intersection
/// entirely instead, the same as if it had never been given any tags at all.
async fn common_member_tags(state: &AppState, archive_ids: &[ArchiveId]) -> String {
    let tag_sets: Vec<HashSet<String>> = join_all(archive_ids.iter().map(|id| async move {
        let tags = state.repos.archives.get(id).await.ok().flatten()?.tags;
        let set: HashSet<String> = tags
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty() && !is_bookkeeping_tag(t))
            .collect();
        (!set.is_empty()).then_some(set)
    }))
    .await
    .into_iter()
    .flatten()
    .collect();

    let Some((first, rest)) = tag_sets.split_first() else {
        return String::new();
    };
    let mut common: Vec<&String> = first
        .iter()
        .filter(|t| rest.iter().all(|s| s.contains(*t)))
        .collect();
    common.sort();
    common.into_iter().cloned().collect::<Vec<_>>().join(", ")
}

/// `date_added`/`timestamp` — see `common_member_tags`'s own docs for why these are excluded from
/// its intersection.
fn is_bookkeeping_tag(tag: &str) -> bool {
    let namespace = tag.split(':').next().unwrap_or(tag);
    namespace == "date_added" || namespace == "timestamp"
}

/// Sorted, comma-joined tag list with every `rating:`-namespaced entry removed — used by
/// `update_tankoubon`'s `updated_at` guard to tell "only the rating changed" apart from "some
/// other tag changed too", by comparing this against both the pre- and post-update tag strings.
/// Sorted (order-independent) and re-joined without whitespace variance so two tag strings that
/// are semantically identical but differ only in comma-spacing or tag order don't spuriously
/// register as a change.
fn strip_rating_tag(tags: &str) -> String {
    let mut kept: Vec<String> = tags
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter(|t| t.split(':').next().unwrap_or(t) != "rating")
        .map(str::to_string)
        .collect();
    kept.sort();
    kept.join(",")
}

#[derive(Debug, Deserialize)]
pub struct UpdateTankoubonBody {
    archives: Option<Vec<String>>,
    metadata: Option<UpdateTankoubonMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTankoubonMetadata {
    name: Option<String>,
    summary: Option<String>,
    tags: Option<String>,
    append: Option<bool>,
    chapter_names: Option<Vec<lanrurugi_core::entities::ChapterNameEntry>>,
}

async fn delete_tankoubon(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<TankId>,
) -> Response {
    // Fetched before delete purely to know which member archives (if any) need restoring to
    // `TANKGROUPED_KEY`, and the tank's own current name for the title-index cleanup — a missing/
    // already-gone grouping just means nothing to restore/clean up.
    let old = state.repos.groupings.get(&id).await.ok().flatten();
    let old_archives = old.as_ref().map(|g| g.archives.clone()).unwrap_or_default();
    let old_name = old.as_ref().map(|g| g.name.clone()).unwrap_or_default();
    match state.repos.groupings.delete(&id).await {
        Ok(()) => {
            let others = other_groupings(&state, &id).await;
            let (_, left) = tank_membership_delta(&old_archives, &[], &others);
            if let Err(e) =
                lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &[], &left)
                    .await
            {
                tracing::warn!(%id, error = %e, "failed to sync tank search index");
            }
            // Outright deletion is the only thing that removes the tank's own id from the search
            // index — see `sync_tank_membership`'s own docs for why emptying it out must not.
            if let Err(e) =
                lanrurugi_search::indexer::remove_tank_from_index(&state.redis.search, &id).await
            {
                tracing::warn!(%id, error = %e, "failed to remove tank from search index");
            }
            if !old_name.is_empty() {
                if let Err(e) = lanrurugi_search::indexer::remove_title_index(
                    &state.redis.search,
                    &id,
                    &old_name,
                )
                .await
                {
                    tracing::warn!(%id, error = %e, "failed to remove tank title search index");
                }
            }
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::TANKOUBON_DELETE,
                ActivityTarget {
                    id: Some(id.0.clone()),
                    label: Some(old_name).filter(|n| !n.is_empty()),
                    kind: Some("tankoubon".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "delete_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => {
            // A real Redis delete failure on the actual delete attempt (not the earlier best-
            // effort lookup) — the user's delete genuinely didn't take effect.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::TANKOUBON_DELETE,
                ActivityTarget {
                    id: Some(id.0.clone()),
                    label: Some(old_name).filter(|n| !n.is_empty()),
                    kind: Some("tankoubon".to_string()),
                },
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_tankoubon",
                e.to_string(),
            )
        }
    }
}

async fn add_to_tankoubon(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, archive)): Path<(TankId, ArchiveId)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "add_to_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_to_tankoubon",
                e.to_string(),
            )
        }
    };
    if state
        .repos
        .archives
        .get(&archive)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "add_to_tankoubon",
            format!("{archive} does not exist in the database."),
        );
    }
    let old_first = grouping.archives.first().cloned();
    let newly_added = !grouping.archives.contains(&archive);
    if newly_added {
        grouping.archives.push(archive.clone());
    }
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            if newly_added {
                if let Err(e) = lanrurugi_search::indexer::sync_tank_membership(
                    &state.redis.search,
                    std::slice::from_ref(&archive.0),
                    &[],
                )
                .await
                {
                    tracing::warn!(%id, %archive, error = %e, "failed to sync tank search index");
                }
                sync_tankoubon_thumbnail_with_first_archive(&state, &grouping, old_first.as_ref())
                    .await;
            }
            if newly_added {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_MEMBER_ADD,
                    ActivityTarget {
                        id: Some(id.0.clone()),
                        label: Some(grouping.name.clone()).filter(|n| !n.is_empty()),
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(json!({ "archive": archive.0 })),
                )
                .await;
            }
            axum::Json(json!({ "operation": "add_to_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => {
            // Only worth recording when this was actually a real member-add attempt (`newly_added`
            // — a no-op re-add that failed to save is uninteresting, same triviality reasoning
            // legacy would apply). A real Redis save failure here is a genuine failed add.
            if newly_added {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_MEMBER_ADD,
                    ActivityTarget {
                        id: Some(id.0.clone()),
                        label: None,
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    None,
                )
                .await;
            }
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_to_tankoubon",
                e.to_string(),
            )
        }
    }
}

async fn remove_from_tankoubon(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, archive)): Path<(TankId, ArchiveId)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "remove_from_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_from_tankoubon",
                e.to_string(),
            )
        }
    };
    let old_first = grouping.archives.first().cloned();
    let was_present = grouping.archives.contains(&archive);
    grouping.archives.retain(|a| a != &archive);
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            if was_present {
                let others = other_groupings(&state, &id).await;
                let still_in_another_tank = others.iter().any(|g| g.archives.contains(&archive));
                let left: Vec<String> = if still_in_another_tank {
                    Vec::new()
                } else {
                    vec![archive.0.clone()]
                };
                if let Err(e) =
                    lanrurugi_search::indexer::sync_tank_membership(&state.redis.search, &[], &left)
                        .await
                {
                    tracing::warn!(%id, %archive, error = %e, "failed to sync tank search index");
                }
                sync_tankoubon_thumbnail_with_first_archive(&state, &grouping, old_first.as_ref())
                    .await;
            }
            if was_present {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_MEMBER_REMOVE,
                    ActivityTarget {
                        id: Some(id.0.clone()),
                        label: Some(grouping.name.clone()).filter(|n| !n.is_empty()),
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Success,
                    Some(json!({ "archive": archive.0 })),
                    None,
                )
                .await;
            }
            axum::Json(json!({ "operation": "remove_from_tankoubon", "success": 1 }))
                .into_response()
        }
        Err(e) => {
            // Same "only a real attempted change" gating as `add_to_tankoubon`'s own failure
            // record — `was_present` means the archive really was being removed, not a no-op.
            if was_present {
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::TANKOUBON_MEMBER_REMOVE,
                    ActivityTarget {
                        id: Some(id.0.clone()),
                        label: None,
                        kind: Some("tankoubon".to_string()),
                    },
                    Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    None,
                )
                .await;
            }
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_from_tankoubon",
                e.to_string(),
            )
        }
    }
}

/// POST /api/tankoubons/{id}/ai/rename-suggestions
///
/// Sends the tankoubon title, member archive names, and order to DeepSeek and returns
/// AI-generated rename suggestions (tankoubon name + per-archive chapter names).
/// The LLM key must be configured; returns 400 if not.
async fn ai_rename_suggestions(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::TankId>,
) -> Response {
    let tank = match state.repos.groupings.get(&id).await {
        Ok(Some(t)) => t,
        Ok(None) => return not_found("ai_rename", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ai_rename",
                e.to_string(),
            )
        }
    };

    // Collect member archive titles in order
    let mut members: Vec<(String, String)> = Vec::new();
    for member_id in &tank.archives {
        if let Ok(Some(archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(member_id.to_string()))
            .await
        {
            members.push((member_id.to_string(), archive.title.clone()));
        }
    }
    if members.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "ai_rename",
            "Tankoubon has no member archives.",
        );
    }

    let member_list: Vec<String> = members
        .iter()
        .enumerate()
        .map(|(i, (_id, title))| format!("{}. {}", i + 1, title))
        .collect();

    let system = "你是一个漫画/同人志系列的命名助手。给定一个编号的档案列表，你需要：\n\
         1. 根据标题中的卷号/话数信息（日文「巻の壱」「巻の弐」、中文「第一卷」、英文 Vol.1 等）分析正确的阅读顺序\n\
         2. 为这个系列提供至少 2 个候选名称\n\
         3. 为每个档案建议一个章节标题\n\n\
         只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
         ```typescript\n\
         interface AiResponse {\n\
           suggestions: {\n\
             tank_name: string\n\
             // chapter entries — one per input archive, reordered by correct reading order\n\
             chapters: {\n\
               original_index: number  // matches the input list number (1-based)\n\
               sorted_index: number    // position in correct reading order (1-based)\n\
               name: string            // suggested chapter title, preserve volume/chapter numbers\n\
             }[]\n\
           }[]\n\
         }\n\
         ```\n\n\
         示例输出（json）：\n\
         {\"suggestions\":[{\"tank_name\":\"系列名\",\"chapters\":[{\"original_index\":3,\"sorted_index\":1,\"name\":\"卷一\"},{\"original_index\":1,\"sorted_index\":2,\"name\":\"卷二\"}]},{\"tank_name\":\"系列名 完全版\",\"chapters\":[{\"original_index\":1,\"sorted_index\":1,\"name\":\"第一章\"},{\"original_index\":3,\"sorted_index\":2,\"name\":\"第二章\"}]}]}".to_string();
    let user = format!(
        "当前单行本名称：{}

成员档案（编号在前）：
{}

偏好原始作品语言。",
        tank.name,
        member_list.join(
            "
"
        ),
    );

    #[derive(serde::Deserialize)]
    struct AiResponse {
        suggestions: Vec<RenameSuggestion>,
    }

    #[derive(serde::Deserialize, serde::Serialize)]
    struct RenameSuggestion {
        tank_name: String,
        chapters: Vec<ChapterEntry>,
    }

    #[derive(serde::Deserialize, serde::Serialize)]
    struct ChapterEntry {
        original_index: u32,
        sorted_index: u32,
        name: String,
    }

    match lanrurugi_llm::json_chat::<AiResponse>(
        &state.redis.config,
        &system,
        &user,
        0.7,
        2000,
    )
    .await
    {
        Ok(response) => axum::Json(serde_json::json!({
            "suggestions": response.suggestions,
            "original_member_names": members.iter().map(|(id, title)|
                serde_json::json!({"id": id, "title": title, "index": members.iter().position(|(mid, _)| mid == id).map(|i| i + 1).unwrap_or(0)})
            ).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(msg) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "ai_rename",
            msg,
        ),
    }
}

/// POST /api/tankoubons/{id}/ai/rename-chapter
///
/// Renames a single member archive's chapter name via AI, given the full series
/// context (other members' titles and any already-set chapter names). Returns a
/// single `{name}` suggestion.
#[derive(Debug, Deserialize)]
struct RenameChapterBody {
    archive_index: usize,
}

async fn ai_rename_chapter(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    axum::Json(body): axum::Json<RenameChapterBody>,
) -> Response {
    let tank = match state.repos.groupings.get(&id).await {
        Ok(Some(t)) => t,
        Ok(None) => return not_found("ai_rename_chapter", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ai_rename_chapter",
                e.to_string(),
            )
        }
    };

    // Index is 1-based from the frontend (matches `original_member_names[].index`)
    let idx = body.archive_index;
    if idx < 1 || idx > tank.archives.len() {
        return error(
            StatusCode::BAD_REQUEST,
            "ai_rename_chapter",
            format!(
                "archive_index {idx} out of range (1–{})",
                tank.archives.len()
            ),
        );
    }
    let target_id = &tank.archives[idx - 1];

    let target_archive = match state.repos.archives.get(target_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return not_found(
                "ai_rename_chapter",
                format!("member archive {target_id} does not exist."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ai_rename_chapter",
                e.to_string(),
            )
        }
    };

    // Build context: all members with their current chapter names (if any)
    let mut context = String::new();
    for (i, member_id) in tank.archives.iter().enumerate() {
        let title = if member_id == target_id {
            target_archive.title.clone()
        } else if let Ok(Some(a)) = state.repos.archives.get(member_id).await {
            a.title.clone()
        } else {
            continue;
        };
        let cn = tank
            .chapter_names
            .iter()
            .find(|c| c.id == member_id.as_str())
            .map(|c| format!(" → {}", c.name))
            .unwrap_or_default();
        context.push_str(&format!("{}. {}{}\n", i + 1, title, cn));
    }

    let system = "你是一个漫画/同人志系列的章节命名助手。你会收到系列名称、一个目标档案的标题、以及所有成员档案的上下文（编号从 1 开始，已命名的章节会以 → 显示）。请为目标档案建议一个合适的章节标题，保留卷号/话数信息。只输出 json：\n\n```typescript\ntype RenameChapter = { name: string }\n```\n\n示例输出（json）：\n{\"name\":\"第壱巻\"}".to_string();
    let user = format!(
        "单行本：{}

成员列表（第 {idx} 项是目标）：
{context}

请为第 {idx} 项建议章节标题。偏好原始作品语言。",
        tank.name,
    );

    #[derive(serde::Deserialize)]
    struct ChapterSuggestion {
        name: String,
    }

    match lanrurugi_llm::json_chat::<ChapterSuggestion>(
        &state.redis.config,
        &system,
        &user,
        0.5,
        200,
    )
    .await
    {
        Ok(suggestion) => {
            axum::Json(serde_json::json!({ "name": suggestion.name })).into_response()
        }
        Err(msg) => error(StatusCode::SERVICE_UNAVAILABLE, "ai_rename_chapter", msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_rating_tag_removes_rating_regardless_of_position() {
        assert_eq!(
            strip_rating_tag("category:cosplay,rating:4.5"),
            "category:cosplay"
        );
        assert_eq!(
            strip_rating_tag("rating:4.5,category:cosplay"),
            "category:cosplay"
        );
        assert_eq!(strip_rating_tag("rating:4.5"), "");
        assert_eq!(strip_rating_tag(""), "");
    }

    #[test]
    fn strip_rating_tag_ignores_comma_spacing_and_order() {
        // Order-independent (sorted) and whitespace-independent (trimmed) — a rating-only change
        // submitted with different tag ordering/spacing than what's already stored must still
        // compare as "unchanged" once the rating is stripped from both sides.
        assert_eq!(
            strip_rating_tag("artist:foo, category:cosplay"),
            strip_rating_tag("category:cosplay,artist:foo")
        );
    }

    #[test]
    fn strip_rating_tag_does_not_touch_non_rating_namespaces_with_similar_names() {
        // A namespace like `top_rating:` or a bare tag containing the substring "rating" must
        // NOT be stripped — only an exact `rating` namespace match.
        assert_eq!(strip_rating_tag("top_rating:5"), "top_rating:5");
        assert_eq!(strip_rating_tag("myrating"), "myrating");
    }
}
