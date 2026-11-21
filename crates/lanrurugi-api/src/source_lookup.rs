//! `POST /archives/lookup` — the batch counterpart of `GET /search?filter=source:<url>`.
//!
//! A thin client checking "is this gallery already in the library?" had to ask once per gallery,
//! and every one of those calls is a full search: a `KEYS` scan of the archive keyspace, a title
//! `ZSCAN`, and (with grouping on) a second `KEYS` for Tankoubons. Bulk-checking a clipboard list
//! that way is hundreds of those walks. This endpoint answers the same question for a whole list
//! from the tag index alone.
//!
//! Read-only in spite of the verb (like `POST /plugin-wizard/lookup`), and deliberately outside the
//! legacy-mirroring `/search*` group: `source:` here means "the archive carrying this source tag",
//! with no Tankoubon folding, no sorting, no pagination and no query syntax.

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use lanrurugi_storage::keys::PENDING_VISIBILITY_SET;

use crate::common::{error, ok};
use crate::AppState;

/// Upper bound on one request. Clients are expected to chunk well below this (the e-hentai
/// userscript uses 200); the cap keeps one request's Redis pipelines bounded no matter what is
/// sent, and one request body far below axum's own 2 MB limit.
const MAX_SOURCES: usize = 500;

pub fn router() -> Router<AppState> {
    Router::new().route("/archives/lookup", post(lookup_sources))
}

#[derive(Debug, Deserialize)]
pub struct LookupRequest {
    /// Sources in any spelling a plugin may have stored: a bare `host/path`, a full URL with either
    /// scheme, with or without a trailing slash, optionally already prefixed with `source:`.
    pub sources: Vec<String>,
}

/// Answer "which archives carry this source tag?" for a batch of sources.
///
/// One entry per distinct input, keyed by exactly the string that was sent (trimmed), so a client
/// never has to guess which normalization was applied. `arcids` is a list, not a single id: two
/// archives can carry the same `source:` tag after a duplicate import or a repair. A source no
/// archived record answers comes back with an empty list rather than being omitted.
///
/// `title` is the first surviving archive's own title, or `null` — enough for a client to label a
/// hit without a second request.
///
/// Answers at the archive level: an archive filed into a Tankoubon is still reported (grouping
/// never hides it here), and a Tankoubon's own aggregate id is never reported as an archive.
///
/// A record is only reported when it is readable: archives catalogued but not yet published
/// (`PENDING_VISIBILITY_SET`) and records whose file is still on its way in (an empty `file`) are
/// omitted, matching what `/search` shows and what the reader can actually open.
///
/// At most 500 sources per request; larger lists are rejected rather than truncated. Fails with
/// `400` for an empty list.
async fn lookup_sources(
    State(state): State<AppState>,
    Json(request): Json<LookupRequest>,
) -> Response {
    if request.sources.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "lookup_sources",
            "No sources given.",
        );
    }
    if request.sources.len() > MAX_SOURCES {
        return error(
            StatusCode::BAD_REQUEST,
            "lookup_sources",
            format!("At most {MAX_SOURCES} sources per request; split larger lists."),
        );
    }

    // Deduplicate while keeping the caller's own spelling — the response is keyed by it.
    let mut unique: Vec<String> = Vec::with_capacity(request.sources.len());
    let mut seen: HashSet<&str> = HashSet::with_capacity(request.sources.len());
    for source in &request.sources {
        let trimmed = source.trim();
        if !trimmed.is_empty() && seen.insert(trimmed) {
            unique.push(trimmed.to_string());
        }
    }
    if unique.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "lookup_sources",
            "No sources given.",
        );
    }

    let source_equivalences = crate::settings::read_source_equivalences(&state).await;
    let ids_by_source = match lanrurugi_search::engine::lookup_sources(
        &state.redis.search,
        &state.equivalence,
        &unique,
        &source_equivalences,
    )
    .await
    {
        Ok(ids) => ids,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "lookup_sources",
                e.to_string(),
            )
        }
    };

    let mut archive_conn = match state.redis.archive.get().await {
        Ok(conn) => conn,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "lookup_sources",
                e.to_string(),
            )
        }
    };
    let unpublished: HashSet<String> = archive_conn
        .smembers(PENDING_VISIBILITY_SET)
        .await
        .unwrap_or_default();

    // One round trip for every candidate record, rather than one per id: `file` answers "are the
    // bytes here yet", `title` labels the hit.
    let mut wanted: Vec<String> = Vec::new();
    {
        let mut queued: HashSet<&String> = HashSet::new();
        for ids in &ids_by_source {
            for id in ids {
                if !unpublished.contains(id) && queued.insert(id) {
                    wanted.push(id.clone());
                }
            }
        }
    }
    let mut fetched: HashMap<String, (String, String)> = HashMap::with_capacity(wanted.len());
    if !wanted.is_empty() {
        let mut pipe = deadpool_redis::redis::pipe();
        for id in &wanted {
            let mut cmd = deadpool_redis::redis::cmd("HMGET");
            cmd.arg(id).arg("title").arg("file");
            pipe.add_command(cmd);
        }
        let rows: std::result::Result<Vec<Vec<Option<String>>>, _> =
            pipe.query_async(&mut *archive_conn).await;
        let rows = match rows {
            Ok(rows) => rows,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "lookup_sources",
                    e.to_string(),
                )
            }
        };
        for (id, row) in wanted.iter().zip(rows) {
            let title = row.first().cloned().flatten().unwrap_or_default();
            let file = row.get(1).cloned().flatten().unwrap_or_default();
            // An id the index still lists but the archive hash no longer holds is a stale index
            // entry (a deleted archive whose `INDEX_*` membership was missed), not a hit.
            if title.is_empty() && file.is_empty() {
                continue;
            }
            fetched.insert(id.clone(), (title, file));
        }
    }

    let mut matches: Map<String, Value> = Map::with_capacity(unique.len());
    for (index, source) in unique.iter().enumerate() {
        let mut arcids: Vec<&String> = Vec::new();
        let mut title: Option<String> = None;
        for id in &ids_by_source[index] {
            let Some((found_title, file)) = fetched.get(id) else {
                continue;
            };
            if file.is_empty() {
                continue;
            }
            if title.is_none() {
                title = Some(found_title.clone());
            }
            arcids.push(id);
        }
        matches.insert(
            source.clone(),
            json!({
                "arcids": arcids,
                "title": title,
            }),
        );
    }

    ok("lookup_sources", [("matches", Value::Object(matches))])
}
