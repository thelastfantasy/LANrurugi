//! `categories` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Category.pm` (constitution Principle II).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::Category;
use lanrurugi_core::ids::{ArchiveId, CategoryId};
use serde::Deserialize;
use serde_json::json;

use crate::common::{error, not_found};
use crate::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;

const BOOKMARK_LINK_FIELD: &str = "bookmark_link";

fn category_json(c: &Category) -> serde_json::Value {
    json!({
        "id": c.catid,
        "name": c.name,
        "pinned": if c.pinned { 1 } else { 0 },
        "search": c.search,
        "archives": c.archives,
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/categories", get(list_categories).put(create_category))
        .route(
            "/categories/bookmark_link",
            get(get_bookmark_link).delete(remove_bookmark_link),
        )
        .route("/categories/bookmark_link/{id}", put(update_bookmark_link))
        .route(
            "/categories/{id}",
            get(get_category)
                .put(update_category)
                .delete(delete_category),
        )
        .route(
            "/categories/{id}/{archive}",
            put(add_to_category).delete(remove_from_category),
        )
}

async fn list_categories(State(state): State<AppState>) -> Response {
    match state.repos.categories.list_all().await {
        Ok(categories) => {
            let json: Vec<_> = categories.iter().map(category_json).collect();
            axum::Json(json).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_category_list",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateCategoryParams {
    name: String,
    search: Option<String>,
    #[serde(default)]
    pinned: bool,
}

async fn create_category(
    State(state): State<AppState>,
    axum::Form(params): axum::Form<CreateCategoryParams>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut catid = CategoryId(format!("SET_{now}"));
    // Legacy bumps the timestamp by 1s on collision until a free key is found; mirrored here.
    let mut attempt = now;
    while state
        .repos
        .categories
        .get(&catid)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        attempt += 1;
        catid = CategoryId(format!("SET_{attempt}"));
    }

    let category = Category {
        catid: catid.clone(),
        name: params.name,
        search: params.search.filter(|s| !s.is_empty()),
        archives: Vec::new(),
        pinned: params.pinned,
    };
    match state.repos.categories.save(&category).await {
        Ok(()) => axum::Json(json!({
            "operation": "create_category",
            "category_id": catid,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "create_category",
            e.to_string(),
        ),
    }
}

/// `GET /categories/bookmark_link` — the static category (if any) the reader's bookmark icon
/// toggles archive membership in. Verified against `Model/Category.pm::get_bookmark_link`:
/// `LRR_CONFIG` hash field `bookmark_link`, empty string when unconfigured.
async fn get_bookmark_link(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_bookmark_link",
                e.to_string(),
            )
        }
    };
    let category_id: String = conn
        .hget(CONFIG_KEY, BOOKMARK_LINK_FIELD)
        .await
        .unwrap_or_default();
    axum::Json(json!({
        "operation": "get_bookmark_link",
        "success": 1,
        "category_id": category_id,
    }))
    .into_response()
}

/// `DELETE /categories/bookmark_link` — unlinks the bookmark icon from whatever category it was
/// pointed at, returning that category's id (legacy `Model::Category::remove_bookmark_link`).
async fn remove_bookmark_link(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_bookmark_link",
                e.to_string(),
            )
        }
    };
    let category_id: String = conn
        .hget(CONFIG_KEY, BOOKMARK_LINK_FIELD)
        .await
        .unwrap_or_default();
    let _: Result<(), _> = conn.hdel(CONFIG_KEY, BOOKMARK_LINK_FIELD).await;
    axum::Json(json!({
        "operation": "remove_bookmark_link",
        "success": 1,
        "category_id": category_id,
    }))
    .into_response()
}

/// `PUT /categories/bookmark_link/{id}` — links the bookmark icon to a *static* category only
/// (legacy rejects dynamic/search-predicate categories here: `Model::Category::update_bookmark_link`
/// — a dynamic category has no fixed archive list to add/remove membership from).
async fn update_bookmark_link(
    State(state): State<AppState>,
    Path(id): Path<CategoryId>,
) -> Response {
    match state.repos.categories.get(&id).await {
        Ok(Some(c)) if c.search.is_some() => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "operation": "update_bookmark_link",
                "error": "Cannot link bookmark to a dynamic category.",
                "success": 0,
            })),
        )
            .into_response(),
        Ok(Some(_)) => {
            let mut conn = match state.redis.config.get().await {
                Ok(c) => c,
                Err(e) => {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "update_bookmark_link",
                        e.to_string(),
                    )
                }
            };
            let _: Result<(), _> = conn
                .hset(CONFIG_KEY, BOOKMARK_LINK_FIELD, id.as_str())
                .await;
            axum::Json(json!({
                "operation": "update_bookmark_link",
                "success": 1,
                "category_id": id,
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({
                "operation": "update_bookmark_link",
                "error": "Category does not exist!",
                "success": 0,
            })),
        )
            .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_bookmark_link",
            e.to_string(),
        ),
    }
}

async fn get_category(State(state): State<AppState>, Path(id): Path<CategoryId>) -> Response {
    match state.repos.categories.get(&id).await {
        Ok(Some(c)) => axum::Json(category_json(&c)).into_response(),
        Ok(None) => not_found(
            "get_category",
            format!("{id} doesn't exist in the database!"),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_category",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateCategoryParams {
    name: Option<String>,
    search: Option<String>,
    #[serde(default)]
    pinned: bool,
}

async fn update_category(
    State(state): State<AppState>,
    Path(id): Path<CategoryId>,
    axum::Form(params): axum::Form<UpdateCategoryParams>,
) -> Response {
    let mut category = match state.repos.categories.get(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return not_found(
                "update_category",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_category",
                e.to_string(),
            )
        }
    };
    if let Some(name) = params.name {
        category.name = name;
    }
    if let Some(search) = params.search {
        category.search = Some(search).filter(|s| !s.is_empty());
    }
    category.pinned = params.pinned;
    match state.repos.categories.save(&category).await {
        Ok(()) => axum::Json(json!({
            "operation": "update_category",
            "category_id": id,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_category",
            e.to_string(),
        ),
    }
}

async fn delete_category(State(state): State<AppState>, Path(id): Path<CategoryId>) -> Response {
    match state.repos.categories.delete(&id).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "delete_category", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_category",
            e.to_string(),
        ),
    }
}

async fn add_to_category(
    State(state): State<AppState>,
    Path((id, archive)): Path<(CategoryId, ArchiveId)>,
) -> Response {
    match add_archive_to_category(&state, id.as_str(), archive.as_str()).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "add_to_category", "success": 1 })).into_response()
        }
        Err(AddToCategoryError::CategoryNotFound) => not_found(
            "add_to_category",
            format!("{id} doesn't exist in the database!"),
        ),
        Err(AddToCategoryError::Dynamic) => error(
            StatusCode::BAD_REQUEST,
            "add_to_category",
            format!("{id} is a favorite search/dynamic category, can't add archives to it."),
        ),
        Err(AddToCategoryError::ArchiveNotFound) => error(
            StatusCode::BAD_REQUEST,
            "add_to_category",
            format!("{archive} does not exist in the database."),
        ),
        Err(AddToCategoryError::Storage(e)) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "add_to_category", e)
        }
    }
}

pub enum AddToCategoryError {
    CategoryNotFound,
    Dynamic,
    ArchiveNotFound,
    Storage(String),
}

/// The read-modify-write "add this archive to this category, unless it's already a member"
/// operation itself (spec FR-018: downloaded, un-bundled multi-resource archives all join the
/// same user-selected category) — factored out of [`add_to_category`]'s HTTP handler so
/// `plugins::run_managed_downloads` can call the exact same logic instead of duplicating it.
pub async fn add_archive_to_category(
    state: &AppState,
    catid: &str,
    archive: &str,
) -> Result<(), AddToCategoryError> {
    let mut category = state
        .repos
        .categories
        .get(&CategoryId(catid.to_string()))
        .await
        .map_err(|e| AddToCategoryError::Storage(e.to_string()))?
        .ok_or(AddToCategoryError::CategoryNotFound)?;
    if category.is_dynamic() {
        return Err(AddToCategoryError::Dynamic);
    }
    let archive_id = ArchiveId(archive.to_string());
    if state
        .repos
        .archives
        .get(&archive_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(AddToCategoryError::ArchiveNotFound);
    }
    if !category.archives.contains(&archive_id) {
        category.archives.push(archive_id);
    }
    state
        .repos
        .categories
        .save(&category)
        .await
        .map_err(|e| AddToCategoryError::Storage(e.to_string()))
}

async fn remove_from_category(
    State(state): State<AppState>,
    Path((id, archive)): Path<(CategoryId, ArchiveId)>,
) -> Response {
    let mut category = match state.repos.categories.get(&id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return not_found(
                "remove_from_category",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_from_category",
                e.to_string(),
            )
        }
    };
    category.archives.retain(|a| a != &archive);
    match state.repos.categories.save(&category).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "remove_from_category", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "remove_from_category",
            e.to_string(),
        ),
    }
}
