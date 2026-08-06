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
use axum::routing::{get, put};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::{Archive, TocEntry};
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::common::{error, not_found, ok};
use crate::settings::{DEFAULT_READER_QUALITY, DEFAULT_SIZE_THRESHOLD};
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
/// archive's own `date_added:<unix>` tag) is more than N days old — an archive without a
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/archives", get(list_archives))
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
        .route(
            "/archives/{id}/isnew",
            put(set_new_flag).delete(clear_new_flag),
        )
        .route("/archives/{id}/progress/{page}", put(update_progress))
        .route("/regen_thumbs", axum::routing::post(regen_thumbs))
}

async fn list_archives(State(state): State<AppState>) -> Response {
    match state.repos.archives.list_all().await {
        Ok(archives) => {
            let json: Vec<ArchiveMetadataJson> = archives.iter().map(Into::into).collect();
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

async fn delete_archive(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found("delete_archive", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_archive",
                e.to_string(),
            )
        }
    };
    match state.repos.archives.delete(&id).await {
        Ok(()) => {
            // The only success-path log line this handler had before this was added — every other
            // outcome below is a best-effort cleanup failure (`warn!`) or silently discarded
            // (`let _ =`), so a normal, successful delete previously produced zero tracing output
            // at all, leaving no audit trail in `general.log` for what is a real, irreversible,
            // user-triggered destructive action (unlinks the archive's file from disk, not just a
            // DB record).
            tracing::info!(%id, filename = %archive.name, "deleted archive");
            // Best-effort, matching every other indexer call site in this file (`update_title_index`/
            // `update_tag_indexes` above) — a search-index cleanup failure shouldn't undo an already
            // committed archive deletion, just leave a ghost id behind (logged) for a future rescan
            // to eventually reconcile.
            if let Err(e) = lanrurugi_search::indexer::remove_archive_index(
                &state.redis.search,
                &id,
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
            axum::Json(json!({
                "operation": "delete_archive",
                "id": id,
                "filename": archive.name,
                "success": 1,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_archive",
            e.to_string(),
        ),
    }
}

async fn get_archive_categories(
    State(state): State<AppState>,
    Path(id): Path<lanrurugi_core::ids::ArchiveId>,
) -> Response {
    match state.repos.categories.list_all().await {
        Ok(categories) => {
            let matching: Vec<_> = categories
                .into_iter()
                .filter(|c| !c.is_dynamic() && c.archives.contains(&id))
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
    match state.repos.groupings.list_all().await {
        Ok(groupings) => {
            let ids: Vec<String> = groupings
                .into_iter()
                .filter(|g| g.archives.contains(&id))
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
    match tokio::fs::read(&archive.file).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "download_archive",
            e.to_string(),
        ),
    }
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
}

/// `<temp_dir>/resize_page/<id>/<sha1(path)>_<threshold>_<quality>.jpg` — matches legacy's own
/// `resize_page/$id/$path/$threshold/$quality` cache key (`Model/Archive.pm::serve_page`), with
/// the in-archive `path` component sha1-hashed rather than embedded verbatim so a path containing
/// `/`, `..`, or other filesystem-unsafe characters can't affect the cache file's location.
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
        .join(format!("{path_hash}_{threshold}_{quality}.jpg"))
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
/// `sizethreshold` KB get downscaled/recompressed to JPEG at `readerquality` and cached under
/// `temp_dir`, rather than served as their original bytes every time. Disabled by default
/// (`enableresize = false`), matching legacy's own default.
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

    let result = state
        .page_singleflight
        .run((id.to_string(), params.path.clone()), {
            let state = state.clone();
            let id = id.clone();
            let path = params.path.clone();
            let archive_file = archive.file.clone();
            let corrupted_pages = archive.corrupted_pages.clone();
            move || async move {
                fetch_page(&state, id.as_str(), &path, &archive_file, &corrupted_pages).await
            }
        })
        .await;

    match result {
        Ok((content_type, bytes)) => {
            ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
        }
        Err(msg) => error(StatusCode::INTERNAL_SERVER_ERROR, "serve_page", msg),
    }
}

/// The actual per-`(archive, entry path)` work [`AppState::page_singleflight`] collapses
/// concurrent duplicate calls onto — see [`get_page`]'s own doc comment.
async fn fetch_page(
    state: &AppState,
    id: &str,
    path: &str,
    archive_file: &str,
    corrupted_pages: &[String],
) -> Result<(&'static str, bytes::Bytes), String> {
    // Short-circuits before any archive I/O — a page already known to be corrupt (flagged by
    // `generate_page_thumbnails`'s own decode attempt) is served straight from this static asset
    // every time, never re-attempting the decode that already failed once. `corrupted_pages` comes
    // from the same `Archive` fetch `get_page` already did (not a second Redis round trip here).
    // See `Archive::corrupted_pages`'s own docs for why this is keyed by entry name, not index.
    if corrupted_pages.iter().any(|p| p == path) {
        return Ok((
            "image/svg+xml",
            bytes::Bytes::from_static(CORRUPTED_PAGE_PLACEHOLDER),
        ));
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
    if !enable_resize {
        return Ok((image_content_type(&raw), bytes::Bytes::from(raw)));
    }
    let threshold: i64 = fields
        .get("sizethreshold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SIZE_THRESHOLD);
    let quality: i64 = fields
        .get("readerquality")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_READER_QUALITY);

    let cache_path = resize_cache_path(&state.library.temp_dir, id, path, threshold, quality);
    if let Ok(cached) = tokio::fs::read(&cache_path).await {
        return Ok(("image/jpeg", bytes::Bytes::from(cached)));
    }

    match lanrurugi_scanner::resize::resize_if_over_threshold(raw.clone(), quality as u8, threshold)
        .await
    {
        Ok(Some(resized)) => {
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &resized).await;
            Ok(("image/jpeg", bytes::Bytes::from(resized)))
        }
        Ok(None) => Ok((image_content_type(&raw), bytes::Bytes::from(raw))),
        Err(e) => Err(e.to_string()),
    }
}

/// Detects an image's MIME type from its magic bytes so the browser displays it inline rather
/// than triggering a download (issue #62 — `application/octet-stream` causes auto-download in
/// every browser). Falls back to `application/octet-stream` for truly unrecognized content.
fn image_content_type(raw: &[u8]) -> &'static str {
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
    } else {
        "application/octet-stream"
    }
}

/// `GET /archives/{id}/files` — lists page URLs (verified: `api-archive#get_file_list`). Since
/// LANrurugi extracts pages on demand rather than pre-extracting to a cache directory, there's no
/// separate background "extract" job to report here (`job` is always `0`) — pages are simply
/// available immediately via `/archives/{id}/page`.
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
    match lanrurugi_scanner::archive_format::list_pages(std::path::Path::new(&archive.file)) {
        Ok(pages) => {
            let urls: Vec<String> = pages
                .iter()
                .map(|p| format!("/api/archives/{id}/page?path={p}"))
                .collect();
            axum::Json(json!({ "job": 0, "pages": urls })).into_response()
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

    let total = archive.pagecount;
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
