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

#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub filter: String,
    pub category: Option<Category>,
    pub sortby: Option<String>,
    pub order_desc: bool,
    pub newonly: bool,
    pub untaggedonly: bool,
    pub hidecompleted: bool,
    pub groupby_tanks: bool,
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
            let cat_set: HashSet<String> = category.archives.iter().cloned().collect();
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
        let ids = token_matches(&mut archive_conn, &mut search_conn, token, &filtered).await?;
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
    let ordered = sort_ids(&mut archive_conn, &mut search_conn, sortkey, &filtered).await?;
    let ordered = if params.order_desc {
        ordered.into_iter().rev().collect()
    } else {
        ordered
    };

    Ok(SearchResult {
        total,
        filtered_count: ordered.len(),
        ids: ordered,
    })
}

/// pages:/read: numeric filters plus the general tag-index/title-fuzzy-match lookup, matching
/// `search_uncached`'s per-token logic.
async fn token_matches(
    archive_conn: &mut deadpool_redis::Connection,
    search_conn: &mut deadpool_redis::Connection,
    token: &Token,
    scope: &HashSet<String>,
) -> Result<HashSet<String>> {
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

    let mut ids = HashSet::new();

    if token.isexact {
        let members: Vec<String> = search_conn
            .smembers(format!("INDEX_{}", token.tag))
            .await
            .unwrap_or_default();
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
        return Ok(result);
    }

    if sortkey == "lastread" {
        let mut pairs: Vec<(String, u64)> = Vec::new();
        for id in filtered {
            let t: u64 = archive_conn.hget(id, "lastreadtime").await.unwrap_or(0);
            if t > 0 {
                pairs.push((id.clone(), t));
            }
        }
        pairs.sort_by_key(|p| std::cmp::Reverse(p.1));
        return Ok(pairs.into_iter().map(|(id, _)| id).collect());
    }

    // Sort by an arbitrary tag namespace: archives with that namespace first (alphabetically by
    // value), archives without it at the back.
    let mut keyed: Vec<(String, String)> = Vec::new();
    let mut unkeyed: Vec<String> = Vec::new();
    for id in filtered {
        let tags: String = archive_conn.hget(id, "tags").await.unwrap_or_default();
        let value = tags.split(',').find_map(|t| {
            let t = t.trim();
            t.strip_prefix(&format!("{sortkey}:"))
        });
        match value {
            Some(v) => keyed.push((id.clone(), v.to_ascii_lowercase())),
            None => unkeyed.push(id.clone()),
        }
    }
    keyed.sort_by(|a, b| a.1.cmp(&b.1));
    let mut result: Vec<String> = keyed.into_iter().map(|(id, _)| id).collect();
    result.extend(unkeyed);
    Ok(result)
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
}
