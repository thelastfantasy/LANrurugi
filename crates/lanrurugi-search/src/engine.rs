//! Search execution, verified against `~/LANraragi/lib/LANraragi/Model/Search.pm::do_search`/
//! `search_uncached`. Ports the filtering/sorting logic directly onto Redis; the search-results
//! cache (`LRR_SEARCHCACHE`, with its cache-key-inversion trick for opposite sort order) is a
//! documented Phase 1 simplification — every search here does the "uncached" full pass. Response
//! correctness doesn't depend on the cache (it's purely a latency optimization); SC-008's
//! benchmark (US8) is where a cache would earn its complexity back if warranted.

use std::collections::HashSet;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::entities::Category;
use thiserror::Error;

use crate::grammar::{compute_search_filter, Token};
use crate::keys::{NEW_KEY, TANKGROUPED_KEY, TITLES_KEY, UNTAGGED_KEY};

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
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
    /// IANA timezone identifier (e.g. `"Asia/Tokyo"`, `"UTC"`) used only by `date_added:YYYY-MM-DD`
    /// date-range tokens — the day an archive was added is computed in this timezone, not the
    /// viewer's browser timezone, so two viewers always agree on which archives belong to a given
    /// searched date. API layer reads this from the `timezone` setting; defaults to UTC upstream.
    pub timezone: String,
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
            hidecompleted: false,
            groupby_tanks: true,
            timezone: "UTC".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub total: i64,
    pub filtered_count: usize,
    pub ids: Vec<String>,
}

const ARCHIVE_KEY_GLOB: &str = "????????????????????????????????????????";

/// `hidecompleted`'s own "counts as finished" threshold — matches legacy's
/// `Model/Search.pm::search_uncached` (`$progress / $pagecount > 0.85`), not a Phase-1 invention.
const HIDE_COMPLETED_THRESHOLD: f64 = 0.85;

pub async fn search(
    archive_pool: &Pool,
    search_pool: &Pool,
    params: &SearchParams,
) -> Result<SearchResult> {
    let mut archive_conn = archive_pool.get().await?;
    let mut search_conn = search_pool.get().await?;

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

    let mut tokens = compute_search_filter(&params.filter);

    if let Some(category) = &params.category {
        if let Some(predicate) = &category.search {
            tokens.extend(compute_search_filter(predicate));
        } else {
            let cat_set: HashSet<String> =
                category.archives.iter().map(|a| a.to_string()).collect();
            filtered.retain(|id| cat_set.contains(id));
        }
    }

    if params.untaggedonly {
        let untagged: HashSet<String> = search_conn
            .smembers::<_, Vec<String>>(UNTAGGED_KEY)
            .await?
            .into_iter()
            .collect();
        filtered.retain(|id| untagged.contains(id));
    }

    if params.newonly {
        let new_set: HashSet<String> = search_conn
            .smembers::<_, Vec<String>>(NEW_KEY)
            .await?
            .into_iter()
            .collect();
        filtered.retain(|id| new_set.contains(id));
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

    for token in &tokens {
        if filtered.is_empty() {
            break;
        }
        let ids = token_matches(
            &mut archive_conn,
            &mut search_conn,
            token,
            &filtered,
            &params.timezone,
        )
        .await?;
        if ids.is_empty() && !token.isneg {
            filtered.clear();
            break;
        }
        if token.isneg {
            filtered.retain(|id| !ids.contains(id));
        } else {
            filtered.retain(|id| ids.contains(id));
        }
    }

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
        sortkey,
        params.order_desc,
        &filtered,
    )
    .await?;

    Ok(SearchResult {
        total,
        filtered_count: ordered.len(),
        ids: ordered,
    })
}

/// pages:/read: numeric filters, the additive `date_added:YYYY-MM-DD` date-range filter, and the
/// general tag-index/title-fuzzy-match lookup, matching `search_uncached`'s per-token logic (the
/// date-range branch is an additive improvement over legacy, which has no "search by calendar day"
/// at all — see [`parse_date_range`]).
async fn token_matches(
    archive_conn: &mut deadpool_redis::Connection,
    search_conn: &mut deadpool_redis::Connection,
    token: &Token,
    scope: &HashSet<String>,
    timezone: &str,
) -> Result<HashSet<String>> {
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
    let exact_key = format!("INDEX_{}", token.tag);
    let exact_hit = token.isexact && search_conn.exists(&exact_key).await.unwrap_or(false);
    if exact_hit {
        let members: Vec<String> = search_conn.smembers(&exact_key).await.unwrap_or_default();
        ids.extend(members);
    } else {
        let pattern = if token.tag.contains(':') {
            format!("INDEX_{}*", token.tag)
        } else {
            format!("INDEX_*{}*", token.tag)
        };
        let keys: Vec<String> = search_conn.keys(pattern).await?;
        for key in keys {
            let members: Vec<String> = search_conn.smembers(&key).await?;
            ids.extend(members);
        }
    }

    // Fuzzy title match: LRR_TITLES members are "title\0id".
    let name_pattern = if token.isexact {
        format!("{}\0*", token.tag)
    } else {
        format!("*{}*", token.tag)
    };
    let title_members: Vec<String> = search_conn
        .zrangebyscore(TITLES_KEY, "-inf", "+inf")
        .await
        .unwrap_or_default();
    for member in title_members {
        if glob_match(&name_pattern, &member) {
            if let Some(pos) = member.find('\0') {
                ids.insert(member[pos + 1..].to_string());
            }
        }
    }

    Ok(ids)
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
    Some((day_start, day_end))
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
    sortkey: &str,
    order_desc: bool,
    filtered: &HashSet<String>,
) -> Result<Vec<String>> {
    if sortkey == "title" {
        let ordered: Vec<String> = search_conn
            .zrangebyscore(TITLES_KEY, "-inf", "+inf")
            .await?;
        let mut result = Vec::new();
        for member in ordered {
            if let Some(pos) = member.find('\0') {
                let id = &member[pos + 1..];
                if filtered.contains(id) {
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

    // Sort by an arbitrary tag namespace, mirroring legacy `Model/Search.pm`'s `sort_results`:
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
    // Tank's own tags live at zset score -2 (`tags_<value>` — `GroupingRepository`'s
    // `SCORE_TAGS`; the same member shape `GroupingRepository::get` decodes).
    let own: Vec<String> = archive_conn
        .zrangebyscore(tank_id, -2, -2)
        .await
        .unwrap_or_default();
    if let Some(tag) = own.first().and_then(|m| m.strip_prefix("tags_")) {
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

    fn test_pools() -> Option<(Pool, Pool)> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let archive = deadpool_redis::Config::from_url(format!("{}/0", base.trim_end_matches('/')))
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()?;
        let search = deadpool_redis::Config::from_url(format!("{}/3", base.trim_end_matches('/')))
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .ok()?;
        Some((archive, search))
    }

    /// Issue regression (2026-08-04): a descending `date_added` sort used to `rev()` the *whole*
    /// list at the call site, flipping unkeyed ids — Tankoubons, which have no archive `tags`
    /// hash — to the front, so a library sorted newest-first showed its (older) Tankoubons above
    /// freshly-added archives. Legacy sorts only the keyed section and pushes unkeyed ids on at
    /// the back regardless of direction.
    #[tokio::test]
    async fn date_added_descending_keeps_unkeyed_tankoubons_at_the_back() {
        let Some((archive_pool, search_pool)) = test_pools() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let mut aconn = archive_pool.get().await.unwrap();
        let id_old = "a".repeat(40);
        let id_new = "b".repeat(40);
        let id_no_date = "c".repeat(40);
        let tank = "TANK_1785750000";

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
            "date_added",
            true,
            &filtered,
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
        let Some((archive_pool, search_pool)) = test_pools() else {
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
            "date_added",
            true,
            &filtered,
        )
        .await
        .unwrap();

        // Descending: tank (imputed 3000) above member (3000, ties broken by input order) and
        // both above the older archive; nothing unkeyed.
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], tank, "tank must impute its member's newest date");
        assert_eq!(ids[2], id_old, "oldest archive last");

        for id in [&id_old, &member_new, &tank.to_string()] {
            let _: () = aconn.del(id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn finds_archive_by_tag_and_respects_negation() {
        let Some((archive_pool, search_pool)) = test_pools() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
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

        crate::indexer::index_new_archive(&search_pool, &id_a, "Book A")
            .await
            .unwrap();
        crate::indexer::index_new_archive(&search_pool, &id_b, "Book B")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &id_a, "", "artist:jane")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &id_b, "", "artist:bob")
            .await
            .unwrap();

        let params = SearchParams {
            filter: "artist:jane".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let result = search(&archive_pool, &search_pool, &params).await.unwrap();
        assert_eq!(result.ids, vec![id_a.clone()]);

        let neg_params = SearchParams {
            filter: "-artist:jane".to_string(),
            groupby_tanks: true,
            ..Default::default()
        };
        let neg_result = search(&archive_pool, &search_pool, &neg_params)
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
        let _: () = sconn.del(UNTAGGED_KEY).await.unwrap();
        let _: () = sconn.del(NEW_KEY).await.unwrap();
        let _: () = sconn.del(TANKGROUPED_KEY).await.unwrap();
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
        let Some((archive_pool, search_pool)) = test_pools() else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
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

        crate::indexer::index_new_archive(&search_pool, &id, "Book C")
            .await
            .unwrap();
        crate::indexer::update_tag_indexes(&search_pool, &id, "", tags)
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
            let result = search(&archive_pool, &search_pool, &params).await.unwrap();
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
        let negated_result = search(&archive_pool, &search_pool, &negated_params)
            .await
            .unwrap();
        assert!(!negated_result.ids.contains(&id));

        let _: () = aconn.del(&id).await.unwrap();
        let mut sconn = search_pool.get().await.unwrap();
        let _: () = sconn.del("INDEX_female:huge breasts").await.unwrap();
        let _: () = sconn.del("INDEX_female:milf").await.unwrap();
        let _: () = sconn.del(UNTAGGED_KEY).await.unwrap();
        let _: () = sconn.del(NEW_KEY).await.unwrap();
        let _: () = sconn.del(TANKGROUPED_KEY).await.unwrap();
        let _: () = sconn
            .zrem(TITLES_KEY, "book c\0".to_string() + &id)
            .await
            .unwrap();
    }
}
