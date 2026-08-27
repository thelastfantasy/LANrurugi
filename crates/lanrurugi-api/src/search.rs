//! `search` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml`.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use futures_util::future::join_all;
use lanrurugi_search::engine::{search, SearchParams};
use serde::Deserialize;
use serde_json::json;

use crate::archives::ArchiveMetadataJson;
use crate::common::error;
use crate::AppState;

/// Resolves one search-result id to its JSON card shape — a real archive via
/// `ArchiveMetadataJson`, or (when `groupby_tanks` left a `TANK_`-prefixed id in the result set) a
/// synthetic aggregate entry shaped the same way, mirroring legacy's own `build_tank_json`
/// (`~/LANraragi/lib/LANraragi/Utils/Database.pm`) — same field names as a real archive
/// (`arcid`/`title`/`tags`/`isnew`/`pagecount`/`lastreadtime`/`size`) plus `archive_count`, so the
/// frontend can render both through one card component. Before this, a `TANK_` id here was passed
/// straight to `state.repos.archives.get`, which always returns `None` for a non-archive key —
/// silently dropping every grouped Tankoubon out of search results entirely.
pub(crate) async fn resolve_search_entry(state: &AppState, id: &str) -> Option<serde_json::Value> {
    if let Some(tankid) = id.strip_prefix("TANK_").map(|_| id) {
        let grouping = state
            .repos
            .groupings
            .get(&lanrurugi_core::ids::TankId(tankid.to_string()))
            .await
            .ok()??;
        // Fetches every member archive in parallel (`join_all`), not a sequential `await`-per-
        // iteration loop — issue #66: `groupby_tanks` defaults to `true`, so this is on the
        // Library homepage's own default page-load path, and a sequential loop over a library
        // with multiple multi-volume Tankoubons could mean hundreds of serial Redis round trips
        // just to render one page of search results.
        let members = join_all(
            grouping
                .archives
                .iter()
                .map(|archive_id| state.repos.archives.get(archive_id)),
        )
        .await;

        let mode = crate::settings::read_new_badge_mode(state).await;
        let mut aggregate_names = Vec::new();
        let mut aggregate_isnew = false;
        let mut aggregate_pagecount: u64 = 0;
        let mut aggregate_size: u64 = 0;
        let mut latest_readtime: u64 = 0;
        let mut tags: Vec<String> = Vec::new();
        for a in members.into_iter().flatten().flatten() {
            aggregate_names.push(a.title.clone());
            // Per-member `effective_isnew` (not the raw flag) so a time-windowed or
            // until-finished mode also governs a Tankoubon's aggregate badge — a tank whose every
            // member's badge has lapsed no longer advertises itself as "new".
            aggregate_isnew = aggregate_isnew || crate::archives::effective_isnew(&a, &mode);
            aggregate_pagecount += u64::from(a.pagecount);
            aggregate_size += a.arcsize;
            latest_readtime = latest_readtime.max(a.lastreadtime);
            if !a.tags.is_empty() {
                tags.push(a.tags.clone());
            }
        }
        return Some(json!({
            "arcid": grouping.tankid,
            "title": grouping.name,
            "filename": "",
            "tags": if !grouping.tags.is_empty() { grouping.tags.clone() } else { tags.join(", ") },
            "summary": format!("Tankoubon containing: {}", aggregate_names.join(", ")),
            "isnew": aggregate_isnew,
            "extension": ".tank",
            "progress": grouping.progress,
            "pagecount": aggregate_pagecount,
            "lastreadtime": latest_readtime,
            "size": aggregate_size,
            "toc": [],
            "archive_count": grouping.archives.len(),
        }));
    }
    let a = state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(id.to_string()))
        .await
        .ok()??;
    let mode = crate::settings::read_new_badge_mode(state).await;
    let mut json = serde_json::to_value(ArchiveMetadataJson::from(&a)).ok()?;
    let obj = json.as_object_mut()?;
    obj.insert("archive_count".into(), json!(null));
    // `ArchiveMetadataJson` carries the raw stored flag; apply the configured mode on top so the
    // badge rendering sees the *effective* value (see `archives::effective_isnew`'s docs).
    obj.insert(
        "isnew".into(),
        json!(crate::archives::effective_isnew(&a, &mode)),
    );
    // Additive, not part of `ArchiveMetadataJson` itself — that struct's own shape is pinned to
    // the legacy-recorded contract (constitution Principle II), so a new field (issue #77's own
    // follow-on design — the library-grid patch badge) goes here instead, same pattern
    // `archive_count` above already established for a LANrurugi-only addition.
    obj.insert("has_patch".into(), json!(a.has_patch));
    Some(json)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/search", get(search_archives))
        .route("/search/ids", get(search_archive_ids))
        .route("/search/random", get(search_random))
        .route("/search/cache", delete(discard_search_cache))
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchQuery {
    category: Option<String>,
    filter: Option<String>,
    start: Option<i64>,
    sortby: Option<String>,
    order: Option<String>,
    newonly: Option<bool>,
    untaggedonly: Option<bool>,
    hidecompleted: Option<bool>,
    groupby_tanks: Option<bool>,
    tankonly: Option<bool>,
}

/// 007-guest-restricted-access: the union of archive ids across every `visible_to_guest: true`
/// category — a static category's own `archives` list contributes directly; a dynamic category
/// (has a `search` predicate instead) is resolved by actually running that predicate through the
/// search engine itself (`groupby_tanks: false`, since guest scoping must operate on real archive
/// ids, not `TANK_`-grouped aggregates a caller elsewhere ungroups again) and unioning in whatever
/// ids it matches — computed fresh per call, never cached, matching `procedure.rs`'s own
/// eligibility check (spec FR-015: a config change must take effect on the very next request).
pub(crate) async fn guest_visible_archive_ids(
    state: &AppState,
) -> Result<
    std::collections::HashSet<lanrurugi_core::ids::ArchiveId>,
    lanrurugi_storage::repository::RepositoryError,
> {
    let categories = state.repos.categories.list_all().await?;
    let mut ids = std::collections::HashSet::new();
    for category in categories.iter().filter(|c| c.visible_to_guest) {
        match &category.search {
            Some(predicate) if !predicate.is_empty() => {
                let params = SearchParams {
                    filter: predicate.clone(),
                    groupby_tanks: false,
                    ..SearchParams::default()
                };
                if let Ok(result) = search(&state.redis.archive, &state.redis.search, &params).await
                {
                    ids.extend(result.ids.into_iter().map(lanrurugi_core::ids::ArchiveId));
                }
            }
            _ => {
                ids.extend(category.archives.iter().cloned());
            }
        }
    }
    Ok(ids)
}

async fn build_params(
    state: &AppState,
    q: &SearchQuery,
) -> Result<SearchParams, lanrurugi_storage::repository::RepositoryError> {
    let category = match &q.category {
        Some(id) => {
            state
                .repos
                .categories
                .get(&lanrurugi_core::ids::CategoryId(id.clone()))
                .await?
        }
        None => None,
    };
    // `date_added:YYYY-MM-DD` date-range tokens resolve to a timestamp range in this timezone —
    // read from the same `LRR_CONFIG` hash as the Settings page writes, defaulting to UTC if the
    // field isn't set yet (e.g. a Redis DB written before this feature existed).
    let timezone = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(lanrurugi_storage::keys::CONFIG_KEY, "timezone")
            .await
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "UTC".to_string()),
        Err(_) => "UTC".to_string(),
    };
    Ok(SearchParams {
        filter: q.filter.clone().unwrap_or_default(),
        category,
        sortby: q.sortby.clone(),
        order_desc: q.order.as_deref() == Some("desc"),
        newonly: q.newonly.unwrap_or(false),
        untaggedonly: q.untaggedonly.unwrap_or(false),
        hidecompleted: q.hidecompleted.unwrap_or(false),
        groupby_tanks: q.groupby_tanks.unwrap_or(true),
        tankonly: q.tankonly.unwrap_or(false),
        timezone,
        new_badge_mode: crate::settings::read_new_badge_mode(state).await,
        // 007-guest-restricted-access: populated by the caller (search_archives/
        // search_archive_ids) when the request is from a guest_visitor, not computed here —
        // build_params has no access to the request's AuthContext. See those two handlers' own
        // call sites.
        restrict_to_archive_ids: None,
    })
}

fn paginate(ids: &[String], start: Option<i64>) -> Vec<String> {
    const PAGE_SIZE: usize = 100;
    match start {
        Some(s) if s < 0 => ids.to_vec(),
        Some(s) => ids
            .iter()
            .skip(s as usize)
            .take(PAGE_SIZE)
            .cloned()
            .collect(),
        None => ids.iter().take(PAGE_SIZE).cloned().collect(),
    }
}

async fn search_archives(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let mut params = match build_params(&state, &q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "search",
                e.to_string(),
            )
        }
    };
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(crate::auth_context::AuthMethod::GuestVisitor)
    ) {
        params.restrict_to_archive_ids = match guest_visible_archive_ids(&state).await {
            Ok(ids) => Some(ids),
            Err(e) => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "search",
                    e.to_string(),
                )
            }
        };
    }
    match search(&state.redis.archive, &state.redis.search, &params).await {
        Ok(result) => {
            let page = paginate(&result.ids, q.start);
            let mut data = Vec::with_capacity(page.len());
            for id in &page {
                if let Some(entry) = resolve_search_entry(&state, id).await {
                    data.push(entry);
                }
            }
            axum::Json(json!({
                "data": data,
                "recordsFiltered": result.filtered_count,
                "recordsTotal": result.total,
            }))
            .into_response()
        }
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "search",
            e.to_string(),
        ),
    }
}

async fn search_archive_ids(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Query(q): Query<SearchQuery>,
) -> Response {
    let mut params = match build_params(&state, &q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "search_ids",
                e.to_string(),
            )
        }
    };
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(crate::auth_context::AuthMethod::GuestVisitor)
    ) {
        params.restrict_to_archive_ids = match guest_visible_archive_ids(&state).await {
            Ok(ids) => Some(ids),
            Err(e) => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "search_ids",
                    e.to_string(),
                )
            }
        };
    }
    match search(&state.redis.archive, &state.redis.search, &params).await {
        Ok(result) => {
            let page = paginate(&result.ids, q.start);
            axum::Json(json!({
                "data": page,
                "recordsFiltered": result.filtered_count,
                "recordsTotal": result.total,
            }))
            .into_response()
        }
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "search_ids",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct RandomQuery {
    category: Option<String>,
    filter: Option<String>,
    count: Option<usize>,
    newonly: Option<bool>,
    untaggedonly: Option<bool>,
    hidecompleted: Option<bool>,
    groupby_tanks: Option<bool>,
    tankonly: Option<bool>,
}

async fn search_random(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    Query(q): Query<RandomQuery>,
) -> Response {
    let search_q = SearchQuery {
        category: q.category,
        filter: q.filter,
        start: Some(-1),
        sortby: None,
        order: None,
        newonly: q.newonly,
        untaggedonly: q.untaggedonly,
        hidecompleted: q.hidecompleted,
        groupby_tanks: q.groupby_tanks,
        tankonly: q.tankonly,
    };
    let mut params = match build_params(&state, &search_q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "get_random_archives",
                e.to_string(),
            )
        }
    };
    // 007-guest-restricted-access: not part of T043's own explicit scope (that task names only
    // search_archives/search_archive_ids), but left unguarded this endpoint would otherwise be a
    // real, live way for a guest_visitor to surface out-of-scope archive ids/titles — the same
    // information leakage FR-012 exists to prevent, just via a different endpoint than the two
    // T043 called out by name. Tracked as a spec/tasks.md gap, not a scope-creep addition.
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(crate::auth_context::AuthMethod::GuestVisitor)
    ) {
        params.restrict_to_archive_ids = match guest_visible_archive_ids(&state).await {
            Ok(ids) => Some(ids),
            Err(e) => {
                return error(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "get_random_archives",
                    e.to_string(),
                )
            }
        };
    }
    match search(&state.redis.archive, &state.redis.search, &params).await {
        Ok(result) => {
            use rand::seq::SliceRandom;
            let mut ids = result.ids;
            ids.shuffle(&mut rand::rng());
            let count = q.count.unwrap_or(5).min(ids.len());
            let mut data = Vec::with_capacity(count);
            // Walks the whole shuffled list, not just the first `count` ids — a search-index
            // entry that outlived its archive (a "ghost" id left behind by an incomplete delete;
            // `remove_archive_index` now prevents new ones, but stale data from before that fix
            // can still exist) is silently skipped here rather than shrinking the result below
            // `count` or, worse, coming back empty when every one of the first `count` picks
            // happens to be a ghost.
            for id in ids.iter() {
                if data.len() == count {
                    break;
                }
                if let Some(entry) = resolve_search_entry(&state, id).await {
                    data.push(entry);
                }
            }
            // Legacy's `get_random_archives` (`~/LANraragi/lib/LANraragi/Controller/Api/Search.pm`)
            // reports the number of ids actually handed back, not the full corpus count.
            let records_total = data.len();
            axum::Json(json!({ "data": data, "recordsTotal": records_total })).into_response()
        }
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "get_random_archives",
            e.to_string(),
        ),
    }
}

/// No search-results cache is implemented in Phase 1 (see `lanrurugi-search::engine` module
/// docs), so there is nothing to discard — this just returns success, matching the observable
/// contract (a client calling this always gets a fresh/uncached search either way).
async fn discard_search_cache() -> Response {
    axum::Json(json!({ "operation": "clear_cache", "success": 1 })).into_response()
}
