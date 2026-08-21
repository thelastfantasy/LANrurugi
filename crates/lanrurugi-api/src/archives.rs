//! `archives` endpoint group. Paths/response shapes verified against `~/LANraragi/tools/openapi.yaml`
//! and `Utils/Database.pm::build_json` (constitution Principle II — the existing contract must not
//! change).
//!
//! Two paths from `tasks.md`'s wording (`GET .../progress/{page}`, `GET .../toc`) don't actually
//! exist as GET in the legacy contract — verified: `/archives/{id}/progress/{page}` is PUT-only
//! (`update_progress`), and `/archives/{id}/toc` is PUT/DELETE-only (`add_toc`/`remove_toc`); ToC
//! content itself is already exposed via `/archives/{id}/metadata`'s `toc` field. This module
//! implements what the verified contract actually has rather than inventing new GET methods.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::{Archive, TocEntry};
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::common::{error, not_found, ok};
use crate::settings::{DEFAULT_READER_QUALITY, DEFAULT_SIZE_THRESHOLD};
use crate::state::FetchedPage;
use crate::AppState;
use lanrurugi_storage::id::ARCHIVE_ID_LEN;
use lanrurugi_storage::keys::{CONFIG_KEY, TOTAL_PAGES_STAT_KEY};

const PLACEHOLDER_THUMBNAIL: &[u8] = include_bytes!("../assets/no_thumb.png");
/// Served by `fetch_page` in place of a page entry name listed in `Archive::corrupted_pages` (see
/// that field's own docs on how/where a page gets flagged) — an actual image, not a 1x1
/// placeholder like [`PLACEHOLDER_THUMBNAIL`], since this fills the reader's *entire page area*
/// and needs to visibly communicate "this page is broken" rather than render as blank space. SVG
/// (not a raster format) so it stays crisp at any page/viewport size without needing its own
/// resize handling.
const CORRUPTED_PAGE_PLACEHOLDER: &[u8] = include_bytes!("../assets/corrupted_page.svg");

/// Computes whether an archive's "new" badge should be *shown*, applying `LRR_CONFIG`'s
/// `newbadgemode` setting (see `settings::STRING_FIELDS`'s `newbadgemode` doc): `until_opened`
/// shows it whenever the flag is set (legacy's own behavior); `until_finished` hides it once
/// `lastreadpage` reaches `pagecount`; a `Nd` window hides it once `date_added` (parsed from the
/// archive's own `date_added:<unix>` tag) is more than N days old, **or** once the archive is
/// finished (same `lastreadpage >= pagecount` threshold `until_finished` uses) — whichever comes
/// first. Without that second condition, an archive read to completion on day one of a `3d`
/// window kept showing 🆕 for the rest of the window while also silently dropping out of the
/// Library homepage's "On Deck" carousel (that carousel's own `hidecompleted` filter only looks
/// at progress, never at `isnew`) — same underlying flag, two independent display rules that used
/// to disagree about whether a finished archive was still "new". An archive without a
/// `date_added` tag is treated as still-new (conservative: the flag was set, there's just no
/// timestamp to age it against). Never mutates the stored flag — the mode decides *display*
/// only; explicit clearing (`DELETE /archives/{id}/isnew`) stays orthogonal, and
/// `lanrurugi_search::engine`'s `newonly` filter applies the same mode so the "New Archives"
/// button and the badges never disagree.
pub(crate) fn effective_isnew(a: &lanrurugi_core::entities::Archive, mode: &str) -> bool {
    if !a.isnew {
        return false;
    }
    match mode {
        "until_opened" => true,
        "until_finished" => a.lastreadpage < a.pagecount,
        _ => {
            if a.pagecount > 0 && a.lastreadpage >= a.pagecount {
                return false;
            }
            let Some(days) = mode.strip_suffix('d').and_then(|d| d.parse::<u64>().ok()) else {
                // Unknown mode — fall back to showing the badge rather than silently hiding it.
                return true;
            };
            let Some(added) = a.tags.split(',').find_map(|t| {
                t.trim()
                    .strip_prefix("date_added:")
                    .and_then(|v| v.parse::<u64>().ok())
            }) else {
                return true;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now.saturating_sub(added) < days * 24 * 60 * 60
        }
    }
}

/// A legitimate archive ID is always a 40-character lowercase-hex SHA-1 digest
/// (`lanrurugi_storage::id::{legacy_id, size_aware_id}`) — anything else (in particular anything
/// containing `/`, `\`, or `.`) cannot be a real ID and must never be used to build a filesystem
/// path.
fn is_valid_archive_id(id: &str) -> bool {
    id.len() == ARCHIVE_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Debug, Serialize)]
pub struct ArchiveMetadataJson {
    pub arcid: String,
    pub title: String,
    pub filename: String,
    pub tags: String,
    pub summary: Option<String>,
    pub isnew: bool,
    pub extension: String,
    pub progress: u32,
    pub pagecount: u32,
    pub lastreadtime: u64,
    pub size: u64,
    pub toc: Vec<TocJson>,
}

#[derive(Debug, Serialize)]
pub struct TocJson {
    pub name: String,
    pub page: u32,
}

impl From<&Archive> for ArchiveMetadataJson {
    fn from(a: &Archive) -> Self {
        Self {
            arcid: a.id.to_string(),
            title: a.title.clone(),
            filename: a.name.clone(),
            tags: a.tags.clone(),
            summary: (!a.summary.is_empty()).then(|| a.summary.clone()),
            isnew: a.isnew,
            extension: a.extension(),
            progress: a.lastreadpage,
            pagecount: a.pagecount,
            lastreadtime: a.lastreadtime,
            size: a.arcsize,
            toc: a
                .toc
                .iter()
                .map(|t| TocJson {
                    name: t.name.clone(),
                    page: t.page,
                })
                .collect(),
        }
    }
}

/// By-value counterpart of the `&Archive` conversion above — for call sites where the caller's
/// own `Archive` isn't needed afterward (e.g. `list_archives` mapping a whole `Vec<Archive>` it
/// never reads again), this moves each field instead of deep-cloning it.
impl From<Archive> for ArchiveMetadataJson {
    fn from(a: Archive) -> Self {
        Self {
            arcid: a.id.to_string(),
            extension: a.extension(),
            isnew: a.isnew,
            progress: a.lastreadpage,
            pagecount: a.pagecount,
            lastreadtime: a.lastreadtime,
            size: a.arcsize,
            summary: (!a.summary.is_empty()).then_some(a.summary),
            title: a.title,
            filename: a.name,
            tags: a.tags,
            toc: a
                .toc
                .into_iter()
                .map(|t| TocJson {
                    name: t.name,
                    page: t.page,
                })
                .collect(),
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/archives",
            get(list_archives).delete(batch_delete_archives),
        )
        .route("/archives/untagged", get(untagged_archives))
        .route(
            "/archives/{id}",
            get(get_archive_deprecated).delete(delete_archive),
        )
        .route(
            "/archives/{id}/metadata",
            get(get_archive_metadata).put(update_archive_metadata),
        )
        .route("/archives/{id}/page", get(get_page))
        .route("/archives/{id}/files", get(get_files))
        .route("/archives/{id}/page-dimensions", get(get_page_dimensions))
        .route(
            "/archives/{id}/files/thumbnails",
            axum::routing::post(generate_page_thumbnails),
        )
        .route(
            "/archives/{id}/thumbnails",
            axum::routing::get(get_page_thumbnails),
        )
        .route(
            "/archives/{id}/thumbnail",
            get(get_archive_thumbnail).put(update_thumbnail),
        )
        .route("/archives/{id}/categories", get(get_archive_categories))
        .route("/archives/{id}/tankoubons", get(get_archive_tankoubons))
        .route(
            "/archives/{id}/toc",
            put(add_toc_entry).delete(remove_toc_entry),
        )
        .route("/archives/{id}/download", get(download_archive))
        .route("/archives/{id}/patch", delete(delete_patch))
        .route("/archives/{id}/rename", put(rename_archive))
        .route(
            "/archives/{id}/isnew",
            put(set_new_flag).delete(clear_new_flag),
        )
        .route("/archives/{id}/progress/{page}", put(update_progress))
        .route("/regen_thumbs", axum::routing::post(regen_thumbs))
}

/// `date_added:<unix_seconds>` lives inside the comma-separated `tags` string, not as its own
/// Redis hash field or `Archive` struct field — same extraction `lanrurugi_search::engine` already
/// does inline at its own two call sites (`newonly` mode's day-window check and `sortby=date_added`
/// itself), duplicated here rather than shared since neither call site takes an `&Archive` (both
/// work off raw Redis hash fields fetched separately). Missing/unparseable → `None`, sorted last
/// (see `list_archives`) rather than defaulting to `0`/`u64::MAX`, which would silently and
/// incorrectly claim to be either the oldest or newest archive in the library.
fn parse_date_added(tags: &str) -> Option<u64> {
    tags.split(',').find_map(|t| {
        t.trim()
            .strip_prefix("date_added:")
            .and_then(|v| v.parse().ok())
    })
}

async fn list_archives(State(state): State<AppState>) -> Response {
    match state.repos.archives.list_all().await {
        Ok(mut archives) => {
            // Ingestion order, newest first — same `sortby=date_added&order=desc` semantics the
            // Library homepage's own default search already uses (`lanrurugi_search::engine`'s
            // `sort_ids`), including that function's own explicit rule that an archive missing
            // `date_added` entirely stays pinned at the very back regardless of sort direction
            // (that engine keeps unkeyed ids in a side list appended after reversing, specifically
            // to avoid flipping them to the *front* under a descending sort — see that function's
            // own comment). `Reverse(Option<u64>)` reproduces the same result from one `sort_by_key`
            // call: `None` sorts as greater than any `Some(_)` once reversed, so an archive with no
            // parseable `date_added` always lands last, and archives that do have one sort newest
            // first — this is the one page in the app that reads `GET /archives` directly rather
            // than going through `/search`'s own `sortby` param (issue #63's own follow-on), so it
            // has to reproduce that ordering itself rather than getting it for free.
            archives.sort_by_key(|a| std::cmp::Reverse(parse_date_added(&a.tags)));
            let json: Vec<ArchiveMetadataJson> = archives.into_iter().map(Into::into).collect();
            axum::Json(json).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_all_archives",
            e.to_string(),
        ),
    }
}

async fn untagged_archives(State(state): State<AppState>) -> Response {
    match state.repos.archives.list_all().await {
        Ok(archives) => {
            let untagged: Vec<String> = archives
                .into_iter()
                .filter(|a| !has_meaningful_tags(&a.tags))
                .map(|a| a.id.into_string())
                .collect();
            axum::Json(untagged).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_untagged_archives",
            e.to_string(),
        ),
    }
}

/// Legacy `update_indexes` treats artist:/parody:/series:/language:/event:/group:/date_added:/
/// timestamp:/source: tags as "basic" and NOT sufficient to count as tagged (verified:
/// `Utils/Database.pm`).
fn has_meaningful_tags(tags: &str) -> bool {
    tags.split(',').any(|t| {
        let t = t.trim().to_ascii_lowercase();
        if t.is_empty() {
            return false;
        }
        let is_basic = [
            "artist:",
            "parody:",
            "series:",
            "language:",
            "event:",
            "group:",
            "date_added:",
            "timestamp:",
            "source:",
        ]
        .iter()
        .any(|ns| t.starts_with(ns));
        !is_basic
    })
}

/// Same `TANK_`-prefix resolution as `search::resolve_search_entry` — before this, a `TANK_` id
/// here always hit `state.repos.archives.get`, which never has a matching key, so a single
/// Tankoubon's own metadata could never be fetched by ID at all (only ever seen indirectly via a
/// search result already containing one). This is the same latent bug the search endpoints had,
/// just on a different endpoint.
async fn get_archive_metadata(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    match crate::search::resolve_search_entry(&state, id.as_str()).await {
        Some(json) => axum::Json(json).into_response(),
        None => not_found("get_archive_metadata", format!("{id} does not exist.")),
    }
}

/// `GET /archives/{id}` — deprecated alias of `/archives/{id}/metadata` (verified: both map to
/// `api-archive#serve_metadata` in the legacy router).
async fn get_archive_deprecated(
    state: State<AppState>,
    path: Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    get_archive_metadata(state, path).await
}

/// Outcome of deleting one archive's DB record + best-effort on-disk/index/cache cleanup — shared
/// by the single-archive [`delete_archive`] handler and [`batch_delete_archives`]'s per-id loop so
/// the two never drift on what "delete an archive" actually does (file removal, search-index
/// cleanup, recommend-cache eviction, stale download-queue entries, thumbnails, sidecar patch).
enum DeleteOneOutcome {
    NotFound,
    Deleted { filename: String },
    Error(String),
}

async fn delete_one_archive(
    state: &AppState,
    id: &lanrurugi_core::ids::ArchiveId,
    auth: Option<&crate::auth_context::AuthContext>,
) -> DeleteOneOutcome {
    let archive = match state.repos.archives.get(id).await {
        Ok(Some(a)) => a,
        Ok(None) => return DeleteOneOutcome::NotFound,
        Err(e) => return DeleteOneOutcome::Error(e.to_string()),
    };
    match state.repos.archives.delete(id).await {
        Ok(()) => {
            // The only success-path log line this handler had before this was added — every other
            // outcome below is a best-effort cleanup failure (`warn!`) or silently discarded
            // (`let _ =`), so a normal, successful delete previously produced zero tracing output
            // at all, leaving no audit trail in `general.log` for what is a real, irreversible,
            // user-triggered destructive action (unlinks the archive's file from disk, not just a
            // DB record).
            tracing::info!(%id, filename = %archive.name, "deleted archive");
            crate::activity::record_manual(
                state,
                auth,
                lanrurugi_storage::activity::action_types::ARCHIVE_DELETE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: Some(id.to_string()),
                    label: Some(archive.name.clone()),
                    kind: Some("archive".to_string()),
                },
                None,
                None,
            )
            .await;
            // Best-effort, matching every other indexer call site in this file (`update_title_index`/
            // `update_tag_indexes` above) — a search-index cleanup failure shouldn't undo an already
            // committed archive deletion, just leave a ghost id behind (logged) for a future rescan
            // to eventually reconcile.
            if let Err(e) = lanrurugi_search::indexer::remove_archive_index(
                &state.redis.search,
                id,
                &archive.title,
                &archive.tags,
            )
            .await
            {
                tracing::warn!(%id, error = %e, "failed to remove deleted archive from search index");
            }
            if let Err(e) = state.recommend_cache.delete_for(id.as_str()).await {
                tracing::warn!(%id, error = %e, "failed to remove deleted archive from recommendation cache");
            }
            // A completed download-queue entry references the archive(s) it produced via
            // `archive_ids` — deleting the archive without also deleting this entry left a
            // "successful download" row in the Upload page's queue pointing at an id that no
            // longer resolves to anything (confirmed live: reported as confusing, expected the
            // queue entry to go too). No reverse index from archive id to queue item exists (a
            // queue entry's `archive_ids` can hold more than one id for a multi-resource
            // download), so this scans the full queue — acceptable here since the queue is orders
            // of magnitude smaller than the archive library and this only runs once per delete,
            // not on any hot read path. Best-effort, same reasoning as the cleanups above.
            match state.download_queue.list_all().await {
                Ok(items) => {
                    let stale: Vec<String> = items
                        .into_iter()
                        .filter(|item| {
                            item.archive_ids
                                .as_ref()
                                .is_some_and(|ids| ids.iter().any(|a| a == id.as_str()))
                        })
                        .map(|item| item.id)
                        .collect();
                    if !stale.is_empty() {
                        if let Err(e) = state.download_queue.delete_many(&stale).await {
                            tracing::warn!(%id, error = %e, "failed to remove deleted archive's download-queue entries");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(%id, error = %e, "failed to scan download queue for deleted archive's entries");
                }
            }
            // Real legacy's own `delete_archive` (`~/LANraragi/lib/LANraragi/Model/Archive.pm`)
            // unconditionally deletes the archive file, its cover thumbnail, and its per-page
            // thumbnail cache directory too — not an opt-in checkbox. This was missing entirely
            // here (confirmed via a real deleted archive whose ~97MB file and DB record's absence
            // still left the file sitting on disk indefinitely), silently accumulating orphaned
            // files on every delete. Best-effort (logged, not propagated) for the same reason as
            // the search-index cleanup above — the DB record is already gone either way.
            if let Err(e) = tokio::fs::remove_file(&archive.file).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%id, file = %archive.file, error = %e, "failed to remove deleted archive's file from disk");
                }
            }
            // A sidecar `.patch.zip` (issue #77's own follow-on design) is associated purely by
            // filename convention (same directory, same stem) — leaving it behind after its
            // target archive is gone would either sit as a dangling orphan forever, or (worse)
            // get silently picked up by some future, differently-content archive that happens to
            // land on the same filename, splicing in pages that were never meant to apply to it.
            // Best-effort, same reasoning as every other cleanup here.
            let patch_path =
                lanrurugi_scanner::patch::patch_path_for(std::path::Path::new(&archive.file));
            if let Err(e) = tokio::fs::remove_file(&patch_path).await {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%id, patch_path = %patch_path.display(), error = %e, "failed to remove deleted archive's patch file from disk");
                }
            }
            let shard = &id[0..2.min(id.len())];
            for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
                let thumb_path = state
                    .library
                    .thumb_dir
                    .join(shard)
                    .join(format!("{id}.{}", format.extension()));
                let _ = tokio::fs::remove_file(&thumb_path).await;
            }
            let pages_dir = state.library.thumb_dir.join(shard).join(id.as_str());
            let _ = tokio::fs::remove_dir_all(&pages_dir).await;
            DeleteOneOutcome::Deleted {
                filename: archive.name,
            }
        }
        Err(e) => DeleteOneOutcome::Error(e.to_string()),
    }
}

async fn delete_archive(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
) -> Response {
    match delete_one_archive(&state, &id, auth.as_ref().map(|e| &e.0)).await {
        DeleteOneOutcome::NotFound => not_found("delete_archive", format!("{id} does not exist.")),
        DeleteOneOutcome::Deleted { filename } => axum::Json(json!({
            "operation": "delete_archive",
            "id": id,
            "filename": filename,
            "success": 1,
        }))
        .into_response(),
        DeleteOneOutcome::Error(e) => error(StatusCode::INTERNAL_SERVER_ERROR, "delete_archive", e),
    }
}

/// `DELETE /archives` (issue #63), body `{ids: [...]}` — same "plural resource, `DELETE` with a
/// JSON body carrying which ones" shape `activity.rs::bulk_delete_activity` (`DELETE /activity`)
/// already established in this codebase, not a `POST .../batch-delete` action-style path; `DELETE`
/// *can* carry a request body per RFC 9110 (silently dropped by some older proxies/clients in
/// practice, but Axum and this project's own frontend both handle it fine, and precedent already
/// exists here). The Batch page's own "delete archives" operation previously fired
/// `DELETE /archives/{id}` once per selected archive from the frontend as a plain sequential loop
/// (no atomicity, no single audit point, and — since each request only ever reported its own
/// pass/fail — no way for the caller to know *which* ids failed without diffing the library
/// before/after). This does the same per-id work (`delete_one_archive`, identical cleanup to the
/// single-delete endpoint) server-side in one request, and reports a per-id result list so the
/// frontend can show exactly what happened rather than assuming full success.
///
/// Session-only (`route_policy.csv` denies every token role and `anonymous`) — deliberately
/// stricter than every other archive-mutating endpoint in this file, all of which a Token (Admin
/// or Guest) can call. A single wrong/leaked API token being able to wipe an arbitrary-sized slice
/// of the library in one request is a materially larger blast radius than any one of the
/// single-target endpoints token access already permits, and this project has no legitimate
/// automation use case for bulk deletion the way it does for e.g. scripted uploads or metadata
/// edits — so the extra restriction has no real workflow it breaks.
async fn batch_delete_archives(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    axum::Json(body): axum::Json<BatchDeleteRequest>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    let mut results = Vec::with_capacity(body.ids.len());
    let mut deleted_count = 0u32;
    for id in &body.ids {
        let outcome = delete_one_archive(&state, id, auth).await;
        results.push(match outcome {
            DeleteOneOutcome::NotFound => BatchDeleteResult {
                id: id.to_string(),
                success: false,
                filename: None,
                error: Some(format!("{id} does not exist.")),
            },
            DeleteOneOutcome::Deleted { filename } => {
                deleted_count += 1;
                BatchDeleteResult {
                    id: id.to_string(),
                    success: true,
                    filename: Some(filename),
                    error: None,
                }
            }
            DeleteOneOutcome::Error(e) => BatchDeleteResult {
                id: id.to_string(),
                success: false,
                filename: None,
                error: Some(e),
            },
        });
    }
    axum::Json(json!({
        "operation": "batch_delete_archives",
        "success": 1,
        "deleted": deleted_count,
        "total": body.ids.len(),
        "results": results,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BatchDeleteRequest {
    ids: Vec<lanrurugi_core::ids::ArchiveId>,
}

#[derive(Debug, Serialize)]
struct BatchDeleteResult {
    id: String,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn get_archive_categories(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    // Issue #67: reverse-index-backed (`CategoryRepository::for_archive`) — no longer a
    // `list_all()` full-library scan + linear `.contains()` check per category.
    match state.repos.categories.for_archive(&id).await {
        Ok(categories) => {
            let matching: Vec<_> = categories
                .into_iter()
                .map(|c| {
                    json!({
                        "id": c.catid,
                        "name": c.name,
                        "pinned": if c.pinned { 1 } else { 0 },
                        "search": c.search,
                        "archives": c.archives,
                    })
                })
                .collect();
            axum::Json(json!({
                "operation": "find_arc_categories",
                "success": 1,
                "categories": matching,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "find_arc_categories",
            e.to_string(),
        ),
    }
}

async fn get_archive_tankoubons(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    // Issue #67: reverse-index-backed (`GroupingRepository::for_archive`) — no longer a
    // `list_all()` full-library scan + linear `.contains()` check per grouping.
    match state.repos.groupings.for_archive(&id).await {
        Ok(groupings) => {
            let ids: Vec<String> = groupings
                .into_iter()
                .map(|g| g.tankid.into_string())
                .collect();
            axum::Json(json!({
                "operation": "find_arc_tankoubons",
                "success": 1,
                "tankoubons": ids,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "find_arc_tankoubons",
            e.to_string(),
        ),
    }
}

/// Reserved chapter/ToC identifiers the frontend's quick-add presets and 0-9 keyboard shortcut
/// send as `title` instead of localized display text (`c1`-`c20` for "Chapter N", `toc` for
/// "Table of Contents") — kept in sync with the frontend's own `RESERVED_TOC_IDENTIFIERS`
/// (`apps/frontend/src/lib/tocValidation.ts`). 20 chapters covers real doujin/manga volume counts
/// comfortably (the frontend's own quick-add `<select>` offers the same range).
fn is_reserved_toc_identifier(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "toc"
        || (2..=3).contains(&lower.len())
            && lower.starts_with('c')
            && lower[1..]
                .parse::<u8>()
                .is_ok_and(|n| (1..=20).contains(&n))
}

#[derive(Debug, Deserialize)]
pub struct TocParams {
    page: u32,
    title: Option<String>,
}

async fn add_toc_entry(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<TocParams>,
) -> Response {
    if id.0.starts_with("TANK_") {
        return error(
            StatusCode::BAD_REQUEST,
            "update_toc",
            "Tankoubon entries do not have their own ToC — use the member archive's ID instead.",
        );
    }
    let Some(title) = params.title else {
        return error(StatusCode::BAD_REQUEST, "update_toc", "title is required.");
    };
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("update_toc", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_toc",
                e.to_string(),
            )
        }
    };
    archive.toc.retain(|t| t.page != params.page);
    // Reserved chapter/ToC identifiers (`c1`-`c20`, `toc`) are meant to be unique per archive —
    // the frontend's quick-add/keyboard-shortcut flows send these instead of localized display
    // text specifically so re-setting "chapter 4" on a new page moves it rather than creating a
    // second, stale "chapter 4" entry at the old page. Plain free-text titles (anything not one
    // of these reserved identifiers) keep the old page-only dedup — a user is free to have two
    // different pages both titled the same custom text if they want.
    if is_reserved_toc_identifier(&title) {
        archive.toc.retain(|t| t.name != title);
    }
    archive.toc.push(TocEntry {
        page: params.page,
        name: title,
    });
    archive.toc.sort_by_key(|t| t.page);
    match state.repos.archives.save(&archive).await {
        Ok(()) => ok(
            "update_toc",
            [(
                "successMessage",
                json!(format!("Added ToC entry for page {}.", params.page)),
            )],
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_toc",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct TocDeleteParams {
    page: u32,
}

async fn remove_toc_entry(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<TocDeleteParams>,
) -> Response {
    if id.0.starts_with("TANK_") {
        return error(
            StatusCode::BAD_REQUEST,
            "remove_toc",
            "Tankoubon entries do not have their own ToC — use the member archive's ID instead.",
        );
    }
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("remove_toc", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_toc",
                e.to_string(),
            )
        }
    };
    archive.toc.retain(|t| t.page != params.page);
    match state.repos.archives.save(&archive).await {
        Ok(()) => ok(
            "remove_toc",
            [(
                "successMessage",
                json!(format!("Removed ToC entry for page {}.", params.page)),
            )],
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "remove_toc",
            e.to_string(),
        ),
    }
}

async fn download_archive(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("download_archive", "No archive ID specified."),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "download_archive",
                e.to_string(),
            )
        }
    };
    // A sidecar `.patch.zip` (issue #77's own follow-on design) means the reader is showing pages
    // that don't exist in the archive file itself — downloading the raw file as-is would silently
    // drop them. `build_merged_zip` returns `None` (repackage cost skipped entirely) whenever
    // there's no patch, or a patch exists but failed to load/apply, in which case this falls
    // through to the same plain-file-read behavior as before patches existed.
    let archive_file = archive.file.clone();
    let merged = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::patch::build_merged_zip(std::path::Path::new(&archive_file))
    })
    .await;
    if let Ok(Ok(Some(bytes))) = merged {
        return ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response();
    }
    match tokio::fs::read(&archive.file).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "download_archive",
            e.to_string(),
        ),
    }
}

/// `DELETE /archives/{id}/patch` — deletes the sidecar `.patch.zip` (if any) and clears the
/// archive's `has_patch` flag. Lets the user undo a patch — e.g. to replace it with pages from a
/// different source after re-running the comparison flow — without touching the archive itself.
async fn delete_patch(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
) -> Response {
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("delete_patch", format!("{id} not found.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_patch",
                e.to_string(),
            )
        }
    };
    let patch_path = lanrurugi_scanner::patch::patch_path_for(std::path::Path::new(&archive.file));
    if patch_path.exists() {
        if let Err(e) = tokio::fs::remove_file(&patch_path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "delete_patch",
                    e.to_string(),
                );
            }
        }
    }
    if archive.has_patch {
        archive.has_patch = false;
        if let Err(e) = state.repos.archives.save(&archive).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_patch",
                e.to_string(),
            );
        }
    }

    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::ARCHIVE_PATCH_DELETE,
        lanrurugi_storage::activity::ActivityTarget {
            id: Some(id.to_string()),
            label: Some(archive.name.clone()),
            kind: Some("archive".to_string()),
        },
        None,
        None,
    )
    .await;

    axum::Json(json!({ "operation": "delete_patch", "success": 1 })).into_response()
}

/// Most filesystems this app is realistically deployed on (ext4, ZFS, APFS, NTFS) cap a single
/// path *component* (not the full path) at 255 bytes — not 255 characters: a CJK filename, common
/// in this library's own real usage, is 3 bytes/char in UTF-8, so this limit bites at 85 characters
/// long before any character-count check would warn. Checked against the *patch* filename (see
/// below), the tighter of the two constraints, not the archive filename directly.
const MAX_FILENAME_BYTES: usize = 255;

#[derive(Debug, Deserialize)]
struct RenameArchiveParams {
    /// The new filename *stem* only, no extension — the Edit page's rename modal renders the
    /// extension as a fixed, non-editable suffix precisely so the backend never has to guess
    /// whether a caller's string already includes one (see `renameArchiveDialog`'s own docs on
    /// why that's a real footgun otherwise: silently doubling `.zip.zip`, or worse, actually
    /// changing the file's apparent type). The real extension is always taken from the archive's
    /// existing `file` path, never from this field.
    stem: String,
}

/// `PUT /archives/{id}/rename` — additive, no legacy equivalent (legacy has no archive-rename
/// feature at all; verified via `~/LANraragi/lib/LANraragi/Controller/*.pm` and `public/js/edit.js`
/// having no `rename` reference anywhere). Renames the archive's on-disk file (not just its stored
/// `title`/`name`), keeping `Archive::file` and `LRR_FILEMAP` in sync — modeled on
/// `download_manager::ingest`'s own staging-file-to-`archive_dir` rename fixup, the closest
/// existing precedent for "the archive's file path changed, update the two places that track it".
///
/// A sidecar `.patch.zip` (see `delete_patch`'s own docs on how it's associated — same directory,
/// same stem, purely by filename convention) is renamed alongside the archive itself: leaving it
/// under the old stem would silently break `patch::patch_path_for`'s lookup for every future
/// request, making an existing patch invisible without deleting anything or erroring — the kind of
/// silent data-loss-adjacent bug that's much worse than a rename this function could instead just
/// reject outright.
async fn rename_archive(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    axum::Json(params): axum::Json<RenameArchiveParams>,
) -> Response {
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("rename_archive", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rename_archive",
                e.to_string(),
            )
        }
    };

    let old_path = std::path::PathBuf::from(&archive.file);
    let extension = old_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    // `sanitize_filename` (built for a whole filename, e.g. `foo.zip`) still does the right thing
    // for a bare stem with no extension — it just strips any directory-traversal component off
    // whatever's in the last path segment, which a dot-less string still has exactly one of.
    let requested_stem = crate::upload::sanitize_filename(params.stem.trim());
    if requested_stem.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_archive",
            "filename cannot be empty".to_string(),
        );
    }

    let new_filename = if extension.is_empty() {
        requested_stem.clone()
    } else {
        format!("{requested_stem}.{extension}")
    };
    let patch_filename = format!("{requested_stem}.patch.zip");
    // Checked against the patch filename, not `new_filename` — `.patch.zip` (10 bytes) is longer
    // than every real archive extension this app's own scanner accepts (`.zip`/`.cbz`/`.rar`/
    // `.7z`/`.pdf`/... — 4-5 bytes), so a stem that just barely fits under 255 bytes with its real
    // extension could still overflow once `.patch.zip` is appended — better to reject the rename
    // outright here than let it silently produce a patch file the OS can't create at all (the
    // archive itself would still succeed, since it's checked separately below, leaving a renamed
    // archive with an now-permanently-unreachable-by-name existing patch).
    if patch_filename.len() > MAX_FILENAME_BYTES {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_archive",
            format!(
                "filename too long ({} bytes once combined with the patch sidecar's own \
                 .patch.zip suffix; {MAX_FILENAME_BYTES} bytes max)",
                patch_filename.len()
            ),
        );
    }
    if new_filename.len() > MAX_FILENAME_BYTES {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_archive",
            format!(
                "filename too long ({} bytes; {MAX_FILENAME_BYTES} bytes max)",
                new_filename.len()
            ),
        );
    }

    if new_filename
        == old_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    {
        // No-op rename (e.g. only whitespace trimmed away) — nothing to do, not an error.
        return axum::Json(
            json!({ "operation": "rename_archive", "success": 1, "filename": new_filename }),
        )
        .into_response();
    }

    // Held across the conflict check, the actual `rename`, and the DB/`LRR_FILEMAP` writes below
    // — the same `FilenameLocks` a concurrent download/upload/watcher-driven ingest would take out
    // on either name (`lanrurugi_scanner::pipeline::run`'s own per-event lock, `download_manager::
    // ingest`'s staging-to-`archive_dir` fixup). Without this, a download that happens to resolve
    // to the exact same target filename this rename is moving *to* (or a watcher event for the
    // *old* name racing this handler before the `fs::rename` below completes) could interleave
    // with this handler's own check-then-write sequence — the same kind of real, observed
    // corruption `FilenameLocks`'s own module docs describe for the two ingest paths it already
    // guards against each other. Two locks, not one, since both names are live for part of this
    // handler's duration; always old-then-new (a fixed order, even though no other caller
    // currently locks two names at once) to rule out a lock-order deadlock if that ever changes.
    let old_filename_str = old_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let _old_guard = state.lock_filename(&old_filename_str).await;
    let _new_guard = state.lock_filename(&new_filename).await;

    match state.repos.archives.find_by_filename(&new_filename).await {
        Ok(Some(existing)) if existing.id != id => {
            return error(
                StatusCode::CONFLICT,
                "rename_archive",
                format!(
                    "an archive with filename {new_filename:?} already exists ({}).",
                    existing.id
                ),
            )
        }
        Ok(_) => {}
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rename_archive",
                e.to_string(),
            )
        }
    }

    let new_path = old_path.with_file_name(&new_filename);
    // Journaled *before* the disk rename, cleared only after the archive record below is durably
    // saved — a crash in between (disk already renamed, DB record still pointing at the old path)
    // otherwise leaves `repair_zombie_archives`' own repair guessing purely from `archive.name`,
    // which may not have been saved yet either. This entry gives the startup repair the exact
    // old/new paths this rename was mid-flight on, no guessing required. Best-effort: if Redis is
    // briefly unreachable here, the rename still proceeds — a real Redis outage already blocks the
    // rest of this handler's own writes (the `archives.save()` below) as effectively as it would
    // block writing this journal entry.
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn
            .hset(
                lanrurugi_storage::keys::PENDING_RENAME_KEY,
                id.as_str(),
                format!("{}\n{}", old_path.display(), new_path.display()),
            )
            .await;
    }

    if let Err(e) = tokio::fs::rename(&old_path, &new_path).await {
        if let Ok(mut conn) = state.redis.config.get().await {
            let _: Result<(), _> = conn
                .hdel(lanrurugi_storage::keys::PENDING_RENAME_KEY, id.as_str())
                .await;
        }
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rename_archive",
            format!("failed to rename file on disk: {e}"),
        );
    }

    if archive.has_patch {
        let old_patch_path = lanrurugi_scanner::patch::patch_path_for(&old_path);
        let new_patch_path = old_path.with_file_name(&patch_filename);
        if old_patch_path.exists() {
            if let Err(e) = tokio::fs::rename(&old_patch_path, &new_patch_path).await {
                // The archive file itself already moved — rolling that back here would trade one
                // inconsistent state (renamed archive, stale-named patch) for another (original
                // name restored, but the caller was already told the whole operation failed,
                // leaving no clean signal either way) without actually fixing anything, since the
                // real problem is whatever made this specific rename fail (permissions, a second
                // conflicting file already at `new_patch_path`, ...) and would just as likely
                // reject the rollback `rename` too. Surface the failure; the archive's own rename
                // already succeeded and is reported as such below — this is logged, not fatal.
                tracing::error!(%id, old_patch_path = %old_patch_path.display(), new_patch_path = %new_patch_path.display(), error = %e, "renamed archive file but failed to rename its sidecar patch file");
            }
        }
    }

    archive.file = new_path.to_string_lossy().to_string();
    // `name` (legacy field, exposed to the frontend as `filename`) is the *stem* on its own —
    // stored independently of `file`, not derived from it (unlike `extension()`, which always
    // re-reads `file` itself) — so it goes stale unless updated here too. Left alone, the Edit
    // page's "Current File Name" field would keep showing the pre-rename name even though the
    // file on disk (and every other place `file` is read from) had already moved.
    archive.name = requested_stem.clone();
    if let Err(e) = state.repos.archives.save(&archive).await {
        // The journal entry deliberately stays — it's still true that the disk file is at
        // `new_path` while the DB record isn't caught up yet, exactly the state the journal
        // exists to record. `repair_zombie_archives` will finish the job on next startup; a
        // manual retry of this same rename request would also just re-do this save.
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rename_archive",
            format!("renamed file on disk but failed to save the archive record: {e}"),
        );
    }

    if let Ok(mut conn) = state.redis.config.get().await {
        let old_str = old_path.to_string_lossy().to_string();
        let new_str = new_path.to_string_lossy().to_string();
        let _: Result<(), _> = conn
            .hdel(lanrurugi_storage::keys::FILEMAP_KEY, &old_str)
            .await;
        let _: Result<(), _> = conn
            .hset(lanrurugi_storage::keys::FILEMAP_KEY, &new_str, id.as_str())
            .await;
        let _: Result<(), _> = conn
            .hdel(lanrurugi_storage::keys::PENDING_RENAME_KEY, id.as_str())
            .await;
    }

    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::ARCHIVE_RENAME,
        lanrurugi_storage::activity::ActivityTarget {
            id: Some(id.to_string()),
            label: Some(new_filename.clone()),
            kind: Some("archive".to_string()),
        },
        // `name` — matches `tankoubon.rename`/`token.rename`'s own `before`/`after` field name for
        // the identical "this resource's own display name changed" shape, so the frontend's
        // activity-description renderer can read one shared `{ name: string }` shape across every
        // rename-type action type instead of special-casing this one's field name (`filename`, its
        // pre-unification name) apart from the others.
        Some(json!({ "name": old_filename_str })),
        Some(json!({ "name": new_filename })),
    )
    .await;

    axum::Json(json!({ "operation": "rename_archive", "success": 1, "filename": new_filename }))
        .into_response()
}

/// Cover thumbnail path, matching legacy `extract_thumbnail`'s sharding: `<thumb_dir>/<id[0:2]>/<id>.<ext>`.
/// Probes both thumbnail formats (`ThumbFormat::ALL`) since a library-wide format switch's regen
/// job may still be in flight.
///
/// A cache miss (e.g. `thumb_dir` was never mounted, or its contents were lost/corrupted — a
/// real, previously-silent failure mode: the archive itself is fine, only its thumbnail is
/// missing) triggers on-demand regeneration from the source archive file rather than serving
/// [`PLACEHOLDER_THUMBNAIL`] forever. Bounded via [`AppState::thumbnail_singleflight`] (see
/// `lanrurugi_core::singleflight`'s own doc comment) since a page like the homepage requests
/// dozens of *different* covers in one load — both same-ID dedup (two browser tabs, a retry,
/// several cards referencing the same cover) and a total-concurrency cap (so a wiped thumb
/// directory can't turn one page load into an unbounded burst of heavy decode/decompress work)
/// matter here.
///
/// If generation itself fails (source archive also missing/corrupted, decode error, ...), this
/// still degrades to the placeholder rather than an error response — matches the original
/// steady-state contract (never 500s), it just no longer *masks a fixable problem* the way an
/// unconditional placeholder-on-miss did.
#[derive(Debug, Deserialize)]
pub struct GetThumbnailParams {
    /// `0`/absent selects the cover thumbnail (`<thumb_dir>/<shard>/<id>.<ext>`); `N > 0` selects
    /// a per-page thumbnail (`<thumb_dir>/<shard>/<id>/<N>.<ext>`) — matches legacy's real
    /// `serve_thumbnail` (`Model::Archive.pm`: `$is_first_page = $page == 0`), which the reader's
    /// archive-overview page grid (`ArchiveOverviewOverlay.tsx`) relies on to show a distinct
    /// thumbnail per page rather than the cover repeated for every page.
    page: Option<u32>,
}

async fn get_archive_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<GetThumbnailParams>,
) -> Response {
    // Archive IDs are always a 40-char lowercase-hex SHA-1 digest (`lanrurugi_storage::id`) —
    // enforcing that shape here, before `id` ever reaches a path-join, closes a path-traversal
    // hole: without it, an `id` containing `/` or `..` (e.g. `../../../../etc/passwd`) would let
    // a caller read arbitrary `.jpg`/`.webp` files elsewhere on the host.
    if !is_valid_archive_id(id.as_str()) {
        return not_found("serve_thumbnail", "No archive ID specified.");
    }
    let page = params.page.unwrap_or(0);
    if let Some((content_type, bytes)) = read_thumbnail_from_disk(&state, id.as_str(), page).await {
        return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    if let Some((content_type, bytes)) =
        regenerate_thumbnail_on_demand(&state, id.as_str(), page).await
    {
        return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response()
}

fn thumbnail_disk_path(
    state: &AppState,
    id: &str,
    page: u32,
    format: lanrurugi_scanner::thumbnail::ThumbFormat,
) -> std::path::PathBuf {
    let shard = &id[0..2];
    if page == 0 {
        state
            .library
            .thumb_dir
            .join(shard)
            .join(format!("{id}.{}", format.extension()))
    } else {
        state
            .library
            .thumb_dir
            .join(shard)
            .join(id)
            .join(format!("{page}.{}", format.extension()))
    }
}

async fn read_thumbnail_from_disk(
    state: &AppState,
    id: &str,
    page: u32,
) -> Option<(&'static str, bytes::Bytes)> {
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = thumbnail_disk_path(state, id, page, format);
        if let Ok(contents) = tokio::fs::read(&path).await {
            return Some((format.content_type(), bytes::Bytes::from(contents)));
        }
    }
    None
}

/// Generates a missing cover or per-page thumbnail on the spot and returns it, or `None` if
/// generation isn't possible (unknown ID, missing/corrupt source archive, decode failure) — the
/// caller falls back to the placeholder in that case. `page == 0` selects the cover (generated
/// from the archive's own page 1); `page > 0` selects that page directly. See
/// [`get_archive_thumbnail`]'s doc comment for the singleflight/concurrency-cap rationale — keyed
/// by `"{id}:{page}"` so a cover regen and a per-page regen for the same archive don't collide on
/// the same singleflight slot.
async fn regenerate_thumbnail_on_demand(
    state: &AppState,
    id: &str,
    page: u32,
) -> Option<(&'static str, bytes::Bytes)> {
    let archive = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(id.to_string()))
        .await
        .ok()
        .flatten()?;

    state
        .thumbnail_singleflight
        .run(format!("{id}:{page}"), {
            let state = state.clone();
            let id = id.to_string();
            move || async move {
                let thumb_settings = match state.redis.config.get().await {
                    Ok(mut conn) => lanrurugi_scanner::thumbnail::read_settings(&mut conn).await,
                    Err(_) => return None,
                };
                let output = thumbnail_disk_path(&state, &id, page, thumb_settings.format);
                let source_page = if page == 0 { 1 } else { page as usize };
                let result = lanrurugi_scanner::thumbnail::generate(
                    std::path::PathBuf::from(&archive.file),
                    source_page,
                    output,
                    thumb_settings.format,
                    thumb_settings.quality,
                )
                .await;
                match result {
                    Ok(_) => read_thumbnail_from_disk(&state, &id, page).await,
                    Err(e) => {
                        tracing::warn!(id = %id, page, error = %e, "on-demand thumbnail generation failed");
                        None
                    }
                }
            }
        })
        .await
}

#[derive(Debug, Deserialize)]
pub struct UpdateThumbnailParams {
    page: Option<u32>,
}

/// Sets the archive's cover thumbnail to a given page (legacy: `Model::Archive::update_thumbnail`,
/// backed by `Utils::Archive::extract_thumbnail($thumbdir, $id, $page, 1, 1)` — the reader
/// overview overlay's "set as thumbnail" hover icon on the page grid, `reader.js`'s
/// `.set-thumbnail` click handler, calls this). `page` defaults to `1` (legacy: `$page = 1 unless
/// $page`). Only overwrites `thumbhash` when `generate` actually returns one — matches legacy's
/// own behavior, which only ever hashes on `set_cover`, so setting a non-cover page as thumbnail
/// still refreshes `thumbhash` (unlike the cover-page-only condition inside `generate` itself,
/// legacy's `extract_thumbnail` hashes on *any* `set_cover=1` call regardless of which page was
/// picked — `generate`'s narrower `page == 1` condition is a deliberate divergence documented on
/// its own doc comment for duplicate-detection purposes, so a non-cover "set as thumbnail" here
/// intentionally leaves the existing `thumbhash` untouched rather than hashing the wrong page).
async fn update_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<UpdateThumbnailParams>,
) -> Response {
    if !is_valid_archive_id(&id) {
        return not_found("update_thumbnail", "No archive ID specified.");
    }
    let page = params.page.unwrap_or(1);
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("update_thumbnail", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_thumbnail",
                e.to_string(),
            )
        }
    };
    let thumb_settings = match state.redis.config.get().await {
        Ok(mut conn) => lanrurugi_scanner::thumbnail::read_settings(&mut conn).await,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_thumbnail",
                e.to_string(),
            )
        }
    };
    let shard = &id[0..2];
    let output = state
        .library
        .thumb_dir
        .join(shard)
        .join(format!("{id}.{}", thumb_settings.format.extension()));
    match lanrurugi_scanner::thumbnail::generate(
        std::path::PathBuf::from(&archive.file),
        page as usize,
        output.clone(),
        thumb_settings.format,
        thumb_settings.quality,
    )
    .await
    {
        Ok(thumbhash) => {
            if thumbhash.is_some() {
                archive.thumbhash = thumbhash;
                if let Err(e) = state.repos.archives.save(&archive).await {
                    tracing::warn!(id = %id, error = %e, "failed to persist updated thumbhash");
                }
            }
            ok(
                "update_thumbnail",
                [("new_thumbnail", json!(output.display().to_string()))],
            )
        }
        Err(e) => error(StatusCode::BAD_REQUEST, "update_thumbnail", e.to_string()),
    }
}

async fn set_new_flag(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    set_isnew(&state, &id, true).await
}

async fn clear_new_flag(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    set_isnew(&state, &id, false).await
}

async fn set_isnew(state: &AppState, id: &lanrurugi_core::ids::ArchiveId, isnew: bool) -> Response {
    let operation = if isnew { "add_new" } else { "clear_new" };
    let mut archive = match state.repos.archives.get(id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found(operation, format!("{id} does not exist.")),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, operation, e.to_string()),
    };
    archive.isnew = isnew;
    match state.repos.archives.save(&archive).await {
        Ok(()) => {
            if let Err(e) =
                lanrurugi_search::indexer::set_isnew_index(&state.redis.search, id.as_str(), isnew)
                    .await
            {
                tracing::warn!(%id, error = %e, "failed to update isnew search index");
            }
            axum::Json(json!({ "operation": operation, "id": id, "success": 1 })).into_response()
        }
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, operation, e.to_string()),
    }
}

async fn update_progress(
    State(state): State<AppState>,
    Path((id, page)): Path<(lanrurugi_core::ids::ArchiveId, u32)>,
) -> Response {
    if state.repos.archives.get(&id).await.ok().flatten().is_none() {
        return not_found("update_progress", format!("{id} does not exist."));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match state.repos.archives.set_progress(&id, page, now).await {
        Ok(()) => {
            // Matches legacy exactly (`Controller/Api/Archive.pm`: `$redis_cfg->incr("LRR_TOTALPAGESTAT")`)
            // — incremented unconditionally on every successful call, not just forward progress.
            if let Ok(mut conn) = state.redis.config.get().await {
                let _: Result<i64, _> =
                    deadpool_redis::redis::AsyncCommands::incr(&mut conn, TOTAL_PAGES_STAT_KEY, 1)
                        .await;
            }
            axum::Json(json!({
                "operation": "update_progress",
                "id": id,
                "page": page,
                "lastreadtime": now,
                "success": 1,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_progress",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct RegenThumbsParams {
    #[serde(default)]
    force: bool,
}

/// Queues thumbnail (re)generation for every archive missing one (or all archives, if
/// `force=true`), reusing `thumbnail::generate` (T043) — the same logic new-archive cataloguing
/// uses. Runs as a background task tracked via `lanrurugi-core::jobs` (T013).
async fn regen_thumbs(
    State(state): State<AppState>,
    Query(params): Query<RegenThumbsParams>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
) -> Response {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "regen_thumbnails",
                e.to_string(),
            )
        }
    };
    let thumb_settings = match state.redis.config.get().await {
        Ok(mut conn) => lanrurugi_scanner::thumbnail::read_settings(&mut conn).await,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "regen_thumbnails",
                e.to_string(),
            )
        }
    };

    // Recorded here (the manually-triggered `POST /regen_thumbs` entry point), not inside
    // `spawn_regen_thumbnails_job` itself — that function is shared with `settings::put_settings`'s
    // own `enablewebp`-change codepath, whose regeneration is a side effect of a `settings.update`
    // entry already being recorded there, not a separate user-initiated action of its own.
    crate::activity::record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        lanrurugi_storage::activity::action_types::ARCHIVE_THUMB_REGEN,
        lanrurugi_storage::activity::ActivityTarget {
            id: None,
            label: None,
            kind: Some("database".to_string()),
        },
        None,
        Some(json!({ "force": params.force, "archive_count": archives.len() })),
    )
    .await;

    let job_id = spawn_regen_thumbnails_job(&state, archives, thumb_settings, params.force).await;

    axum::Json(json!({
        "operation": "regen_thumbnails",
        "success": 1,
        "job": job_id,
    }))
    .into_response()
}

/// Shared regen-thumbnails job body: used by [`regen_thumbs`] and by `settings::put_settings`
/// when `enablewebp` changes (a format switch must regenerate every thumbnail so the library
/// stays in a single, uniform format rather than a jpg/webp mix). `force=true` regenerates every
/// archive's thumbnail unconditionally, matching `regen_thumbs`'s own `force` query param; a
/// format-switch caller always passes `true` since a fresh-format thumbnail never already exists
/// at the new path the first time around.
pub async fn spawn_regen_thumbnails_job(
    state: &AppState,
    archives: Vec<Archive>,
    thumb_settings: lanrurugi_scanner::thumbnail::ThumbSettings,
    force: bool,
) -> String {
    let thumb_dir = state.library.thumb_dir.clone();
    let jobs = state.jobs.clone();
    let job_id = jobs.create("regen_thumbnails").await;
    let job_id_for_task = job_id.clone();
    let archive_repo = state.repos.archives.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        let total = archives.len().max(1);

        // Which archives actually need a freshly generated cover (force, or the file for the
        // *current* format doesn't exist yet) — only these pay the CPU-heavy decode/resize/encode
        // cost below. `regen_jobs` keeps each one's index into `archives` so results can be
        // zipped back in the sequential pass afterward; every other archive is a no-op there.
        let mut regen_jobs: Vec<(usize, std::path::PathBuf, std::path::PathBuf)> = Vec::new();
        for (i, archive) in archives.iter().enumerate() {
            let shard = &archive.id[0..2.min(archive.id.len())];
            let output = thumb_dir.join(shard).join(format!(
                "{}.{}",
                archive.id,
                thumb_settings.format.extension()
            ));
            if force || !output.exists() {
                regen_jobs.push((i, std::path::PathBuf::from(&archive.file), output));
            }
        }

        // Decode/resize/encode every needed cover in one batch, spread across rayon's whole
        // thread pool, instead of one `spawn_blocking` round-trip per archive run sequentially —
        // this job runs library-wide (every automatic `enablewebp` toggle triggers it), so this is
        // the highest-impact place in the codebase for constitution Principle III's "off the
        // async reactor" requirement to also mean genuine parallelism, not just serialized
        // off-reactor dispatch.
        let job_inputs: Vec<(std::path::PathBuf, usize, std::path::PathBuf)> = regen_jobs
            .iter()
            .map(|(_, path, output)| (path.clone(), 1, output.clone()))
            .collect();
        let results = match lanrurugi_scanner::thumbnail::generate_batch(
            job_inputs,
            thumb_settings.format,
            thumb_settings.quality,
        )
        .await
        {
            Ok(results) => results,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };
        let mut results_by_index: std::collections::HashMap<usize, _> = regen_jobs
            .into_iter()
            .map(|(i, ..)| i)
            .zip(results)
            .collect();

        for (i, mut archive) in archives.into_iter().enumerate() {
            if let Some(result) = results_by_index.remove(&i) {
                let shard = archive.id[0..2.min(archive.id.len())].to_string();
                match result {
                    Ok(thumbhash) => {
                        archive.thumbhash = thumbhash;
                        if let Err(e) = archive_repo.save(&archive).await {
                            tracing::warn!(id = %archive.id, error = %e, "failed to persist regenerated thumbhash");
                        }
                        // Clean up sibling files left over in the *other* thumbnail format —
                        // otherwise a stale jpg (or webp) from before a format switch lingers
                        // forever and get_archive_thumbnail's probe order arbitrarily picks it.
                        for other in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
                            if other == thumb_settings.format {
                                continue;
                            }
                            let stale = thumb_dir.join(&shard).join(format!(
                                "{}.{}",
                                archive.id,
                                other.extension()
                            ));
                            let _ = tokio::fs::remove_file(stale).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(id = %archive.id, error = %e, "regen_thumbs failed for archive")
                    }
                }
            }
            jobs.set_progress(&job_id_for_task, (i + 1) as f32 / total as f32)
                .await;
        }
        jobs.finish(&job_id_for_task, json!({ "regenerated": total }))
            .await;
    });

    job_id
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateMetadataParams {
    title: Option<String>,
    tags: Option<String>,
    summary: Option<String>,
}

/// `PUT /archives/{id}/metadata` (US3): overwrites title/tags/summary and keeps the search
/// indexes (title index, tag indexes, untagged set) in sync, matching legacy `set_title`/
/// `set_tags`/`set_summary`'s side effects (`Utils/Database.pm`).
async fn update_archive_metadata(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Query(params): Query<UpdateMetadataParams>,
) -> Response {
    let mut archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("update_metadata", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_metadata",
                e.to_string(),
            )
        }
    };

    let old_title = archive.title.clone();
    let old_tags = archive.tags.clone();
    let old_summary = archive.summary.clone();

    if let Some(title) = params.title.filter(|t| !t.is_empty()) {
        archive.title = title;
    }
    if let Some(tags) = params.tags {
        archive.tags = tags;
    }
    if let Some(summary) = params.summary {
        archive.summary = summary;
    }

    match state.repos.archives.save(&archive).await {
        Ok(()) => {
            // `tags_added`/`tags_removed` — the actual set difference, computed here rather than
            // handing the frontend two raw comma-joined strings to parse and diff itself: this is
            // the one place that already knows both the old and new tag string, Rust's own
            // `HashSet` difference is no more code than the frontend's would be, and precomputing
            // it means the activity feed's own tag-diff display (added in green, removed
            // strikethrough-red) never needs a second, independently-maintained copy of "how to
            // split/compare a tag string" living in TypeScript.
            let old_tag_set: std::collections::HashSet<&str> = old_tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            let new_tag_set: std::collections::HashSet<&str> = archive
                .tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect();
            let tags_added: Vec<&str> = new_tag_set.difference(&old_tag_set).copied().collect();
            let tags_removed: Vec<&str> = old_tag_set.difference(&new_tag_set).copied().collect();

            // A rating-only change (the Library page's right-click star widget's own write path,
            // `useLibrary.ts::updateRating`) rebuilds and resends the *entire* tags string just to
            // add/change/remove one `rating:` tag, title/summary always untouched — recorded under
            // its own dedicated action type instead of the generic `ARCHIVE_METADATA_UPDATE`, so
            // "rated this" doesn't read as an ambiguous "updated metadata"/tag-diff entry (direct
            // feedback: a rating change buried in a full tag-diff obscured that it was *just* a
            // rating). At most one `rating:` tag can ever exist at a time (`RatingWidget`'s own
            // `setRating` always filters the old one out before appending the new one), so a real
            // rating-only change is exactly "≤1 added and ≤1 removed, both `rating:`-prefixed" —
            // anything else touching tags (even alongside a rating change) falls through to the
            // generic path below instead of trying to represent a mixed change as two entries.
            let is_rating_only_change = archive.title == old_title
                && archive.summary == old_summary
                && tags_added.len() <= 1
                && tags_removed.len() <= 1
                && tags_added.iter().all(|t| t.starts_with("rating:"))
                && tags_removed.iter().all(|t| t.starts_with("rating:"))
                && (!tags_added.is_empty() || !tags_removed.is_empty());

            if is_rating_only_change {
                crate::activity::record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    lanrurugi_storage::activity::action_types::ARCHIVE_RATING_UPDATE,
                    lanrurugi_storage::activity::ActivityTarget {
                        id: Some(id.to_string()),
                        label: Some(archive.title.clone()),
                        kind: Some("archive".to_string()),
                    },
                    Some(json!({ "rating": tags_removed.first() })),
                    Some(json!({ "rating": tags_added.first() })),
                )
                .await;
            } else {
                crate::activity::record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    lanrurugi_storage::activity::action_types::ARCHIVE_METADATA_UPDATE,
                    lanrurugi_storage::activity::ActivityTarget {
                        id: Some(id.to_string()),
                        label: Some(archive.title.clone()),
                        kind: Some("archive".to_string()),
                    },
                    Some(json!({ "title": old_title, "summary": old_summary })),
                    Some(json!({
                        "title": archive.title,
                        "summary": archive.summary,
                        "tags_added": tags_added,
                        "tags_removed": tags_removed,
                    })),
                )
                .await;
            }
            if archive.title != old_title {
                if let Err(e) = lanrurugi_search::indexer::update_title_index(
                    &state.redis.search,
                    &id,
                    &old_title,
                    &archive.title,
                )
                .await
                {
                    tracing::warn!(%id, error = %e, "failed to update title search index");
                }
            }
            if archive.tags != old_tags {
                if let Err(e) = lanrurugi_search::indexer::update_tag_indexes(
                    &state.redis.search,
                    &id,
                    &old_tags,
                    &archive.tags,
                )
                .await
                {
                    tracing::warn!(%id, error = %e, "failed to update tag search index");
                }
            }
            if archive.title != old_title {
                let state = state.clone();
                let id = id.to_string();
                let title = archive.title.clone();
                tokio::spawn(async move {
                    crate::recommend_precompute::precompute_one(&state, &id, &title).await;
                });
            }
            ok("update_metadata", [])
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_metadata",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct PageParams {
    path: String,
    /// `"patch"` when this page comes from the archive's own sidecar `.patch.zip`
    /// (`lanrurugi_scanner::patch`) rather than the archive file itself — set on the URL
    /// `/files` itself already generated for a patched page (see [`get_files`]'s own doc comment),
    /// so this endpoint doesn't need to re-derive it. Absent/anything else means "read `path` from
    /// the archive file, as before patches existed."
    #[serde(default)]
    source: Option<String>,
    /// `"1"`/`"true"` explicitly asks for the resize-optimized WebP version (`/files` sets this on
    /// every URL it generates when `enableresize` is on) — the URL is the contract here, so the
    /// browser caches the original and optimized variants as distinct resources. Absent means
    /// "serve the original bytes" (reader "view original" affordances and non-reader callers).
    #[serde(default)]
    optimize: Option<String>,
}

/// `<temp_dir>/resize_page/<id>/<sha1(path)>_<threshold>_<quality>.webp` — matches legacy's own
/// `resize_page/$id/$path/$threshold/$quality` cache key (`Model/Archive.pm::serve_page`), with
/// the in-archive `path` component sha1-hashed rather than embedded verbatim so a path containing
/// `/`, `..`, or other filesystem-unsafe characters can't affect the cache file's location. A
/// `<same name>.dims` sidecar ("WxH") records the original dimensions at encode time so a cache
/// hit can still report them without re-decoding the original.
fn resize_cache_path(
    temp_dir: &std::path::Path,
    id: &str,
    path: &str,
    threshold: i64,
    quality: i64,
) -> std::path::PathBuf {
    let mut hasher = Sha1::new();
    hasher.update(path.as_bytes());
    let path_hash = hex_encode(&hasher.finalize());
    temp_dir
        .join("resize_page")
        .join(id)
        .join(format!("{path_hash}_{threshold}_{quality}.webp"))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Reader resize (legacy `enable_resize`/`Model/Archive.pm::serve_page`): pages over
/// `sizethreshold` KB get downscaled/recompressed to WebP at `readerquality` and cached under
/// `temp_dir`, rather than served as their original bytes every time. Enabled by default
/// (`enableresize = true` — WebP re-encode is cheap enough that the old JPEG-era caution no
/// longer applies).
///
/// The actual archive read (and optional resize) is collapsed through
/// [`AppState::page_singleflight`], keyed by `(id, entry path)` — the reader requests several
/// pages of the same archive at once (prefetch), and this archive format's underlying
/// `read_entry` has no random-access index: reading page N means linearly rescanning the archive
/// from its first entry (see `lanrurugi_scanner::archive_format`'s own doc comment). Without
/// dedup, two overlapping requests for the exact same page (e.g. a fast back-and-forth flip)
/// would each independently pay that full rescan; without the concurrency cap, prefetching many
/// *different* pages at once would launch unbounded concurrent rescans of the same archive file.
async fn get_page(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<PageParams>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("serve_page", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_page",
                e.to_string(),
            )
        }
    };

    let is_patch = params.source.as_deref() == Some("patch");
    let optimize = matches!(params.optimize.as_deref(), Some("1" | "true"));
    // Singleflight key includes the `patch:`/`orig:` prefix and the optimize flag — a patch page
    // and an original page can legitimately share the same entry *name* (they live in two separate
    // zips), and the optimized variant is a distinct resource from the original bytes.
    let cache_key_path = format!(
        "{}:{}{}",
        if is_patch { "patch" } else { "orig" },
        params.path,
        if optimize { ":opt" } else { "" },
    );
    let result = state
        .page_singleflight
        .run((id.to_string(), cache_key_path), {
            let state = state.clone();
            let id = id.clone();
            let path = params.path.clone();
            let archive_file = archive.file.clone();
            let corrupted_pages = archive.corrupted_pages.clone();
            move || async move {
                fetch_page(
                    &state,
                    id.as_str(),
                    &path,
                    &archive_file,
                    &corrupted_pages,
                    is_patch,
                    optimize,
                )
                .await
            }
        })
        .await;

    match result {
        Ok(page) => {
            let mut header_map = header::HeaderMap::new();
            header_map.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static(page.content_type),
            );
            if page.resized {
                // Lets the reader's file-info bar show "converted WebP" vs the original entry
                // (the URL's own `path` still carries the original name — e.g. `foo.png` — which
                // is the page's identity in the archive, not a claim about the served format).
                header_map.insert(
                    header::HeaderName::from_static("x-lrr-resized"),
                    header::HeaderValue::from_static("webp"),
                );
                header_map.insert(
                    header::HeaderName::from_static("x-lrr-original-size"),
                    header::HeaderValue::from_str(&page.orig_size.to_string())
                        .unwrap_or(header::HeaderValue::from_static("0")),
                );
                header_map.insert(
                    header::HeaderName::from_static("x-lrr-original-dimensions"),
                    header::HeaderValue::from_str(&format!(
                        "{}x{}",
                        page.orig_width, page.orig_height
                    ))
                    .unwrap_or(header::HeaderValue::from_static("0x0")),
                );
            }
            (header_map, page.bytes).into_response()
        }
        Err(msg) => error(StatusCode::INTERNAL_SERVER_ERROR, "serve_page", msg),
    }
}

/// The actual per-`(archive, entry path)` work [`AppState::page_singleflight`] collapses
/// concurrent duplicate calls onto — see [`get_page`]'s own doc comment.
///
/// `is_patch` (set from the `source=patch` query param [`get_files`]'s own patch-aware URLs
/// carry) skips the corrupted-pages check and resize cache entirely — both are keyed by/scoped to
/// entries inside the archive file itself, and a patch page is a different file's content
/// (`lanrurugi_scanner::patch::read_page` reads it from the sidecar `.patch.zip`, not
/// `archive_file`) that neither of those apply to.
async fn fetch_page(
    state: &AppState,
    id: &str,
    path: &str,
    archive_file: &str,
    corrupted_pages: &[String],
    is_patch: bool,
    optimize: bool,
) -> Result<FetchedPage, String> {
    if is_patch {
        let archive_file_owned = archive_file.to_string();
        let entry_name = path.to_string();
        let raw = lanrurugi_core::concurrency::run_blocking(move || {
            lanrurugi_scanner::patch::read_page(
                std::path::Path::new(&archive_file_owned),
                &lanrurugi_scanner::patch::EffectivePage::Patched(entry_name),
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let content_type = image_content_type(&raw);
        let orig_size = raw.len() as u64;
        return Ok(FetchedPage {
            content_type,
            bytes: bytes::Bytes::from(raw),
            resized: false,
            orig_size,
            orig_width: 0,
            orig_height: 0,
        });
    }

    // Short-circuits before any archive I/O — a page already known to be corrupt (flagged by
    // `generate_page_thumbnails`'s own decode attempt) is served straight from this static asset
    // every time, never re-attempting the decode that already failed once. `corrupted_pages` comes
    // from the same `Archive` fetch `get_page` already did (not a second Redis round trip here).
    // See `Archive::corrupted_pages`'s own docs for why this is keyed by entry name, not index.
    if corrupted_pages.iter().any(|p| p == path) {
        return Ok(FetchedPage {
            content_type: "image/svg+xml",
            bytes: bytes::Bytes::from_static(CORRUPTED_PAGE_PLACEHOLDER),
            resized: false,
            orig_size: 0,
            orig_width: 0,
            orig_height: 0,
        });
    }

    // `read_entry` does blocking libarchive FFI + file IO (constitution Principle III: never run
    // inline on an async worker thread) — `resize_if_over_threshold` below already goes through
    // `run_blocking` internally, this call didn't before this change.
    let archive_file_owned = archive_file.to_string();
    let entry_path = path.to_string();
    let raw = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::archive_format::read_entry(
            std::path::Path::new(&archive_file_owned),
            &entry_path,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let fields: HashMap<String, String> = conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    let enable_resize = fields
        .get("enableresize")
        .map(|v| v != "0")
        .unwrap_or(false);
    let threshold: i64 = fields
        .get("sizethreshold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SIZE_THRESHOLD);
    let quality: i64 = fields
        .get("readerquality")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_READER_QUALITY);

    let content_type = image_content_type(&raw);
    // Formats browsers render natively — BMP included (Chrome/Firefox/Edge all decode it; it just
    // wastes bandwidth, so it still goes through the *threshold-based* resize path like any other
    // renderable format). Everything else (TIFF/unknown — Safari-only TIFF aside, no mainstream
    // browser) gets a *forced* WebP conversion under `optimize=1`, regardless of the resize
    // setting, since serving those bytes as-is would just show a broken image. A client-side
    // `img.onError` retry in the reader covers any format this list misses per-browser.
    let renderable = matches!(
        content_type,
        "image/jpeg"
            | "image/png"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/svg+xml"
            | "image/bmp"
    );
    let force_convert = !renderable;

    // `optimize` is the per-request contract (set by `/files`'s own generated URLs); without it,
    // serve the original bytes. A renderable page with resize disabled also passes through.
    if !optimize || (!enable_resize && renderable) {
        let orig_size = raw.len() as u64;
        return Ok(FetchedPage {
            content_type,
            bytes: bytes::Bytes::from(raw),
            resized: false,
            orig_size,
            orig_width: 0,
            orig_height: 0,
        });
    }

    // Byte-size check first (no decode needed) — under-threshold *renderable* pages pass through
    // untouched; a forced conversion skips this gate entirely.
    if !force_convert && (raw.len() / 1024) as i64 <= threshold {
        let orig_size = raw.len() as u64;
        return Ok(FetchedPage {
            content_type,
            bytes: bytes::Bytes::from(raw),
            resized: false,
            orig_size,
            orig_width: 0,
            orig_height: 0,
        });
    }

    let cache_path = resize_cache_path(&state.library.temp_dir, id, path, threshold, quality);
    // Cache hit: the webp bytes plus the `.dims` sidecar (original "WxH") written at encode
    // time — no re-decode of the original needed. A missing sidecar (older cache entry) just
    // degrades to zero dims in the headers.
    if let Ok(cached) = tokio::fs::read(&cache_path).await {
        let dims = tokio::fs::read_to_string(cache_path.with_extension("webp.dims"))
            .await
            .ok()
            .and_then(|s| {
                let (w, h) = s.trim().split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            });
        return Ok(FetchedPage {
            content_type: "image/webp",
            bytes: bytes::Bytes::from(cached),
            resized: true,
            orig_size: raw.len() as u64,
            orig_width: dims.map(|d| d.0).unwrap_or(0),
            orig_height: dims.map(|d| d.1).unwrap_or(0),
        });
    }

    // `raw` is still needed after this call regardless of which branch runs below (`Ok(Some(_))`
    // reads its length; `Ok(None)` serves it back as-is), while `convert_to_webp`/
    // `resize_if_over_threshold` both take their input by value (they `move` it into
    // `run_blocking`) — so the source bytes have to be cloned into the conversion call either
    // way. `orig_size` is captured up front so the `Ok(None)` branch can move the original `raw`
    // out directly instead of cloning it a second time.
    let orig_size = raw.len() as u64;
    let converted = if force_convert {
        lanrurugi_scanner::resize::convert_to_webp(raw.clone(), quality as u8)
            .await
            .map(Some)
    } else {
        lanrurugi_scanner::resize::resize_if_over_threshold(raw.clone(), quality as u8, threshold)
            .await
    };
    match converted {
        Ok(Some((resized, orig_width, orig_height))) => {
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &resized).await;
            let _ = tokio::fs::write(
                cache_path.with_extension("webp.dims"),
                format!("{orig_width}x{orig_height}"),
            )
            .await;
            Ok(FetchedPage {
                content_type: "image/webp",
                bytes: bytes::Bytes::from(resized),
                resized: true,
                orig_size,
                orig_width,
                orig_height,
            })
        }
        Ok(None) => {
            let content_type = image_content_type(&raw);
            Ok(FetchedPage {
                content_type,
                bytes: bytes::Bytes::from(raw),
                resized: false,
                orig_size,
                orig_width: 0,
                orig_height: 0,
            })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Detects an image's MIME type from its magic bytes so the browser displays it inline rather
/// than triggering a download (issue #62 — `application/octet-stream` causes auto-download in
/// every browser). Falls back to `application/octet-stream` for truly unrecognized content.
///
/// `pub(crate)` — also used by `download_queue.rs`'s own comparison-sample page-image endpoint
/// (issue #77), which needs the exact same "show inline, don't download" behavior for a page
/// pulled from a not-yet-cataloged staged download, not just an already-cataloged archive.
pub(crate) fn image_content_type(raw: &[u8]) -> &'static str {
    if raw.len() < 8 {
        return "application/octet-stream";
    }
    if &raw[..2] == b"\xFF\xD8" {
        "image/jpeg"
    } else if &raw[..8] == b"\x89PNG\r\n\x1A\n" {
        "image/png"
    } else if &raw[..4] == b"GIF8" {
        "image/gif"
    } else if &raw[..4] == b"RIFF" && raw.len() >= 12 && &raw[8..12] == b"WEBP" {
        "image/webp"
    } else if &raw[..2] == b"BM" {
        "image/bmp"
    } else if raw.len() >= 12
        && &raw[4..8] == b"ftyp"
        && (&raw[8..12] == b"avif" || &raw[8..12] == b"avis")
    {
        "image/avif"
    } else if (&raw[..4] == b"II*\0") || (&raw[..4] == b"MM\0*") {
        "image/tiff"
    } else if raw.starts_with(b"<svg") || raw.starts_with(b"<?xml") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

/// `GET /archives/{id}/files` — lists page URLs (verified: `api-archive#get_file_list`). Since
/// LANrurugi extracts pages on demand rather than pre-extracting to a cache directory, there's no
/// separate background "extract" job to report here (`job` is always `0`) — pages are simply
/// available immediately via `/archives/{id}/page`.
///
/// Merges in a sidecar `.patch.zip`'s own pages, if one exists (`lanrurugi_scanner::patch`,
/// issue #77's own follow-on design) — a URL for a patch-sourced page carries `&source=patch` so
/// [`get_page`] knows to read it from the patch zip instead of the archive file itself; every
/// other consumer of page URLs (OPDS, `pagecount`, the scan health-check) still calls
/// `archive_format::list_pages` directly and never sees patched pages at all (confirmed design —
/// patches are a reader-facing-only concept, not part of the archive's own catalogued state).
async fn get_files(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("get_file_list", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_file_list",
                e.to_string(),
            )
        }
    };
    let archive_path = std::path::Path::new(&archive.file);
    // `optimize=1` is stamped on every generated URL — the reader then gets the WebP variant
    // where `fetch_page` decides it's warranted (setting-enabled + over-threshold, or a format
    // browsers can't render at all), while a caller hitting the same URL without the param gets
    // the original bytes.
    let opt_suffix = "&optimize=1";
    match lanrurugi_scanner::archive_format::list_pages(archive_path) {
        Ok(pages) => {
            let effective = lanrurugi_scanner::patch::effective_pages(archive_path, &pages);
            let pages: Vec<serde_json::Value> = effective
                .iter()
                .map(|p| match p {
                    lanrurugi_scanner::patch::EffectivePage::Original(name) => {
                        json!({ "url": format!("/api/archives/{id}/page?path={name}{opt_suffix}"), "is_patch": false })
                    }
                    lanrurugi_scanner::patch::EffectivePage::Patched(name) => {
                        json!({ "url": format!("/api/archives/{id}/page?path={name}&source=patch{opt_suffix}"), "is_patch": true })
                    }
                })
                .collect();
            axum::Json(json!({ "job": 0, "pages": pages })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_file_list",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct PageDimensionsParams {
    /// How many pages, from the start, to actually measure — see [`read_page_dimensions`]'s own
    /// docs for why this is a required, bounded count rather than "all of them."
    ///
    /// [`read_page_dimensions`]: lanrurugi_scanner::archive_format::read_page_dimensions
    count: usize,
}

/// `GET /archives/{id}/page-dimensions?count=N` — natural pixel width/height for the first `N`
/// pages only, in the same order as `/files`'s own `pages` array (so the frontend can zip them
/// together by index without a separate name-matching step). Additive to `/files`, not folded into
/// it — existing callers of `/files` (the third-party-compatible file-list contract) shouldn't have
/// to pay for an archive pass with per-entry image-header parsing they never asked for; only the
/// infinite-scroll reader view (the one caller that actually needs this, to size not-yet-loaded
/// `<img>`s accurately before jumping to a resume position — see `read_page_dimensions`'s own docs
/// for why it's bounded to just the pages before that jump, not the whole archive) calls this
/// endpoint at all.
async fn get_page_dimensions(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
    Query(params): Query<PageDimensionsParams>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("get_page_dimensions", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_page_dimensions",
                e.to_string(),
            )
        }
    };
    let archive_file = archive.file.clone();
    let count = params.count;
    let result = lanrurugi_core::concurrency::run_blocking(move || {
        lanrurugi_scanner::archive_format::read_page_dimensions(
            std::path::Path::new(&archive_file),
            count,
        )
    })
    .await;
    match result {
        Ok(Ok(dimensions)) => {
            let json_dims: Vec<serde_json::Value> = dimensions
                .into_iter()
                .map(|dim| match dim {
                    Some((width, height)) => json!({ "width": width, "height": height }),
                    None => serde_json::Value::Null,
                })
                .collect();
            axum::Json(json!({ "dimensions": json_dims })).into_response()
        }
        Ok(Err(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_page_dimensions",
            e.to_string(),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_page_dimensions",
            e.to_string(),
        ),
    }
}

/// `POST /archives/{id}/files/thumbnails` — generates a thumbnail for every page missing one.
/// Pages are decoded on demand rather than cached, so this always completes synchronously
/// (verified contract's `202`/background-job path is for legacy's pre-extraction step, which
/// LANrurugi's on-demand page serving doesn't need).
async fn generate_page_thumbnails(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("generate_page_thumbnails", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "generate_page_thumbnails",
                e.to_string(),
            )
        }
    };

    let pages =
        match lanrurugi_scanner::archive_format::list_pages(std::path::Path::new(&archive.file)) {
            Ok(p) => p,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "generate_page_thumbnails",
                    e.to_string(),
                )
            }
        };

    let thumb_settings = match state.redis.config.get().await {
        Ok(mut conn) => lanrurugi_scanner::thumbnail::read_settings(&mut conn).await,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "generate_page_thumbnails",
                e.to_string(),
            )
        }
    };

    let shard = &id[0..2.min(id.len())];
    // Every missing page's thumbnail is generated in one batch, spread across rayon's whole
    // thread pool, instead of one page at a time each paying its own `spawn_blocking` round-trip
    // — a real difference for large-page-count volumes (constitution Principle III: CPU-bound
    // decode/resize/encode work belongs off the async reactor, and batched here for genuine
    // parallelism rather than just serialized off-reactor dispatch).
    let jobs: Vec<(std::path::PathBuf, usize, std::path::PathBuf)> = pages
        .iter()
        .enumerate()
        .filter_map(|(i, _)| {
            let output = state
                .library
                .thumb_dir
                .join(shard)
                .join(id.as_str())
                .join(format!("{}.{}", i + 1, thumb_settings.format.extension()));
            (!output.exists()).then(|| (std::path::PathBuf::from(&archive.file), i + 1, output))
        })
        .collect();
    let page_indices: Vec<usize> = jobs.iter().map(|(_, page, _)| *page).collect();

    let results = match lanrurugi_scanner::thumbnail::generate_batch(
        jobs,
        thumb_settings.format,
        thumb_settings.quality,
    )
    .await
    {
        Ok(results) => results,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "generate_page_thumbnails",
                e.to_string(),
            )
        }
    };

    // Newly-discovered corrupt pages this pass — collected rather than saved one at a time so a
    // failure partway through doesn't leave `archive` and its persisted record diverging, and so
    // the common case (no corrupt pages found) never touches the archive record at all.
    let mut newly_corrupted: Vec<String> = Vec::new();
    for (page, result) in page_indices.into_iter().zip(results) {
        let Err(e) = result else { continue };
        let entry_name = &pages[page - 1];
        tracing::warn!(%id, page, error = %e, "page thumbnail generation failed");
        // Only an actual image-decode failure means *this page's bytes* are corrupt — an I/O
        // error writing the thumbnail file, a failed blocking-task join, etc. say nothing
        // about the page itself and must not get it wrongly marked corrupted (which would
        // make the reader serve a placeholder for a perfectly good page forever, since
        // `heal`-style fields are sticky by design — see `Archive::corrupted_pages`'s docs).
        if matches!(e, lanrurugi_scanner::thumbnail::ThumbnailError::Decode(_))
            && !archive.corrupted_pages.contains(entry_name)
            && !newly_corrupted.contains(entry_name)
        {
            newly_corrupted.push(entry_name.clone());
        }
    }
    if !newly_corrupted.is_empty() {
        let mut updated = archive;
        updated.corrupted_pages.extend(newly_corrupted);
        if let Err(e) = state.repos.archives.save(&updated).await {
            tracing::warn!(%id, error = %e, "failed to persist newly-detected corrupted pages");
        }
    }

    axum::Json(json!({
        "operation": "generate_page_thumbnails",
        "success": 1,
        "message": "Thumbnails generated.",
    }))
    .into_response()
}

/// `GET /api/archives/{id}/thumbnails?from=N&count=M`
///
/// Returns paginated page-metadata so the overview grid can load incrementally
/// instead of rendering every cell at once (issue #71). Each entry carries the
/// resolved `arcId` + `localPage` so the frontend can build the correct thumbnail
/// URL even for a Tankoubon whose pages span multiple member archives.
#[derive(Debug, Deserialize)]
struct PageThumbnailsQuery {
    from: Option<u32>,
    count: Option<u32>,
}

async fn get_page_thumbnails(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PageThumbnailsQuery>,
) -> Response {
    let from = q.from.unwrap_or(1).max(1);
    let count = q.count.unwrap_or(60).min(100);

    // Tankoubon: resolve global pages across member archives
    if id.starts_with("TANK_") {
        let tank_id = TankId(id.clone());
        let tank = match state.repos.groupings.get(&tank_id).await {
            Ok(Some(t)) => t,
            Ok(None) => return not_found("get_page_thumbnails", format!("{id} does not exist.")),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "get_page_thumbnails",
                    e.to_string(),
                )
            }
        };

        let mut total = 0u32;
        let mut member_page_counts: Vec<(ArchiveId, u32)> = Vec::new();
        for member_id in &tank.archives {
            if let Ok(Some(archive)) = state.repos.archives.get(member_id).await {
                let pc = archive.pagecount;
                member_page_counts.push((member_id.clone(), pc));
                total += pc;
            }
        }

        let mut pages: Vec<serde_json::Value> = Vec::with_capacity(count as usize);
        let end = (from + count - 1).min(total);
        let mut global = 1u32;
        for (arc_id, pc) in &member_page_counts {
            let member_start = global;
            let member_end = global + pc - 1;
            global += pc;

            let overlap_start = member_start.max(from);
            let overlap_end = member_end.min(end);
            if overlap_start > overlap_end {
                continue;
            }
            for gp in overlap_start..=overlap_end {
                let local_page = gp - member_start + 1;
                pages.push(serde_json::json!({
                    "page": gp,
                    "arcId": arc_id,
                    "localPage": local_page,
                }));
                if pages.len() >= count as usize {
                    break;
                }
            }
            if pages.len() >= count as usize {
                break;
            }
        }

        return axum::Json(serde_json::json!({ "pages": pages, "total": total })).into_response();
    }

    // Regular archive
    let archive_id = ArchiveId(id);
    let archive = match state.repos.archives.get(&archive_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return not_found(
                "get_page_thumbnails",
                format!("{archive_id} does not exist."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_page_thumbnails",
                e.to_string(),
            )
        }
    };

    // The effective (patch-merged) page count, not the raw `pagecount` field — that field is
    // never patch-aware (confirmed design: patches don't touch the archive's own catalogued
    // state), so using it here would under-report the total whenever a sidecar `.patch.zip`
    // exists, leaving the overview grid unable to scroll to the patched-in pages at all. Real
    // disk I/O (`list_pages` + the patch-existence check), so run off the async reactor like every
    // other archive-format read in this file.
    let archive_file = archive.file.clone();
    let total = match lanrurugi_core::concurrency::run_blocking(move || {
        let path = std::path::Path::new(&archive_file);
        let original = lanrurugi_scanner::archive_format::list_pages(path)?;
        Ok::<usize, lanrurugi_scanner::archive_format::ArchiveFormatError>(
            lanrurugi_scanner::patch::effective_pages(path, &original).len(),
        )
    })
    .await
    {
        Ok(Ok(n)) => n as u32,
        _ => archive.pagecount,
    };
    let end = (from + count - 1).min(total);
    let pages: Vec<serde_json::Value> = (from..=end)
        .map(|p| {
            serde_json::json!({
                "page": p,
                "arcId": archive_id.0,
                "localPage": p,
            })
        })
        .collect();

    axum::Json(serde_json::json!({ "pages": pages, "total": total })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_archive_id_accepts_a_real_sha1_hex_digest() {
        assert!(is_valid_archive_id(
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        ));
    }

    #[test]
    fn is_valid_archive_id_rejects_path_traversal_and_malformed_input() {
        // Regression guard: `get_archive_thumbnail` builds a filesystem path directly from `id`
        // with no repository lookup first (unlike every other per-archive endpoint), so an
        // unvalidated `id` containing `/` or `..` could escape `thumb_dir` entirely.
        assert!(!is_valid_archive_id("../../../../etc/passwd"));
        assert!(!is_valid_archive_id("/etc/passwd"));
        assert!(!is_valid_archive_id(""));
        assert!(!is_valid_archive_id("short"));
        assert!(!is_valid_archive_id(
            "da39a3ee5e6b4b0d3255bfef95601890afd8070/"
        ));
    }

    #[test]
    fn image_content_type_detects_jpeg_from_magic_bytes() {
        assert_eq!(
            image_content_type(b"\xFF\xD8\xFF\xE0\x00\x10JFIF"),
            "image/jpeg"
        );
    }

    #[test]
    fn image_content_type_detects_png_from_magic_bytes() {
        assert_eq!(
            image_content_type(b"\x89PNG\r\n\x1A\n\x00\x00\x00\rIHDR"),
            "image/png"
        );
    }

    #[test]
    fn image_content_type_detects_gif_from_magic_bytes() {
        assert_eq!(image_content_type(b"GIF89a\x00\x00"), "image/gif");
    }

    #[test]
    fn image_content_type_detects_webp_from_magic_bytes() {
        let mut header = vec![0u8; 12];
        header[..4].copy_from_slice(b"RIFF");
        header[8..12].copy_from_slice(b"WEBP");
        assert_eq!(image_content_type(&header), "image/webp");
    }

    #[test]
    fn image_content_type_detects_bmp_from_magic_bytes() {
        assert_eq!(
            image_content_type(b"BM\x00\x00\x00\x00\x00\x00\x00\x00"),
            "image/bmp"
        );
    }

    #[test]
    fn image_content_type_falls_back_to_octet_stream_for_unrecognized() {
        assert_eq!(
            image_content_type(b"\x00\x00\x00\x00\x00\x00\x00\x00"),
            "application/octet-stream"
        );
    }

    #[test]
    fn image_content_type_falls_back_for_too_short_input() {
        assert_eq!(image_content_type(b"\xFF"), "application/octet-stream");
    }
}
