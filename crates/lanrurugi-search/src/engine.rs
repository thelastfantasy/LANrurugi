//! Search execution, verified against `~/LANraragi/lib/LANraragi/Model/Search.pm::do_search`/
//! `search_uncached`. Ports the filtering/sorting logic directly onto Redis; the search-results
//! cache (`LRR_SEARCHCACHE`, with its cache-key-inversion trick for opposite sort order) is a
//! documented Phase 1 simplification — every search here does the "uncached" full pass. Response
//! correctness doesn't depend on the cache (it's purely a latency optimization); SC-008's
//! benchmark (US8) is where a cache would earn its complexity back if warranted.

use std::collections::HashSet;

use std::sync::Arc;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::entities::Category;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_equivalence::Equivalence;
use thiserror::Error;

use crate::grammar::{parse_query, Expr, Field, Token};
use crate::keys::{NEW_KEY, TANKGROUPED_KEY, TITLES_FOLDED_KEY, TITLES_KEY, UNTAGGED_KEY};

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    /// The attribute operators that read *other repositories* rather than raw Redis: bookmarks
    /// live behind their own error type, categories behind the shared repository one. Both narrow
    /// to the same underlying Redis/pool failures this enum already carries.
    #[error("bookmark repository error: {0}")]
    Bookmarks(#[from] lanrurugi_storage::bookmarks::BookmarksError),
    #[error("repository error: {0}")]
    Repository(#[from] lanrurugi_storage::repository::RepositoryError),
}

type Result<T> = std::result::Result<T, SearchError>;

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub filter: String,
    pub category: Option<Category>,
    pub sortby: Option<String>,
    pub order_desc: bool,
    pub newonly: bool,
    pub untaggedonly: bool,
    pub hidecompleted: bool,
    pub groupby_tanks: bool,
    /// When true, only keep Tankoubon (TANK_-prefixed) entries in the filtered set.
    pub tankonly: bool,
    /// IANA timezone identifier (e.g. `"Asia/Tokyo"`, `"UTC"`) used only by `date_added:YYYY-MM-DD`
    /// date-range tokens — the day an archive was added is computed in this timezone, not the
    /// viewer's browser timezone, so two viewers always agree on which archives belong to a given
    /// searched date. API layer reads this from the `timezone` setting; defaults to UTC upstream.
    pub timezone: String,
    /// `LRR_CONFIG`'s `newbadgemode` setting — how long an archive's "new" flag counts as new:
    /// `until_opened` (legacy), `until_finished`, or a `Nd` time window. Applied to the `newonly`
    /// filter so the "New Archives" button and the badges never disagree (see
    /// `lanrurugi_api::archives::effective_isnew` for the identical display-side logic).
    pub new_badge_mode: String,
    /// Archive ids that currently have an LLM split suggestion, computed by the caller when (and
    /// only when) the query actually mentions `has:split-suggestion` (`mentions_split_suggestion`).
    /// Split-suggestion records live in the *config* logical DB, which this engine is not wired to
    /// (it holds the archive and search pools only); rather than thread a third pool through every
    /// entry point for one niche operator, the caller supplies the set — the same shape
    /// `restrict_to_archive_ids` below already uses. `None` means "not computed", and the operator
    /// then matches nothing rather than silently matching everything.
    pub split_suggestion_archive_ids: Option<HashSet<String>>,
    /// 007-guest-restricted-access: when `Some`, narrows the candidate set to exactly these
    /// archive ids before any other filter runs — the union of archive ids across every
    /// `visible_to_guest` category, computed once per request by the caller. `None` means no
    /// caller-imposed scope restriction (the normal, non-guest case); `Some(empty set)` means
    /// "nothing is in scope" and correctly excludes everything, it is never conflated with `None`.
    pub restrict_to_archive_ids: Option<HashSet<ArchiveId>>,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            filter: String::new(),
            category: None,
            sortby: None,
            order_desc: false,
            newonly: false,
            untaggedonly: false,
            tankonly: false,
            hidecompleted: false,
            groupby_tanks: true,
            timezone: "UTC".to_string(),
            new_badge_mode: "until_opened".to_string(),
            split_suggestion_archive_ids: None,
            restrict_to_archive_ids: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub total: i64,
    pub filtered_count: usize,
    pub ids: Vec<String>,
}

/// Owns the two Redis pools and the canonical `Equivalence` used by every search/index entry
/// point. Construct once at startup (`Arc<SearchEngine>`) so query-time `fold` and index-time
/// `fold` always share the exact same config/pipeline.
pub struct SearchEngine {
    archive_pool: Pool,
    search_pool: Pool,
    equivalence: Arc<Equivalence>,
}

impl SearchEngine {
    pub fn new(archive_pool: Pool, search_pool: Pool, equivalence: Arc<Equivalence>) -> Self {
        Self {
            archive_pool,
            search_pool,
            equivalence,
        }
    }

    pub fn equivalence(&self) -> &Equivalence {
        &self.equivalence
    }

    pub async fn search(&self, params: &SearchParams) -> Result<SearchResult> {
        search(
            &self.archive_pool,
            &self.search_pool,
            &self.equivalence,
            params,
        )
        .await
    }

    pub async fn search_exists(&self, filter: &str, timezone: &str) -> Result<bool> {
        search_exists(
            &self.archive_pool,
            &self.search_pool,
            &self.equivalence,
            filter,
            timezone,
        )
        .await
    }
}

const ARCHIVE_KEY_GLOB: &str = "????????????????????????????????????????";

/// `hidecompleted`'s own "counts as finished" threshold — matches legacy's
/// `Model/Search.pm::search_uncached` (`$progress / $pagecount > 0.85`), not a Phase-1 invention.
const HIDE_COMPLETED_THRESHOLD: f64 = 0.85;

/// A from-scratch existence check — "does at least one archive match this filter", not "give me
/// the results" — for callers like `lanrurugi_api::search::guest_has_any_visible_archive` that
/// only ever need a yes/no answer. Deliberately not built on top of [`search`] itself: `search`
/// always pays for sorting (`sort_ids`) and the `total`/`filtered_count` bookkeeping regardless of
/// how many results actually matter to the caller, none of which this function needs.
///
/// Evaluates the same expression tree `search` does (via [`eval_expr`]), so a predicate using `|`
/// or grouping is answered correctly here too — the pre-expression-tree version took a flat token
/// list and could only ever express AND. What it still saves over `search`: [`eval_expr`]'s `And`
/// stops the moment the running candidate set empties, and this returns on the first non-empty
/// result rather than materializing a sorted page. Scoped to what guest eligibility actually needs
/// — no `groupby_tanks`, `newonly`, `hidecompleted`, or `restrict_to_archive_ids`, all of which
/// `search` supports but this deliberately doesn't (adding a filter kind here that never gets
/// exercised is dead complexity, not future-proofing).
pub async fn search_exists(
    archive_pool: &Pool,
    search_pool: &Pool,
    eq: &Equivalence,
    filter: &str,
    timezone: &str,
) -> Result<bool> {
    let mut archive_conn = archive_pool.get().await?;
    let mut search_conn = search_pool.get().await?;
    let legacy_fallback = !crate::indexer::fold_fingerprint_matches(search_pool, eq)
        .await
        .unwrap_or(false);

    let expr = parse_query(filter);
    if matches!(&expr, Expr::And(children) if children.is_empty()) {
        // A query with no conditions matches everything — "does anything exist" reduces to "is the
        // library non-empty at all", the one case this function doesn't otherwise have a fast path
        // for.
        let any: Vec<String> = archive_conn.keys(ARCHIVE_KEY_GLOB).await?;
        return Ok(!any.is_empty());
    }

    let candidates: HashSet<String> = archive_conn
        .keys::<_, Vec<String>>(ARCHIVE_KEY_GLOB)
        .await?
        .into_iter()
        .collect();

    let mut ctx = EvalCtx {
        archive_pool,
        archive_conn: &mut archive_conn,
        search_conn: &mut search_conn,
        eq,
        legacy_fallback,
        timezone,
        split_suggestions: None,
        category_depth: 0,
    };
    let matched = eval_expr(&mut ctx, &expr, candidates).await?;
    Ok(!matched.is_empty())
}

/// The inputs that stay identical for every node of one `search`/`search_exists` evaluation,
/// bundled so the recursive evaluator and its leaf matcher keep a readable signature as operators
/// gain data sources — the bookmark operator is the first that needs the *pool* alongside a
/// connection, which is what pushed both functions past clippy's argument limit when threaded
/// individually.
struct EvalCtx<'a> {
    archive_pool: &'a Pool,
    archive_conn: &'a mut deadpool_redis::Connection,
    search_conn: &'a mut deadpool_redis::Connection,
    eq: &'a Equivalence,
    legacy_fallback: bool,
    timezone: &'a str,
    /// Caller-supplied `has:split-suggestion` membership (see
    /// [`SearchParams::split_suggestion_archive_ids`]); `None` matches nothing.
    split_suggestions: Option<&'a HashSet<String>>,
    /// How many `category:` operators deep this evaluation already is. A dynamic category's own
    /// `search` predicate is itself a query, so it can name another `category:` — including,
    /// through a cycle, this one. Capped rather than cycle-detected: a chain that deep is a
    /// misconfiguration either way, and matching nothing is the safe outcome.
    category_depth: u8,
}

/// Whether a filter string mentions the `has:split-suggestion` operator — the API's cue to compute
/// [`SearchParams::split_suggestion_archive_ids`] before searching, instead of paying a config-DB
/// round trip on every request that will never look at it.
///
/// Parses rather than substring-matches, so a substring of some *other* token (e.g. a tag value
/// that merely contains these characters) can't trigger the fetch. It deliberately does **not** try
/// to exclude a quoted spelling: quoting cannot escape a reserved operator namespace (see this
/// module's own notes on `has:`), so `"has:split-suggestion"` really is the operator, and the helper
/// reporting `true` for it is correct.
pub fn mentions_split_suggestion(filter: &str) -> bool {
    fn walk(expr: &Expr) -> bool {
        match expr {
            Expr::Term(token) => token.tag == "has:split-suggestion",
            Expr::And(children) | Expr::Or(children) => children.iter().any(walk),
            Expr::Not(inner) => walk(inner),
        }
    }
    walk(&parse_query(filter))
}

/// The `(text, field)` pairs a relevance sort scores against: every leaf the query *requires*.
/// `Not` subtrees are skipped — a term that must be absent cannot make a match more relevant — and
/// wildcards are stripped, since a glob is a matching aid rather than text that literally appears
/// in an archive. `title:`-scoped leaves keep their field so the scorer can weight title hits
/// above tag hits instead of scoring them identically.
fn scoring_terms(expr: &Expr) -> Vec<(String, Field)> {
    fn walk(expr: &Expr, out: &mut Vec<(String, Field)>) {
        match expr {
            Expr::Term(token) => {
                let text = token.tag.replace(['?', '*'], "");
                if !text.is_empty() {
                    out.push((text, token.field));
                }
            }
            Expr::And(children) | Expr::Or(children) => {
                children.iter().for_each(|child| walk(child, out));
            }
            Expr::Not(_) => {}
        }
    }
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

/// Evaluates a parsed query against `scope`, returning the subset of it that matches.
///
/// Invariant: the result is always a subset of `scope`. `And` threads its own running candidate set
/// down to each child in order, so a cheap leaf still narrows the set before an expensive one scans
/// it — the same narrowing the pre-expression-tree engine got from its single `retain` loop. `Or`
/// evaluates every branch against the *incoming* scope and unions (a branch can't be narrowed by a
/// sibling it might not need), and `Not` subtracts from the incoming scope, which is what makes
/// `-(a | b)` mean "neither".
async fn eval_expr(
    ctx: &mut EvalCtx<'_>,
    expr: &Expr,
    scope: HashSet<String>,
) -> Result<HashSet<String>> {
    match expr {
        Expr::And(children) => {
            let mut current = scope;
            for child in children {
                if current.is_empty() {
                    break;
                }
                current = Box::pin(eval_expr(ctx, child, current)).await?;
            }
            Ok(current)
        }
        Expr::Or(children) => {
            let mut matched = HashSet::new();
            for child in children {
                let branch = Box::pin(eval_expr(ctx, child, scope.clone())).await?;
                matched.extend(branch);
            }
            Ok(matched)
        }
        Expr::Not(inner) => {
            let excluded = Box::pin(eval_expr(ctx, inner, scope.clone())).await?;
            Ok(scope.difference(&excluded).cloned().collect())
        }
        Expr::Term(token) => token_matches(ctx, token, &scope).await,
    }
}

pub async fn search(
    archive_pool: &Pool,
    search_pool: &Pool,
    eq: &Equivalence,
    params: &SearchParams,
) -> Result<SearchResult> {
    let mut archive_conn = archive_pool.get().await?;
    let mut search_conn = search_pool.get().await?;
    let legacy_fallback = !crate::indexer::fold_fingerprint_matches(search_pool, eq)
        .await
        .unwrap_or(false);

    let mut filtered: HashSet<String> = if params.groupby_tanks {
        search_conn
            .smembers::<_, Vec<String>>(TANKGROUPED_KEY)
            .await?
            .into_iter()
            .collect()
    } else {
        archive_conn
            .keys::<_, Vec<String>>(ARCHIVE_KEY_GLOB)
            .await?
            .into_iter()
            .collect()
    };

    let mut expr = parse_query(&params.filter);

    if let Some(category) = &params.category {
        if let Some(predicate) = &category.search {
            // The category's own predicate is one more ANDed condition — but as a real expression,
            // so a saved category is free to use `|`/grouping too.
            expr = Expr::And(vec![expr, parse_query(predicate)]);
        } else {
            let cat_set: HashSet<String> =
                category.archives.iter().map(|a| a.to_string()).collect();
            filtered.retain(|id| cat_set.contains(id));
        }
    }

    if let Some(allowed) = &params.restrict_to_archive_ids {
        // Union in *before* the `retain` below, `groupby_tanks: true` only: `filtered`'s own
        // initial candidate set there is `TANKGROUPED_KEY` — Tankoubon ids plus whichever
        // standalone archives were never folded into one — and a raw archive id that *has* been
        // folded into some Tankoubon is `srem`d out of it at fold time (`indexer::
        // sync_tank_membership`), so it was never a candidate in the first place. If that same
        // raw id is guest-visible (e.g. it belongs to a `visible_to_guest` category directly, but
        // the Tankoubon it's actually folded into does not), it would otherwise be silently
        // unreachable — allowed by the caller's own scope computation, yet never appearing in any
        // result, because the whole grouped-candidate mechanism the `retain` below narrows never
        // included it to begin with. This surfaces it as its own standalone result, same as it
        // would appear searched by an admin with `groupby_tanks: false`, rather than trying to
        // fold it into a Tankoubon result the caller has already decided is out of scope.
        if params.groupby_tanks {
            filtered.extend(allowed.iter().map(|id| id.as_str().to_string()));
        }
        let allowed_set: HashSet<&str> = allowed.iter().map(ArchiveId::as_str).collect();
        filtered.retain(|id| allowed_set.contains(id.as_str()));
    }

    if params.untaggedonly {
        let untagged: HashSet<String> = search_conn
            .smembers::<_, Vec<String>>(UNTAGGED_KEY)
            .await?
            .into_iter()
            .collect();
        filtered.retain(|id| untagged.contains(id));
    }

    if params.tankonly {
        filtered.retain(|id| id.starts_with("TANK_"));
    }

    if params.newonly {
        let new_set: HashSet<String> = search_conn
            .smembers::<_, Vec<String>>(NEW_KEY)
            .await?
            .into_iter()
            .collect();
        // Applies `new_badge_mode` on top of the raw `LRR_NEW` membership, mirroring
        // `lanrurugi_api::archives::effective_isnew` — under a time window or until-finished
        // mode, an archive whose badge has lapsed must not keep surfacing through the "New
        // Archives" filter. `retain`'s closure can't `await`, hence the explicit loop.
        let mut keep = HashSet::new();
        for id in &filtered {
            if !new_set.contains(id) {
                continue;
            }
            let lapsed = match params.new_badge_mode.as_str() {
                "until_opened" => false,
                "until_finished" => {
                    let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
                    let pagecount: u32 = archive_conn.hget(id, "pagecount").await.unwrap_or(0);
                    pagecount > 0 && progress >= pagecount
                }
                mode => {
                    // A time-window mode also lapses once the archive is finished (same
                    // threshold `until_finished` uses above), mirroring
                    // `lanrurugi_api::archives::effective_isnew`'s own identical addition — see
                    // that function's own docs for why (an archive read to completion on day one
                    // of a `3d` window used to keep matching `newonly` for the rest of the
                    // window while also having already dropped out of the "On Deck" carousel's
                    // own unrelated `hidecompleted` filter).
                    let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
                    let pagecount: u32 = archive_conn.hget(id, "pagecount").await.unwrap_or(0);
                    if pagecount > 0 && progress >= pagecount {
                        true
                    } else {
                        // Unknown mode or a missing/unparseable `date_added` tag → treat as
                        // still new (same conservative fallback as `effective_isnew`).
                        let Some(days) = mode.strip_suffix('d').and_then(|d| d.parse::<u64>().ok())
                        else {
                            continue;
                        };
                        let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
                        let Some(added) = tags.split(',').find_map(|t| {
                            t.trim()
                                .strip_prefix("date_added:")
                                .and_then(|v| v.parse::<u64>().ok())
                        }) else {
                            continue;
                        };
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        now.saturating_sub(added) >= days * 24 * 60 * 60
                    }
                }
            };
            if !lapsed {
                keep.insert(id.clone());
            }
        }
        filtered = keep;
    }

    if params.hidecompleted {
        let mut keep = HashSet::new();
        for id in &filtered {
            if id.starts_with("TANK") {
                keep.insert(id.clone());
                continue;
            }
            let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
            let pagecount: u32 = archive_conn.hget(id, "pagecount").await.unwrap_or(0);
            let completed =
                pagecount > 0 && (progress as f64 / pagecount as f64) > HIDE_COMPLETED_THRESHOLD;
            if !completed {
                keep.insert(id.clone());
            }
        }
        filtered = keep;
    }

    let filtered = {
        let mut ctx = EvalCtx {
            archive_pool,
            archive_conn: &mut archive_conn,
            search_conn: &mut search_conn,
            eq,
            legacy_fallback,
            timezone: &params.timezone,
            split_suggestions: params.split_suggestion_archive_ids.as_ref(),
            category_depth: 0,
        };
        eval_expr(&mut ctx, &expr, filtered).await?
    };

    let total: i64 = if params.groupby_tanks {
        search_conn.scard(TANKGROUPED_KEY).await?
    } else {
        let tank_count: i64 = archive_conn
            .keys::<_, Vec<String>>("TANK_??????????")
            .await?
            .len() as i64;
        let title_count: i64 = search_conn.zcard(TITLES_KEY).await?;
        title_count - tank_count
    };

    let sortkey = params.sortby.as_deref().unwrap_or("title");
    let ordered = sort_ids(
        &mut archive_conn,
        &mut search_conn,
        eq,
        sortkey,
        params.order_desc,
        &filtered,
        &scoring_terms(&expr),
    )
    .await?;

    Ok(SearchResult {
        total,
        filtered_count: ordered.len(),
        ids: ordered,
    })
}

/// The `is:` / `has:` / `in:` / `size:` attribute operators — questions about an archive's *state*
/// rather than its text, answered straight from the search-side sets and the archive hash fields
/// this module already reads. Same shape as the `pages:`/`read:`/`rating:`/`date_added:` special
/// cases below, all of which run before the generic tag/title lookup. `None` means "not an
/// attribute operator", so ordinary tag searches fall through untouched.
///
/// These four namespaces are therefore reserved: an *unrecognized* value (`is:nonsense`) yields the
/// empty set rather than falling back to a tag lookup, so a typo can never silently match unrelated
/// archives that happen to carry a tag of that name.
async fn attribute_filter(
    ctx: &mut EvalCtx<'_>,
    token: &Token,
    scope: &HashSet<String>,
) -> Result<Option<HashSet<String>>> {
    let Some((namespace, value)) = token.tag.split_once(':') else {
        return Ok(None);
    };
    // `grammar::normalize` rewrites every `_` to `?` before this point (legacy's own glob-escape
    // convention), so undo that on both halves for the namespaces this function owns — the same
    // step `parse_date_range` already takes for `date_added:`.
    let namespace = namespace.replace('?', "_");
    let value = value.replace('?', "_");
    // Handled before the split borrows below: a dynamic category's own `search` predicate is a
    // nested query, so this arm needs the whole context rather than pieces of it.
    if namespace == "category" {
        return Ok(Some(category_filter(ctx, &value, scope).await?));
    }
    let archive_pool = ctx.archive_pool;
    let archive_conn = &mut *ctx.archive_conn;
    let search_conn = &mut *ctx.search_conn;
    match namespace.as_str() {
        "is" => Ok(Some(
            state_filter(archive_conn, search_conn, &value, scope).await?,
        )),
        "has" => Ok(Some(
            has_filter(
                archive_pool,
                archive_conn,
                ctx.split_suggestions,
                &value,
                scope,
            )
            .await?,
        )),
        "in" => Ok(Some(in_filter(search_conn, &value, scope).await?)),
        "size" => Ok(Some(size_filter(archive_conn, &value, scope).await?)),
        "bookmark" => Ok(Some(bookmark_name_filter(ctx, &value, scope).await?)),
        "phrase" => Ok(Some(
            phrase_filter(archive_conn, ctx.eq, &value, scope).await?,
        )),
        _ => Ok(None),
    }
}

/// `is:` — reading/grouping state.
async fn state_filter(
    archive_conn: &mut deadpool_redis::Connection,
    search_conn: &mut deadpool_redis::Connection,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    match value {
        "new" => intersect_members(search_conn, NEW_KEY, scope).await,
        "untagged" => intersect_members(search_conn, UNTAGGED_KEY, scope).await,
        "tank" => Ok(scope
            .iter()
            .filter(|id| id.starts_with("TANK_"))
            .cloned()
            .collect()),
        // "Completed" is deliberately the same 85% threshold `hidecompleted` uses
        // (`HIDE_COMPLETED_THRESHOLD`, itself legacy's own), so `is:completed` and the
        // hide-completed toggle can never disagree about a given archive.
        "completed" | "incomplete" => {
            let want_completed = value == "completed";
            let mut ids = HashSet::new();
            for id in scope {
                if id.starts_with("TANK") {
                    // A Tankoubon aggregate carries its members' summed page count but no
                    // meaningful progress of its own — excluded, like every other per-candidate
                    // numeric filter here.
                    continue;
                }
                let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
                let pagecount: u32 = archive_conn.hget(id, "pagecount").await.unwrap_or(0);
                let completed = pagecount > 0
                    && (progress as f64 / pagecount as f64) > HIDE_COMPLETED_THRESHOLD;
                if completed == want_completed {
                    ids.insert(id.clone());
                }
            }
            Ok(ids)
        }
        "read" | "unread" => {
            let want_read = value == "read";
            let mut ids = HashSet::new();
            for id in scope {
                if id.starts_with("TANK") {
                    continue;
                }
                let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
                if (progress > 0) == want_read {
                    ids.insert(id.clone());
                }
            }
            Ok(ids)
        }
        _ => Ok(HashSet::new()),
    }
}

/// `has:` — a piece of associated state rather than text.
///
/// `patch` reads the archive hash directly. `bookmark` is the one operator here that needs another
/// repository (see its own arm below for why that is still one lookup and not a new index).
/// Category / split-suggestion membership stays unimplemented: those would mean reaching further
/// into storage from the query evaluator, which is a design decision rather than a missing line.
async fn has_filter(
    archive_pool: &Pool,
    archive_conn: &mut deadpool_redis::Connection,
    split_suggestions: Option<&HashSet<String>>,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    match value {
        // "This archive has at least one bookmarked page." Bookmark storage already keeps the
        // archive->latest-timestamp map this needs (`UPDATED_AT_HASH_KEY`, maintained on every
        // add/remove), so this is one HGETALL rather than a new search-side index — and therefore
        // has no migration/rebuild-index story to get wrong. If it ever measures slow on a large
        // library, a maintained `LRR_BOOKMARKED` membership set is the optimization, not a
        // prerequisite for the feature.
        //
        // Deliberately *not* gated for `guest_visitor`: the result set is already narrowed to that
        // caller's visible archives by `restrict_to_archive_ids`, and for any archive a guest can
        // see they can already read its bookmarks directly (`route_policy.csv` allows
        // `/api/archives/:id/bookmarks`), so this discloses nothing new. Tankoubon aggregates are
        // skipped, like every other per-archive attribute here — a tank has no bookmarks of its
        // own, and a bookmarked member does not bubble up to it.
        // "This archive has a pending split suggestion" — the caller-supplied set (see
        // `SearchParams::split_suggestion_archive_ids` for why it can't be read from here).
        "split-suggestion" => Ok(scope
            .iter()
            .filter(|id| split_suggestions.is_some_and(|ids| ids.contains(id.as_str())))
            .cloned()
            .collect()),
        // "This archive is filed in at least one category." Static membership only — a dynamic
        // category's members are whatever its predicate currently matches, queried through
        // `category:<name>` instead of being evaluated for every archive here (that would mean one
        // nested search per dynamic category on every `has:category`, plus a cycle to guard, for no
        // benefit). The tests pin both halves of that split rather than leaving it implicit.
        "category" => {
            let categories =
                lanrurugi_storage::repository::CategoryRepository::new(archive_pool.clone())
                    .list_all()
                    .await?;
            Ok(scope
                .iter()
                .filter(|id| {
                    categories
                        .iter()
                        .filter(|c| c.search.is_none())
                        .any(|c| c.archives.iter().any(|a| a.as_str() == id.as_str()))
                })
                .cloned()
                .collect())
        }
        "bookmark" => {
            let bookmarked =
                lanrurugi_storage::bookmarks::BookmarksRepository::new(archive_pool.clone())
                    .latest_bookmark_per_archive()
                    .await?;
            Ok(scope
                .iter()
                .filter(|id| !id.starts_with("TANK") && bookmarked.contains_key(id.as_str()))
                .cloned()
                .collect())
        }
        "patch" => {
            let mut ids = HashSet::new();
            for id in scope {
                if id.starts_with("TANK") {
                    continue;
                }
                let has: String = archive_conn.hget(id, "has_patch").await.unwrap_or_default();
                if has == "true" {
                    ids.insert(id.clone());
                }
            }
            Ok(ids)
        }
        _ => Ok(HashSet::new()),
    }
}

/// `in:tank` — a real archive currently folded into some Tankoubon. That is exactly "absent from
/// `LRR_TANKGROUPED`", the set that holds standalone archives plus the Tankoubons themselves (see
/// that key's own docs) — so this is a set complement, not a scan.
async fn in_filter(
    search_conn: &mut deadpool_redis::Connection,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    match value {
        "tank" => {
            let ungrouped: HashSet<String> = search_conn
                .smembers::<_, Vec<String>>(TANKGROUPED_KEY)
                .await?
                .into_iter()
                .collect();
            Ok(scope
                .iter()
                .filter(|id| !id.starts_with("TANK_") && !ungrouped.contains(*id))
                .cloned()
                .collect())
        }
        _ => Ok(HashSet::new()),
    }
}

/// `size:>=100M` / `size:<10K` / `size:1G` — archive size, binary suffixes (K/M/G = 1024^n).
/// No suffix means bytes. Deliberately a single comparator (not a range): two `size:` conditions
/// AND together naturally, e.g. `size:>=10M size:<100M`.
async fn size_filter(
    archive_conn: &mut deadpool_redis::Connection,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    let Some((op, size)) = parse_comparison::<u64>(value, parse_size_bytes) else {
        return Ok(HashSet::new());
    };
    let mut ids = HashSet::new();
    for id in scope {
        if id.starts_with("TANK") {
            continue;
        }
        let arcsize: u64 = archive_conn.hget(id, "arcsize").await.unwrap_or(0);
        if compare_ord(op, arcsize, size) {
            ids.insert(id.clone());
        }
    }
    Ok(ids)
}

/// `category:<id-or-name>` — every archive filed in a matching category.
///
/// The value matches a category's **id** exactly (the `SET_<...>` form the UI shows in URLs), or its
/// **name** with the same folded substring/glob semantics every other text token uses, so
/// `category:cosplay` and `category:"My Series"` both work without memorising ids. All matches are
/// unioned, like any other operator that can legitimately hit more than one thing.
///
/// A *dynamic* category (`search` predicate present) contributes whatever its predicate currently
/// matches, evaluated as a real nested query against the same scope — so a saved search composed
/// with `|`/`is:`/`phrase:` keeps working when referenced this way. `category_depth` bounds a
/// predicate that references categories in a cycle.
async fn category_filter(
    ctx: &mut EvalCtx<'_>,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    const MAX_CATEGORY_DEPTH: u8 = 4;
    if value.is_empty() {
        return Ok(HashSet::new());
    }
    let categories =
        lanrurugi_storage::repository::CategoryRepository::new(ctx.archive_pool.clone())
            .list_all()
            .await?;
    // Names are compared folded and lowercased on both sides (`fold` handles CJK/kana; the
    // lowercasing has to be explicit — same split as `indexer::folded_tag_key`). The id path is a
    // plain case-insensitive equality instead: an id is not text to be globbed.
    let folded = ctx.eq.fold_pattern(value);
    let name_pattern = format!("*{folded}*");
    let mut matched = HashSet::new();
    for category in categories {
        let matches = category.catid.as_str().eq_ignore_ascii_case(value)
            || glob_match(&name_pattern, &ctx.eq.fold(&category.name.to_lowercase()));
        if !matches {
            continue;
        }
        match &category.search {
            Some(predicate) if !predicate.is_empty() => {
                if ctx.category_depth >= MAX_CATEGORY_DEPTH {
                    continue;
                }
                let expr = parse_query(predicate);
                ctx.category_depth += 1;
                // Boxed for the same reason `eval_expr`'s own recursive arms are: the cycle
                // eval_expr -> token_matches -> attribute_filter -> category_filter -> eval_expr
                // has to be broken at one edge or the future's type is infinitely sized.
                let nested = Box::pin(eval_expr(ctx, &expr, scope.clone())).await;
                ctx.category_depth -= 1;
                matched.extend(nested?);
            }
            _ => {
                matched.extend(
                    category
                        .archives
                        .iter()
                        .filter(|a| scope.contains(a.as_str()))
                        .map(|a| a.as_str().to_string()),
                );
            }
        }
    }
    Ok(matched)
}

/// `phrase:"two words"` — the *ordered, adjacent* occurrence of that text anywhere in an archive's
/// own searchable text (title plus tag text), which is the one thing the quoting syntax cannot
/// express: quotes match a contiguous run inside a *single* tag key, or the title, and never across
/// the two.
///
/// Deliberately a per-candidate scan rather than an index: a real positional index would be a new
/// Redis structure with its own migration/rebuild story (and the constitution's own bar for a
/// second *store*), while this reuses data every archive already has. Cost is O(candidates × text),
/// which the `And` evaluation order keeps honest — a `phrase:` combined with anything else only
/// scans what that anything else already narrowed to. If a large library ever measures this as the
/// bottleneck, the fix is a maintained folded-text field (or a positional index), not a rewrite of
/// the semantics.
///
/// The value is matched **literally**: `?`/`*` globs mean nothing here (adjacency and wildcards
/// don't compose into anything a user could predict), and the grammar has already lowercased it.
async fn phrase_filter(
    archive_conn: &mut deadpool_redis::Connection,
    eq: &Equivalence,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    if value.is_empty() {
        return Ok(HashSet::new());
    }
    let folded_needle = eq.fold(value);
    let mut ids = HashSet::new();
    for id in scope {
        if id.starts_with("TANK") {
            continue;
        }
        let title: String = archive_conn.hget(id, "title").await.unwrap_or_default();
        let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
        let raw = format!("{title} {tags}").to_lowercase();
        // Fast path first: an unfolded containment check answers the common case (ASCII, already
        // matching case) without paying for the equivalence folding below.
        let hit = raw.contains(value) || eq.fold(&raw).contains(folded_needle.as_ref());
        if hit {
            ids.insert(id.clone());
        }
    }
    Ok(ids)
}

/// `bookmark:"name"` — archives with a bookmarked page whose *name* matches. The mirror image of
/// `has:bookmark` (which only asks whether a bookmark exists at all): this one is a text match, so
/// it deliberately reuses every other text token's semantics — one HGETALL of the bookmark data,
/// both sides pushed through the same canonical folding the tag/title indexes use, `?`/`*` globs
/// preserved by the grammar's own escaping, multi-word names kept together only by quoting.
///
/// Quoting does *not* narrow this to a whole-name equality, matching the rule the title half
/// already follows: for a free-text field, quotes mean "these words stay one contiguous phrase",
/// which is what makes `bookmark:"chapter 1"` different from two separate `bookmark:` terms. An
/// empty value (`bookmark:`) therefore means "some *named* bookmark exists" — unnamed bookmarks
/// never match, since there is no text to match against.
///
/// Tankoubon aggregates are skipped like every other per-archive attribute.
async fn bookmark_name_filter(
    ctx: &mut EvalCtx<'_>,
    value: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    let folded = ctx.eq.fold_pattern(value);
    let pattern = format!("*{folded}*");
    let bookmarks =
        lanrurugi_storage::bookmarks::BookmarksRepository::new(ctx.archive_pool.clone())
            .list_all()
            .await?;
    Ok(bookmarks
        .into_iter()
        .filter(|b| !b.archive_id.starts_with("TANK") && scope.contains(&b.archive_id))
        .filter(|b| {
            // Lowercased *before* folding, exactly like `indexer::folded_tag_key` does for tag text:
            // `fold` handles CJK/kana equivalences, not case, and the query side arrives already
            // lowercased by `grammar::normalize` — folding the raw name would make every
            // capitalised bookmark name unmatchable.
            b.name
                .as_deref()
                .is_some_and(|name| glob_match(&pattern, &ctx.eq.fold(&name.to_lowercase())))
        })
        .map(|b| b.archive_id)
        .collect())
}

/// `smembers(key) ∩ scope`.
async fn intersect_members(
    search_conn: &mut deadpool_redis::Connection,
    key: &str,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    let members: HashSet<String> = search_conn
        .smembers::<_, Vec<String>>(key)
        .await?
        .into_iter()
        .collect();
    Ok(scope.intersection(&members).cloned().collect())
}

/// `>`, `>=`, `<`, `<=`, `=` (bare value means `=`) followed by a value parsed by `parse`.
fn parse_comparison<T>(rest: &str, parse: impl Fn(&str) -> Option<T>) -> Option<(&'static str, T)> {
    for (prefix, op) in [
        (">=", ">="),
        ("<=", "<="),
        (">", ">"),
        ("<", "<"),
        ("=", "="),
    ] {
        if let Some(value) = rest.strip_prefix(prefix) {
            return parse(value).map(|parsed| (op, parsed));
        }
    }
    parse(rest).map(|parsed| ("=", parsed))
}

fn compare_ord<T: PartialOrd>(op: &str, actual: T, expected: T) -> bool {
    match op {
        ">=" => actual >= expected,
        "<=" => actual <= expected,
        ">" => actual > expected,
        "<" => actual < expected,
        "=" => actual == expected,
        _ => false,
    }
}

/// `100`, `100K`, `10M`, `2G` — binary suffixes, case-insensitive.
fn parse_size_bytes(value: &str) -> Option<u64> {
    let value = value.trim();
    let (digits, multiplier) = match value.chars().last()?.to_ascii_lowercase() {
        'k' => (&value[..value.len() - 1], 1024u64),
        'm' => (&value[..value.len() - 1], 1024 * 1024),
        'g' => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    digits.trim().parse::<u64>().ok()?.checked_mul(multiplier)
}

/// `read:>=80%` / `progress:<50%` — progress as a share of page count, for the one question the
/// absolute `read:` form can't express ("mostly read, whatever its length").
fn parse_percent_filter(tag: &str) -> Option<(&'static str, f64)> {
    let (namespace, rest) = tag.split_once(':')?;
    if !matches!(namespace.replace('?', "_").as_str(), "read" | "progress") {
        return None;
    }
    let (op, number) = parse_comparison::<f64>(rest, |v| {
        // `grammar::normalize` rewrites `%` to `*` (legacy's own glob-escaping convention) before
        // the engine ever sees the token, so the percent sign arrives here as `*` — accept both
        // spellings rather than making the documented `read:>=80%` form silently match nothing.
        let digits = v.strip_suffix('%').or_else(|| v.strip_suffix('*'))?;
        digits.trim().parse::<f64>().ok()
    })?;
    Some((op, number))
}

/// pages:/read: numeric filters, the additive `date_added:YYYY-MM-DD` date-range filter, and the
/// general tag-index/title-fuzzy-match lookup, matching `search_uncached`'s per-token logic (the
/// date-range branch is an additive improvement over legacy, which has no "search by calendar day"
/// at all — see [`parse_date_range`]).
async fn token_matches(
    ctx: &mut EvalCtx<'_>,
    token: &Token,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
    // The attribute operators get the whole context (the bookmark one needs the pool, not just a
    // connection), so this has to run *before* the fields below are borrowed out of `ctx`.
    if let Some(ids) = attribute_filter(ctx, token, scope).await? {
        return Ok(ids);
    }
    let (archive_conn, search_conn, eq, legacy_fallback, timezone) = (
        &mut *ctx.archive_conn,
        &mut *ctx.search_conn,
        ctx.eq,
        ctx.legacy_fallback,
        ctx.timezone,
    );
    if let Some((op, percent)) = parse_percent_filter(&token.tag) {
        let mut ids = HashSet::new();
        for id in scope {
            if id.starts_with("TANK") {
                continue;
            }
            let progress: u32 = archive_conn.hget(id, "progress").await.unwrap_or(0);
            let pagecount: u32 = archive_conn.hget(id, "pagecount").await.unwrap_or(0);
            if pagecount == 0 {
                continue;
            }
            let percent_read = 100.0 * f64::from(progress) / f64::from(pagecount);
            if compare_ord(op, percent_read, percent) {
                ids.insert(id.clone());
            }
        }
        return Ok(ids);
    }
    if let Some((start, end)) = parse_date_range(&token.tag, timezone) {
        let mut ids = HashSet::new();
        for id in scope {
            if id.starts_with("TANK") {
                continue;
            }
            // `date_added` lives inside the archive's comma-separated `tags` string as
            // `date_added:<unix_seconds>`, not as its own Redis hash field — so unlike
            // `pages:`/`read:` (which read a dedicated `pagecount`/`progress` field) this has to
            // scan the tags string for the matching namespace. Tolerates a malformed/missing
            // numeric value (treated as not matching) rather than erroring, matching the rest of
            // this function's own `unwrap_or_default()` resilience.
            let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
            let matches = tags.split(',').any(|t| {
                t.trim()
                    .strip_prefix("date_added:")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .is_some_and(|ts| ts >= start && ts < end)
            });
            if matches {
                ids.insert(id.clone());
            }
        }
        return Ok(ids);
    }
    if let Some((col, op, value)) = parse_numeric_filter(&token.tag) {
        let mut ids = HashSet::new();
        for id in scope {
            if id.starts_with("TANK") {
                continue;
            }
            let count: u32 = archive_conn.hget(id, col).await.unwrap_or(0);
            let matches = match op {
                "=" => count == value,
                ">" => count > value,
                ">=" => count >= value,
                "<" => count < value,
                "<=" => count <= value,
                _ => false,
            };
            if matches {
                ids.insert(id.clone());
            }
        }
        return Ok(ids);
    }
    if let Some((op, value)) = parse_rating_filter(&token.tag) {
        let mut ids = HashSet::new();
        for id in scope {
            if id.starts_with("TANK") {
                continue;
            }
            // `rating:` lives inside the archive's comma-separated `tags` string, same as
            // `date_added:` above — not a dedicated Redis hash field the way `pages:`/`read:`'s
            // `pagecount`/`progress` are, so this scans the tags string rather than a direct
            // `HGET`. An archive with no `rating:` tag at all never matches any comparison
            // (`None` from `find_map` short-circuits the whole `is_some_and` to `false`) rather
            // than being treated as `rating:0` — an unrated archive isn't the same claim as one
            // explicitly rated zero stars, and `rating:>=0` matching every unrated archive in the
            // library would be a surprising, almost certainly unwanted result for that query.
            let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
            let matches = tags
                .split(',')
                .find_map(|t| t.trim().strip_prefix("rating:")?.trim().parse::<f64>().ok())
                .is_some_and(|rating| match op {
                    "=" => rating == value,
                    ">" => rating > value,
                    ">=" => rating >= value,
                    "<" => rating < value,
                    "<=" => rating <= value,
                    _ => false,
                });
            if matches {
                ids.insert(id.clone());
            }
        }
        return Ok(ids);
    }

    // `date_added` (once its `_`/`%` glob-escaping is undone — see `parse_date_range`'s own docs)
    // only ever supports the `YYYY-MM-DD` day-range form above; a bare-timestamp write like
    // `date_added:1784871857` deliberately returns *empty*, not "whatever the generic tag-index/
    // title-fuzzy-match fallback below happens to find". `date_added` was never tag-indexed to
    // begin with (it's in `indexer::BASIC_NAMESPACES`, the same "too noisy to index" list as
    // `source`/`artist`/etc. — see that constant's own docs), so falling through here already
    // returned empty in practice, but only as an *accidental* consequence of that separate
    // indexing decision — a future change to what gets indexed could silently resurrect
    // second-precision timestamp search as an unintended side effect. This makes "date_added only
    // supports day-range search" a real, explicit guarantee instead of a coincidence two unrelated
    // pieces of code happen to agree on today.
    if token.tag.replace('?', "_").starts_with("date_added:") {
        return Ok(HashSet::new());
    }

    let mut ids = HashSet::new();

    // `Search.pm::search_uncached`: for an exact-tag search, checks `exists("INDEX_$tag")` first
    // and only trusts a direct `smembers` on that literal key if it's really there — otherwise
    // (even though `isexact` is true) it falls through to the exact same glob-key lookup the
    // non-exact branch below uses. This fallback is load-bearing, not a redundant legacy quirk:
    // `_`/`%` in a tag are unconditionally glob-escaped to `?`/`*` *before* this check runs (both
    // here and in legacy), with no exception for underscores that are part of a namespace name
    // rather than an intentional wildcard (e.g. `date_added:...$` normalizes its own tag to
    // `date?added:...`) — so the literal `INDEX_date?added:...` key almost never actually exists,
    // and skipping the fallback (an earlier version of this function did) silently returned zero
    // results for exact-match searches on any namespace containing an underscore, a real
    // live-confirmed regression from legacy's own actual (if accidental) behavior.
    // Canonicalize the query token's literal segments; `*`/`?` wildcards are preserved by
    // `fold_pattern` and only the literal runs between them are folded, so glob semantics stay
    // intact and OpenCC phrase mappings never cross a wildcard.
    let folded = eq.fold_pattern(&token.tag);

    // `title:`/`filename:` skip the tag index entirely — that field scoping is the one thing
    // legacy's syntax had no way to express, since a bare token always searched tags *and* the
    // title. `folded` is still needed by the title half below.
    if token.field == Field::Any {
        let exact_key = format!("INDEX_{folded}");
        let exact_hit = token.isexact && search_conn.exists(&exact_key).await.unwrap_or(false);
        if exact_hit {
            let members: Vec<String> = search_conn.smembers(&exact_key).await.unwrap_or_default();
            ids.extend(members);
        } else {
            let pattern = if folded.contains(':') {
                format!("INDEX_{folded}*")
            } else {
                format!("INDEX_*{folded}*")
            };
            let keys: Vec<String> = search_conn.keys(pattern).await?;
            for key in keys {
                let members: Vec<String> = search_conn.smembers(&key).await?;
                ids.extend(members);
            }
        }
    }

    // Title match: LRR_TITLES_FOLDED members are "<folded title>\0id". `isexact` deliberately does
    // *not* narrow this to the legacy whole-title-equality behavior (legacy's own
    // `$isexact ? "$tag\x00*" : "*$tag*"` on a `"<title>\x00id"` member only ever matched a title
    // equal to the quoted string, since `\0` had to follow the tag immediately) — a quoted query
    // now means "this exact phrase appears anywhere in the title", which is what a user typing
    // `"tari tari"` is actually asking for. Real titles in this library are shaped
    // `[circle] Title [DL版]`, so whole-title equality made quoting useless for exactly the
    // title-phrase lookup it looks like it should serve; see `grammar.rs`'s own docs. Strictly
    // additive: every title the old form matched still contains that same text.
    collect_title_matches(search_conn, TITLES_FOLDED_KEY, &folded, &mut ids).await?;

    // Migration bridge: when the stored fingerprint doesn't match this config, the pre-folded
    // `INDEX_*`/`LRR_TITLES` indexes from before this change are still the only trustworthy copy
    // for old archives. Read both and union; once `rebuild-index` writes the fingerprint, this
    // branch is disabled and only the canonical indexes are used.
    if legacy_fallback {
        if token.field == Field::Any {
            let raw_exact_key = format!("INDEX_{}", token.tag);
            let raw_exact_hit =
                token.isexact && search_conn.exists(&raw_exact_key).await.unwrap_or(false);
            if raw_exact_hit {
                let members: Vec<String> = search_conn
                    .smembers(&raw_exact_key)
                    .await
                    .unwrap_or_default();
                ids.extend(members);
            } else {
                let raw_pattern = if token.tag.contains(':') {
                    format!("INDEX_{}*", token.tag)
                } else {
                    format!("INDEX_*{}*", token.tag)
                };
                let keys: Vec<String> = search_conn.keys(raw_pattern).await?;
                for key in keys {
                    let members: Vec<String> = search_conn.smembers(&key).await?;
                    ids.extend(members);
                }
            }
        }

        // Same title semantics as the canonical branch above (phrase anywhere in the title), and
        // the same title-only glob — see that helper's own comments.
        collect_title_matches(search_conn, TITLES_KEY, &token.tag, &mut ids).await?;
    }

    // Invariant every caller depends on: the returned set is a subset of `scope`. The tag-index and
    // title halves above collect from *global* Redis sets/keys — `scope` only ever reaches the
    // per-candidate filters (and the early-returning operator branches, which build from `scope`
    // directly) — so without this intersection a leaf would hand back archives that the caller had
    // already excluded. The pre-expression-tree engine got this for free from its own
    // `filtered.retain(|id| ids.contains(id))`; `eval_expr`'s `Not`/`Or` correctness depends on it
    // (subtracting a non-subset, or unioning one, silently re-admits excluded ids — caught live by
    // `contract_api`'s tank-member test and `settings_toggles`'s guest-scoping test).
    ids.retain(|id| scope.contains(id));

    Ok(ids)
}

/// Unions every archive whose title contains `needle` into `ids`, reading them out of the
/// `"<title>\0<id>"` sorted set at `key`.
///
/// Driven by `ZSCAN ... MATCH`: the set holds one entry per archive, so reading it whole would move
/// the entire library's titles per query token (legacy scanned for the same reason). The server's
/// `MATCH` does the coarse substring filter and, because a member's id half is part of the member,
/// over-matches; the glob below re-checks the title half only — legacy globbed the whole member,
/// which let a query like `dead` hit any archive whose 40-char hex id merely contained it, and then
/// read the id back out of the very substring that matched.
async fn collect_title_matches(
    search_conn: &mut deadpool_redis::Connection,
    key: &str,
    needle: &str,
    ids: &mut HashSet<String>,
) -> Result<()> {
    let pattern = format!("*{needle}*");
    let mut cursor = String::from("0");
    loop {
        let (next, members): (String, Vec<String>) = deadpool_redis::redis::cmd("ZSCAN")
            .arg(key)
            .arg(&cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(500)
            .query_async(&mut *search_conn)
            .await?;
        for member in members {
            let Some(pos) = member.find('\0') else {
                continue;
            };
            if glob_match(&pattern, &member[..pos]) {
                ids.insert(member[pos + 1..].to_string());
            }
        }
        if next == "0" {
            return Ok(());
        }
        cursor = next;
    }
}

/// Parses an additive `date_added:YYYY-MM-DD` token into the half-open Unix-timestamp range
/// `[start, end)` covering that calendar day in the given IANA timezone — returns `None` for any
/// tag that isn't exactly that shape (other namespaces, partial dates, comparison operators, or
/// the bare timestamp form `date_added:1784871857` all fall through to the existing numeric/tag
/// paths). The day boundaries (00:00:00 inclusive, 00:00:00 next day exclusive) are computed in
/// `timezone`, then converted back to absolute UTC seconds — which is what `date_added` tags
/// actually store — so two viewers in different timezones searching the same string resolve to
/// the same set of archives, by design (see `SearchParams::timezone`'s own docs for the why).
///
/// Legacy LANraragi has no equivalent — its `date_added:` is a plain Unix-timestamp tag with no
/// calendar-day search at all; this is a purely additive improvement, not a port.
fn parse_date_range(tag: &str, timezone: &str) -> Option<(u64, u64)> {
    let (ns, rest) = tag.split_once(':')?;
    // Comparison forms (`date_added:>=2026-01-01`) come first: a bare date means "that whole
    // calendar day" (the original, still-supported spelling), while the operators express
    // "on/after", "on/before", "after" and "before" — the range queries legacy's single-day form
    // had no way to ask for.
    let (op, rest) = [
        (">=", ">="),
        ("<=", "<="),
        (">", ">"),
        ("<", "<"),
        ("=", "="),
    ]
    .into_iter()
    .find_map(|(prefix, op)| rest.strip_prefix(prefix).map(|rest| (op, rest)))
    .unwrap_or(("=", rest));
    // `token.tag` arrives here *after* `grammar::normalize` has already rewritten every `_` to `?`
    // (legacy's own glob-escape convention — see `grammar.rs`), so `date_added:` shows up as
    // `date?added:`. Reverse that rewrite on the namespace half only before comparing, so the
    // literal `date_added` namespace is recognized without disturbing the `?` wildcard semantics
    // the rest of the search engine depends on for actual wildcard queries.
    let ns = ns.replace('?', "_");
    if ns != "date_added" {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(rest, "%Y-%m-%d").ok()?;
    let tz: chrono_tz::Tz = timezone.parse().ok().unwrap_or(chrono_tz::UTC);
    let day_start = date
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(tz)
        .single()?
        .timestamp() as u64;
    let day_end = date
        .succ_opt()?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(tz)
        .single()?
        .timestamp() as u64;
    // Half-open `[start, end)` in every case, using the timestamp bounds the caller already
    // compares against: `u64::MAX` is "no upper bound" and `0` is "no lower bound" (the epoch is
    // the earliest `date_added` that can exist).
    Some(match op {
        "=" => (day_start, day_end),
        ">=" => (day_start, u64::MAX),
        ">" => (day_end, u64::MAX),
        "<=" => (0, day_end),
        "<" => (0, day_start),
        _ => return None,
    })
}

fn parse_numeric_filter(tag: &str) -> Option<(&'static str, &'static str, u32)> {
    let (col_str, rest) = tag.split_once(':')?;
    let col = match col_str {
        "pages" => "pagecount",
        "read" => "progress",
        _ => return None,
    };
    for (op_str, op) in [(">=", ">="), ("<=", "<="), (">", ">"), ("<", "<")] {
        if let Some(num) = rest.strip_prefix(op_str) {
            return num.parse().ok().map(|n| (col, op, n));
        }
    }
    rest.parse().ok().map(|n| (col, "=", n))
}

/// `rating:>=1`/`rating:<4`/`rating:=5`-style comparison filters — additive on top of the plain
/// `rating:5` exact-tag search the generic tag-index lookup already handles (`token_matches`'s own
/// fallthrough at the bottom of this function's call site). Deliberately requires an explicit
/// operator (`>=`/`<=`/`>`/`<`/`=`) and returns `None` for a bare `rating:5` with no operator at
/// all — unlike `parse_numeric_filter` above (whose `pages:100` bare-number form is already the
/// *only* way to filter by page count, since there's no separate `pages:` tag-index to fall back
/// to), `rating:5` already has a working, separately-tested exact-match path via the tag index
/// (`INDEX_rating:5`), and this function claiming that same bare form would bypass it for a
/// float-comparison path that produces an identical result the slower way (a full tags-string scan
/// instead of an index lookup) for zero behavior change — no reason to duplicate work that already
/// happens correctly. `f64`, not `u32` like `parse_numeric_filter` — `rating:` values are decimal
/// (`rating:4.5`, one-tenth precision; see `apps/frontend/src/lib/utils/rating.ts`'s own docs on
/// the storage format), so a `rating:>=4.5` filter has to compare against a real fraction, not
/// truncate/reject it the way parsing straight into a `u32` would.
fn parse_rating_filter(tag: &str) -> Option<(&'static str, f64)> {
    let rest = tag.strip_prefix("rating:")?;
    for (op_str, op) in [
        (">=", ">="),
        ("<=", "<="),
        (">", ">"),
        ("<", "<"),
        ("=", "="),
    ] {
        if let Some(num) = rest.strip_prefix(op_str) {
            return num.parse().ok().map(|n| (op, n));
        }
    }
    None
}

/// Minimal glob matcher supporting `*` (any run) and `?` (single char) — the two wildcard forms
/// `grammar.rs` normalizes into (legacy relies on Redis's own glob-capable `SCAN`/`ZSCAN MATCH`;
/// this reimplements the same semantics in-process since we fetch title members directly).
fn glob_match(pattern: &str, text: &str) -> bool {
    fn helper(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => helper(&p[1..], t) || (!t.is_empty() && helper(p, &t[1..])),
            Some('?') => !t.is_empty() && helper(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && helper(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    helper(&p, &t)
}

async fn sort_ids(
    archive_conn: &mut deadpool_redis::Connection,
    search_conn: &mut deadpool_redis::Connection,
    eq: &Equivalence,
    sortkey: &str,
    order_desc: bool,
    filtered: &HashSet<String>,
    terms: &[(String, Field)],
) -> Result<Vec<String>> {
    // Deliberately opt-in and never the default: every other sort here is a total order a caller
    // can predict from stored fields, while this one changes meaning as the query changes. It also
    // cannot reuse the tag-namespace fallback below (which would happily treat the literal string
    // "relevance" as a namespace), so it is matched by name first.
    //
    // Scoring is intentionally explainable rather than clever: a title hit is worth 3, a tag hit 1,
    // per required term; ties break on title order. No recency term — two archives that match the
    // query equally well should not be ordered by anything but the query.
    if sortkey == "relevance" {
        let folded: Vec<(String, Field)> = terms
            .iter()
            .map(|(text, field)| (eq.fold(&text.to_lowercase()).into_owned(), *field))
            .collect();
        let mut scored: Vec<(i64, String)> = Vec::with_capacity(filtered.len());
        for id in filtered {
            let title: String = archive_conn.hget(id, "title").await.unwrap_or_default();
            let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
            let folded_title = eq.fold(&title.to_lowercase()).into_owned();
            let folded_tags = eq.fold(&tags.to_lowercase()).into_owned();
            let mut score = 0i64;
            for (needle, field) in &folded {
                if !needle.is_empty() && folded_title.contains(needle.as_str()) {
                    score += 3;
                }
                if *field == Field::Any
                    && !needle.is_empty()
                    && folded_tags.contains(needle.as_str())
                {
                    score += 1;
                }
            }
            scored.push((score, id.clone()));
        }
        // Highest score first by default (`order_desc == false`, i.e. the API's own default) — an
        // ascending relevance sort is never what a caller means by asking for relevance, so
        // `order=asc` is what flips this to lowest-first.
        scored.sort_by(|a, b| {
            if order_desc {
                a.0.cmp(&b.0)
            } else {
                b.0.cmp(&a.0)
            }
            .then_with(|| a.1.cmp(&b.1))
        });
        return Ok(scored.into_iter().map(|(_, id)| id).collect());
    }
    if sortkey == "title" {
        let ordered: Vec<String> = search_conn
            .zrangebyscore(TITLES_KEY, "-inf", "+inf")
            .await?;
        // This is the one sort branch that walks the *index* rather than the candidate set, so it is
        // also the only place a duplicated index member can turn into a duplicated search result.
        // That is not hypothetical: `zadd` keys a member by `"<title>\0<id>"`, so an archive whose
        // title changed without the old member being removed (or a test fixture a killed run never
        // cleaned up, or a leftover from a run pointed at the wrong Redis) leaves two members for
        // one id — and every other branch here can't, because it iterates `filtered`, a set.
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for member in ordered {
            if let Some(pos) = member.find('\0') {
                let id = &member[pos + 1..];
                if filtered.contains(id) && seen.insert(id.to_string()) {
                    result.push(id.to_string());
                }
            }
        }
        if order_desc {
            result.reverse();
        }
        return Ok(result);
    }

    if sortkey == "lastread" {
        let mut pairs: Vec<(String, u64)> = Vec::new();
        for id in filtered {
            let t: u64 = if id.starts_with("TANK") {
                // A Tankoubon has no `lastreadtime` field of its own — matches legacy's own
                // Lua-scripted sort (`Model/Search.pm`'s `sort_results`): its effective sort key
                // is the MAX `lastreadtime` across its member archives (`ZRANGEBYSCORE id 1
                // '+inf'`, the same score range `GroupingRepository::get` uses to read real
                // member archive ids back out of the tank's own zset — scores 0 and below are
                // reserved for the tank's own name/summary/tags/progress fields).
                let members: Vec<String> = archive_conn
                    .zrangebyscore(id, 1, "+inf")
                    .await
                    .unwrap_or_default();
                let mut max_time = 0u64;
                for member in &members {
                    let t: u64 = archive_conn.hget(member, "lastreadtime").await.unwrap_or(0);
                    max_time = max_time.max(t);
                }
                max_time
            } else {
                archive_conn.hget(id, "lastreadtime").await.unwrap_or(0)
            };
            if t > 0 {
                pairs.push((id.clone(), t));
            }
        }
        pairs.sort_by_key(|p| std::cmp::Reverse(p.1));
        return Ok(pairs.into_iter().map(|(id, _)| id).collect());
    }

    // Sort by an arbitrary tag namespace (every `sortby` value that isn't `title`/`lastread`/
    // `relevance`/a date namespace lands here), mirroring legacy `Model/Search.pm`'s
    // `sort_results`:
    // ids carrying the namespace sort first by their value (`date_added`/`timestamp` values are
    // numeric Unix timestamps, compared numerically like legacy's own `ncmp`; other namespaces
    // alphabetically), ids without it go to the back in an unspecified order. Descending order
    // reverses only the keyed section — legacy applies `reverse` to `@keyed_ids` before pushing
    // the unkeyed ones on, so ids missing the sort namespace always stay at the very back
    // regardless of direction. (The port's original whole-list `rev()` at the call site inverted
    // that and wrongly flipped unkeyed ids — the Tankoubons with no `date_added` — to the front
    // under a descending `date_added` sort.)
    //
    // A Tankoubon is a zset, not an archive hash, so `HGET <id> tags` fails outright — its sort
    // value is imputed from its member archives exactly like legacy's `_impute_tank_date_tags`:
    // the tank's own `date_added`/`timestamp` tag wins if present, otherwise the MAX across its
    // members' tags of the same namespace (`get_tank_unified_tags`'s coalescing).
    let sortkey_prefix = format!("{sortkey}:");
    let is_date_sort = sortkey == "date_added" || sortkey == "timestamp";
    let mut unkeyed: Vec<String> = Vec::new();
    if is_date_sort {
        let mut keyed: Vec<(String, u64)> = Vec::new();
        for id in filtered {
            let value = if id.starts_with("TANK") {
                tank_date_sort_value(archive_conn, id, &sortkey_prefix).await
            } else {
                let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
                tags.split(',').find_map(|t| {
                    t.trim()
                        .strip_prefix(&sortkey_prefix)
                        .and_then(|v| v.parse().ok())
                })
            };
            match value {
                Some(v) => keyed.push((id.clone(), v)),
                None => unkeyed.push(id.clone()),
            }
        }
        keyed.sort_by_key(|(_, v)| *v);
        if order_desc {
            keyed.reverse();
        }
        let mut result: Vec<String> = keyed.into_iter().map(|(id, _)| id).collect();
        result.extend(unkeyed);
        return Ok(result);
    }

    let mut keyed: Vec<(String, String)> = Vec::new();
    for id in filtered {
        let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
        let value = tags.split(',').find_map(|t| {
            let t = t.trim();
            t.strip_prefix(&sortkey_prefix)
        });
        match value {
            Some(v) => keyed.push((id.clone(), v.to_ascii_lowercase())),
            None => unkeyed.push(id.clone()),
        }
    }
    keyed.sort_by(|a, b| a.1.cmp(&b.1));
    if order_desc {
        keyed.reverse();
    }
    let mut result: Vec<String> = keyed.into_iter().map(|(id, _)| id).collect();
    result.extend(unkeyed);
    Ok(result)
}

/// Imputes a Tankoubon's `date_added`/`timestamp` sort value (legacy `_impute_tank_date_tags` /
/// `get_tank_unified_tags`): the tank's own tag of that namespace first, else the MAX numeric
/// value across its member archives' tags. Returns the numeric timestamp (`u64`) or `None` when
/// neither the tank nor any member carries the namespace.
async fn tank_date_sort_value(
    archive_conn: &mut deadpool_redis::Connection,
    tank_id: &str,
    sortkey_prefix: &str,
) -> Option<u64> {
    // Priority: updated_at > created_at > Tank ID timestamp > member MAX (fallback).
    // The `updated_at`/`created_at` zset members (scores -9/-8, `GroupingRepository`'s
    // `SCORE_UPDATED_AT`/`SCORE_CREATED_AT`) are stored as `updated_at_<u64>` /
    // `created_at_<u64>` — same prefix pattern as the other metadata members.
    use deadpool_redis::redis::AsyncCommands;
    let own_members: Vec<String> = archive_conn
        .zrangebyscore(tank_id, -9, -8)
        .await
        .unwrap_or_default();
    let mut found_updated: Option<u64> = None;
    let mut found_created: Option<u64> = None;
    for m in &own_members {
        if let Some(v) = m.strip_prefix("updated_at_").and_then(|s| s.parse().ok()) {
            found_updated = Some(v);
        } else if let Some(v) = m.strip_prefix("created_at_").and_then(|s| s.parse().ok()) {
            found_created = Some(v);
        }
    }

    if let Some(ts) = found_updated.or(found_created) {
        return Some(ts);
    }

    // Fallback 1: extract from Tank ID (`TANK_{unix_timestamp}`).
    if let Some(ts) = tank_id
        .strip_prefix("TANK_")
        .and_then(|rest| rest.parse::<u64>().ok())
    {
        return Some(ts);
    }

    // Fallback 2: tank's own tags (zset score -2, `tags_<value>` — `SCORE_TAGS`).
    let own_tags: Vec<String> = archive_conn
        .zrangebyscore(tank_id, -2, -2)
        .await
        .unwrap_or_default();
    if let Some(tag) = own_tags.first().and_then(|m| m.strip_prefix("tags_")) {
        if let Some(v) = tag
            .split(',')
            .find_map(|t| t.trim().strip_prefix(sortkey_prefix))
            .and_then(|v| v.parse().ok())
        {
            return Some(v);
        }
    }
    let members: Vec<String> = archive_conn
        .zrangebyscore(tank_id, 1, "+inf")
        .await
        .unwrap_or_default();
    let mut max = None;
    for member in &members {
        let tags: String = archive_conn.hget(member, "tags").await.unwrap_or_default();
        if let Some(v) = tags
            .split(',')
            .find_map(|t| t.trim().strip_prefix(sortkey_prefix))
            .and_then(|v| v.parse::<u64>().ok())
        {
            max = Some(max.map_or(v, |m: u64| m.max(v)));
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pools() -> Option<(Pool, Pool)> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let archive_url = format!("{}/0", base.trim_end_matches('/'));
        let search_url = format!("{}/3", base.trim_end_matches('/'));
        let archive = lanrurugi_storage::test_support::test_pool_for_url(&archive_url).await?;
        let search = lanrurugi_storage::test_support::test_pool_for_url(&search_url).await?;
        Some((archive, search))
    }

    fn test_eq() -> Equivalence {
        Equivalence::new(lanrurugi_equivalence::FoldConfig::all())
    }

    /// Issue regression (2026-08-04): a descending `date_added` sort used to `rev()` the *whole*
    /// list at the call site, flipping unkeyed ids — Tankoubons, which have no archive `tags`
    /// hash — to the front, so a library sorted newest-first showed its (older) Tankoubons above
    /// freshly-added archives. Legacy sorts only the keyed section and pushes unkeyed ids on at
    /// the back regardless of direction.
    #[tokio::test]
    async fn date_added_descending_keeps_unkeyed_tankoubons_at_the_back() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let mut aconn = archive_pool.get().await.unwrap();
        let id_old = "a".repeat(40);
        let id_new = "b".repeat(40);
        let id_no_date = "c".repeat(40);
        // Deliberately NOT the real `TANK_<unix-timestamp>` shape (`tankoubons.rs`'s own
        // `TankId(format!("TANK_{now}"))`) — `tank_date_sort_value`'s Fallback 1 legitimately
        // extracts a sort timestamp straight from an ID in that shape (a real Tankoubon's ID
        // literally IS its creation time), so a real-shaped id here would always resolve to
        // "keyed" regardless of tags/zset content, defeating the "completely unkeyed" scenario
        // this test means to set up. A non-numeric suffix guarantees Fallback 1 can't match.
        let tank = "TANK_test_no_date";

        for (id, tags) in [
            (id_old.as_str(), "date_added:1000"),
            (id_new.as_str(), "date_added:2000"),
            (id_no_date.as_str(), "artist:x"),
        ] {
            let _: () = aconn.hset(id, "tags", tags).await.unwrap();
        }
        // The tank's only member has no date tag either → the tank itself stays unkeyed.
        let _: () = aconn
            .zadd_multiple(tank, &[(1, id_no_date.clone())])
            .await
            .unwrap();

        let filtered: HashSet<String> = [&id_old, &id_new, &id_no_date, tank]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let ids = sort_ids(
            &mut aconn,
            &mut search_pool.get().await.unwrap(),
            &test_eq(),
            "date_added",
            true,
            &filtered,
            // No relevance terms: this test is about date ordering, and `sortby` here is never
            // `relevance`, so the scorer's inputs are unused.
            &[],
        )
        .await
        .unwrap();

        // Descending: keyed (newest first) then unkeyed (both at the back, order unspecified).
        assert_eq!(ids[0], id_new, "newest archive must come first");
        assert_eq!(ids[1], id_old, "older archive second");
        assert_eq!(ids.len(), 4, "nothing may be dropped from the result");
        assert!(
            ids[2] == id_no_date || ids[2] == tank,
            "unkeyed ids must be at the back, got {ids:?}"
        );
        assert!(
            ids[3] == id_no_date || ids[3] == tank,
            "unkeyed ids must be at the back, got {ids:?}"
        );
        assert_ne!(ids[2], ids[3]);

        for id in [&id_old, &id_new, &id_no_date, &tank.to_string()] {
            let _: () = aconn.del(id).await.unwrap();
        }
    }

    /// The other half of the same issue: a Tankoubon's own `date_added` sort value is imputed
    /// from its member archives (legacy `_impute_tank_date_tags` — MAX across members), so the
    /// tank participates in keyed ordering instead of always trailing as unkeyed.
    #[tokio::test]
    async fn date_added_sort_imputes_tank_value_from_member_archives() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let mut aconn = archive_pool.get().await.unwrap();
        let id_old = "d".repeat(40);
        let member_new = "e".repeat(40);
        let tank = "TANK_1785750001";

        let _: () = aconn
            .hset(&id_old, "tags", "date_added:1000")
            .await
            .unwrap();
        let _: () = aconn
            .hset(&member_new, "tags", "date_added:3000")
            .await
            .unwrap();
        // Tank's own tags (zset score -2, `tags_` member) carry no date; its member does.
        let _: () = aconn
            .zadd_multiple(tank, &[(-2, "tags_artist:x"), (1, member_new.as_str())])
            .await
            .unwrap();

        let filtered: HashSet<String> = [&id_old, &member_new, tank]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let ids = sort_ids(
            &mut aconn,
            &mut search_pool.get().await.unwrap(),
            &test_eq(),
            "date_added",
            true,
            &filtered,
            // No relevance terms: this test is about date ordering, and `sortby` here is never
            // `relevance`, so the scorer's inputs are unused.
            &[],
        )
        .await
        .unwrap();

        // Descending: both the tank (imputed 3000) and its member (3000) must sort above the
        // older archive (1000); their relative order when values are equal is deterministic
        // (tie-broken by ID) but the test should not depend on which comes first.
        assert_eq!(ids.len(), 3);
        assert!(
            ids[0] == tank || ids[1] == tank,
            "tank (imputed 3000) must be in top 2"
        );
        assert!(
            ids[0] == member_new || ids[1] == member_new,
            "member (3000) must be in top 2"
        );
        assert_eq!(ids[2], id_old, "oldest archive last");

        for id in [&id_old, &member_new, &tank.to_string()] {
            let _: () = aconn.del(id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn finds_archive_by_tag_and_respects_negation() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let eq = test_eq();
        let id_a = "1".repeat(40);
        let id_b = "2".repeat(40);

        let mut aconn = archive_pool.get().await.unwrap();
        let _: () = aconn
            .hset_multiple(
                &id_a,
                &[
                    ("tags", "artist:jane"),
                    ("pagecount", "10"),
                    ("progress", "0"),
                ],
            )
            .await
            .unwrap();
        let _: () = aconn
            .hset_multiple(
                &id_b,
                &[
                    ("tags", "artist:bob"),
                    ("pagecount", "10"),
                    ("progress", "0"),
                ],
            )
            .await
            .unwrap();

        crate::indexer::index_new_archive(&search_pool, &eq, &id_a, "Book A")
            .await
            .unwrap();
        crate::indexer::index_new_archive(&search_pool, &eq, &id_b, "Book B")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &eq, &id_a, "", "artist:jane")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &eq, &id_b, "", "artist:bob")
            .await
            .unwrap();

        let params = SearchParams {
            filter: "artist:jane".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let result = search(&archive_pool, &search_pool, &eq, &params)
            .await
            .unwrap();
        assert_eq!(result.ids, vec![id_a.clone()]);

        let neg_params = SearchParams {
            filter: "-artist:jane".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let neg_result = search(&archive_pool, &search_pool, &eq, &neg_params)
            .await
            .unwrap();
        assert!(neg_result.ids.contains(&id_b));
        assert!(!neg_result.ids.contains(&id_a));

        for id in [&id_a, &id_b] {
            let _: () = aconn.del(id).await.unwrap();
        }
        let mut sconn = search_pool.get().await.unwrap();
        let _: () = sconn.del("INDEX_artist:jane").await.unwrap();
        let _: () = sconn.del("INDEX_artist:bob").await.unwrap();
        // `srem` this test's own ids, not `del` the whole set — `UNTAGGED_KEY`/`NEW_KEY`/
        // `TANKGROUPED_KEY` are shared across every test in this file (and `indexer.rs`'s own
        // tests, same DB3), and `cargo test` runs `#[tokio::test]`s concurrently by default. A
        // wholesale `del` here could wipe another concurrently-running test's just-written
        // membership out from under it — a real, observed flake (issue #86), not hypothetical.
        for id in [&id_a, &id_b] {
            let _: () = sconn.srem(UNTAGGED_KEY, id).await.unwrap();
            let _: () = sconn.srem(NEW_KEY, id).await.unwrap();
            let _: () = sconn.srem(TANKGROUPED_KEY, id).await.unwrap();
        }
        let _: () = sconn
            .zrem(
                TITLES_KEY,
                vec![
                    "book a\0".to_string() + &id_a,
                    "book b\0".to_string() + &id_b,
                ],
            )
            .await
            .unwrap();
    }

    /// 007-guest-restricted-access: `restrict_to_archive_ids` narrows the candidate set exactly
    /// like `category`/`untaggedonly`/`tankonly` already do — `None` is a no-op, an empty `Some`
    /// set excludes everything (never conflated with `None`, per the field's own doc comment), and
    /// a non-empty set narrows correctly alongside an active keyword filter.
    #[tokio::test]
    async fn restrict_to_archive_ids_narrows_the_candidate_set() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let eq = test_eq();
        // NOT a single-character-repeated id (`"3".repeat(40)`, `"f".repeat(40)`, ...) — this
        // whole module (and `indexer.rs` in the same crate, sharing the same CI Redis instance/DB
        // via `test_pools()`'s fixed `LANRURUGI_TEST_REDIS_URL`) already has a same-shaped test for
        // nearly every single character 0-9/a-g, and CI runs tests in parallel against one shared
        // Redis unlike the local dev container's own per-invocation instance. Confirmed live
        // twice, 2026-08-27: first `"3".repeat(40)` collided with
        // `multi_word_tag_value_is_findable_...` elsewhere in this file, then `"f".repeat(40)`
        // (picked specifically to dodge that) turned out to *also* collide with
        // `indexer.rs::new_archive_lands_in_untagged_new_and_ungrouped_sets`'s own `"f".repeat(40)`
        // — that test `srem`s `TANKGROUPED_KEY` for its id right as this test's own `groupby_tanks`
        // read raced it, intermittently removing `id_a` from the candidate set between this test's
        // two assertions. A real (non-repeated) hex string exhausts the "single digit repeated"
        // collision space this module and its sibling have been drawing from entirely.
        let id_a = "f00dfeed".repeat(5);
        let id_b = "deadbeef".repeat(5);

        let mut aconn = archive_pool.get().await.unwrap();
        for (id, title) in [(&id_a, "Book A"), (&id_b, "Book B")] {
            let _: () = aconn
                .hset_multiple(id, &[("tags", ""), ("pagecount", "10"), ("progress", "0")])
                .await
                .unwrap();
            crate::indexer::index_new_archive(&search_pool, &eq, id, title)
                .await
                .unwrap();
        }

        // `None` — no restriction — both archives are candidates.
        let unrestricted = SearchParams {
            groupby_tanks: true,
            ..Default::default()
        };
        let result = search(&archive_pool, &search_pool, &eq, &unrestricted)
            .await
            .unwrap();
        assert!(result.ids.contains(&id_a));
        assert!(result.ids.contains(&id_b));

        // `Some({id_a})` — only id_a is in scope, even though id_b would otherwise match too.
        let scoped = SearchParams {
            groupby_tanks: true,
            restrict_to_archive_ids: Some([ArchiveId(id_a.clone())].into_iter().collect()),
            ..Default::default()
        };
        let scoped_result = search(&archive_pool, &search_pool, &eq, &scoped)
            .await
            .unwrap();
        assert!(scoped_result.ids.contains(&id_a));
        assert!(!scoped_result.ids.contains(&id_b));

        // `Some({})` — an empty scope excludes everything, not treated the same as `None`.
        let empty_scope = SearchParams {
            groupby_tanks: true,
            restrict_to_archive_ids: Some(HashSet::new()),
            ..Default::default()
        };
        let empty_result = search(&archive_pool, &search_pool, &eq, &empty_scope)
            .await
            .unwrap();
        assert!(!empty_result.ids.contains(&id_a));
        assert!(!empty_result.ids.contains(&id_b));

        for id in [&id_a, &id_b] {
            let _: () = aconn.del(id).await.unwrap();
        }
        let mut sconn = search_pool.get().await.unwrap();
        for id in [&id_a, &id_b] {
            let _: () = sconn.srem(UNTAGGED_KEY, id).await.unwrap();
            let _: () = sconn.srem(NEW_KEY, id).await.unwrap();
            let _: () = sconn.srem(TANKGROUPED_KEY, id).await.unwrap();
        }
        let _: () = sconn
            .zrem(
                TITLES_KEY,
                vec![
                    "book a\0".to_string() + &id_a,
                    "book b\0".to_string() + &id_b,
                ],
            )
            .await
            .unwrap();
    }

    // Issue #59, end-to-end (not just token-level, unlike grammar.rs's own tests): a real archive
    // with a genuinely multi-word tag value, searched via both accepted quoting spellings plus a
    // second, space-separated ANDed term — through the actual `search()` entry point, hitting real
    // Redis indexes, the same way a live request does.
    #[tokio::test]
    async fn multi_word_tag_value_is_findable_both_quoted_forms_and_ands_with_a_second_term() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let eq = test_eq();
        let id = "3".repeat(40);
        let tags = "female:huge breasts,female:milf";

        let mut aconn = archive_pool.get().await.unwrap();
        let _: () = aconn
            .hset_multiple(
                &id,
                &[("tags", tags), ("pagecount", "10"), ("progress", "0")],
            )
            .await
            .unwrap();

        crate::indexer::index_new_archive(&search_pool, &eq, &id, "Book C")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &eq, &id, "", tags)
            .await
            .unwrap();

        for filter in [
            // Whole-token quote form.
            "\"female:huge breasts\"",
            // Value-only quote form (e-hentai's own literal syntax).
            "female:\"huge breasts\"",
            // Space-separated AND with a second, unquoted single-word term — the exact shape of
            // the originally-reported bug (`female:huge breasts female:milf` returning 0).
            "female:\"huge breasts\" female:milf",
        ] {
            let params = SearchParams {
                filter: filter.to_string(),
                groupby_tanks: true,
                ..Default::default()
            };
            let result = search(&archive_pool, &search_pool, &eq, &params)
                .await
                .unwrap();
            assert_eq!(
                result.ids,
                vec![id.clone()],
                "filter {filter:?} should match"
            );
        }

        // Deliberately NOT asserted here: "the unquoted form finds nothing". That was the
        // original claim, but it's false for *this* fixture specifically — `female:huge breasts`
        // splits into `female:huge` and `breasts`, and both fragments independently re-match the
        // very same tag they were cut from (`female:huge` prefix-globs `INDEX_female:huge
        // breasts*`; `breasts` substring-fuzzy-matches the same tag text), so the AND of the two
        // still finds this archive — not a coincidence of leftover data, an unavoidable structural
        // property of any fixture whose multi-word value contains its own prefix/substring as a
        // token boundary. (A local run against the host's own persistent test Redis happened to
        // "pass" this assertion — an artifact of unrelated pre-existing data in `filtered`'s scope
        // masking the real logic, not evidence the assertion was ever correct; CI's clean-slate
        // Redis exposed it immediately.) Space genuinely being a token delimiter now is already
        // covered without this trap by grammar.rs's own `space_separates_tokens_like_comma`
        // (token-level, no shared-substring fixture involved).

        // Negation (`-`) combined with the value-only quote form — still excludes the archive it
        // matches, same as any other token, whether quoted or not.
        let negated_params = SearchParams {
            filter: "-female:\"huge breasts\"".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let negated_result = search(&archive_pool, &search_pool, &eq, &negated_params)
            .await
            .unwrap();
        assert!(!negated_result.ids.contains(&id));

        let _: () = aconn.del(&id).await.unwrap();
        let mut sconn = search_pool.get().await.unwrap();
        let _: () = sconn.del("INDEX_female:huge breasts").await.unwrap();
        let _: () = sconn.del("INDEX_female:milf").await.unwrap();
        // `srem` this test's own id, not `del` the whole set — see the sibling test above (issue
        // #86) for why a wholesale `del` on a set shared across concurrently-running tests is a
        // real flake, not just a theoretical one.
        let _: () = sconn.srem(UNTAGGED_KEY, &id).await.unwrap();
        let _: () = sconn.srem(NEW_KEY, &id).await.unwrap();
        let _: () = sconn.srem(TANKGROUPED_KEY, &id).await.unwrap();
        let _: () = sconn
            .zrem(TITLES_KEY, "book c\0".to_string() + &id)
            .await
            .unwrap();
    }

    // A quoted multi-word query is a *title/phrase* lookup for most real uses ("find the work
    // called Tari Tari"), and real titles here are shaped `[circle] Title [DL版]` — so the phrase
    // sits in the middle of the title, never as the whole of it. Legacy's `"<title>\0id"`-with-a-
    // trailing-`\0` pattern only ever matched a title *equal* to the quoted string, which made
    // quoting return nothing for exactly this case (verified live: `"pink album"` → 0 against a
    // title stored as `[新堂エル] the pink album [dl版]`, while the bare `pink album` → 1). The
    // canonical branch now matches on "title contains the phrase" for both exact and fuzzy tokens,
    // while the tag half of an exact token still goes through the literal `INDEX_<tag>` key.
    #[tokio::test]
    async fn quoted_phrase_matches_a_title_that_only_contains_it_mid_string() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let eq = test_eq();
        let id = "7".repeat(40);
        // Deliberately no tag containing the phrase, so a match can only come from the title.
        let tags = "artist:jane";

        let mut aconn = archive_pool.get().await.unwrap();
        let _: () = aconn
            .hset_multiple(
                &id,
                &[("tags", tags), ("pagecount", "10"), ("progress", "0")],
            )
            .await
            .unwrap();

        crate::indexer::index_new_archive(&search_pool, &eq, &id, "[circle] Tari Tari [DL版]")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &eq, &id, "", tags)
            .await
            .unwrap();

        for filter in [
            // The phrase, quoted — the case legacy returned nothing for.
            "\"tari tari\"",
            // Same phrase via the `$` spelling, which is the same token as far as the parser is
            // concerned.
            "tari tari$",
            // And the plain AND spelling keeps working (it matched before this change too).
            "tari tari",
        ] {
            let params = SearchParams {
                filter: filter.to_string(),
                groupby_tanks: true,
                ..Default::default()
            };
            let result = search(&archive_pool, &search_pool, &eq, &params)
                .await
                .unwrap();
            assert_eq!(
                result.ids,
                vec![id.clone()],
                "filter {filter:?} should match the mid-title phrase"
            );
        }

        // A quoted phrase that is not present must still not match — "contains" is not "matches
        // loosely".
        let params = SearchParams {
            filter: "\"tari tokoyo\"".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let result = search(&archive_pool, &search_pool, &eq, &params)
            .await
            .unwrap();
        assert!(result.ids.is_empty());

        let _: () = aconn.del(&id).await.unwrap();
        // `remove_archive_index` builds the title members with the same helpers the indexer wrote
        // them with, so this test can't drift on the folded form of `[circle] Tari Tari [DL版]`.
        crate::indexer::remove_archive_index(
            &search_pool,
            &eq,
            &id,
            "[circle] Tari Tari [DL版]",
            tags,
        )
        .await
        .unwrap();
    }

    // `parse_rating_filter` is a pure function — no Redis needed, unlike the `token_matches`-level
    // integration tests above.
    #[tokio::test]
    async fn canonical_folding_makes_cn_jp_kana_and_latin_variants_findable() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let eq = test_eq();
        let mut aconn = archive_pool.get().await.unwrap();

        let id_jp = "a1".repeat(20);
        let id_cn = "b1".repeat(20);
        let id_half = "c1".repeat(20);
        let id_latin = "d1".repeat(20);

        for (id, title, tags) in [
            (&id_jp, "龍が如く", "series:龍が如く"),
            (&id_cn, "龙与虎", "series:龙与虎"),
            (&id_half, "スーパー", "series:スーパー"),
            (&id_latin, "MAR", "series:MAR"),
        ] {
            let _: () = aconn
                .hset_multiple(
                    id,
                    &[("tags", tags), ("pagecount", "10"), ("progress", "0")],
                )
                .await
                .unwrap();
            crate::indexer::index_new_archive(&search_pool, &eq, id, title)
                .await
                .unwrap();
            crate::indexer::update_tag_indexes(&search_pool, &eq, id, "", tags)
                .await
                .unwrap();
        }

        for (filter, expected) in [
            ("series:龙が如く", &id_jp),
            ("series:龍が如く", &id_jp),
            ("series:ｽｰﾊﾟｰ", &id_half),
            ("series:MÄR", &id_latin),
            ("龙*", &id_cn),
        ] {
            let params = SearchParams {
                filter: filter.to_string(),
                groupby_tanks: true,
                ..Default::default()
            };
            let result = search(&archive_pool, &search_pool, &eq, &params)
                .await
                .unwrap();
            assert!(
                result.ids.contains(expected),
                "filter {filter:?} should match {expected:?}, got {:?}",
                result.ids
            );
        }

        for id in [&id_jp, &id_cn, &id_half, &id_latin] {
            let _: () = aconn.del(id).await.unwrap();
        }
        let mut sconn = search_pool.get().await.unwrap();
        for id in [&id_jp, &id_cn, &id_half, &id_latin] {
            let _: () = sconn.srem(UNTAGGED_KEY, id).await.unwrap();
            let _: () = sconn.srem(NEW_KEY, id).await.unwrap();
            let _: () = sconn.srem(TANKGROUPED_KEY, id).await.unwrap();
        }
        // Remove this test's own canonical indexes (folded tag keys + both title zsets).
        for key in [
            "INDEX_series:龙が如く",
            "INDEX_series:龙与虎",
            "INDEX_series:すーぱー",
            "INDEX_series:MAR",
        ] {
            let _: () = sconn.del(key).await.unwrap();
        }
        for (title, id) in [
            ("龍が如く", &id_jp),
            ("龙与虎", &id_cn),
            ("スーパー", &id_half),
            ("MAR", &id_latin),
        ] {
            let _: () = sconn
                .zrem(TITLES_KEY, format!("{}\0{id}", title.to_lowercase()))
                .await
                .unwrap();
            let lower = title.to_lowercase();
            let folded = eq.fold(&lower).into_owned();
            let _: () = sconn
                .zrem(TITLES_FOLDED_KEY, format!("{folded}\0{id}"))
                .await
                .unwrap();
        }
    }

    #[test]
    fn parse_rating_filter_recognizes_every_operator() {
        assert_eq!(parse_rating_filter("rating:>=1"), Some((">=", 1.0)));
        assert_eq!(parse_rating_filter("rating:<=4"), Some(("<=", 4.0)));
        assert_eq!(parse_rating_filter("rating:>3"), Some((">", 3.0)));
        assert_eq!(parse_rating_filter("rating:<2"), Some(("<", 2.0)));
        assert_eq!(parse_rating_filter("rating:=5"), Some(("=", 5.0)));
    }

    #[test]
    fn parse_rating_filter_supports_decimal_precision() {
        assert_eq!(parse_rating_filter("rating:>=4.5"), Some((">=", 4.5)));
    }

    // A bare `rating:5` (no operator) is deliberately NOT claimed by this function — it falls
    // through to the existing exact-match tag-index lookup instead (see this function's own docs
    // on why duplicating that path would be pointless).
    #[test]
    fn parse_rating_filter_ignores_bare_value_with_no_operator() {
        assert_eq!(parse_rating_filter("rating:5"), None);
    }

    #[test]
    fn parse_rating_filter_ignores_other_namespaces() {
        assert_eq!(parse_rating_filter("pages:>=5"), None);
        assert_eq!(parse_rating_filter("artist:jane"), None);
    }

    #[test]
    fn parse_rating_filter_rejects_unparseable_numbers() {
        assert_eq!(parse_rating_filter("rating:>=abc"), None);
    }

    /// The API's cue to fetch the split-suggestion set at all: must parse rather than
    /// substring-match, so a *quoted* spelling (a literal tag search for that name) does not
    /// trigger a config-DB round trip on every such request.
    #[test]
    fn mentions_split_suggestion_only_for_the_real_operator() {
        assert!(mentions_split_suggestion("has:split-suggestion"));
        assert!(mentions_split_suggestion("artist:x | has:split-suggestion"));
        assert!(mentions_split_suggestion(
            "artist:x -(artist:y has:split-suggestion)"
        ));
        assert!(!mentions_split_suggestion("artist:x"));
        // Quoting does *not* turn a reserved operator namespace back into literal tag text, so the
        // quoted spelling is still the operator — asserted rather than assumed, because the
        // opposite would silently skip the set fetch and make the operator match nothing.
        assert!(mentions_split_suggestion(r#""has:split-suggestion""#));
        // A tag that merely contains the characters is a different token entirely.
        assert!(!mentions_split_suggestion("artist:has:split-suggestion"));
    }

    // End-to-end coverage for the expression tree and the first batch of attribute/field
    // operators: OR (`|`), grouping, `title:`, `is:`, `has:`, `in:` and `size:` — all through the
    // public `search()` entry point against real Redis indexes. Deliberately its own synthetic ids
    // (never a single-character repeat, per this file's own collision history).
    #[tokio::test]
    async fn boolean_structure_and_attribute_operators() {
        let Some((archive_pool, search_pool)) = test_pools().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };

        let eq = test_eq();
        let id_a = "8".repeat(40);
        let id_b = "9".repeat(40);
        let id_c = "e1".repeat(20);

        let mut aconn = archive_pool.get().await.unwrap();
        for (id, title, tags, arcsize, patch, progress) in [
            // a: the only one whose *title* carries the phrase, the biggest, the patched one.
            (
                &id_a,
                "Orword Fixture",
                "artist:alpha,female:milf",
                "2000000",
                "true",
                "0",
            ),
            // b: same phrase, but as a *tag* — this is what makes the `title:` assertion meaningful.
            (
                &id_b,
                "Second Fixture",
                "series:orword,artist:beta",
                "1024",
                "false",
                "0",
            ),
            (
                &id_c,
                "Third Fixture",
                "artist:gamma",
                "500000",
                "false",
                "6",
            ),
        ] {
            // The `title` field is written here as well as into the title *index* below, exactly as
            // a real archive record is (`ArchiveRepository::save` writes both): the `phrase:`
            // operator scans the archive's own hash, so a fixture with only the index entry would
            // silently test nothing.
            let _: () = aconn
                .hset_multiple(
                    id,
                    &[
                        ("title", title),
                        ("tags", tags),
                        ("pagecount", "10"),
                        ("progress", progress),
                        ("arcsize", arcsize),
                        ("has_patch", patch),
                    ],
                )
                .await
                .unwrap();
        }

        for (id, title) in [
            (&id_a, "Orword Fixture"),
            (&id_b, "Second Fixture"),
            (&id_c, "Third Fixture"),
        ] {
            crate::indexer::index_new_archive(&search_pool, &eq, id, title)
                .await
                .unwrap();
        }
        for (id, tags) in [
            (&id_a, "artist:alpha,female:milf"),
            (&id_b, "series:orword,artist:beta"),
            (&id_c, "artist:gamma"),
        ] {
            crate::indexer::update_tag_indexes(&search_pool, &eq, id, "", tags)
                .await
                .unwrap();
        }

        async fn ids_for(
            archive_pool: &Pool,
            search_pool: &Pool,
            eq: &Equivalence,
            filter: &str,
            groupby_tanks: bool,
        ) -> Vec<String> {
            let params = SearchParams {
                filter: filter.to_string(),
                groupby_tanks,
                ..Default::default()
            };
            let mut result = search(archive_pool, search_pool, eq, &params)
                .await
                .unwrap()
                .ids;
            result.sort();
            result
        }

        let search_ids = |filter: &'static str| {
            let (archive_pool, search_pool, eq) =
                (archive_pool.clone(), search_pool.clone(), test_eq());
            async move { ids_for(&archive_pool, &search_pool, &eq, filter, true).await }
        };

        // `|` is OR; adjacency is still AND, and binds tighter.
        let mut expected = vec![id_a.clone(), id_b.clone()];
        expected.sort();
        assert_eq!(search_ids("artist:alpha | artist:beta").await, expected);
        assert_eq!(
            search_ids("artist:alpha | artist:beta artist:gamma").await,
            vec![id_a.clone()]
        );
        // Grouping: `(alpha | beta) gamma` is unsatisfiable (no archive carries gamma *and* one of
        // the other two), where `alpha | (beta gamma)` is just alpha.
        assert!(search_ids("(artist:alpha | artist:beta) artist:gamma")
            .await
            .is_empty());
        assert_eq!(
            search_ids("artist:alpha | (artist:beta artist:gamma)").await,
            vec![id_a.clone()]
        );
        // Negation over a whole group.
        assert_eq!(
            search_ids("artist:gamma -(artist:alpha | artist:gamma)").await,
            Vec::<String>::new()
        );

        // `title:` matches b's *tag* text too when unscoped, and only a's title once scoped — the
        // one thing legacy's syntax could not express.
        let mut both = vec![id_a.clone(), id_b.clone()];
        both.sort();
        assert_eq!(search_ids("orword").await, both);
        assert_eq!(search_ids("title:orword").await, vec![id_a.clone()]);
        assert!(!search_ids("-title:orword").await.contains(&id_a));

        // `is:new` (all three were just indexed as new), `is:untagged` (none are).
        assert_eq!(search_ids("artist:alpha is:new").await, vec![id_a.clone()]);
        assert!(search_ids("artist:alpha is:untagged").await.is_empty());

        // `has:patch`.
        assert_eq!(
            search_ids("artist:alpha has:patch").await,
            vec![id_a.clone()]
        );
        assert!(search_ids("artist:beta has:patch").await.is_empty());

        // `has:bookmark` — read from the bookmark repository's own archive->timestamp map (no
        // search-side index), so it is asserted through a real add/remove round trip rather than a
        // hand-written Redis key. `beta` gets two bookmarked pages (pinning "at least one"), and is
        // cleaned up again so a repeat run starts where this one did.
        let bookmarks =
            lanrurugi_storage::bookmarks::BookmarksRepository::new(archive_pool.clone());
        assert!(search_ids("artist:beta has:bookmark").await.is_empty());
        bookmarks.add(&id_b, 1, 1_700_000_000).await.unwrap();
        bookmarks.add(&id_b, 4, 1_700_000_060).await.unwrap();
        assert_eq!(
            search_ids("artist:beta has:bookmark").await,
            vec![id_b.clone()]
        );
        assert!(search_ids("artist:alpha has:bookmark").await.is_empty());
        // Composes like any other atom: OR across namespaces, and negation.
        assert_eq!(
            search_ids("artist:alpha has:bookmark | artist:beta has:bookmark").await,
            vec![id_b.clone()]
        );
        assert!(search_ids("artist:beta -has:bookmark").await.is_empty());

        // `bookmark:"name"` — fuzzy over the bookmark's own name, the text counterpart of
        // `has:bookmark`. A synthetic name keeps this independent of any real library content.
        assert!(search_ids(r#"bookmark:"Fixture Chapter""#).await.is_empty());
        bookmarks
            .set_name(&id_b, 4, Some("Fixture Chapter One"))
            .await
            .unwrap();
        assert_eq!(
            search_ids(r#"bookmark:"Fixture Chapter""#).await,
            vec![id_b.clone()],
            "quoted multi-word name must stay one contiguous phrase"
        );
        assert_eq!(
            search_ids("bookmark:chapter").await,
            vec![id_b.clone()],
            "unquoted single word is a substring match on the name"
        );
        assert_eq!(
            search_ids("bookmark:Fixture*One").await,
            vec![id_b.clone()],
            "grammar wildcards reach the name the same way they reach tags"
        );
        assert!(search_ids("bookmark:nosuchname").await.is_empty());
        // The name is per bookmarked *page*: clearing it drops the archive out of the name match
        // while `has:bookmark` stays true (page 1 is still bookmarked, just unnamed).
        bookmarks.set_name(&id_b, 4, None).await.unwrap();
        assert!(search_ids("bookmark:chapter").await.is_empty());
        assert_eq!(
            search_ids("artist:beta has:bookmark").await,
            vec![id_b.clone()]
        );

        bookmarks.remove_all_for_archive(&id_b).await.unwrap();
        assert!(search_ids("artist:beta has:bookmark").await.is_empty());

        // `phrase:` — ordered adjacency across the archive's whole text (title + tags), which no
        // quoting spelling can express. `id_a`'s title is "Orword Fixture" and `id_b`'s tag is
        // `series:orword`, so only a phrase check that spans whichever text holds the words can
        // see either; the synthetic phrases below are deliberately not real library content.
        assert!(search_ids(r#"phrase:"Orword Fixture""#)
            .await
            .contains(&id_a));
        assert!(!search_ids(r#"phrase:"Fixture Orword""#)
            .await
            .contains(&id_a));
        assert!(search_ids("phrase:FIXTURE").await.contains(&id_a));

        // `date_added:` comparison forms. The fixtures carry no `date_added` tag at all, so the
        // assertion is about the *shape* being recognized: `>=`/`<=`/`<`/`>` must resolve to a
        // range (an unrecognized token would instead fall through to a tag lookup and match
        // nothing here anyway, so each is checked against a bound that must include/exclude a
        // freshly-stamped archive).
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut aconn2 = archive_pool.get().await.unwrap();
        let _: () = aconn2
            .hset(&id_c, "tags", format!("artist:gamma,date_added:{stamp}"))
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(
            &search_pool,
            &eq,
            &id_c,
            "artist:gamma",
            &format!("artist:gamma,date_added:{stamp}"),
        )
        .await
        .unwrap();
        assert_eq!(
            search_ids("artist:gamma date_added:>=1970-01-02").await,
            vec![id_c.clone()]
        );
        assert!(search_ids("artist:gamma date_added:<1970-01-02")
            .await
            .is_empty());
        assert_eq!(
            search_ids("artist:gamma date_added:>1970-01-02").await,
            vec![id_c.clone()]
        );
        assert!(search_ids("artist:gamma date_added:<=1969-12-31")
            .await
            .is_empty());

        // `has:split-suggestion` — the caller-supplied set. Unset (`None`, which is what a plain
        // `SearchParams::default()` gives) must match *nothing* rather than everything, and a set
        // supplied by the caller must be intersected with the rest of the query like any other leaf.
        assert!(search_ids("artist:alpha has:split-suggestion")
            .await
            .is_empty());
        let with_suggestion = SearchParams {
            filter: "artist:alpha has:split-suggestion".to_string(),
            split_suggestion_archive_ids: Some([id_a.clone()].into_iter().collect()),
            groupby_tanks: true,
            ..Default::default()
        };
        assert_eq!(
            search(&archive_pool, &search_pool, &eq, &with_suggestion)
                .await
                .unwrap()
                .ids,
            vec![id_a.clone()]
        );
        let wrong_archive = SearchParams {
            filter: "artist:alpha has:split-suggestion".to_string(),
            split_suggestion_archive_ids: Some([id_b.clone()].into_iter().collect()),
            groupby_tanks: true,
            ..Default::default()
        };
        assert!(search(&archive_pool, &search_pool, &eq, &wrong_archive)
            .await
            .unwrap()
            .ids
            .is_empty());

        // `sortby=relevance` — opt-in, and the only sort whose order depends on the query. `orword`
        // matches id_a through its *title* and id_b through a *tag*, so a title hit outranking a tag
        // hit is exactly what this pins.
        let params = SearchParams {
            filter: "orword".to_string(),
            sortby: Some("relevance".to_string()),
            groupby_tanks: true,
            ..Default::default()
        };
        let ordered = search(&archive_pool, &search_pool, &eq, &params)
            .await
            .unwrap()
            .ids;
        let position = |wanted: &str| ordered.iter().position(|id| id == wanted);
        assert!(
            position(&id_a) < position(&id_b),
            "a title hit must outrank a tag hit: {ordered:?}"
        );

        // `category:` and `has:category` — one static category (by id and by name) plus one
        // *dynamic* one whose predicate must be evaluated as a nested query. Synthetic ids/names
        // keep this independent of any real library content; deleted again below so a repeat run
        // starts where this one did.
        let categories =
            lanrurugi_storage::repository::CategoryRepository::new(archive_pool.clone());
        let category_id = lanrurugi_core::ids::CategoryId("SET_9999000001".to_string());
        let dynamic_id = lanrurugi_core::ids::CategoryId("SET_9999000002".to_string());
        categories
            .save(&lanrurugi_core::entities::Category {
                catid: category_id.clone(),
                name: "Fixture Static".to_string(),
                search: None,
                archives: vec![lanrurugi_core::ids::ArchiveId(id_b.clone())],
                pinned: false,
                visible_to_guest: false,
            })
            .await
            .unwrap();
        categories
            .save(&lanrurugi_core::entities::Category {
                catid: dynamic_id.clone(),
                name: "Fixture Dynamic".to_string(),
                search: Some("artist:alpha".to_string()),
                archives: Vec::new(),
                pinned: false,
                visible_to_guest: false,
            })
            .await
            .unwrap();
        assert_eq!(
            search_ids("category:SET_9999000001").await,
            vec![id_b.clone()],
            "a static category is addressable by its id"
        );
        assert_eq!(
            search_ids(r#"category:"Fixture Static""#).await,
            vec![id_b.clone()],
            "…and by its name, quoted so the two words stay one pattern"
        );
        assert_eq!(
            search_ids(r#"category:"Fixture Dynamic""#).await,
            vec![id_a.clone()],
            "a dynamic category contributes its predicate's own result set"
        );
        // `has:category` is deliberately static-only: `id_b` is filed in the static one, while
        // `id_a` is only ever a *member of the dynamic category's result*, not of any stored list.
        let with_category = search_ids("has:category").await;
        assert!(with_category.contains(&id_b));
        assert!(!with_category.contains(&id_a));
        // Composes with the rest of the language, including negation.
        assert_eq!(
            search_ids("has:category -artist:beta").await,
            Vec::<String>::new()
        );
        categories.delete(&category_id).await.unwrap();
        categories.delete(&dynamic_id).await.unwrap();

        // `size:` with a binary suffix, and its natural range spelled as two conditions.
        assert_eq!(
            search_ids("artist:alpha size:>=1M").await,
            vec![id_a.clone()]
        );
        assert!(search_ids("artist:beta size:>=1M").await.is_empty());
        assert!(search_ids("size:>=100K size:<1M").await.contains(&id_c));

        // `read:` as a percentage of page count (c is 6/10 pages in).
        assert_eq!(
            search_ids("artist:gamma read:>=50%").await,
            vec![id_c.clone()]
        );
        assert!(search_ids("artist:gamma read:>=90%").await.is_empty());

        // `in:tank` flips once the archive leaves the ungrouped set (what a real Tankoubon
        // membership change does) — asserted through the ungrouped entry point, since the default
        // grouped one no longer has that id as a candidate at all.
        let mut sconn = search_pool.get().await.unwrap();
        let _: () = sconn.srem(TANKGROUPED_KEY, &id_a).await.unwrap();
        assert_eq!(
            ids_for(
                &archive_pool,
                &search_pool,
                &eq,
                "artist:alpha in:tank",
                false
            )
            .await,
            vec![id_a.clone()]
        );
        // ...while an unrelated attribute still matches it there: leaving the ungrouped set is a
        // statement about Tankoubon membership, not about the archive's own state.
        assert_eq!(
            ids_for(
                &archive_pool,
                &search_pool,
                &eq,
                "artist:alpha is:new",
                false
            )
            .await,
            vec![id_a.clone()]
        );

        for id in [&id_a, &id_b, &id_c] {
            let _: () = aconn.del(id).await.unwrap();
        }
        let _: () = sconn.srem(NEW_KEY, &id_b).await.unwrap();
        let _: () = sconn.srem(NEW_KEY, &id_c).await.unwrap();
        let _: () = sconn.srem(TANKGROUPED_KEY, &id_b).await.unwrap();
        let _: () = sconn.srem(TANKGROUPED_KEY, &id_c).await.unwrap();
        for (id, title, tags) in [
            (&id_a, "Orword Fixture", "artist:alpha,female:milf"),
            (&id_b, "Second Fixture", "series:orword,artist:beta"),
            (&id_c, "Third Fixture", "artist:gamma"),
        ] {
            let _ = crate::indexer::remove_archive_index(&search_pool, &eq, id, title, tags).await;
        }
    }
}
