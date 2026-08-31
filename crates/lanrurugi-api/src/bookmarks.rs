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
use axum::routing::{get, post, put};
use axum::Router;
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::activity::record_manual;
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
        // Separate from the `POST`/`DELETE` route above (rather than overloading `PUT` onto that
        // same path) — a bookmark's name is edited far more often relative to add/remove than,
        // say, a category's `search`/`archives` fields are edited independently of each other, so
        // giving it its own explicit sub-path reads clearer than a `PUT` whose body shape has to
        // be inferred from context. `/name` also leaves room for a future non-name `PUT` on the
        // bare `{page}` path (e.g. editing `bookmarked_at` itself) without a collision.
        .route("/archives/{id}/bookmarks/{page}/name", put(rename_bookmark))
        .route(
            "/bookmarks/hover-page-order",
            get(get_hover_page_order).put(put_hover_page_order),
        )
        .route(
            "/bookmarks/only-matching",
            get(get_only_matching_bookmarks).put(put_only_matching_bookmarks),
        )
        // Tankoubon-global-page-addressed equivalents, same reasoning as `stamps.rs`'s own
        // `/tankoubons/{id}/stamps/{page}` — the reader addresses a Tankoubon's pages by a global
        // (Tankoubon-wide) number, so bookmarking one has to resolve that down to the real member
        // archive + local page server-side before it can reach `BookmarksRepository`, which only
        // ever knows about real archives. `GET /tankoubons/{id}/bookmarks` (unpaginated, mirrors
        // `list_bookmarks_for_archive`'s own contract) merges every member's own bookmarks back
        // into the Tankoubon's global page numbering — used by `BookmarkHoverGrid` when hovering a
        // Tankoubon card on `/bookmarks`.
        .route(
            "/tankoubons/{id}/bookmarks",
            get(list_bookmarks_for_tankoubon),
        )
        .route(
            "/tankoubons/{id}/bookmarks/{page}",
            post(add_tank_bookmark).delete(remove_tank_bookmark),
        )
        .route(
            "/tankoubons/{id}/bookmarks/{page}/name",
            put(rename_tank_bookmark),
        )
}

#[derive(Debug, Serialize)]
struct BookmarkedArchiveJson {
    /// A real archive (`ArchiveMetadataJson`-shaped) or, when one or more bookmarks sit on a
    /// Tankoubon member, a synthetic Tankoubon aggregate card — same `resolve_search_entry` shape
    /// the Library homepage's own `groupby_tanks` search results use (`arcid`/`title`/`tags`/
    /// `archive_count`/etc., a real archive's own `archive_count` always `null`), so the frontend
    /// renders both through the one `ArchiveCard`/`CarouselCard` component it already has. `Value`
    /// rather than `ArchiveMetadataJson` because a Tankoubon card carries `archive_count` (and
    /// other fields `ArchiveMetadataJson` doesn't have at all), same reason `search.rs`'s own
    /// `resolve_search_entry` returns `Value` instead of that struct too.
    archive: serde_json::Value,
    /// Ascending, in the *displayed* card's own page numbering — a real archive's local page
    /// numbers, or (for a Tankoubon card) its Tankoubon-global page numbers, i.e. exactly what
    /// `GET /tankoubons/{id}/bookmarks`/`GET /archives/{id}/bookmarks` themselves return. The
    /// first entry is what a hover preview should treat as "the" cover-aligned thumbnail.
    pages: Vec<u32>,
}

/// What a single bookmarked archive_id normalizes to for display/grouping purposes — either
/// itself (not a Tankoubon member, or a member of none) or the id of a Tankoubon it belongs to.
/// An archive that's a member of more than one Tankoubon (issue #67 — the reverse index this is
/// built from is many-to-many) picks the *first* one returned by `GroupingRepository::for_archive`
/// deterministically only insofar as that method's own ordering is — acceptable here the same way
/// `resolve_search_entry`'s own Tankoubon-membership handling doesn't attempt to disambiguate
/// "which Tankoubon" either; a bookmark still shows up exactly once rather than being dropped or
/// duplicated, which is the actual requirement.
async fn representative_id(state: &AppState, archive_id: &str) -> String {
    match state
        .repos
        .groupings
        .for_archive(&ArchiveId::from(archive_id.to_string()))
        .await
    {
        Ok(groupings) if !groupings.is_empty() => groupings[0].tankid.to_string(),
        _ => archive_id.to_string(),
    }
}

/// `q` filter for `GET /bookmarks` — `id` is a representative id (a real archive, or a Tankoubon
/// per [`representative_id`]'s own docs), `pages` its already-collapsed `(page, name)` list
/// (`pages_by_id`'s own value, unchanged since `list_bookmarks` built it straight from
/// `BookmarksRepository::list_all` — for a Tankoubon representative this is every bookmark from
/// *every* one of its members folded together, member-local page numbers, matching
/// `pages_by_id`'s own doc comment on that field). `q_lower` is already trimmed and lowercased by
/// the caller so this doesn't repeat that work per candidate.
///
/// `q_lower` is split on whitespace into keywords (like a plain text search, not one literal
/// phrase) — every keyword must appear as a substring of *some* searchable field, but different
/// keywords may each match a different field. Searchable fields: the real archive's title, its
/// basename (`Archive::name`), any of its bookmarks' own names, and any of its bookmarks' own
/// page numbers (decimal, substring — `"2"` matches page `12`, not just a literal page `2`). A
/// Tankoubon's own `name` (not any one member's title — a Tankoubon has no single "basename" of
/// its own) stands in for title/basename, plus the same bookmark-name/page-number check across
/// its collapsed member list.
async fn card_matches_query(
    state: &AppState,
    id: &str,
    pages: &[(u32, Option<String>)],
    q_lower: &str,
) -> bool {
    let keywords: Vec<&str> = q_lower.split_whitespace().collect();
    if keywords.is_empty() {
        return true;
    }

    let mut fields: Vec<String> = Vec::with_capacity(pages.len() * 2 + 1);
    for (page, name) in pages {
        if let Some(n) = name {
            fields.push(n.to_lowercase());
        }
        fields.push(page.to_string());
    }
    if let Some(tankid) = id.strip_prefix("TANK_").map(|_| id) {
        let Ok(Some(grouping)) = state
            .repos
            .groupings
            .get(&lanrurugi_core::ids::TankId(tankid.to_string()))
            .await
        else {
            return false;
        };
        fields.push(grouping.name.to_lowercase());
    } else {
        let Ok(Some(archive)) = state
            .repos
            .archives
            .get(&ArchiveId::from(id.to_string()))
            .await
        else {
            return false;
        };
        fields.push(archive.title.to_lowercase());
        fields.push(archive.name.to_lowercase());
    }

    keywords
        .iter()
        .all(|kw| fields.iter().any(|f| f.contains(kw)))
}

/// Clamped to a small, fixed range regardless of what the client asks for — an unbounded `limit`
/// would force an arbitrarily large in-memory sort/scan every request (same reasoning
/// `activity.rs`'s own `MIN_LIMIT`/`MAX_LIMIT` gives for its Redis-side query, just applied here
/// to the in-memory candidate set instead).
const MIN_LIMIT: usize = 1;
const MAX_LIMIT: usize = 200;
const DEFAULT_LIMIT: usize = 50;

/// Counted in grapheme clusters (`unicode_segmentation::UnicodeSegmentation::graphemes`), not
/// bytes and not bare `char`s either — a name in a multi-byte script (CJK) or carrying a combined
/// emoji shouldn't hit a tighter effective limit than an equal-length ASCII one just because it's
/// encoded (or represented as Unicode scalar values) wider. `char`s alone still over-counts a
/// combined/ZWJ-sequence emoji badly: a single visually-one-glyph family emoji (👨‍👩‍👧‍👦) is 7
/// Unicode scalar values (4 people + 3 zero-width joiners), so `str::chars().count()` would charge
/// it 7 toward the limit for something a user perceives as one character; grapheme-cluster
/// segmentation (Unicode Annex #29, what `graphemes(true)` implements) counts it as the single
/// user-perceived character it actually is.
const MAX_BOOKMARK_NAME_LEN: usize = 200;

#[derive(Debug, Deserialize)]
struct RenameBookmarkBody {
    /// `None`/an all-whitespace string clears the name (`BookmarksRepository::set_name`'s own
    /// docs) — this field is `Option` rather than a bare `String` so an explicit `null` in the
    /// request body reads the same way as an omitted field, both meaning "no name," distinct from
    /// `Some("")`, which also clears (trimmed to empty) but at least confirms the client meant to
    /// send *something*.
    name: Option<String>,
}

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
    /// Free-text filter, matched case-insensitively against the resolved card's own title (real
    /// archive title, or a Tankoubon's own `name`), its basename (`Archive::name`, a Tankoubon has
    /// none), every one of its bookmarks' own names, and every bookmark's page number rendered as
    /// a decimal string — see `card_matches_query`'s own docs for the exact matching rule per
    /// field. `None`/empty behaves exactly as if this param were never sent (every currently
    /// bookmarked card is a candidate), matching `cursor`'s own "absent means start from the
    /// beginning" default.
    q: Option<String>,
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
    // Normalize each bookmark's real `archive_id` to its *representative* id first — a bare
    // archive, or (when it's currently a Tankoubon member) that Tankoubon's own id — same
    // "aggregate at the source, don't list members separately" principle the Library homepage's
    // own `groupby_tanks` search candidate pool applies (`lanrurugi-search::engine`'s
    // `LRR_TANKGROUPED` set), just computed per-bookmark here rather than via a Redis Set, since
    // this endpoint's candidate set is "every archive_id with a bookmark," not the whole library.
    // Bookmarks from two different members of the *same* Tankoubon collapse onto that one
    // representative id here, in this map, before sorting/pagination ever sees them — the entire
    // reason a Tankoubon shows up as one card instead of once per bookmarked member. Each entry
    // keeps its own `name` alongside its `page` (not just the page) — `q` (below) needs it, and
    // `entries`' own final render still only reads the `page` half.
    let mut pages_by_id: HashMap<String, Vec<(u32, Option<String>)>> = HashMap::new();
    for bookmark in all {
        let rep = representative_id(&state, &bookmark.archive_id).await;
        pages_by_id
            .entry(rep)
            .or_default()
            .push((bookmark.page, bookmark.name));
    }

    // `q`: drop every representative id whose card doesn't match before sorting/pagination ever
    // sees it — same "filter first" ordering `Title`/`DateAdded` sort already forces search.rs's
    // own `groupby_tanks` engine to do (a query result set has to be complete *before* an
    // offset-based cursor can slice a stable page out of it, same "known, accepted limitation"
    // `paginate`'s own docs already call out for a bookmark added/removed between two fetches).
    if let Some(q) = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let q_lower = q.to_lowercase();
        let mut matched = HashMap::with_capacity(pages_by_id.len());
        for (id, pages) in pages_by_id {
            if card_matches_query(&state, &id, &pages, &q_lower).await {
                matched.insert(id, pages);
            }
        }
        pages_by_id = matched;
    }

    // Two genuinely different data flows depending on where the sort key lives:
    //
    // `BookmarkedAt` — the key (each *representative* id's most recent bookmark event across all
    // its real underlying archive_ids) comes from the bookmarks Hash itself, no further lookup
    // needed to sort. Sort first, slice to this page, *then* resolve only the `limit` cards this
    // page actually needs.
    //
    // `Title`/`DateAdded` — the key lives on the resolved entity itself (a real `Archive`'s
    // `title`/`date_added`, or a Tankoubon `Grouping`'s own `name` / its earliest member's
    // `date_added`), which means every candidate id has to be resolved before sorting is even
    // possible. That resolution already produces (most of) what the final response needs.
    let (page_ids, next_cursor): (Vec<String>, Option<String>) = match params.sort {
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
            // A representative id's own "most recent bookmark" is the max across every real
            // archive_id folded into it (there's no single `LANRURUGI_BOOKMARKS_UPDATED_AT` entry
            // for a synthetic Tankoubon id, since bookmarks never actually write one there).
            let mut latest_by_rep: HashMap<String, u64> = HashMap::new();
            for (archive_id, ts) in &latest {
                let rep = representative_id(&state, archive_id).await;
                let entry = latest_by_rep.entry(rep).or_insert(0);
                *entry = (*entry).max(*ts);
            }
            let mut ids: Vec<String> = pages_by_id.keys().cloned().collect();
            ids.sort_by_key(|id| Reverse(latest_by_rep.get(id).copied().unwrap_or(0)));
            paginate(&ids, params.cursor.as_deref(), limit)
        }
        BookmarkSort::Title | BookmarkSort::DateAdded => {
            // The sort key lives on the resolved entity itself (a real `Archive`'s `title`/
            // `date_added`, or a Tankoubon `Grouping`'s own `name` / its earliest member's
            // `date_added`), so every candidate id has to be resolved before sorting is possible.
            // This resolution is sort-only — the card itself is still rendered from scratch via
            // `resolve_search_entry` once pagination has picked the final page (below), the single
            // source of truth for a card's JSON shape (real or Tankoubon), so there's no
            // `prefetched`/reuse bookkeeping to carry between the two.
            enum SortKey {
                Title(String),
                DateAdded(u64),
            }
            let mut with_key: Vec<(String, SortKey)> = Vec::with_capacity(pages_by_id.len());
            for id in pages_by_id.keys() {
                if let Some(tankid) = id.strip_prefix("TANK_").map(|_| id.clone()) {
                    let Ok(Some(grouping)) = state
                        .repos
                        .groupings
                        .get(&lanrurugi_core::ids::TankId(tankid))
                        .await
                    else {
                        continue;
                    };
                    let key = match params.sort {
                        BookmarkSort::Title => SortKey::Title(grouping.name),
                        BookmarkSort::DateAdded => {
                            let mut earliest = u64::MAX;
                            for member in &grouping.archives {
                                if let Ok(Some(a)) = state.repos.archives.get(member).await {
                                    earliest = earliest.min(a.date_added().unwrap_or(u64::MAX));
                                }
                            }
                            SortKey::DateAdded(if earliest == u64::MAX { 0 } else { earliest })
                        }
                        BookmarkSort::BookmarkedAt => unreachable!("handled in the branch above"),
                    };
                    with_key.push((id.clone(), key));
                } else if let Ok(Some(archive)) =
                    state.repos.archives.get(&ArchiveId::from(id.clone())).await
                {
                    let key = match params.sort {
                        BookmarkSort::Title => SortKey::Title(archive.title),
                        BookmarkSort::DateAdded => {
                            SortKey::DateAdded(archive.date_added().unwrap_or(0))
                        }
                        BookmarkSort::BookmarkedAt => unreachable!("handled in the branch above"),
                    };
                    with_key.push((id.clone(), key));
                }
            }
            match params.sort {
                BookmarkSort::Title => with_key.sort_by(|a, b| match (&a.1, &b.1) {
                    (SortKey::Title(x), SortKey::Title(y)) => x.cmp(y),
                    _ => unreachable!(),
                }),
                BookmarkSort::DateAdded => with_key.sort_by_key(|(_, k)| match k {
                    SortKey::DateAdded(d) => Reverse(*d),
                    _ => unreachable!(),
                }),
                BookmarkSort::BookmarkedAt => unreachable!("handled in the branch above"),
            }
            let ids: Vec<String> = with_key.into_iter().map(|(id, _)| id).collect();
            paginate(&ids, params.cursor.as_deref(), limit)
        }
    };

    let mut entries = Vec::with_capacity(page_ids.len());
    for id in page_ids {
        let Some(archive_json) = crate::search::resolve_search_entry(&state, &id).await else {
            continue;
        };
        let mut pages: Vec<u32> = pages_by_id
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .map(|(page, _name)| page)
            .collect();
        pages.sort_unstable();
        // A Tankoubon representative's own `pages` are still real member-local page numbers at
        // this point (each bookmark's own `page` field, unchanged since `pages_by_id` was built
        // straight from `BookmarksRepository::list_all`) — not yet translated into the
        // Tankoubon's global page numbering `GET /tankoubons/{id}/bookmarks` returns. Left as-is
        // here deliberately: `BookmarkHoverGrid`'s own Tankoubon branch re-fetches the full,
        // correctly-translated list from that endpoint rather than trusting this summary field,
        // the same way it already re-fetches a plain archive's own pages instead of trusting the
        // `pages` prop it's handed (see that component's own docs on why `pages` is only ever a
        // first-paint snapshot). This field's only remaining purpose for a Tankoubon card is
        // `pages.len()` / `pages[0]`-style "has bookmarks at all" checks, which member-local
        // numbers already answer correctly.
        entries.push(BookmarkedArchiveJson {
            archive: archive_json,
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
    /// This bookmark's own optional user-given name — `None` for a bookmark never named (which
    /// includes every bookmark saved before this field existed, `BookmarksRepository::list_all`'s
    /// own docs on `NAMES_HASH_KEY`).
    name: Option<String>,
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
            name: b.name,
        })
        .collect();
    axum::Json(result).into_response()
}

/// `PUT /archives/{id}/bookmarks/{page}/name` — sets or clears this bookmark's own name.
/// `404`s if the bookmark itself doesn't exist yet (a name can't outlive the bookmark it names,
/// and setting one on a nonexistent bookmark would silently create an orphaned `NAMES_HASH_KEY`
/// entry no `add_bookmark` call ever cleans up) — matches `add_stamp`'s own "archive must exist
/// first" posture, just checking bookmark existence instead of archive existence.
async fn rename_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((archive_id, page)): Path<(ArchiveId, u32)>,
    axum::Json(body): axum::Json<RenameBookmarkBody>,
) -> Response {
    match state
        .bookmarks
        .is_bookmarked(archive_id.as_str(), page)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return not_found(
                "rename_bookmark",
                format!("no bookmark on page {page} of {archive_id}."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "rename_bookmark",
                e.to_string(),
            )
        }
    }
    // Trimmed and length-capped here (not left to the frontend alone to enforce) — the same
    // "server is the actual source of truth for validation, a client-side check is only a UX
    // nicety" posture every other user-text field in this codebase takes. An all-whitespace name
    // trims down to empty and clears the name, same as an explicitly empty string would.
    let name = body.name.as_deref().map(str::trim);
    if let Some(name) = name {
        use unicode_segmentation::UnicodeSegmentation;
        if name.graphemes(true).count() > MAX_BOOKMARK_NAME_LEN {
            return error(
                StatusCode::BAD_REQUEST,
                "rename_bookmark",
                format!("name must be at most {MAX_BOOKMARK_NAME_LEN} characters."),
            );
        }
    }
    let archive_title = state
        .repos
        .archives
        .get(&archive_id)
        .await
        .ok()
        .flatten()
        .map(|a| a.title);
    let target = ActivityTarget {
        id: Some(archive_id.0.clone()),
        label: archive_title,
        kind: Some("archive".to_string()),
    };
    match state
        .bookmarks
        .set_name(archive_id.as_str(), page, name)
        .await
    {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_RENAME,
                target,
                Outcome::Success,
                None,
                Some(json!({ "page": page, "name": name })),
            )
            .await;
            ok(
                "rename_bookmark",
                [
                    ("archive_id", json!(archive_id)),
                    ("page", json!(page)),
                    ("name", json!(name)),
                ],
            )
        }
        Err(e) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::BOOKMARK_RENAME,
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
                "rename_bookmark",
                e.to_string(),
            )
        }
    }
}

/// `PUT /tankoubons/{id}/bookmarks/{page}/name` — same resolve-then-delegate shape as
/// `add_tank_bookmark`/`remove_tank_bookmark`, onto [`rename_bookmark`].
async fn rename_tank_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((tank_id, page)): Path<(TankId, u32)>,
    axum::Json(body): axum::Json<RenameBookmarkBody>,
) -> Response {
    let (archive_id, local_page) =
        match crate::tankoubons::resolve_tank_page(&state, &tank_id, "rename_bookmark", page).await
        {
            Ok(v) => v,
            Err(r) => return r,
        };
    rename_bookmark(
        State(state),
        auth,
        Path((archive_id, local_page)),
        axum::Json(body),
    )
    .await
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

/// `POST /tankoubons/{id}/bookmarks/{page}` — resolves the Tankoubon-global `page` down to the
/// real member archive + local page (`tankoubons::resolve_tank_page`) and delegates to
/// [`add_bookmark`]'s own logic; the write itself still lands on the real archive id, same as a
/// stamp placed while reading a Tankoubon (`stamps.rs`'s own Tankoubon routes) — `bookmarks.rs`'s
/// underlying `BookmarksRepository` has no `TANK_` concept of its own and never needs one.
async fn add_tank_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((tank_id, page)): Path<(TankId, u32)>,
) -> Response {
    let (archive_id, local_page) =
        match crate::tankoubons::resolve_tank_page(&state, &tank_id, "add_bookmark", page).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    add_bookmark(State(state), auth, Path((archive_id, local_page))).await
}

/// `DELETE /tankoubons/{id}/bookmarks/{page}` — same resolve-then-delegate shape as
/// [`add_tank_bookmark`], onto [`remove_bookmark`].
async fn remove_tank_bookmark(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((tank_id, page)): Path<(TankId, u32)>,
) -> Response {
    let (archive_id, local_page) =
        match crate::tankoubons::resolve_tank_page(&state, &tank_id, "remove_bookmark", page).await
        {
            Ok(v) => v,
            Err(r) => return r,
        };
    remove_bookmark(State(state), auth, Path((archive_id, local_page))).await
}

#[derive(Debug, Serialize)]
struct TankBookmarkedPageJson {
    /// Tankoubon-global page number — what the reader/`BookmarkHoverGrid` actually navigates to
    /// (`?p={page}` on the Tankoubon's own reader route), not any one member's local page.
    page: u32,
    /// This member archive's own local page number that `page` resolves to — shown alongside
    /// `page` so the label can read "chapter N, local page L (global page G)" rather than only
    /// the global number, which alone doesn't say how far into that specific chapter the bookmark
    /// actually sits.
    local_page: u32,
    /// This member archive's own total page count — alongside `local_page`, lets the label read
    /// "chapter N, page L/LL (global page G)" rather than a bare local page number that doesn't
    /// say how far into that specific chapter it is.
    local_pagecount: u32,
    /// 0-based position of the member archive this bookmark belongs to, in the Tankoubon's own
    /// reading order — the "chapter number" half of the label above.
    archive_index: usize,
    archive_id: String,
    filename: Option<String>,
    bookmarked_at: u64,
    stamp_count: u32,
    name: Option<String>,
}

/// `GET /tankoubons/{id}/bookmarks` — every bookmark across every member archive, translated into
/// the Tankoubon's own global page numbering. Unpaginated, ascending by global page — matches
/// `list_bookmarks_for_archive`'s own contract (used the same way, by `BookmarkHoverGrid`, just
/// for a Tankoubon card instead of a plain archive card). A member archive that no longer exists
/// (or has an empty `pagecount`, e.g. not yet scanned) contributes nothing, silently, rather than
/// failing the whole request — same tolerance `TankoubonFullResponse`'s own docs describe for
/// `full_data` silently omitting a missing member.
async fn list_bookmarks_for_tankoubon(
    State(state): State<AppState>,
    Path(tank_id): Path<TankId>,
) -> Response {
    let grouping = match state.repos.groupings.get(&tank_id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "list_bookmarks_for_tankoubon",
                format!("{tank_id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_bookmarks_for_tankoubon",
                e.to_string(),
            )
        }
    };

    let mut result = Vec::new();
    for (index, archive_id) in grouping.archives.iter().enumerate() {
        let bookmarks = match state.bookmarks.list_for_archive(archive_id.as_str()).await {
            Ok(b) => b,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "list_bookmarks_for_tankoubon",
                    e.to_string(),
                )
            }
        };
        if bookmarks.is_empty() {
            continue;
        }
        let archive = state.repos.archives.get(archive_id).await.ok().flatten();
        let filenames: HashMap<u32, String> = match &archive {
            Some(archive) => {
                let archive_path = std::path::Path::new(&archive.file);
                match lanrurugi_scanner::archive_format::list_pages(archive_path) {
                    Ok(names) => {
                        let effective =
                            lanrurugi_scanner::patch::effective_pages(archive_path, &names);
                        effective
                            .iter()
                            .enumerate()
                            .map(|(i, p)| ((i + 1) as u32, p.entry_name().to_string()))
                            .collect()
                    }
                    Err(_) => HashMap::new(),
                }
            }
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

        // Same cumulative-offset math as `GroupingRepository::resolve_local_page`, but computed
        // once per member here (via `Grouping::archives`, already loaded) rather than one query
        // per bookmark — this loop already knows `index`, so it doesn't need that method's own
        // per-archive `for_archive` reverse-index lookup.
        let mut offset = 0u32;
        for id in &grouping.archives[..index] {
            offset += state
                .repos
                .archives
                .get(id)
                .await
                .ok()
                .flatten()
                .map(|a| a.pagecount)
                .unwrap_or(0);
        }

        let local_pagecount = archive.as_ref().map(|a| a.pagecount).unwrap_or(0);
        for b in bookmarks {
            result.push(TankBookmarkedPageJson {
                page: offset + b.page,
                local_page: b.page,
                local_pagecount,
                archive_index: index,
                archive_id: archive_id.to_string(),
                filename: filenames.get(&b.page).cloned(),
                bookmarked_at: b.bookmarked_at,
                stamp_count: stamp_counts.get(&b.page).copied().unwrap_or(0),
                name: b.name,
            });
        }
    }
    result.sort_unstable_by_key(|b| b.page);

    axum::Json(result).into_response()
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

/// `GET /bookmarks/only-matching` — the saved "only show bookmarks that themselves matched `q`"
/// preference (`false` when never set, matching the historical always-show-everything behavior).
async fn get_only_matching_bookmarks(State(state): State<AppState>) -> Response {
    match state.bookmarks.only_matching_bookmarks().await {
        Ok(value) => axum::Json(json!({ "only_matching": value })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_only_matching_bookmarks",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct PutOnlyMatchingBookmarksBody {
    only_matching: bool,
}

/// Not recorded to the activity log — same display-only posture `put_hover_page_order` takes.
async fn put_only_matching_bookmarks(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<PutOnlyMatchingBookmarksBody>,
) -> Response {
    match state
        .bookmarks
        .set_only_matching_bookmarks(body.only_matching)
        .await
    {
        Ok(()) => ok("put_only_matching_bookmarks", []),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "put_only_matching_bookmarks",
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

    fn test_archive(id: &str) -> lanrurugi_core::entities::Archive {
        lanrurugi_core::entities::Archive {
            id: ArchiveId(id.to_string()),
            name: format!("{id}-basename"),
            title: format!("{id}-title"),
            file: format!("/nonexistent/{id}.zip"),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 10,
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

    #[tokio::test]
    async fn rename_bookmark_404s_when_the_bookmark_does_not_exist() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_id = format!("bm-rename-missing-{}", uuid::Uuid::new_v4());
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id), 1)),
            axum::Json(RenameBookmarkBody {
                name: Some("won't stick".to_string()),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rename_bookmark_sets_trims_and_clears_a_name() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_id = format!("bm-rename-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 3, 0).await.unwrap();

        // Leading/trailing whitespace is trimmed before it's stored.
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 3)),
            axum::Json(RenameBookmarkBody {
                name: Some("  决战前夜  ".to_string()),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);
        let bookmarks = state.bookmarks.list_for_archive(&archive_id).await.unwrap();
        assert_eq!(bookmarks[0].name, Some("决战前夜".to_string()));

        // An all-whitespace name clears it, same as an explicitly empty one.
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 3)),
            axum::Json(RenameBookmarkBody {
                name: Some("   ".to_string()),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);
        let bookmarks = state.bookmarks.list_for_archive(&archive_id).await.unwrap();
        assert_eq!(bookmarks[0].name, None);

        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 3, 0).await.unwrap();
    }

    #[tokio::test]
    async fn rename_bookmark_rejects_a_name_over_the_length_limit() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_id = format!("bm-rename-toolong-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 1, 0).await.unwrap();

        let too_long = "x".repeat(MAX_BOOKMARK_NAME_LEN + 1);
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 1)),
            axum::Json(RenameBookmarkBody {
                name: Some(too_long),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::BAD_REQUEST);
        // The rejected name never got written.
        let bookmarks = state.bookmarks.list_for_archive(&archive_id).await.unwrap();
        assert_eq!(bookmarks[0].name, None);

        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 1, 0).await.unwrap();
    }

    /// A combined/ZWJ-sequence emoji must count as the single user-perceived character it visually
    /// is, not as however many Unicode scalar values make it up — `MAX_BOOKMARK_NAME_LEN`'s own
    /// docs cover why a plain `chars().count()` would get this wrong (a family emoji alone is 7
    /// scalar values). 200 repetitions of a real family emoji (👨‍👩‍👧‍👦, 7 scalar values each) is
    /// exactly at the grapheme-cluster limit — accepted — while 201 repetitions goes one grapheme
    /// cluster over — rejected. A naive `chars().count()` check would have rejected both (1,400 and
    /// 1,407 scalar values respectively), so this test would fail under the old implementation.
    #[tokio::test]
    async fn rename_bookmark_counts_a_combined_emoji_as_one_grapheme_cluster_not_seven_chars() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_id = format!("bm-rename-emoji-{}", uuid::Uuid::new_v4());
        state
            .repos
            .archives
            .save(&test_archive(&archive_id))
            .await
            .unwrap();
        state.bookmarks.add(&archive_id, 1, 0).await.unwrap();

        let family_emoji = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
        let at_limit = family_emoji.repeat(MAX_BOOKMARK_NAME_LEN);
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 1)),
            axum::Json(RenameBookmarkBody {
                name: Some(at_limit.clone()),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::OK);
        let bookmarks = state.bookmarks.list_for_archive(&archive_id).await.unwrap();
        assert_eq!(bookmarks[0].name, Some(at_limit));

        let over_limit = family_emoji.repeat(MAX_BOOKMARK_NAME_LEN + 1);
        let resp = rename_bookmark(
            State(state.clone()),
            None,
            Path((ArchiveId(archive_id.clone()), 1)),
            axum::Json(RenameBookmarkBody {
                name: Some(over_limit),
            }),
        )
        .await;
        assert_eq!(resp.into_response().status(), StatusCode::BAD_REQUEST);

        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id.clone()))
            .await
            .unwrap();
        state.bookmarks.remove(&archive_id, 1, 0).await.unwrap();
    }

    #[tokio::test]
    async fn card_matches_query_matches_title_basename_bookmark_name_and_page_number() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archive_id = format!("bm-search-{}", uuid::Uuid::new_v4());
        let mut archive = test_archive(&archive_id);
        archive.title = "The Great Battle".to_string();
        archive.name = "great-battle-basename".to_string();
        state.repos.archives.save(&archive).await.unwrap();

        let pages = vec![(12u32, Some("决战前夜".to_string())), (7u32, None)];

        // Title match, case-insensitive.
        assert!(card_matches_query(&state, &archive_id, &pages, "great battle").await);
        // Basename match.
        assert!(card_matches_query(&state, &archive_id, &pages, "basename").await);
        // Bookmark-name match.
        assert!(card_matches_query(&state, &archive_id, &pages, "决战").await);
        // Page-number substring match — "2" matches page 12, not just a literal page 2.
        assert!(card_matches_query(&state, &archive_id, &pages, "12").await);
        // No match at all.
        assert!(!card_matches_query(&state, &archive_id, &pages, "nonexistent-query").await);
        // Multi-keyword AND — each keyword matches a *different* field (title vs. basename), not
        // one literal phrase; whitespace-tolerant. `card_matches_query` expects an already-
        // lowercased `q_lower` (its caller lowercases before calling), so the mixed-case
        // "BASENAME" here — unlike a real caller — must be pre-lowered by the test itself.
        assert!(card_matches_query(&state, &archive_id, &pages, "great basename").await);
        assert!(card_matches_query(&state, &archive_id, &pages, "  battle   决战  ").await);
        // One keyword matches, the other doesn't — the AND must still fail overall.
        assert!(!card_matches_query(&state, &archive_id, &pages, "great nonexistent").await);

        state
            .repos
            .archives
            .delete(&ArchiveId(archive_id))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn card_matches_query_matches_a_tankoubon_by_its_own_name() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let tankid = lanrurugi_core::ids::TankId(format!(
            "TANK_{}",
            uuid::Uuid::new_v4().as_u128() % 10_000_000_000
        ));
        state
            .repos
            .groupings
            .save(&lanrurugi_core::entities::Grouping {
                tankid: tankid.clone(),
                name: "My Series".to_string(),
                summary: String::new(),
                tags: String::new(),
                progress: 0,
                archives: vec![],
                thumbnail_manual: false,
                thumbnail_source_archive: None,
                thumbnail_source_page: None,
                chapter_names: Default::default(),
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();

        let pages: Vec<(u32, Option<String>)> = vec![];
        assert!(card_matches_query(&state, tankid.as_str(), &pages, "my series").await);
        assert!(!card_matches_query(&state, tankid.as_str(), &pages, "unrelated").await);

        state.repos.groupings.delete(&tankid).await.unwrap();
    }
}
