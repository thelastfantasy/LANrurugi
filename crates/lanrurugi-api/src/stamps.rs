//! `stamps` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Stamp.pm` (constitution Principle II) — closes the Stamp-entity gap `data-model.md`
//! flagged (present in legacy data but not named in the feature spec's Key Entities).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

use lanrurugi_core::ids::{ArchiveId, StampId};

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct StampJson {
    pub id: String,
    pub position: String,
    pub content: String,
    pub icon: String,
    pub rect: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/archives/{id}/stamps", get(stamped_pages))
        .route(
            "/archives/{id}/stamps/{index}",
            get(stamps_by_page).put(add_stamp),
        )
        .route(
            "/stamps/{id}",
            get(get_stamp).put(update_stamp).delete(delete_stamp),
        )
}

/// `GET /archives/{id}/stamps` — pages that have at least one stamp (legacy `get_stamped_pages`).
async fn stamped_pages(State(state): State<AppState>, Path(id): Path<ArchiveId>) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "get_stamped_pages",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_stamped_pages",
                e.to_string(),
            )
        }
    };

    let mut pages = std::collections::BTreeSet::new();
    for stamp_id in &archive.stamp_ids {
        if let Some(page) = page_of(stamp_id.as_str()) {
            pages.insert(page);
        }
    }
    let result: Vec<String> = pages.into_iter().map(|p| p.to_string()).collect();
    axum::Json(json!({ "result": result })).into_response()
}

pub(crate) fn page_of(stamp_id: &str) -> Option<u32> {
    stamp_id
        .strip_prefix("STAMPS_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|p| p.parse().ok())
}

async fn stamps_by_page(
    State(state): State<AppState>,
    Path((id, index)): Path<(ArchiveId, u32)>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "get_stamps_by_page",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_stamps_by_page",
                e.to_string(),
            )
        }
    };

    let mut result = Vec::new();
    for stamp_id in &archive.stamp_ids {
        if page_of(stamp_id.as_str()) != Some(index) {
            continue;
        }
        if let Ok(Some(s)) = state.repos.stamps.get(stamp_id).await {
            result.push(StampJson {
                id: s.stamp_id.into_string(),
                position: s.position,
                content: s.content,
                icon: s.icon,
                rect: s.rect,
            });
        }
    }
    axum::Json(json!({ "result": result })).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct StampParams {
    content: Option<String>,
    position: Option<String>,
    icon: Option<String>,
    rect: Option<String>,
}

async fn add_stamp(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, index)): Path<(ArchiveId, u32)>,
    Query(params): Query<StampParams>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "add_stamp",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_stamp",
                e.to_string(),
            )
        }
    };
    if index == 0 || index > archive.pagecount {
        return error(
            StatusCode::BAD_REQUEST,
            "add_stamp",
            format!("Page {index} out of range."),
        );
    }

    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    match state
        .repos
        .stamps
        .create(
            &id,
            index,
            params.content.as_deref().unwrap_or_default(),
            params.position.as_deref().unwrap_or_default(),
            params.icon.as_deref().unwrap_or_default(),
            params.rect.as_deref().unwrap_or_default(),
            now_millis,
        )
        .await
    {
        Ok(stamp_id) => {
            // issue #97: a fresh stamp on a not-yet-bookmarked page also bookmarks that page,
            // when enabled — merged into one activity record, never a separate one for the stamp
            // itself (stamps.rs has no activity trail of its own outside this linkage).
            if crate::settings::read_stamp_autobookmark(&state).await
                && !state
                    .bookmarks
                    .is_bookmarked(id.as_str(), index)
                    .await
                    .unwrap_or(true)
            {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                match state.bookmarks.add(id.as_str(), index, now).await {
                    Ok(()) => {
                        record_manual(
                            &state,
                            auth.as_ref().map(|e| &e.0),
                            action_types::STAMP_BOOKMARK_SYNC,
                            ActivityTarget {
                                id: Some(id.0.clone()),
                                label: Some(archive.title.clone()),
                                kind: Some("archive".to_string()),
                            },
                            Outcome::Success,
                            None,
                            Some(json!({
                                "stamp": { "stamp_id": stamp_id, "page": index },
                                "bookmark": { "action": "add", "page": index },
                            })),
                        )
                        .await;
                    }
                    Err(e) => {
                        // The stamp itself was created successfully — a failure to also bookmark
                        // the page is a secondary-effect failure, not a reason to fail this
                        // request (same posture `record_manual` itself already takes toward its
                        // own write failures).
                        tracing::warn!(%id, page = index, error = %e, "failed to auto-bookmark page after adding a stamp");
                    }
                }
            }
            axum::Json(json!({
                "operation": "add_stamp",
                "stamp_id": stamp_id,
                "success": 1,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "add_stamp",
            e.to_string(),
        ),
    }
}

async fn get_stamp(State(state): State<AppState>, Path(id): Path<StampId>) -> Response {
    match state.repos.stamps.get(&id).await {
        Ok(Some(s)) => axum::Json(StampJson {
            id: s.stamp_id.into_string(),
            position: s.position,
            content: s.content,
            icon: s.icon,
            rect: s.rect,
        })
        .into_response(),
        Ok(None) => not_found("get_stamp", format!("{id} doesn't exist in the database!")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_stamp",
            e.to_string(),
        ),
    }
}

async fn update_stamp(
    State(state): State<AppState>,
    Path(id): Path<StampId>,
    Query(params): Query<StampParams>,
) -> Response {
    match state
        .repos
        .stamps
        .update(
            &id,
            params.content.as_deref(),
            params.position.as_deref(),
            params.icon.as_deref(),
            params.rect.as_deref(),
        )
        .await
    {
        Ok(()) => axum::Json(json!({ "operation": "update_stamp", "success": 1 })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_stamp",
            e.to_string(),
        ),
    }
}

async fn delete_stamp(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<StampId>,
) -> Response {
    // Read before delete — this is the only chance to learn which archive/page this stamp
    // belonged to (`delete_stamp`'s own path only ever carries `stamp_id`). A lookup miss (id
    // already gone, or malformed) falls through to the exact same unconditional delete this
    // handler always did — no linkage is possible without knowing the archive/page anyway.
    let stamp = state.repos.stamps.get(&id).await.ok().flatten();
    let page = stamp.as_ref().and_then(|s| s.page());

    match state.repos.stamps.delete(&id).await {
        Ok(()) => {
            if let (Some(stamp), Some(page)) = (stamp, page) {
                if crate::settings::read_stamp_autounbookmark(&state).await {
                    // `stamps.delete` above already rewrote the archive's own `stamps` list —
                    // re-fetch to see the post-delete membership, not a stale in-memory copy.
                    if let Ok(Some(archive)) = state.repos.archives.get(&stamp.archive_id).await {
                        let any_left = archive
                            .stamp_ids
                            .iter()
                            .any(|sid| page_of(sid.as_str()) == Some(page));
                        if !any_left
                            && state
                                .bookmarks
                                .is_bookmarked(stamp.archive_id.as_str(), page)
                                .await
                                .unwrap_or(false)
                        {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            match state
                                .bookmarks
                                .remove(stamp.archive_id.as_str(), page, now)
                                .await
                            {
                                Ok(()) => {
                                    record_manual(
                                        &state,
                                        auth.as_ref().map(|e| &e.0),
                                        action_types::STAMP_BOOKMARK_SYNC,
                                        ActivityTarget {
                                            id: Some(stamp.archive_id.0.clone()),
                                            label: Some(archive.title.clone()),
                                            kind: Some("archive".to_string()),
                                        },
                                        Outcome::Success,
                                        None,
                                        Some(json!({
                                            "stamp": { "stamp_id": id, "page": page },
                                            "bookmark": { "action": "remove", "page": page },
                                        })),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    // The stamp itself was already deleted successfully — same
                                    // "secondary effect failure doesn't fail the request" posture
                                    // as add_stamp's own equivalent branch above.
                                    tracing::warn!(
                                        archive_id = %stamp.archive_id, page, error = %e,
                                        "failed to auto-remove bookmark after deleting a page's last stamp"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            axum::Json(json!({ "operation": "delete_stamp", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_stamp",
            e.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_core::entities::Archive;
    use lanrurugi_storage::keys::CONFIG_KEY;

    use super::*;
    use crate::plugins::tests::test_state;

    fn test_archive(id: &str, pagecount: u32) -> Archive {
        Archive {
            id: ArchiveId(id.to_string()),
            name: format!("stamp-sync-{id}"),
            title: format!("stamp-sync-{id}"),
            file: format!("/nonexistent/{id}.zip"),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
        }
    }

    async fn set_config_bool(state: &AppState, field: &str, value: bool) {
        let mut conn = state.redis.config.get().await.unwrap();
        let _: () = conn
            .hset(CONFIG_KEY, field, if value { "1" } else { "0" })
            .await
            .unwrap();
    }

    async fn clear_config_field(state: &AppState, field: &str) {
        let mut conn = state.redis.config.get().await.unwrap();
        let _: () = conn.hdel(CONFIG_KEY, field).await.unwrap();
    }

    /// issue #97: placing a stamp on a not-yet-bookmarked page bookmarks it (main switch on,
    /// the default), and the linkage produces exactly one merged activity record — not a
    /// separate one for the stamp (stamps.rs writes none of its own outside this linkage).
    #[tokio::test]
    async fn add_stamp_bookmarks_the_page_when_enabled() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        set_config_bool(&state, "stampautobookmark", true).await;
        let archive_id = format!("stamp-add-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id, 5))
            .await
            .unwrap();

        let resp = add_stamp(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 2)),
            Query(StampParams::default()),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        assert!(state.bookmarks.is_bookmarked(&archive_id, 2).await.unwrap());

        clear_config_field(&state, "stampautobookmark").await;
        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 2, 0).await.unwrap();
    }

    /// The sub-option gate: deleting the *only* stamp on a page removes that page's bookmark
    /// when `stampautounbookmark` is on.
    #[tokio::test]
    async fn delete_last_stamp_on_page_removes_the_bookmark_when_enabled() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        set_config_bool(&state, "stampautobookmark", false).await; // isolate delete-path behavior
        set_config_bool(&state, "stampautounbookmark", true).await;
        let archive_id = format!("stamp-del-only-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id, 5))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 3, 0).await.unwrap();

        let create = add_stamp(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 3)),
            Query(StampParams::default()),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let stamp_id = StampId(json["stamp_id"].as_str().unwrap().to_string());

        let resp = delete_stamp(State(state.clone()), None, Path(stamp_id)).await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        assert!(!state.bookmarks.is_bookmarked(&archive_id, 3).await.unwrap());

        clear_config_field(&state, "stampautobookmark").await;
        clear_config_field(&state, "stampautounbookmark").await;
        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id))
            .await
            .unwrap();
    }

    /// Deleting one of *two* stamps on the same page must NOT touch the bookmark — the other
    /// stamp still justifies it.
    #[tokio::test]
    async fn delete_one_of_two_stamps_on_page_keeps_the_bookmark() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        set_config_bool(&state, "stampautobookmark", false).await;
        set_config_bool(&state, "stampautounbookmark", true).await;
        let archive_id = format!("stamp-del-one-of-two-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id, 5))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 4, 0).await.unwrap();

        for _ in 0..2 {
            add_stamp(
                State(state.clone()),
                None,
                Path((ArchiveId(archive_id.clone()), 4)),
                Query(StampParams::default()),
            )
            .await;
        }
        let remaining = state
            .repos
            .archives
            .get(&ArchiveId(archive_id.clone()))
            .await
            .unwrap()
            .unwrap()
            .stamp_ids;
        assert_eq!(remaining.len(), 2);

        let resp = delete_stamp(State(state.clone()), None, Path(remaining[0].clone())).await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        assert!(state.bookmarks.is_bookmarked(&archive_id, 4).await.unwrap());

        clear_config_field(&state, "stampautobookmark").await;
        clear_config_field(&state, "stampautounbookmark").await;
        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 4, 0).await.unwrap();
    }

    /// The sub-option off: deleting the last stamp on a page must leave the bookmark alone.
    #[tokio::test]
    async fn delete_last_stamp_keeps_the_bookmark_when_sub_option_disabled() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        set_config_bool(&state, "stampautobookmark", false).await;
        set_config_bool(&state, "stampautounbookmark", false).await;
        let archive_id = format!("stamp-del-suboff-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id, 5))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 1, 0).await.unwrap();

        let create = add_stamp(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 1)),
            Query(StampParams::default()),
        )
        .await
        .into_response();
        let body = axum::body::to_bytes(create.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let stamp_id = StampId(json["stamp_id"].as_str().unwrap().to_string());

        delete_stamp(State(state.clone()), None, Path(stamp_id)).await;

        assert!(state.bookmarks.is_bookmarked(&archive_id, 1).await.unwrap());

        clear_config_field(&state, "stampautobookmark").await;
        clear_config_field(&state, "stampautounbookmark").await;
        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 1, 0).await.unwrap();
    }

    /// Adding a stamp to an already-bookmarked page doesn't double-write or error.
    #[tokio::test]
    async fn add_stamp_on_already_bookmarked_page_is_a_no_op_for_the_bookmark() {
        let Some(state) = test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        set_config_bool(&state, "stampautobookmark", true).await;
        let archive_id = format!("stamp-add-already-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id, 5))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 2, 42).await.unwrap();

        let resp = add_stamp(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 2)),
            Query(StampParams::default()),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);

        // The bookmark's own timestamp is unchanged — proof this wasn't re-added.
        let bookmarks = state.bookmarks.list_for_archive(&archive_id).await.unwrap();
        let entry = bookmarks.iter().find(|b| b.page == 2).unwrap();
        assert_eq!(entry.bookmarked_at, 42);

        clear_config_field(&state, "stampautobookmark").await;
        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 2, 0).await.unwrap();
    }
}
