//! `search` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml`.

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use lanrurugi_search::engine::{search, SearchParams};
use serde::Deserialize;
use serde_json::json;

use crate::archives::ArchiveMetadataJson;
use crate::common::error;
use crate::AppState;

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
}

async fn build_params(
    state: &AppState,
    q: &SearchQuery,
) -> Result<SearchParams, lanrurugi_storage::repository::RepositoryError> {
    let category = match &q.category {
        Some(id) => state.repos.categories.get(id).await?,
        None => None,
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

async fn search_archives(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    let params = match build_params(&state, &q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "search",
                e.to_string(),
            )
        }
    };
    match search(&state.redis.archive, &state.redis.search, &params).await {
        Ok(result) => {
            let page = paginate(&result.ids, q.start);
            let mut data = Vec::with_capacity(page.len());
            for id in &page {
                if let Ok(Some(a)) = state.repos.archives.get(id).await {
                    data.push(ArchiveMetadataJson::from(&a));
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
    Query(q): Query<SearchQuery>,
) -> Response {
    let params = match build_params(&state, &q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "search_ids",
                e.to_string(),
            )
        }
    };
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
}

async fn search_random(State(state): State<AppState>, Query(q): Query<RandomQuery>) -> Response {
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
    };
    let params = match build_params(&state, &search_q).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "get_random_archives",
                e.to_string(),
            )
        }
    };
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
                if let Ok(Some(a)) = state.repos.archives.get(id).await {
                    data.push(ArchiveMetadataJson::from(&a));
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
