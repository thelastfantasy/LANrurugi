//! `categories` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Category.pm` (constitution Principle II).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use lanrurugi_core::entities::Category;
use lanrurugi_core::ids::{ArchiveId, CategoryId};
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

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
    auth: Option<axum::extract::Extension<AuthContext>>,
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
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::CATEGORY_CREATE,
                ActivityTarget {
                    id: Some(catid.0.clone()),
                    label: Some(category.name.clone()),
                    kind: Some("category".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({
                "operation": "create_category",
                "category_id": catid,
                "success": 1,
            }))
            .into_response()
        }
        Err(e) => {
            // The category was actually built and a Redis write was attempted (not a validation
            // rejection) — worth recording so a silent Redis failure isn't invisible in the audit
            // trail.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::CATEGORY_CREATE,
                ActivityTarget {
                    id: Some(catid.0.clone()),
                    label: Some(category.name.clone()),
                    kind: Some("category".to_string()),
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
                "create_category",
                e.to_string(),
            )
        }
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

async fn delete_category(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<CategoryId>,
) -> Response {
    let existing = state.repos.categories.get(&id).await.ok().flatten();
    match state.repos.categories.delete(&id).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::CATEGORY_DELETE,
                ActivityTarget {
                    id: Some(id.0.clone()),
                    label: existing.map(|c| c.name),
                    kind: Some("category".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "delete_category", "success": 1 })).into_response()
        }
        Err(e) => {
            // Deletion was actually attempted against a category id that was found (or at least
            // looked up) beforehand — a genuine storage failure, not a not-found/validation
            // rejection, so it belongs in the audit trail.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::CATEGORY_DELETE,
                ActivityTarget {
                    id: Some(id.0.clone()),
                    label: existing.map(|c| c.name),
                    kind: Some("category".to_string()),
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
                "delete_category",
                e.to_string(),
            )
        }
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
    // Legacy's own `Model::Category::add_to_category` (`$redis->exists($arc_id)`) is a generic
    // Redis key-existence test, not "must specifically be an archive hash" — a `TANK_`-prefixed
    // Tankoubon key satisfies it just as well, so legacy genuinely supports adding a Tankoubon to
    // a category. Mirrored here as an explicit branch (rather than a single generic `EXISTS`
    // call) since this port's repositories are already split by entity type; the practical result
    // is the same permissive check.
    let exists = if crate::tankoubons::is_valid_tankoubon_id(archive) {
        state
            .repos
            .groupings
            .get(&lanrurugi_core::ids::TankId(archive.to_string()))
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        state
            .repos
            .archives
            .get(&archive_id)
            .await
            .ok()
            .flatten()
            .is_some()
    };
    if !exists {
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
