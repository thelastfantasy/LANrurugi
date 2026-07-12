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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::common::{error, not_found, ok};
use crate::AppState;

const CONFIG_KEY: &str = "LRR_CONFIG";

const PLACEHOLDER_THUMBNAIL: &[u8] = include_bytes!("../assets/no_thumb.png");

/// A legitimate archive ID is always a 40-character lowercase-hex SHA-1 digest
/// (`lanrurugi_storage::id::{legacy_id, size_aware_id}`) — anything else (in particular anything
/// containing `/`, `\`, or `.`) cannot be a real ID and must never be used to build a filesystem
/// path.
fn is_valid_archive_id(id: &str) -> bool {
    id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit())
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
            arcid: a.id.clone(),
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
        .route(
            "/archives/{id}/files/thumbnails",
            axum::routing::post(generate_page_thumbnails),
        )
        .route("/archives/{id}/thumbnail", get(get_archive_thumbnail))
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
                .map(|a| a.id)
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

async fn get_archive_metadata(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.repos.archives.get(&id).await {
        Ok(Some(archive)) => axum::Json(ArchiveMetadataJson::from(&archive)).into_response(),
        Ok(None) => not_found("get_archive_metadata", format!("{id} does not exist.")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_archive_metadata",
            e.to_string(),
        ),
    }
}

/// `GET /archives/{id}` — deprecated alias of `/archives/{id}/metadata` (verified: both map to
/// `api-archive#serve_metadata` in the legacy router).
async fn get_archive_deprecated(state: State<AppState>, path: Path<String>) -> Response {
    get_archive_metadata(state, path).await
}

async fn delete_archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
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

async fn get_archive_categories(State(state): State<AppState>, Path(id): Path<String>) -> Response {
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

async fn get_archive_tankoubons(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.repos.groupings.list_all().await {
        Ok(groupings) => {
            let ids: Vec<String> = groupings
                .into_iter()
                .filter(|g| g.archives.contains(&id))
                .map(|g| g.tankid)
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

#[derive(Debug, Deserialize)]
pub struct TocParams {
    page: u32,
    title: Option<String>,
}

async fn add_toc_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<TocParams>,
) -> Response {
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
    Path(id): Path<String>,
    Query(params): Query<TocDeleteParams>,
) -> Response {
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

async fn download_archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
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
/// job may still be in flight — falls back to a placeholder if neither is on disk yet, so the
/// endpoint degrades gracefully rather than 500ing.
async fn get_archive_thumbnail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Archive IDs are always a 40-char lowercase-hex SHA-1 digest (`lanrurugi_storage::id`) —
    // enforcing that shape here, before `id` ever reaches a path-join, closes a path-traversal
    // hole: without it, an `id` containing `/` or `..` (e.g. `../../../../etc/passwd`) would let
    // a caller read arbitrary `.jpg`/`.webp` files elsewhere on the host, since (unlike every
    // other per-archive endpoint here) this one never looks the ID up in the archive repository
    // first.
    if !is_valid_archive_id(&id) {
        return not_found("serve_thumbnail", "No archive ID specified.");
    }
    let shard = &id[0..2];
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state
            .library
            .thumb_dir
            .join(shard)
            .join(format!("{id}.{}", format.extension()));
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return ([(header::CONTENT_TYPE, format.content_type())], bytes).into_response();
        }
    }
    ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response()
}

async fn set_new_flag(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_isnew(&state, &id, true).await
}

async fn clear_new_flag(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_isnew(&state, &id, false).await
}

async fn set_isnew(state: &AppState, id: &str, isnew: bool) -> Response {
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
                lanrurugi_search::indexer::set_isnew_index(&state.redis.search, id, isnew).await
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
    Path((id, page)): Path<(String, u32)>,
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
                    deadpool_redis::redis::AsyncCommands::incr(&mut conn, "LRR_TOTALPAGESTAT", 1)
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
        for (i, mut archive) in archives.into_iter().enumerate() {
            let shard = archive.id[0..2.min(archive.id.len())].to_string();
            let output = thumb_dir.join(&shard).join(format!(
                "{}.{}",
                archive.id,
                thumb_settings.format.extension()
            ));
            if force || !output.exists() {
                match regenerate_one(&archive.file, output, thumb_settings).await {
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

async fn regenerate_one(
    archive_file: &str,
    output: std::path::PathBuf,
    thumb_settings: lanrurugi_scanner::thumbnail::ThumbSettings,
) -> Result<Option<String>, lanrurugi_scanner::thumbnail::ThumbnailError> {
    lanrurugi_scanner::thumbnail::generate(
        std::path::PathBuf::from(archive_file),
        1,
        output,
        thumb_settings.format,
        thumb_settings.quality,
    )
    .await
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
    Path(id): Path<String>,
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
async fn get_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
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
    let raw = match lanrurugi_scanner::archive_format::read_entry(
        std::path::Path::new(&archive.file),
        &params.path,
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_page",
                e.to_string(),
            )
        }
    };

    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serve_page",
                e.to_string(),
            )
        }
    };
    let fields: HashMap<String, String> = conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    let enable_resize = fields
        .get("enableresize")
        .map(|v| v != "0")
        .unwrap_or(false);
    if !enable_resize {
        return ([(header::CONTENT_TYPE, "application/octet-stream")], raw).into_response();
    }
    let threshold: i64 = fields
        .get("sizethreshold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let quality: i64 = fields
        .get("readerquality")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let cache_path = resize_cache_path(
        &state.library.temp_dir,
        &id,
        &params.path,
        threshold,
        quality,
    );
    if let Ok(cached) = tokio::fs::read(&cache_path).await {
        return ([(header::CONTENT_TYPE, "image/jpeg")], cached).into_response();
    }

    match lanrurugi_scanner::resize::resize_if_over_threshold(raw.clone(), quality as u8, threshold)
        .await
    {
        Ok(Some(resized)) => {
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &resized).await;
            ([(header::CONTENT_TYPE, "image/jpeg")], resized).into_response()
        }
        Ok(None) => ([(header::CONTENT_TYPE, "application/octet-stream")], raw).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "serve_page",
            e.to_string(),
        ),
    }
}

/// `GET /archives/{id}/files` — lists page URLs (verified: `api-archive#get_file_list`). Since
/// LANrurugi extracts pages on demand rather than pre-extracting to a cache directory, there's no
/// separate background "extract" job to report here (`job` is always `0`) — pages are simply
/// available immediately via `/archives/{id}/page`.
async fn get_files(State(state): State<AppState>, Path(id): Path<String>) -> Response {
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

/// `POST /archives/{id}/files/thumbnails` — generates a thumbnail for every page missing one.
/// Pages are decoded on demand rather than cached, so this always completes synchronously
/// (verified contract's `202`/background-job path is for legacy's pre-extraction step, which
/// LANrurugi's on-demand page serving doesn't need).
async fn generate_page_thumbnails(
    State(state): State<AppState>,
    Path(id): Path<String>,
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
    for (i, _) in pages.iter().enumerate() {
        let output = state.library.thumb_dir.join(shard).join(&id).join(format!(
            "{}.{}",
            i + 1,
            thumb_settings.format.extension()
        ));
        if output.exists() {
            continue;
        }
        if let Err(e) = lanrurugi_scanner::thumbnail::generate(
            std::path::PathBuf::from(&archive.file),
            i + 1,
            output,
            thumb_settings.format,
            thumb_settings.quality,
        )
        .await
        {
            tracing::warn!(%id, page = i + 1, error = %e, "page thumbnail generation failed");
        }
    }

    axum::Json(json!({
        "operation": "generate_page_thumbnails",
        "success": 1,
        "message": "Thumbnails generated.",
    }))
    .into_response()
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
}
