//! Account-scoped recent searches, for the same reason `preferences.rs` exists: browser-local state
//! gives one account a different history per browser. Keeping it here also lets the engine decide
//! which entries relate to what is being typed — client-side containment cannot see that `クロガネ`
//! and `くろがね` are the same word.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::preferences::account_owner;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/search/history",
        get(get_search_history)
            .post(remember_search)
            .delete(forget_search),
    )
}

/// Matches the cap the `localStorage` version kept, so the move changes nothing for a reader.
const HISTORY_CAP: isize = 8;

fn history_key(owner: &str) -> String {
    format!("LANRURUGI_SEARCH_HISTORY_{owner}")
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// Present means "only what relates to this"; absent means the whole history.
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RememberBody {
    filter: String,
}

/// `Result` with a boxed error response: an inline `Response` in the `Err` variant is large enough
/// that clippy's `result_large_err` rejects it outright.
type History<T> = Result<T, Box<Response>>;

fn history_response(entries: Vec<String>) -> Response {
    axum::Json(json!({ "history": entries })).into_response()
}

/// A caller without an account has no history: reads answer empty rather than erroring, writes say so.
fn require_owner(auth: Option<&AuthContext>) -> History<&'static str> {
    account_owner(auth).ok_or_else(|| {
        Box::new(error(
            StatusCode::FORBIDDEN,
            "search_history",
            "Search history belongs to a signed-in account.".to_string(),
        ))
    })
}

async fn read_history(state: &AppState, owner: &str) -> History<Vec<String>> {
    let mut conn = state.redis.config.get().await.map_err(|e| {
        Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_history",
            e.to_string(),
        ))
    })?;
    conn.lrange(history_key(owner), 0, -1).await.map_err(|e| {
        Box::new(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_history",
            e.to_string(),
        ))
    })
}

/// `GET /search/history` — newest first, optionally narrowed to what relates to `filter`.
async fn get_search_history(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let Ok(owner) = require_owner(auth.as_deref()) else {
        return history_response(Vec::new());
    };
    match read_history(&state, owner).await {
        Ok(entries) => history_response(match q.filter.as_deref() {
            Some(filter) => related_history(&state.equivalence, filter, &entries),
            None => entries,
        }),
        Err(response) => *response,
    }
}

/// `POST /search/history` — remember one applied filter.
async fn remember_search(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<RememberBody>,
) -> Response {
    let owner = match require_owner(auth.as_deref()) {
        Ok(owner) => owner,
        Err(response) => return *response,
    };
    let trimmed = body.filter.trim();
    if trimmed.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "search_history",
            "An empty filter is not history.".to_string(),
        );
    }
    let mut conn = match state.redis.config.get().await {
        Ok(conn) => conn,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "search_history",
                e.to_string(),
            )
        }
    };
    let key = history_key(owner);
    // Remove-then-push, atomically: a re-run query moves to the front instead of appearing twice.
    let result: Result<(), _> = deadpool_redis::redis::pipe()
        .atomic()
        .lrem(&key, 0, trimmed)
        .lpush(&key, trimmed)
        .ltrim(&key, 0, HISTORY_CAP - 1)
        .query_async(&mut conn)
        .await;
    match result {
        Ok(()) => match read_history(&state, owner).await {
            Ok(entries) => history_response(entries),
            Err(response) => *response,
        },
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_history",
            e.to_string(),
        ),
    }
}

/// `DELETE /search/history?filter=…` — forget one entry (the panel's `Delete` key on a history row).
async fn forget_search(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let owner = match require_owner(auth.as_deref()) {
        Ok(owner) => owner,
        Err(response) => return *response,
    };
    let Some(filter) = q.filter.as_deref().map(str::trim).filter(|f| !f.is_empty()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "search_history",
            "A filter to forget is required.".to_string(),
        );
    };
    let Ok(mut conn) = state.redis.config.get().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_history",
            "Redis unavailable.".to_string(),
        );
    };
    // Every occurrence, so a list edited by hand cannot leave a row the reader cannot dismiss.
    let removed: Result<usize, _> = conn.lrem(history_key(owner), 0, filter).await;
    match removed {
        Ok(_) => match read_history(&state, owner).await {
            Ok(entries) => history_response(entries),
            Err(response) => *response,
        },
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "search_history",
            e.to_string(),
        ),
    }
}

/// The subset of `entries` related to `query`, per the engine's own equivalence rules.
///
/// Loosely and symmetrically "related": one term containing the other, either way round, so typing
/// `気分は` keeps a stored `気分`. A single shared term is enough for a multi-term query.
fn related_history(
    equivalence: &lanrurugi_search::Equivalence,
    query: &str,
    entries: &[String],
) -> Vec<String> {
    let wanted = history_terms(equivalence, query);
    if wanted.is_empty() {
        // Nothing to relate to (empty box, or separators only): the whole history, as an address bar.
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| {
            history_terms(equivalence, entry).iter().any(|term| {
                wanted
                    .iter()
                    .any(|wanted| term.contains(wanted.as_str()) || wanted.contains(term.as_str()))
            })
        })
        .cloned()
        .collect()
}

/// Split on the raw text (folding cannot add or remove separators), compare on the folded forms.
fn history_terms(equivalence: &lanrurugi_search::Equivalence, input: &str) -> Vec<String> {
    input
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|term| !term.is_empty())
        .map(|term| equivalence.fold(&term.to_lowercase()).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn equivalence() -> lanrurugi_search::Equivalence {
        lanrurugi_search::Equivalence::default()
    }

    fn history(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn an_empty_query_keeps_the_whole_history() {
        let entries = history(&["くろがね", "気分 is:reading"]);
        assert_eq!(
            related_history(&equivalence(), "", &entries),
            entries,
            "an empty box shows the whole history, separators included"
        );
        assert_eq!(related_history(&equivalence(), "   ", &entries), entries);
    }

    #[test]
    fn kana_spellings_of_one_word_relate_to_each_other() {
        // The whole reason this is server-side: the engine folds ひらがな/カタカナ, so a stored
        // katakana query is still offered while the hiragana one is being typed.
        let entries = history(&["クロガネ", "くろがね", "bookmark:\"书签\" project"]);
        assert_eq!(
            related_history(&equivalence(), "くろ", &entries),
            history(&["クロガネ", "くろがね"])
        );
        assert_eq!(
            related_history(&equivalence(), "クロ", &entries),
            history(&["クロガネ", "くろがね"])
        );
    }

    #[test]
    fn related_is_symmetric_and_per_term() {
        let entries = history(&["気分は", "気分 is:reading", "is:reading has:category"]);
        // Typing the longer spelling keeps the stored shorter one (containment, either way round).
        assert_eq!(
            related_history(&equivalence(), "気分は", &entries),
            history(&["気分は", "気分 is:reading"])
        );
        // A shared term is enough: `has:category` is unrelated, `気分` is not.
        assert_eq!(
            related_history(&equivalence(), "気分 has:category", &entries),
            entries
        );
        // Nothing in common at all.
        assert_eq!(
            related_history(&equivalence(), "zzzz", &entries),
            Vec::<String>::new()
        );
    }
}
