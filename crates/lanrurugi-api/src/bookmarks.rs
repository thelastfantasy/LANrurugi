//! `bookmarks` endpoint group — page-level reading bookmarks, additive with no legacy equivalent.
//! Replaces the old "link a static category to be *the* bookmark" mechanism (removed from
//! `categories.rs`): a bookmark is now `(archive_id, page)`, so a single archive can carry any
//! number of independent bookmarks, one per page, and the feature works with zero prior setup.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanrurugi_core::entities::Archive;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::activity::record_manual;
use crate::archives::ArchiveMetadataJson;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found, ok};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // Cross-archive aggregate listing — not any one archive's own sub-resource, so this stays
        // at the top level rather than nesting under `/archives/{id}` like the two routes below.
        .route("/bookmarks", get(list_bookmarks))
        // Nested under `/archives/{id}` (not a standalone `/bookmarks/{archive_id}`) — matches
        // this project's own existing convention for an archive's sub-resources
        // (`archives.rs`'s own `/archives/{id}/categories`, `/archives/{id}/progress/{page}`), and
        // for a many-to-one relationship written from the "owning" side
        // (`categories.rs`'s own `/categories/{id}/{archive}` for category membership) — a
        // bookmark's natural owner is the archive it's on, not the reverse.
        .route("/archives/{id}/bookmarks", get(list_bookmarks_for_archive))
        .route(
            "/archives/{id}/bookmarks/{page}",
            post(add_bookmark).delete(remove_bookmark),
        )
        .route(
            "/bookmarks/hover-page-order",
            get(get_hover_page_order).put(put_hover_page_order),
        )
}

#[derive(Debug, Serialize)]
struct BookmarkedArchiveJson {
    archive: ArchiveMetadataJson,
    /// Ascending — the first entry is what a hover preview should treat as "the" cover-aligned
    /// thumbnail.
    pages: Vec<u32>,
}

/// Clamped to a small, fixed range regardless of what the client asks for — an unbounded `limit`
/// would force an arbitrarily large in-memory sort/scan every request (same reasoning
/// `activity.rs`'s own `MIN_LIMIT`/`MAX_LIMIT` gives for its Redis-side query, just applied here
/// to the in-memory candidate set instead).
const MIN_LIMIT: usize = 1;
const MAX_LIMIT: usize = 200;
const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BookmarkSort {
    #[default]
    BookmarkedAt,
    Title,
    DateAdded,
}

#[derive(Debug, Deserialize)]
struct ListBookmarksParams {
    #[serde(default)]
    sort: BookmarkSort,
    cursor: Option<String>,
    limit: Option<usize>,
}

/// Offset-encoded — a plain decimal string, not `activity.rs`'s own composite `{timestamp}_{id}`
/// cursor (that shape exists for its `ZREVRANGEBYSCORE` exclusive-bound Redis query; this endpoint
/// sorts an in-memory `Vec` and slices it, so an offset is the simplest correct cursor for that).
/// Known, accepted limitation shared with any offset-based pagination scheme: a bookmark
/// added/removed between two page fetches can shift later pages by one, occasionally
/// duplicating/skipping an entry — not fixed here (see this module's own docs on why no Redis
/// index backs any of the three sort orders).
fn paginate<T: Clone>(items: &[T], cursor: Option<&str>, limit: usize) -> (Vec<T>, Option<String>) {
    let offset: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
    let page: Vec<T> = items.iter().skip(offset).take(limit).cloned().collect();
    let next_offset = offset + page.len();
    let next_cursor = (next_offset < items.len()).then(|| next_offset.to_string());
    (page, next_cursor)
}

/// `GET /bookmarks` — every bookmark, aggregated by archive, sorted and paginated per `sort`/
/// `cursor`/`limit`. A bookmark whose archive has since been deleted is silently dropped from the
/// result rather than erroring the whole list (same tolerance `BookmarksRepository::list_all`
/// already applies to malformed individual entries — one dangling reference shouldn't take down
/// every other still-valid bookmark).
async fn list_bookmarks(
    State(state): State<AppState>,
    Query(params): Query<ListBookmarksParams>,
) -> Response {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(MIN_LIMIT, MAX_LIMIT);

    let all = match state.bookmarks.list_all().await {
        Ok(all) => all,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_bookmarks",
                e.to_string(),
            )
        }
    };
    let mut pages_by_archive: HashMap<String, Vec<u32>> = HashMap::new();
    for bookmark in all {
        pages_by_archive
            .entry(bookmark.archive_id)
            .or_default()
            .push(bookmark.page);
    }

    // Two genuinely different data flows depending on where the sort key lives:
    //
    // `BookmarkedAt` — the key (each archive's most recent bookmark) comes from the bookmarks
    // Hash itself, no `Archive` lookup needed to sort. Sort first, slice to this page, *then*
    // fetch only the `limit` archives this page actually needs — O(limit) archive lookups.
    //
    // `Title`/`DateAdded` — the key lives on `Archive`, which means every candidate archive_id
    // has to be fetched before sorting is even possible. That fetch already produces everything
    // the final response needs, so the post-sort slice reuses those already-fetched `Archive`
    // values directly — deliberately NOT re-fetched a second time after slicing (`archives.get`
    // is two Redis round-trips each; doing this fetch once and reusing it, rather than once to
    // sort and again to serialize, is the entire reason this branch is structured as
    // fetch-then-sort-then-slice instead of sort-then-slice-then-fetch).
    let (page_ids, next_cursor, prefetched): (
        Vec<String>,
        Option<String>,
        Option<HashMap<String, Archive>>,
    ) = match params.sort {
        BookmarkSort::BookmarkedAt => {
            let latest = match state.bookmarks.latest_bookmark_per_archive().await {
                Ok(latest) => latest,
                Err(e) => {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "list_bookmarks",
                        e.to_string(),
                    )
                }
            };
            let mut ids: Vec<String> = pages_by_archive.keys().cloned().collect();
            ids.sort_by_key(|id| Reverse(latest.get(id).copied().unwrap_or(0)));
            let (page_ids, next_cursor) = paginate(&ids, params.cursor.as_deref(), limit);
            (page_ids, next_cursor, None)
        }
        BookmarkSort::Title | BookmarkSort::DateAdded => {
            let mut with_archive: Vec<(String, Archive)> =
                Vec::with_capacity(pages_by_archive.len());
            for id in pages_by_archive.keys() {
                if let Ok(Some(archive)) =
                    state.repos.archives.get(&ArchiveId::from(id.clone())).await
                {
                    with_archive.push((id.clone(), archive));
                }
            }
            match params.sort {
                BookmarkSort::Title => with_archive.sort_by(|a, b| a.1.title.cmp(&b.1.title)),
                BookmarkSort::DateAdded => {
                    with_archive.sort_by_key(|(_, a)| Reverse(a.date_added().unwrap_or(0)))
                }
                BookmarkSort::BookmarkedAt => unreachable!("handled in the branch above"),
            }
            let ids: Vec<String> = with_archive.iter().map(|(id, _)| id.clone()).collect();
            let (page_ids, next_cursor) = paginate(&ids, params.cursor.as_deref(), limit);
            let prefetched: HashMap<String, Archive> = with_archive.into_iter().collect();
            (page_ids, next_cursor, Some(prefetched))
        }
    };

    let mut entries = Vec::with_capacity(page_ids.len());
    for id in page_ids {
        let archive = match &prefetched {
            Some(map) => map.get(&id).cloned(),
            None => state
                .repos
                .archives
                .get(&ArchiveId::from(id.clone()))
                .await
                .ok()
                .flatten(),
        };
        let Some(archive) = archive else { continue };
        let mut pages = pages_by_archive.remove(&id).unwrap_or_default();
        pages.sort_unstable();
        entries.push(BookmarkedArchiveJson {
            archive: ArchiveMetadataJson::from(&archive),
            pages,
        });
    }

    axum::Json(json!({ "entries": entries, "next_cursor": next_cursor })).into_response()
}

#[derive(Debug, Serialize)]
struct BookmarkedPageJson {
    page: u32,
    /// The real in-archive filename for this page — same value `GET /archives/{id}/files`'s own
    /// `path` query param on each entry's `url` carries, computed the same way (`list_pages` +
    /// `effective_pages`, so a sidecar `.patch.zip`'s pages are named correctly too). Resolved
    /// here rather than making the caller (`BookmarkHoverGrid`) fetch the *entire* page list via
    /// `GET /archives/{id}/files` just to look up two or three filenames out of it — that endpoint
    /// scans the whole archive regardless of how many entries the caller actually needs, so this
    /// bundles the one scan this endpoint already has to do (to resolve *any* filename) into the
    /// same response instead of costing a second network round-trip on top of it. `None` if the
    /// page number doesn't correspond to any real entry (e.g. the archive was edited/re-scanned
    /// since the bookmark was added and it now has fewer pages).
    filename: Option<String>,
    /// This individual bookmark's own add time (Unix seconds) — already tracked in Redis
    /// (`BookmarksRepository::Bookmark::bookmarked_at`) but previously dropped before reaching
    /// this response. Needed for `BookmarkHoverGrid`'s own "sort by when each page was bookmarked"
    /// option (newest/oldest first) — distinct from the *archive*-level `bookmarks_updated_at`
    /// `GET /bookmarks`' own `sort=bookmarked_at` uses, which only tracks the archive's single
    /// most-recent bookmark event, not each individual page's own timestamp.
    bookmarked_at: u64,
    /// issue #97: how many stamps currently sit on this page — `0` when the page has none.
    /// Computed from the same archive fetch this response already does for `filename` resolution
    /// (`archive.stamp_ids`, grouped by page via `stamps::page_of`), not a second query.
    /// `BookmarkHoverGrid` uses this to render a stamp-count badge on bookmarked pages.
    stamp_count: u32,
}

/// `GET /archives/{id}/bookmarks` — unpaginated, just this one archive's bookmarked pages
/// (ascending, each with its resolved filename). Used by the reader (is the current page
/// bookmarked? — only `page` matters there) and `BookmarkHoverGrid` (needs both `page` and
/// `filename` for its caption, previously a second `GET /archives/{id}/files` call). Doesn't
/// require the archive to exist — same posture as `remove_bookmark`, a nonexistent archive simply
/// has no bookmarks to report (and no filenames to resolve).
async fn list_bookmarks_for_archive(
    State(state): State<AppState>,
    Path(archive_id): Path<ArchiveId>,
) -> Response {
    let bookmarks = match state.bookmarks.list_for_archive(archive_id.as_str()).await {
        Ok(bookmarks) => bookmarks,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_bookmarks_for_archive",
                e.to_string(),
            )
        }
    };
    let mut bookmarks = bookmarks;
    bookmarks.sort_unstable_by_key(|b| b.page);

    // Filenames come from the archive itself (there's no dedicated Redis field for them, same
    // reasoning `archives.rs::get_files` documents), so this needs one archive lookup plus one
    // archive-directory scan — but only one of each per request, not one per bookmarked page.
    // The same fetch also derives `stamp_counts` (issue #97) — no second archive lookup.
    let archive = state.repos.archives.get(&archive_id).await.ok().flatten();
    let filenames: HashMap<u32, String> = match &archive {
        Some(archive) => {
            let archive_path = std::path::Path::new(&archive.file);
            match lanrurugi_scanner::archive_format::list_pages(archive_path) {
                Ok(names) => {
                    let effective = lanrurugi_scanner::patch::effective_pages(archive_path, &names);
                    effective
                        .iter()
                        .enumerate()
                        .map(|(i, p)| ((i + 1) as u32, p.entry_name().to_string()))
                        .collect()
                }
                Err(_) => HashMap::new(),
            }
        }
        // Nonexistent archive or a storage error reading it — no filenames to resolve, but the
        // page numbers themselves are still real (they came from Redis, not from this lookup), so
        // this still returns them rather than failing the whole request.
        None => HashMap::new(),
    };
    let stamp_counts: HashMap<u32, u32> = match &archive {
        Some(archive) => {
            let mut counts = HashMap::new();
            for stamp_id in &archive.stamp_ids {
                if let Some(page) = crate::stamps::page_of(stamp_id.as_str()) {
                    *counts.entry(page).or_insert(0) += 1;
                }
            }
            counts
        }
        None => HashMap::new(),
    };

    let result: Vec<BookmarkedPageJson> = bookmarks
        .into_iter()
        .map(|b| BookmarkedPageJson {
            page: b.page,
            filename: filenames.get(&b.page).cloned(),
            bookmarked_at: b.bookmarked_at,
            stamp_count: stamp_counts.get(&b.page).copied().unwrap_or(0),
        })
        .collect();
    axum::Json(result).into_response()
}

/// `POST /archives/{id}/bookmarks/{page}` — bookmarks does not validate `page` against the
/// archive's real `pagecount`, matching `update_progress`'s own leniency (`archives.rs`) — the
/// reader is the only caller and always passes its own current, already-valid page.
async fn add_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((archive_id, page)): Path<(ArchiveId, u32)>,
) -> Response {
    let Some(archive) = state.repos.archives.get(&archive_id).await.ok().flatten() else {
        return not_found("add_bookmark", format!("{archive_id} does not exist."));
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let target = ActivityTarget {
        id: Some(archive_id.0.clone()),
        label: Some(archive.title.clone()),
        kind: Some("archive".to_string()),
    };
    match state.bookmarks.add(archive_id.as_str(), page, now).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_ADD,
                target,
                Outcome::Success,
                None,
                Some(json!({ "page": page })),
            )
            .await;
            ok(
                "add_bookmark",
                [("archive_id", json!(archive_id)), ("page", json!(page))],
            )
        }
        Err(e) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_ADD,
                target,
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                Some(json!({ "page": page })),
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_bookmark",
                e.to_string(),
            )
        }
    }
}

/// `DELETE /archives/{id}/bookmarks/{page}` — idempotent, same as
/// `BookmarksRepository::remove`'s own contract; doesn't check archive existence first since
/// removing a bookmark for an already-deleted archive is a legitimate cleanup, not an error. The
/// activity-log lookup below is a *best-effort* label fetch only, separate from that leniency — a
/// missing archive still lets the removal itself proceed, just with `target.label: None` (falls
/// back to `target.id` alone, same as any other action type whose resource vanished first).
async fn remove_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((archive_id, page)): Path<(ArchiveId, u32)>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let title = state
        .repos
        .archives
        .get(&archive_id)
        .await
        .ok()
        .flatten()
        .map(|a| a.title);
    let target = ActivityTarget {
        id: Some(archive_id.0.clone()),
        label: title,
        kind: Some("archive".to_string()),
    };
    match state.bookmarks.remove(archive_id.as_str(), page, now).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_REMOVE,
                target,
                Outcome::Success,
                None,
                Some(json!({ "page": page })),
            )
            .await;
            ok(
                "remove_bookmark",
                [("archive_id", json!(archive_id)), ("page", json!(page))],
            )
        }
        Err(e) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_REMOVE,
                target,
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                Some(json!({ "page": page })),
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_bookmark",
                e.to_string(),
            )
        }
    }
}

/// `GET /bookmarks/hover-page-order` — the saved `BookmarkHoverGrid` page-order preference
/// (`None` when never set; the frontend applies its own default, `"pageDesc"`, in that case).
async fn get_hover_page_order(State(state): State<AppState>) -> Response {
    match state.bookmarks.hover_page_order().await {
        Ok(order) => axum::Json(json!({ "order": order })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_hover_page_order",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct PutHoverPageOrderBody {
    order: String,
}

/// Not recorded to the activity log — a display-only preference, same posture `settings.rs`'s own
/// `theme` field takes (no `record_manual` call either).
async fn put_hover_page_order(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<PutHoverPageOrderBody>,
) -> Response {
    match state.bookmarks.set_hover_page_order(&body.order).await {
        Ok(()) => ok("put_hover_page_order", []),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "put_hover_page_order",
            e.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginate_first_page_reports_a_next_cursor_when_more_remain() {
        let items = vec!["a", "b", "c", "d", "e"];
        let (page, next) = paginate(&items, None, 2);
        assert_eq!(page, vec!["a", "b"]);
        assert_eq!(next, Some("2".to_string()));
    }

    #[test]
    fn paginate_middle_page_uses_the_cursor_as_an_offset() {
        let items = vec!["a", "b", "c", "d", "e"];
        let (page, next) = paginate(&items, Some("2"), 2);
        assert_eq!(page, vec!["c", "d"]);
        assert_eq!(next, Some("4".to_string()));
    }

    #[test]
    fn paginate_last_page_reports_no_next_cursor() {
        let items = vec!["a", "b", "c", "d", "e"];
        let (page, next) = paginate(&items, Some("4"), 2);
        assert_eq!(page, vec!["e"]);
        assert_eq!(next, None);
    }

    #[test]
    fn paginate_exact_multiple_still_reports_no_next_cursor_past_the_end() {
        let items = vec!["a", "b", "c", "d"];
        let (page, next) = paginate(&items, Some("2"), 2);
        assert_eq!(page, vec!["c", "d"]);
        assert_eq!(next, None);
    }

    #[test]
    fn paginate_cursor_past_the_end_yields_an_empty_page() {
        let items = vec!["a", "b"];
        let (page, next) = paginate(&items, Some("10"), 2);
        assert!(page.is_empty());
        assert_eq!(next, None);
    }

    #[test]
    fn paginate_malformed_cursor_falls_back_to_offset_zero() {
        let items = vec!["a", "b", "c"];
        let (page, _) = paginate(&items, Some("not-a-number"), 2);
        assert_eq!(page, vec!["a", "b"]);
    }

    #[test]
    fn paginate_empty_input_yields_an_empty_page_and_no_cursor() {
        let items: Vec<&str> = vec![];
        let (page, next) = paginate(&items, None, 10);
        assert!(page.is_empty());
        assert_eq!(next, None);
    }
}
