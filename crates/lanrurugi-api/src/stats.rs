//! `GET /stats/overview` — one aggregated snapshot backing the `/stats` page's dashboard
//! sections (size distribution, container format/mime breakdown, ingestion history, reading
//! history, and a reading-preference analysis). Additive and LANrurugi-only: legacy's own
//! `/stats` page shows four counters and a tag cloud, all of which stay exactly as they were
//! (the vendored upstream OpenAPI document has no `/stats` path to stay compatible with).
//!
//! Everything here is derived from data the library already holds — the archive records
//! themselves, the operator activity log, and two additive Redis counters written by
//! `archives.rs::update_progress` (see `lanrurugi_storage::keys`'s own docs on why reading has no
//! usable time dimension without them). No new storage beyond those two counters, and no model,
//! no index: the "preference analysis" is a deliberately simple, explainable weighting (recency,
//! completion, rating, bookmarks, re-read count) rather than anything learned.
//!
//! Cost is one `list_all()` plus a handful of Redis reads — the same shape `database.rs::stats`
//! already pays for the tag cloud, and acceptable for an operator-facing page rather than a hot
//! path. The only *variable* cost is one search per dynamic category, capped by
//! [`MAX_DYNAMIC_CATEGORIES`] with an honest truncation flag in the response.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::entities::{Archive, Category};
use lanrurugi_search::engine::{search, SearchParams};
use serde::{Deserialize, Serialize};

use crate::auth_context::{AuthContext, AuthMethod};
use crate::common::error;
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityFilter, ActorKind};
use lanrurugi_storage::keys::{
    read_daily_key, CONFIG_KEY, READ_DAILY_SINCE_KEY, READ_FREQ_STAT_KEY, TOTAL_PAGES_STAT_KEY,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/stats/overview", get(overview))
        .route("/stats/timeline", get(timeline))
        .route("/stats/bucket", get(bucket))
}

/// Longest span `GET /stats/timeline` will materialize, in days (~5.5 years). The series is dense
/// and daily, so this is what keeps one request from asking for a million points; a request wider
/// than this is clamped to the *newest* this many days, and the response's `available_from` still
/// reports where the data really starts so the page can say the view is truncated.
const TIMELINE_MAX_DAYS: u64 = 2000;
/// The per-day reading counter's own keys expire after
/// [`lanrurugi_storage::keys::READ_DAILY_TTL_SECS`] (~400 days), so `GET /stats/timeline` never
/// asks Redis for more days than could still exist — older days in the window are simply zero for
/// `pages`, while `last_read` (derived from the archives themselves) still has data there.
const READ_COUNTER_HORIZON_DAYS: u64 = 400;
/// How many recent ingestions / readings / largest archives / re-read works to return.
const RECENT_LIMIT: usize = 10;
/// Rows per namespace in the preference analysis, and how many namespaces get rows at all.
const TOP_TAGS_PER_NAMESPACE: usize = 8;
const MAX_NAMESPACES: usize = 12;
/// How many dynamic categories are resolved through the search engine before this endpoint stops
/// (a static category's membership is free; a dynamic one costs a real search). The flag in the
/// response says whether the cap was hit, so the page can be honest rather than silently partial.
const MAX_DYNAMIC_CATEGORIES: usize = 40;
/// Most recent `archive.ingest` activity entries sampled for the ingestion-source breakdown.
const INGEST_SOURCE_SAMPLE: isize = 2000;

/// The ingestion-source rows the page always shows, in display order, zero-filled — a *closed* set
/// of paths, so a path that has simply never been used still gets a row (and "0 archives" then
/// says "this path exists but is unused", which a silently-omitted row cannot).
///
/// Uploads appear twice because they are the one path a script and a browser reach identically:
/// `upload_browser` and `upload_token` split the same `after.source: "upload"` record by the
/// activity entry's own actor kind, which is where the credential distinction actually lives.
/// Downloads and scans keep one row each — a download's actor is whoever started it or the runner
/// that scheduled it, and that is not a distinction the chart's reader is asking about here.
const INGEST_SOURCE_ROWS: &[&str] = &["download", "scanner", "upload_browser", "upload_token"];

/// Tags that describe the record rather than its content — never preference signals. `rating` is
/// excluded because it is a *weight* on a work rather than a property of one (see
/// [`preference_weights`]); it gets its own breakdown in [`Preferences::ratings`] instead.
const NON_CONTENT_NAMESPACES: &[&str] = &["date_added", "timestamp", "source", "rating"];

/// Tag *values* that are not what their namespace's name promises, dropped from that namespace's
/// own breakdown. `language:translated` is the one case today: it records that a work is a
/// translation, not which language it is written in, so in a "preferred languages" list it just
/// competes with the real languages it describes (and ranked second on this instance's own data
/// before it was dropped). Scoped per namespace rather than globally, since `translated` is a
/// perfectly meaningful value anywhere else.
const NON_AXIS_TAG_VALUES: &[(&str, &str)] = &[("language", "translated")];

/// Namespaces the preferences section promotes into their own list, in display order: the axes a
/// reader actually picks works by (interface language, author, circle, cosplayer), which otherwise
/// share a single "top tags per namespace" list with the long tail of content tags — where a small
/// library can leave them out of the top-[`MAX_NAMESPACES`] entirely, or leave `cosplayer` below the
/// per-namespace cap. `circle` is here for the same reason `artist` is: a doujin circle is a much
/// better predictor of what a reader wants next than the content tags it competes with, and on this
/// library its weight is far below the cap that would otherwise hide it. Whatever appears here is
/// excluded from [`Preferences::by_namespace`], so nothing is listed twice. Adding another axis is a
/// one-line change to this roster.
const HIGHLIGHTED_NAMESPACES: &[&str] = &["language", "artist", "circle", "cosplayer"];

/// Half-life of the recency term in [`preference_weights`], in days: a read half as recent as
/// another counts half as much.
const PREF_HALF_LIFE_DAYS: f64 = 90.0;

/// Bucket boundaries in bytes, upper-exclusive except the last. Chosen around the shape of real
/// manga archives (a single chapter is a few MB, a full volume tens of MB); every boundary is an
/// exact binary unit so the frontend's labels never have to round.
const SIZE_BUCKETS: &[(&str, u64, Option<u64>)] = &[
    ("lt_1mb", 0, Some(1 << 20)),
    ("lt_5mb", 1 << 20, Some(5 << 20)),
    ("lt_10mb", 5 << 20, Some(10 << 20)),
    ("lt_25mb", 10 << 20, Some(25 << 20)),
    ("lt_50mb", 25 << 20, Some(50 << 20)),
    ("lt_100mb", 50 << 20, Some(100 << 20)),
    ("lt_250mb", 100 << 20, Some(250 << 20)),
    ("lt_500mb", 250 << 20, Some(500 << 20)),
    ("gte_500mb", 500 << 20, None),
];

// ─────────────────────────── response shapes ───────────────────────────

#[derive(Debug, Serialize)]
pub struct Overview {
    pub generated_at: u64,
    pub totals: Totals,
    pub size_buckets: Vec<SizeBucketStat>,
    pub formats: Vec<FormatStat>,
    pub ingest: Ingest,
    pub reading: Reading,
    pub preferences: Preferences,
    /// Machine-readable reason the reading sections are empty, when they are —
    /// `no_server_side_reading`. The page turns this into a sentence rather than showing a chart
    /// of zeroes with no explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

#[derive(Debug, Default, Serialize)]
pub struct Totals {
    pub archives: u64,
    pub tankoubons: u64,
    pub categories: u64,
    pub storage_bytes: u64,
    pub average_bytes: u64,
    pub largest_bytes: u64,
    pub pages: u64,
    pub average_pages: u64,
    pub pages_read: u64,
    pub unread: u64,
    pub reading: u64,
    pub finished: u64,
    pub new: u64,
    pub rated: u64,
    pub bookmarks: u64,
}

#[derive(Debug, Serialize)]
pub struct SizeBucketStat {
    /// Stable identifier the frontend localizes — `lt_1mb`, `gte_500mb`, ...
    pub key: &'static str,
    pub min_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    pub archives: u64,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct FormatStat {
    /// Container extension as stored on disk (`zip`, `rar`, `pdf`, ...); empty for a file with no
    /// extension at all.
    pub extension: String,
    pub mime: &'static str,
    pub archives: u64,
    pub bytes: u64,
    pub pages: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Ingest {
    /// `archive.ingest` activity outcomes, keyed by [`INGEST_SOURCE_ROWS`].
    pub sources: Vec<SourceCount>,
    /// How many activity entries the source breakdown actually looked at.
    pub sources_sampled: u64,
    /// True when the activity log held more `archive.ingest` entries than
    /// [`INGEST_SOURCE_SAMPLE`], i.e. the breakdown covers the newest sample only.
    pub sources_sampled_only: bool,
    pub recent: Vec<ArchiveCard>,
}

#[derive(Debug, Serialize)]
pub struct SourceCount {
    /// One of [`INGEST_SOURCE_ROWS`], or `upload` for an upload whose actor is neither a session
    /// nor a token (kept distinct rather than guessed at), or `unknown` for a source this build
    /// does not know.
    pub source: String,
    pub archives: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Reading {
    pub progress: Vec<ProgressBucketStat>,
    pub ratings: Vec<RatingStat>,
    pub recent: Vec<RecentRead>,
    pub most_reread: Vec<RereadArchive>,
}

#[derive(Debug, Serialize)]
pub struct ProgressBucketStat {
    /// `unread` | `started` | `quarter` | `half` | `almost` | `finished`.
    pub key: &'static str,
    pub archives: u64,
}

#[derive(Debug, Serialize)]
pub struct RatingStat {
    /// `1`..=`5`; `0` means "no `rating:` tag".
    pub stars: u8,
    pub archives: u64,
}

#[derive(Debug, Serialize)]
pub struct RecentRead {
    pub id: String,
    pub title: String,
    pub last_read: u64,
    pub page: u32,
    pub pages: u32,
    pub extension: String,
}

#[derive(Debug, Serialize)]
pub struct RereadArchive {
    pub id: String,
    pub title: String,
    pub reads: u64,
    pub page: u32,
    pub pages: u32,
    pub extension: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Preferences {
    /// How many archives carry any reading signal at all — the denominator the rest of this
    /// section is relative to.
    pub read_archives: u64,
    /// Every content tag namespace seen across the read archives, weighted by
    /// [`preference_weights`].
    pub namespaces: Vec<NamespaceStat>,
    /// The promoted axes ([`HIGHLIGHTED_NAMESPACES`]) — always shown, however low they rank.
    pub highlights: Vec<NamespaceTags>,
    /// Top tags within each of the highest-weighted of the *remaining* namespaces.
    pub by_namespace: Vec<NamespaceTags>,
    /// How the read archives split across the reader's own 1–5 ratings (`stars: 0` = unrated),
    /// counted rather than weighted: the rating is already a factor in every weight above, so
    /// re-weighting by it here would count the same signal twice.
    pub ratings: Vec<RatingTierStat>,
    pub categories: Vec<CategoryStat>,
    /// True when [`MAX_DYNAMIC_CATEGORIES`] stopped some categories from being resolved.
    pub categories_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct RatingTierStat {
    /// `1`..=`5`; `0` means "no `rating:` tag".
    pub stars: u8,
    /// Read archives carrying this rating.
    pub archives: u64,
}

#[derive(Debug, Serialize)]
pub struct NamespaceStat {
    pub namespace: String,
    pub weight: f64,
    /// How many read archives carry at least one tag in this namespace.
    pub archives: u64,
}

#[derive(Debug, Serialize)]
pub struct NamespaceTags {
    pub namespace: String,
    pub weight: f64,
    pub tags: Vec<TagStat>,
}

#[derive(Debug, Serialize)]
pub struct TagStat {
    pub text: String,
    pub weight: f64,
    pub archives: u64,
}

#[derive(Debug, Serialize)]
pub struct CategoryStat {
    pub id: String,
    pub name: String,
    pub total: u64,
    pub read: u64,
    /// Summed [`preference_weights`] of the read members — how much of what this category holds
    /// has actually been read, weighted by how much it was liked.
    pub weight: f64,
}

#[derive(Debug, Serialize)]
pub struct ArchiveCard {
    pub id: String,
    pub title: String,
    pub timestamp: u64,
    pub bytes: u64,
    pub pages: u32,
    pub extension: String,
}

// ─────────────────────────── handler ───────────────────────────

/// A short-lived snapshot of the whole-library overview.
///
/// Building it lists every archive, every category and every grouping, then derives several
/// aggregate maps from them. The page is opened occasionally and a chart up to 15 seconds old is
/// not misleading, so repeated opens (and the frontend's own refresh) no longer each pay for a
/// full library pass.
///
/// Guests never read or write it: their view is scope-filtered per request, and serving them a
/// cached unfiltered snapshot would leak library-wide counts.
///
/// Invalidation is exact for anything that goes through the API: every mutating request bumps
/// [`note_library_mutation`], and a cached snapshot is only served while that counter still
/// matches. The age cap below is the fallback for mutations that never pass through a request
/// (the ingest watcher, a background job), where there is nothing to bump.
const OVERVIEW_CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

type OverviewCache = std::sync::Mutex<Option<(u64, std::time::Instant, axum::body::Bytes)>>;

fn overview_cache() -> &'static OverviewCache {
    static CACHE: std::sync::OnceLock<OverviewCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Bumped by every mutating API request (see `procedure::require_api_key`).
static LIBRARY_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Records that something may have changed the library, dropping the stats snapshot.
pub(crate) fn note_library_mutation() {
    LIBRARY_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn library_generation() -> u64 {
    LIBRARY_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Serialized-JSON response, matching what `axum::Json` would produce for the same value.
fn json_body(body: axum::body::Bytes) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

async fn overview(State(state): State<AppState>, auth: Option<Extension<AuthContext>>) -> Response {
    let guest = matches!(
        auth.as_deref().map(|a| &a.method),
        Some(AuthMethod::GuestVisitor)
    );
    if !guest {
        if let Some((generation, at, body)) = overview_cache()
            .lock()
            .expect("overview cache poisoned")
            .as_ref()
        {
            if *generation == library_generation() && at.elapsed() < OVERVIEW_CACHE_MAX_AGE {
                return json_body(body.clone());
            }
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_overview",
                e.to_string(),
            )
        }
    };
    // 007-guest-restricted-access: same scope filter `database::stats` applies to the tag cloud —
    // without it a guest would see aggregate counts (and every tag/preference breakdown derived
    // from them) over archives its own searches correctly exclude. `guest_visitor` is not in
    // `route_policy.csv`'s whitelist today, so this is defense in depth against a frontend route
    // guard being bypassed, exactly like that endpoint's own reasoning.
    if guest {
        match crate::search::guest_visible_archive_ids(&state).await {
            Ok(allowed) => archives.retain(|a| allowed.contains(&a.id)),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stats_overview",
                    e.to_string(),
                )
            }
        }
    }

    let excluded = excluded_namespaces(&state).await;
    let read_counts = read_freq(&state).await.unwrap_or_default();
    let bookmark_counts = bookmark_counts(&state).await.unwrap_or_default();
    let pages_read = total_pages_read(&state).await.unwrap_or(0);

    let weights = preference_weights(&archives, now, &read_counts, &bookmark_counts);
    let read_archives = weights.values().filter(|w| **w > 0.0).count() as u64;

    let tankoubons = state
        .repos
        .groupings
        .list_all()
        .await
        .map(|t| t.len() as u64)
        .unwrap_or(0);
    let categories = state.repos.categories.list_all().await.unwrap_or_default();

    let (sources, sources_sampled, sources_sampled_only) = ingest_sources(&state).await;
    let (category_stats, categories_truncated) =
        category_preferences(&state, &categories, &archives, &weights).await;

    // Nothing has ever been read *through this server*: either a brand-new library, or reader
    // progress kept in the browser (`localprogress`, which never calls `update_progress` at all —
    // that handler is the single write site for every reading counter here).
    let note = (pages_read == 0 && read_archives == 0).then_some("no_server_side_reading");

    let overview = Overview {
        generated_at: now,
        totals: totals(
            &archives,
            &categories,
            tankoubons,
            pages_read,
            &bookmark_counts,
        ),
        size_buckets: size_buckets(&archives),
        formats: format_breakdown(&archives),
        ingest: Ingest {
            sources,
            sources_sampled,
            sources_sampled_only,
            recent: recent_ingests(&archives),
        },
        reading: Reading {
            progress: progress_buckets(&archives),
            ratings: rating_distribution(&archives),
            recent: recent_reads(&archives),
            most_reread: most_reread(&archives, &read_counts),
        },
        preferences: Preferences {
            read_archives,
            namespaces: namespace_weights(&archives, &weights, &excluded)
                .into_iter()
                .take(MAX_NAMESPACES)
                .collect(),
            highlights: tags_for_namespaces(
                &archives,
                &weights,
                &excluded,
                &HIGHLIGHTED_NAMESPACES
                    .iter()
                    .map(|ns| ns.to_string())
                    .collect::<Vec<_>>(),
            ),
            by_namespace: {
                // The promoted axes are already listed on their own above.
                let rest: Vec<String> = namespace_weights(&archives, &weights, &excluded)
                    .into_iter()
                    .take(MAX_NAMESPACES)
                    .map(|ns| ns.namespace)
                    .filter(|ns| !HIGHLIGHTED_NAMESPACES.contains(&ns.as_str()))
                    .collect();
                tags_for_namespaces(&archives, &weights, &excluded, &rest)
            },
            ratings: rating_tiers(&archives, &weights),
            categories: category_stats,
            categories_truncated,
        },
        note,
    };

    // Serialized once and shared: `Json` would serialize a second time, and the cached copy is
    // exactly the bytes a caller gets.
    let body = match serde_json::to_vec(&overview) {
        Ok(body) => axum::body::Bytes::from(body),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_overview",
                e.to_string(),
            )
        }
    };
    if !guest {
        *overview_cache().lock().expect("overview cache poisoned") = Some((
            library_generation(),
            std::time::Instant::now(),
            body.clone(),
        ));
    }
    json_body(body)
}

// ─────────────────────────── timeline endpoint ───────────────────────────

#[derive(Debug, Deserialize)]
pub struct TimelineParams {
    /// `ingest` (archives added, from their own `date_added` tags) or `reading` (pages read and
    /// archives last read, from the archive records plus the additive per-day counter).
    metric: String,
    /// Unix seconds; clamped to the library's own data and to [`TIMELINE_MAX_DAYS`]. Defaults to
    /// the whole available range, which is what the page asks for on first paint so that every
    /// later zoom/pan/granularity change is pure client-side work.
    from: Option<u64>,
    to: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct TimelineResponse {
    pub metric: &'static str,
    /// First and last day actually returned, `YYYY-MM-DD`.
    pub from: String,
    pub to: String,
    /// Where this metric's data really starts — earlier than `from` when the request was clamped
    /// to [`TIMELINE_MAX_DAYS`], which is how the page knows to say the view is truncated.
    pub available_from: String,
    pub available_to: String,
    /// Reading metric only: the day the per-day counter started recording.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages_since: Option<String>,
    /// One entry per day, oldest first, dense. Only the fields the metric has are present.
    pub points: Vec<TimelinePoint>,
}

#[derive(Debug, Serialize)]
pub struct TimelinePoint {
    /// `YYYY-MM-DD` (UTC).
    pub date: String,
    /// `ingest` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archives: Option<u64>,
    /// `reading` only: page turns recorded that day (zero before the counter existed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u64>,
    /// `reading` only: archives whose most recent read fell on that day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_read: Option<u64>,
}

/// `GET /stats/timeline` — one metric's daily series over a requested window.
///
/// Separate from `/stats/overview` because the page drives this one interactively (range presets,
/// ctrl+wheel zoom, drag to pan) while that one is a single expensive snapshot: re-deriving the
/// preference analysis — one search per dynamic category — on every zoom tick would be absurd. The
/// series is daily regardless of the granularity the reader is looking at, so re-bucketing needs no
/// request at all.
async fn timeline(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Query(params): Query<TimelineParams>,
) -> Response {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_timeline",
                e.to_string(),
            )
        }
    };
    // Same guest scoping as the overview: a guest must not learn about archives its own searches
    // exclude, and these series aggregate every archive's `date_added`/`lastreadtime`.
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(AuthMethod::GuestVisitor)
    ) {
        match crate::search::guest_visible_archive_ids(&state).await {
            Ok(allowed) => archives.retain(|a| allowed.contains(&a.id)),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stats_timeline",
                    e.to_string(),
                )
            }
        }
    }

    let (metric, available_from) = match params.metric.as_str() {
        "ingest" => (
            "ingest",
            archives.iter().filter_map(|a| a.date_added()).min(),
        ),
        "reading" => (
            "reading",
            archives
                .iter()
                .filter(|a| a.lastreadtime > 0)
                .map(|a| a.lastreadtime)
                .min(),
        ),
        other => {
            return error(
                StatusCode::BAD_REQUEST,
                "stats_timeline",
                format!("unknown metric \"{other}\"; expected \"ingest\" or \"reading\""),
            )
        }
    };
    // A library with nothing to chart yet still answers: one day, all zeros.
    let available_from = available_from.unwrap_or(now);
    let (from, to) = clamp_window(params.from, params.to, available_from, now);
    let days = dense_days(from, to);

    let points: Vec<TimelinePoint> = match metric {
        "ingest" => {
            let mut tally: HashMap<String, u64> = HashMap::new();
            for archive in &archives {
                if let Some(day) = archive.date_added().and_then(day_of) {
                    *tally.entry(day).or_insert(0) += 1;
                }
            }
            days.into_iter()
                .map(|date| TimelinePoint {
                    archives: Some(tally.get(&date).copied().unwrap_or(0)),
                    date,
                    pages: None,
                    last_read: None,
                })
                .collect()
        }
        _ => {
            let mut last_read: HashMap<String, u64> = HashMap::new();
            for archive in &archives {
                if archive.lastreadtime == 0 {
                    continue;
                }
                if let Some(day) = day_of(archive.lastreadtime) {
                    *last_read.entry(day).or_insert(0) += 1;
                }
            }
            let pages = daily_page_counts(&state, &days).await;
            days.into_iter()
                .map(|date| TimelinePoint {
                    archives: None,
                    pages: Some(pages.get(&date).copied().unwrap_or(0)),
                    last_read: Some(last_read.get(&date).copied().unwrap_or(0)),
                    date,
                })
                .collect()
        }
    };

    // The counter's own start date, or — until that key has ever been written (this deploy's own
    // first read, or data predating it) — the earliest day with a count inside the window, which is
    // exact precisely when the window reaches back that far.
    let pages_since = if metric == "reading" {
        match read_daily_since(&state).await {
            Some(day) => Some(day),
            None => points
                .iter()
                .find(|point| point.pages.unwrap_or(0) > 0)
                .map(|point| point.date.clone()),
        }
    } else {
        None
    };

    axum::Json(TimelineResponse {
        metric,
        from: today(from),
        to: today(to),
        available_from: today(available_from),
        available_to: today(now),
        pages_since,
        points,
    })
    .into_response()
}

/// `"YYYY-MM-DD" -> page turns` for the days the per-day counter could still hold — one pipeline
/// round trip, and never more than [`READ_COUNTER_HORIZON_DAYS`] `GET`s since older keys have
/// expired by construction.
async fn daily_page_counts(state: &AppState, days: &[String]) -> HashMap<String, u64> {
    let Some(mut conn) = state.redis.config.get().await.ok() else {
        return HashMap::new();
    };
    let askable: Vec<&String> = days
        .iter()
        .rev()
        .take(READ_COUNTER_HORIZON_DAYS as usize)
        .collect();
    if askable.is_empty() {
        return HashMap::new();
    }
    let mut pipe = deadpool_redis::redis::pipe();
    for day in &askable {
        pipe.get(read_daily_key(day));
    }
    let Ok(values) = pipe.query_async::<Vec<Option<u64>>>(&mut conn).await else {
        return HashMap::new();
    };
    askable
        .into_iter()
        .zip(values)
        .filter_map(|(day, pages)| pages.map(|p| (day.clone(), p)))
        .collect()
}

// ─────────────────────────── drill-down endpoint ───────────────────────────

/// The most archives one drill-down returns. The page renders them in a scrollable box, and the
/// response also carries the full `total`, so a capped list never misrepresents how many matched.
const BUCKET_ARCHIVE_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct BucketParams {
    /// `ingest` / `read` (both need `from`+`to`), or `size` / `rating` / `progress` (each needs
    /// `key`). Time buckets take an explicit day range rather than a unit+key pair on purpose: the
    /// client already knows the span it drew (it did the bucketing), and resolving a bucket key back
    /// into a span would mean re-implementing week/month arithmetic in a second language.
    metric: String,
    from: Option<String>,
    to: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BucketArchives {
    pub metric: &'static str,
    /// Every archive in the bucket, not just the returned page of them.
    pub total: u64,
    pub truncated: bool,
    pub archives: Vec<ArchiveCard>,
}

/// `GET /stats/bucket` — the archives behind one bar of one chart.
///
/// Separate from `/stats/overview` because it is per-interaction (one click), and separate from
/// the search API because two of these buckets are not search-expressible at all: "last read on day
/// X" has no search operator, and the progress ladder's unread rung means "never *opened*", which
/// the `read:`/`is:unread` filters (keyed on the `progress` field alone) deliberately don't say.
/// Deriving it here keeps the list consistent with the bar that was clicked, by construction.
async fn bucket(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Query(params): Query<BucketParams>,
) -> Response {
    let mut archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_bucket",
                e.to_string(),
            )
        }
    };
    if matches!(
        auth.as_deref().map(|a| &a.method),
        Some(AuthMethod::GuestVisitor)
    ) {
        match crate::search::guest_visible_archive_ids(&state).await {
            Ok(allowed) => archives.retain(|a| allowed.contains(&a.id)),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stats_bucket",
                    e.to_string(),
                )
            }
        }
    }

    let (metric, selected): (&'static str, Vec<&Archive>) = match params.metric.as_str() {
        "ingest" | "read" => {
            let (Some(from), Some(to)) = (params.from.as_deref(), params.to.as_deref()) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    "metric ingest/read needs both from and to (YYYY-MM-DD)".to_string(),
                );
            };
            let reading = params.metric == "read";
            let mut matches: Vec<&Archive> = archives
                .iter()
                .filter(|a| {
                    let stamp = if reading {
                        a.lastreadtime
                    } else {
                        match a.date_added() {
                            Some(ts) => ts,
                            None => return false,
                        }
                    };
                    if reading && stamp == 0 {
                        return false;
                    }
                    day_of(stamp).is_some_and(|day| day.as_str() >= from && day.as_str() <= to)
                })
                .collect();
            matches.sort_by_key(|a| {
                let stamp = if reading {
                    a.lastreadtime
                } else {
                    a.date_added().unwrap_or(0)
                };
                std::cmp::Reverse(stamp)
            });
            (if reading { "read" } else { "ingest" }, matches)
        }
        "size" => {
            let Some(key) = params.key.as_deref() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    "metric size needs a key".to_string(),
                );
            };
            let Some((_, min, max)) = SIZE_BUCKETS.iter().find(|(k, _, _)| *k == key) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    format!("unknown size bucket \"{key}\""),
                );
            };
            let mut matches: Vec<&Archive> = archives
                .iter()
                .filter(|a| a.arcsize >= *min && max.map(|m| a.arcsize < m).unwrap_or(true))
                .collect();
            matches.sort_by_key(|a| std::cmp::Reverse(a.arcsize));
            ("size", matches)
        }
        "rating" => {
            let Some(stars) = params.key.as_deref().and_then(|k| k.parse::<u8>().ok()) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    "metric rating needs a key of 0-5".to_string(),
                );
            };
            if stars > 5 {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    "rating key must be 0-5".to_string(),
                );
            }
            let mut matches: Vec<&Archive> = archives
                .iter()
                .filter(|a| rating_of(a).unwrap_or(0) == stars)
                .collect();
            matches.sort_by(|a, b| {
                rating_of(b)
                    .unwrap_or(0)
                    .cmp(&rating_of(a).unwrap_or(0))
                    .then_with(|| display_title(a).cmp(&display_title(b)))
            });
            ("rating", matches)
        }
        "progress" => {
            let Some(key) = params.key.as_deref() else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    "metric progress needs a key".to_string(),
                );
            };
            let Some(index) = PROGRESS_BUCKET_KEYS.iter().position(|k| *k == key) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "stats_bucket",
                    format!("unknown progress bucket \"{key}\""),
                );
            };
            let mut matches: Vec<&Archive> = archives
                .iter()
                .filter(|a| progress_bucket_index(a) == index)
                .collect();
            matches.sort_by(|a, b| {
                completion_ratio(b)
                    .partial_cmp(&completion_ratio(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| display_title(a).cmp(&display_title(b)))
            });
            ("progress", matches)
        }
        other => {
            return error(
                StatusCode::BAD_REQUEST,
                "stats_bucket",
                format!("unknown metric \"{other}\""),
            )
        }
    };

    let total = selected.len() as u64;
    let truncated = selected.len() > BUCKET_ARCHIVE_LIMIT;
    let archives = selected
        .into_iter()
        .take(BUCKET_ARCHIVE_LIMIT)
        .map(|a| ArchiveCard {
            id: a.id.to_string(),
            title: display_title(a),
            timestamp: a.date_added().unwrap_or(a.lastreadtime),
            bytes: a.arcsize,
            pages: a.pagecount,
            extension: container_extension(a),
        })
        .collect();

    axum::Json(BucketArchives {
        metric,
        total,
        truncated,
        archives,
    })
    .into_response()
}

// ─────────────────────────── Redis reads ───────────────────────────

/// Per-archive progress-update counts — one `HGETALL`, and the only Redis read the overview still
/// needs for reading (the per-day series lives in `GET /stats/timeline`). Best-effort: a Redis
/// hiccup degrades the preference weights to recency/completion only rather than failing the page.
async fn read_freq(state: &AppState) -> Option<HashMap<String, u64>> {
    let mut conn = state.redis.config.get().await.ok()?;
    conn.hgetall(READ_FREQ_STAT_KEY).await.ok()
}

/// The day the per-day counter first recorded anything (`SETNX`-ed by `archives.rs`), if it ever
/// has.
async fn read_daily_since(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
    conn.get(READ_DAILY_SINCE_KEY).await.ok()
}

async fn total_pages_read(state: &AppState) -> Option<u64> {
    let mut conn = state.redis.config.get().await.ok()?;
    conn.get(TOTAL_PAGES_STAT_KEY).await.ok()
}

/// Per-archive bookmark counts, from the repository's own public read rather than the index
/// key's internals.
async fn bookmark_counts(state: &AppState) -> Option<HashMap<String, u64>> {
    let bookmarks = state.bookmarks.list_all().await.ok()?;
    let mut counts: HashMap<String, u64> = HashMap::new();
    for bookmark in bookmarks {
        *counts.entry(bookmark.archive_id).or_insert(0) += 1;
    }
    Some(counts)
}

/// Namespaces never treated as content signals: the built-in record-keeping ones plus whatever
/// the instance excludes from its own tag cloud (`excludednamespaces`), so the preference
/// analysis and the tag cloud agree on what counts as a real tag.
async fn excluded_namespaces(state: &AppState) -> HashSet<String> {
    let mut out: HashSet<String> = NON_CONTENT_NAMESPACES
        .iter()
        .map(|s| s.to_string())
        .collect();
    let Some(mut conn) = state.redis.config.get().await.ok() else {
        return out;
    };
    let raw: Option<String> = conn
        .hget(CONFIG_KEY, "excludednamespaces")
        .await
        .ok()
        .flatten();
    for ns in raw.unwrap_or_default().split(',') {
        let ns = ns.trim().to_ascii_lowercase();
        if !ns.is_empty() {
            out.insert(ns);
        }
    }
    out
}

/// `(sources, sampled, sampled_only)`. Reads the newest [`INGEST_SOURCE_SAMPLE`] `archive.ingest`
/// entries and tallies `after.source` — the unified ingestion record every path funnels through
/// (see `activity::record_archive_ingest`). A read failure yields an empty breakdown rather than a
/// failed page.
async fn ingest_sources(state: &AppState) -> (Vec<SourceCount>, u64, bool) {
    let filter = ActivityFilter {
        action_types: vec![action_types::ARCHIVE_INGEST.to_string()],
        ..ActivityFilter::default()
    };
    let Ok(page) = state
        .activity
        .list_page(&filter, None, INGEST_SOURCE_SAMPLE)
        .await
    else {
        return (Vec::new(), 0, false);
    };
    let sampled_only = page.next_cursor.is_some();
    let mut tally: HashMap<String, u64> = HashMap::new();
    for entry in &page.entries {
        let raw = entry
            .after
            .as_ref()
            .and_then(|a| a.get("source"))
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        *tally
            .entry(ingest_source_key(raw, entry.actor.kind).to_string())
            .or_insert(0) += 1;
    }
    (source_rows(tally), page.entries.len() as u64, sampled_only)
}

/// Which row of [`INGEST_SOURCE_ROWS`] one `archive.ingest` record belongs to. Split out from
/// [`ingest_sources`] so the mapping is unit-testable without a Redis-backed activity log.
fn ingest_source_key(source: &str, actor: ActorKind) -> &'static str {
    match (source, actor) {
        ("upload", ActorKind::Session) => "upload_browser",
        ("upload", ActorKind::Token) => "upload_token",
        // An upload by some other actor (a `System`/`Anonymous` record, or one written before the
        // credential was ever recorded) stays its own row rather than being folded into either
        // credential bucket it might not belong to.
        ("upload", _) => "upload",
        ("download", _) => "download",
        ("scanner", _) => "scanner",
        _ => "unknown",
    }
}

/// Zero-fills the fixed rows (in their display order) and appends anything else that was actually
/// seen, so an unlisted key can never be silently dropped from the total.
fn source_rows(tally: HashMap<String, u64>) -> Vec<SourceCount> {
    let mut rows: Vec<SourceCount> = INGEST_SOURCE_ROWS
        .iter()
        .map(|key| SourceCount {
            source: key.to_string(),
            archives: tally.get(*key).copied().unwrap_or(0),
        })
        .collect();
    let mut extra: Vec<SourceCount> = tally
        .into_iter()
        .filter(|(key, _)| !INGEST_SOURCE_ROWS.contains(&key.as_str()))
        .map(|(source, archives)| SourceCount { source, archives })
        .collect();
    extra.sort_by(|a, b| b.archives.cmp(&a.archives).then(a.source.cmp(&b.source)));
    rows.append(&mut extra);
    rows
}

// ─────────────────────────── aggregation ───────────────────────────

fn totals(
    archives: &[Archive],
    categories: &[Category],
    tankoubons: u64,
    pages_read: u64,
    bookmarks: &HashMap<String, u64>,
) -> Totals {
    let count = archives.len() as u64;
    let storage_bytes: u64 = archives.iter().map(|a| a.arcsize).sum();
    let pages: u64 = archives.iter().map(|a| a.pagecount as u64).sum();
    let finished = archives.iter().filter(|a| is_finished(a)).count() as u64;
    Totals {
        archives: count,
        tankoubons,
        categories: categories.len() as u64,
        storage_bytes,
        average_bytes: storage_bytes.checked_div(count).unwrap_or(0),
        largest_bytes: archives.iter().map(|a| a.arcsize).max().unwrap_or(0),
        pages,
        average_pages: pages.checked_div(count).unwrap_or(0),
        pages_read,
        unread: archives.iter().filter(|a| !is_read(a)).count() as u64,
        reading: archives
            .iter()
            .filter(|a| is_read(a) && !is_finished(a))
            .count() as u64,
        finished,
        new: archives.iter().filter(|a| a.isnew).count() as u64,
        rated: archives.iter().filter(|a| rating_of(a).is_some()).count() as u64,
        bookmarks: bookmarks.values().sum(),
    }
}

fn size_buckets(archives: &[Archive]) -> Vec<SizeBucketStat> {
    SIZE_BUCKETS
        .iter()
        .map(|(key, min, max)| {
            let mut stat = SizeBucketStat {
                key,
                min_bytes: *min,
                max_bytes: *max,
                archives: 0,
                bytes: 0,
            };
            for a in archives {
                let fits = a.arcsize >= *min && (*max).map(|m| a.arcsize < m).unwrap_or(true);
                if fits {
                    stat.archives += 1;
                    stat.bytes += a.arcsize;
                }
            }
            stat
        })
        .collect()
}

/// Container format of each archive, keyed by on-disk extension, largest group first.
fn format_breakdown(archives: &[Archive]) -> Vec<FormatStat> {
    let mut by_ext: HashMap<String, (u64, u64, u64)> = HashMap::new();
    for a in archives {
        let entry = by_ext.entry(container_extension(a)).or_insert((0, 0, 0));
        entry.0 += 1;
        entry.1 += a.arcsize;
        entry.2 += a.pagecount as u64;
    }
    let mut out: Vec<FormatStat> = by_ext
        .into_iter()
        .map(|(extension, (archives, bytes, pages))| FormatStat {
            mime: mime_for_extension(&extension),
            extension,
            archives,
            bytes,
            pages,
        })
        .collect();
    // Ties broken by extension so the ordering is stable across requests (`HashMap` iteration
    // order is not).
    out.sort_by(|a, b| {
        b.archives
            .cmp(&a.archives)
            .then_with(|| a.extension.cmp(&b.extension))
    });
    out
}

/// The container/file type an extension denotes. Covers every extension the scanner either
/// catalogues (`watcher::WATCHED_EXTENSIONS`) or libarchive can open
/// (`archive_format::ARCHIVE_EXTENSIONS`); anything else is honestly reported as
/// `application/octet-stream` rather than guessed at.
fn mime_for_extension(extension: &str) -> &'static str {
    match extension {
        "zip" | "cbz" => "application/zip",
        "rar" | "cbr" => "application/vnd.rar",
        "7z" | "cb7" => "application/x-7z-compressed",
        "tar" | "cbt" => "application/x-tar",
        "gz" => "application/gzip",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "lzma" => "application/x-lzma",
        "lzh" | "lha" => "application/x-lzh-compressed",
        "zst" => "application/zstd",
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "mobi" | "azw" | "prc" => "application/x-mobipocket-ebook",
        "azw3" => "application/vnd.amazon.ebook",
        "djvu" | "djv" => "image/vnd.djvu",
        "fb2" => "application/x-fictionbook+xml",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// The most recently ingested archives, newest first (from each archive's own `date_added`).
fn recent_ingests(archives: &[Archive]) -> Vec<ArchiveCard> {
    let mut with_date: Vec<&Archive> = archives
        .iter()
        .filter(|a| a.date_added().is_some())
        .collect();
    with_date.sort_by_key(|a| std::cmp::Reverse(a.date_added().unwrap_or(0)));
    with_date
        .into_iter()
        .take(RECENT_LIMIT)
        .map(|a| ArchiveCard {
            id: a.id.to_string(),
            title: display_title(a),
            timestamp: a.date_added().unwrap_or(0),
            bytes: a.arcsize,
            pages: a.pagecount,
            extension: container_extension(a),
        })
        .collect()
}

/// `from`/`to` as UTC calendar days, oldest first, dense and zero-filled.
///
/// This is the one place the page's time series come from, and it is deliberately *daily* for every
/// metric: the reader picks the range and the granularity on the page, and the page can only
/// re-bucket instantly (week/month, or zoom and pan) if it already holds the finest series. The
/// cost is a payload of at most [`TIMELINE_MAX_DAYS`] integers.
fn dense_days(from: u64, to: u64) -> Vec<String> {
    day_range(&today(to), ((to.saturating_sub(from)) / 86_400) as u32 + 1)
        .into_iter()
        .filter(|day| day.as_str() >= today(from).as_str())
        .collect()
}

/// Clamps a requested window to `[available_from, now]` and to [`TIMELINE_MAX_DAYS`], newest-first
/// biased: a request wider than the cap loses its *oldest* days rather than its newest.
fn clamp_window(
    requested_from: Option<u64>,
    requested_to: Option<u64>,
    available_from: u64,
    now: u64,
) -> (u64, u64) {
    let to = requested_to.unwrap_or(now).min(now).max(available_from);
    let floor = to.saturating_sub(TIMELINE_MAX_DAYS * 86_400);
    let from = requested_from
        .unwrap_or(available_from)
        .max(available_from)
        .max(floor)
        .min(to);
    (start_of_day(from), start_of_day(to))
}

fn start_of_day(ts: u64) -> u64 {
    ts - ts % 86_400
}

fn rating_of(archive: &Archive) -> Option<u8> {
    archive.tags.split(',').find_map(|t| {
        let value = t.trim().strip_prefix("rating:")?;
        value
            .trim()
            .parse::<u8>()
            .ok()
            .filter(|r| (1..=5).contains(r))
    })
}

fn is_read(archive: &Archive) -> bool {
    archive.lastreadpage > 0 || archive.lastreadtime > 0
}

fn is_finished(archive: &Archive) -> bool {
    archive.pagecount > 0 && archive.lastreadpage >= archive.pagecount
}

/// How far through an archive the reader got, as a 0..=1 ratio. An archive that was opened but
/// whose page count is unknown (a failed catalogue) counts as half-read rather than unread —
/// opening it is a real signal, and `pagecount == 0` must not divide by zero.
fn completion_ratio(archive: &Archive) -> f64 {
    if archive.pagecount == 0 {
        return if is_read(archive) { 0.5 } else { 0.0 };
    }
    (archive.lastreadpage as f64 / archive.pagecount as f64).clamp(0.0, 1.0)
}

/// The preference analysis' entire model, in one function: how much one archive contributes to
/// the tags and categories it belongs to. Every factor is deliberately legible and bounded, so
/// the page can explain the ranking in a sentence and an operator can disagree with it:
///
/// - **completion** — finishing something says more than opening it (0.35 opened → 1.0 finished)
/// - **recency** — halving every [`PREF_HALF_LIFE_DAYS`]; an archive with no timestamp at all
///   (legacy data written before `lastreadtime` existed) counts as a year old rather than "now"
/// - **rating** — the reader's own explicit 1..5, centered on 3 (0.5× … 1.5×)
/// - **bookmarks** — a page worth bookmarking is a page worth remembering (up to 1.75×)
/// - **re-read count** — returning to a work is the strongest implicit signal (up to ~1.5×)
fn preference_weights(
    archives: &[Archive],
    now: u64,
    read_freq: &HashMap<String, u64>,
    bookmarks: &HashMap<String, u64>,
) -> HashMap<String, f64> {
    let mut weights = HashMap::with_capacity(archives.len());
    for a in archives {
        let id = a.id.to_string();
        if !is_read(a) {
            weights.insert(id, 0.0);
            continue;
        }
        let recency = {
            let age_days = if a.lastreadtime == 0 {
                365.0
            } else {
                (now.saturating_sub(a.lastreadtime) as f64 / 86_400.0).max(0.0)
            };
            0.5f64.powf(age_days / PREF_HALF_LIFE_DAYS)
        };
        let completion = 0.35 + 0.65 * completion_ratio(a);
        let rating = match rating_of(a) {
            Some(r) => 1.0 + 0.25 * (r as f64 - 3.0),
            None => 1.0,
        };
        let bookmark = 1.0 + 0.15 * (bookmarks.get(&id).copied().unwrap_or(0).min(5) as f64);
        let reread = {
            let reads = read_freq.get(&id).copied().unwrap_or(0);
            1.0 + 0.15 * ((reads as f64) + 1.0).ln().min(3.0)
        };
        weights.insert(id, recency * completion * rating * bookmark * reread);
    }
    weights
}

/// `ns:value` tags of every archive with a non-zero weight, summed into per-namespace totals. A
/// namespace contributes its archive's weight **once per archive**, however many of its tags that
/// archive carries — otherwise a work with three `female:` tags would read as three times the
/// preference for that namespace as one with a single tag.
/// Every namespace present among the weighted archives, heaviest first — deliberately *not* capped
/// here, because the promoted roster below must be able to find an axis however low it ranks. The
/// chart's own top-[`MAX_NAMESPACES`] cut is applied by that caller; capping inside this function
/// made the roster's "always shown" promise false for any axis outside the top twelve.
fn namespace_weights(
    archives: &[Archive],
    weights: &HashMap<String, f64>,
    excluded: &HashSet<String>,
) -> Vec<NamespaceStat> {
    let mut totals: HashMap<&str, (f64, HashSet<String>)> = HashMap::new();
    for a in archives {
        let w = weights.get(a.id.as_str()).copied().unwrap_or(0.0);
        if w <= 0.0 {
            continue;
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for (ns, _) in content_tags(a, excluded) {
            if !seen.insert(ns) {
                continue;
            }
            let entry = totals.entry(ns).or_default();
            entry.0 += w;
            entry.1.insert(a.id.to_string());
        }
    }
    let mut out: Vec<NamespaceStat> = totals
        .into_iter()
        .map(|(namespace, (weight, archives))| NamespaceStat {
            namespace: namespace.to_string(),
            weight: round3(weight),
            archives: archives.len() as u64,
        })
        .collect();
    out.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.namespace.cmp(&b.namespace))
    });
    out
}

/// Top tags for each namespace in `roster`, in the roster's own order — so a caller can pin the
/// axes it wants to show first ([`HIGHLIGHTED_NAMESPACES`]) instead of only ever getting the
/// highest-weighted ones. Namespaces absent from the library are simply skipped.
fn tags_for_namespaces(
    archives: &[Archive],
    weights: &HashMap<String, f64>,
    excluded: &HashSet<String>,
    roster: &[String],
) -> Vec<NamespaceTags> {
    let all = namespace_weights(archives, weights, excluded);
    let namespaces: Vec<NamespaceStat> = roster
        .iter()
        .filter_map(|wanted| {
            all.iter()
                .find(|ns| ns.namespace == *wanted)
                .map(|ns| NamespaceStat {
                    namespace: ns.namespace.clone(),
                    weight: ns.weight,
                    archives: ns.archives,
                })
        })
        .collect();
    let mut out = Vec::with_capacity(namespaces.len());
    for ns in namespaces {
        let mut totals: HashMap<&str, (f64, u64)> = HashMap::new();
        for a in archives {
            let w = weights.get(a.id.as_str()).copied().unwrap_or(0.0);
            if w <= 0.0 {
                continue;
            }
            let mut seen: HashSet<&str> = HashSet::new();
            for (tag_ns, value) in content_tags(a, excluded) {
                let is_axis_value = NON_AXIS_TAG_VALUES
                    .iter()
                    .any(|(namespace, blocked)| *namespace == tag_ns && *blocked == value);
                if tag_ns != ns.namespace.as_str() || is_axis_value || !seen.insert(value) {
                    continue;
                }
                let entry = totals.entry(value).or_insert((0.0, 0));
                entry.0 += w;
                entry.1 += 1;
            }
        }
        let mut tags: Vec<TagStat> = totals
            .into_iter()
            .map(|(text, (weight, archives))| TagStat {
                text: text.to_string(),
                weight: round3(weight),
                archives,
            })
            .collect();
        tags.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.text.cmp(&b.text))
        });
        tags.truncate(TOP_TAGS_PER_NAMESPACE);
        out.push(NamespaceTags {
            namespace: ns.namespace,
            weight: ns.weight,
            tags,
        });
    }
    out
}

/// `(namespace, value)` pairs of an archive's tags, minus the record-keeping namespaces.
fn content_tags<'a>(archive: &'a Archive, excluded: &HashSet<String>) -> Vec<(&'a str, &'a str)> {
    archive
        .tags
        .split(',')
        .filter_map(|t| t.trim().split_once(':'))
        .map(|(ns, v)| (ns.trim(), v.trim()))
        .filter(|(ns, v)| !ns.is_empty() && !v.is_empty() && !excluded.contains(*ns))
        .collect()
}

/// Reading progress in 20-point bands, plus the two states that are not a percentage at all
/// (never opened, finished). Deliberately even rather than the coarser quarter/half ladder this
/// started as: real libraries concentrate at the bottom (this instance's own: 58% unread, 22%
/// just started, then single digits), so equal 25-point bands spent their resolution where almost
/// nothing was — a reader asking "how much do I abandon after a few pages" needs the low end
/// split, not the middle.
///
/// Each band is inclusive of its own upper bound (`0 < ratio <= 0.20`, and so on), matching the
/// `1-20%`-style labels the page renders.
/// Keys and order of the reading-progress ladder — the single source of truth shared by the
/// overview's histogram and the drill-down that lists one band's archives.
const PROGRESS_BUCKET_KEYS: [&str; 7] = [
    "unread", "1_20", "21_40", "41_60", "61_80", "81_99", "finished",
];

/// Which rung of [`PROGRESS_BUCKET_KEYS`] an archive sits on.
fn progress_bucket_index(archive: &Archive) -> usize {
    let ratio = completion_ratio(archive);
    if !is_read(archive) {
        0
    } else if ratio >= 1.0 {
        6
    } else if ratio > 0.8 {
        5
    } else if ratio > 0.6 {
        4
    } else if ratio > 0.4 {
        3
    } else if ratio > 0.2 {
        2
    } else {
        1
    }
}

fn progress_buckets(archives: &[Archive]) -> Vec<ProgressBucketStat> {
    let mut out = PROGRESS_BUCKET_KEYS
        .into_iter()
        .map(|key| ProgressBucketStat { key, archives: 0 })
        .collect::<Vec<_>>();
    for a in archives {
        out[progress_bucket_index(a)].archives += 1;
    }
    out
}

fn rating_distribution(archives: &[Archive]) -> Vec<RatingStat> {
    let mut out: Vec<RatingStat> = (0..=5u8)
        .map(|stars| RatingStat { stars, archives: 0 })
        .collect();
    for a in archives {
        out[rating_of(a).unwrap_or(0) as usize].archives += 1;
    }
    out
}

/// How the *read* archives split across the reader's ratings — the same ladder as
/// [`rating_distribution`] (which covers the whole library), restricted to what has actually been
/// read, so the preference section can show which tiers the reading is concentrated in.
fn rating_tiers(archives: &[Archive], weights: &HashMap<String, f64>) -> Vec<RatingTierStat> {
    let mut out: Vec<RatingTierStat> = (0..=5u8)
        .map(|stars| RatingTierStat { stars, archives: 0 })
        .collect();
    for a in archives {
        if weights.get(a.id.as_str()).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        out[rating_of(a).unwrap_or(0) as usize].archives += 1;
    }
    out
}

fn recent_reads(archives: &[Archive]) -> Vec<RecentRead> {
    let mut read: Vec<&Archive> = archives.iter().filter(|a| a.lastreadtime > 0).collect();
    read.sort_by_key(|a| std::cmp::Reverse(a.lastreadtime));
    read.into_iter()
        .take(RECENT_LIMIT)
        .map(|a| RecentRead {
            id: a.id.to_string(),
            title: display_title(a),
            last_read: a.lastreadtime,
            page: a.lastreadpage,
            pages: a.pagecount,
            extension: container_extension(a),
        })
        .collect()
}

fn most_reread(archives: &[Archive], read_freq: &HashMap<String, u64>) -> Vec<RereadArchive> {
    let mut rows: Vec<(&Archive, u64)> = archives
        .iter()
        .filter_map(|a| {
            let reads = read_freq.get(a.id.as_str()).copied().unwrap_or(0);
            (reads > 0).then_some((a, reads))
        })
        .collect();
    rows.sort_by_key(|(_, reads)| std::cmp::Reverse(*reads));
    rows.into_iter()
        .take(RECENT_LIMIT)
        .map(|(a, reads)| RereadArchive {
            id: a.id.to_string(),
            title: display_title(a),
            reads,
            page: a.lastreadpage,
            pages: a.pagecount,
            extension: container_extension(a),
        })
        .collect()
}

/// Categories by how much of what they hold has been read — the "which of my collections do I
/// actually read?" view. A static category's membership is its own stored list (free); a dynamic
/// one costs one search, capped at [`MAX_DYNAMIC_CATEGORIES`].
async fn category_preferences(
    state: &AppState,
    categories: &[Category],
    archives: &[Archive],
    weights: &HashMap<String, f64>,
) -> (Vec<CategoryStat>, bool) {
    let known: HashSet<&str> = archives.iter().map(|a| a.id.as_str()).collect();
    let mut dynamic_used = 0usize;
    let mut truncated = false;
    let mut out = Vec::with_capacity(categories.len());
    for category in categories {
        let members: Vec<String> = match &category.search {
            Some(predicate) if !predicate.is_empty() => {
                if dynamic_used >= MAX_DYNAMIC_CATEGORIES {
                    // Left out rather than reported as an empty category — an unresolvable
                    // membership is not the same claim as "nothing in it".
                    truncated = true;
                    continue;
                }
                dynamic_used += 1;
                let params = SearchParams {
                    filter: predicate.clone(),
                    groupby_tanks: false,
                    ..SearchParams::default()
                };
                search(
                    &state.redis.archive,
                    &state.redis.search,
                    &state.equivalence,
                    &state.bookmarks,
                    &params,
                )
                .await
                .map(|result| {
                    result
                        .ids
                        .into_iter()
                        .filter(|id| known.contains(id.as_str()))
                        .collect()
                })
                .unwrap_or_default()
            }
            _ => category
                .archives
                .iter()
                .map(|id| id.as_str().to_string())
                .filter(|id| known.contains(id.as_str()))
                .collect(),
        };
        let read = members
            .iter()
            .filter(|id| weights.get(id.as_str()).copied().unwrap_or(0.0) > 0.0)
            .count() as u64;
        let weight: f64 = members
            .iter()
            .map(|id| weights.get(id.as_str()).copied().unwrap_or(0.0))
            .sum();
        out.push(CategoryStat {
            id: category.catid.to_string(),
            name: category.name.clone(),
            total: members.len() as u64,
            read,
            weight: round3(weight),
        });
    }
    out.sort_by(|a, b| {
        let ratio = |c: &CategoryStat| {
            if c.total == 0 {
                0.0
            } else {
                c.read as f64 / c.total as f64
            }
        };
        ratio(b)
            .partial_cmp(&ratio(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.weight
                    .partial_cmp(&a.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    (out, truncated)
}

// ─────────────────────────── small helpers ───────────────────────────

/// The archive's container extension, for display and grouping. Deliberately not
/// `Archive::extension()`: that one mirrors legacy's own `build_json` (a bare `rsplit('.')`),
/// which for a path containing no dot at all returns the *whole path* — legible enough in a
/// `filename` field, but a format breakdown has to report that case as "no extension" rather than
/// inventing a "format" named after a directory.
fn container_extension(archive: &Archive) -> String {
    archive
        .file
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn display_title(archive: &Archive) -> String {
    if archive.title.trim().is_empty() {
        archive.name.clone()
    } else {
        archive.title.clone()
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn day_of(timestamp: u64) -> Option<String> {
    chrono::DateTime::from_timestamp(timestamp as i64, 0).map(|t| t.format("%Y-%m-%d").to_string())
}

fn today(now: u64) -> String {
    day_of(now).unwrap_or_default()
}

/// The `days` calendar days ending at `end` (inclusive), oldest first. Empty when `end` is not a
/// date (which only happens if the system clock is before the Unix epoch).
fn day_range(end: &str, days: u32) -> Vec<String> {
    let Ok(end) = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d") else {
        return Vec::new();
    };
    (0..days)
        .rev()
        .filter_map(|back| end.checked_sub_days(chrono::Days::new(back as u64)))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::ids::ArchiveId;

    fn archive(id: &str, tags: &str, arcsize: u64, pagecount: u32) -> Archive {
        Archive {
            id: ArchiveId(id.to_string()),
            name: id.to_string(),
            title: format!("Title {id}"),
            file: format!("/content/{id}.zip"),
            tags: tags.to_string(),
            summary: String::new(),
            arcsize,
            pagecount,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: Vec::new(),
            stamp_ids: Vec::new(),
            heal_failed_at: None,
            corrupted_pages: Vec::new(),
            has_patch: false,
        }
    }

    fn read_archive(id: &str, tags: &str, page: u32, pages: u32, lastreadtime: u64) -> Archive {
        Archive {
            lastreadpage: page,
            lastreadtime,
            ..archive(id, tags, 0, pages)
        }
    }

    fn excluded() -> HashSet<String> {
        NON_CONTENT_NAMESPACES
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn size_buckets_cover_every_archive_exactly_once() {
        let archives = vec![
            archive("a", "", 0, 0),
            archive("b", "", 1024, 0),
            archive("c", "", 1 << 20, 0),
            archive("d", "", (5 << 20) - 1, 0),
            archive("e", "", 500 << 20, 0),
        ];
        let buckets = size_buckets(&archives);
        assert_eq!(buckets.iter().map(|b| b.archives).sum::<u64>(), 5);
        assert_eq!(
            buckets.iter().map(|b| b.bytes).sum::<u64>(),
            archives.iter().map(|a| a.arcsize).sum::<u64>()
        );
        // 1 MiB is the first bucket's *upper* bound (exclusive), so it lands in `lt_5mb`.
        assert_eq!(buckets[0].archives, 2);
        assert_eq!(buckets[1].archives, 2);
        assert_eq!(buckets.last().unwrap().archives, 1);
    }

    #[test]
    fn mime_mapping_covers_every_scanner_extension() {
        for ext in [
            "zip", "cbz", "rar", "cbr", "7z", "cb7", "tar", "gz", "lzma", "xz", "zst", "epub",
            "pdf",
        ] {
            assert_ne!(
                mime_for_extension(ext),
                "application/octet-stream",
                "{ext} should map to a real type"
            );
        }
        assert_eq!(mime_for_extension("cbz"), mime_for_extension("zip"));
        assert_eq!(mime_for_extension(""), "application/octet-stream");
        assert_eq!(mime_for_extension("weird"), "application/octet-stream");
    }

    #[test]
    fn format_breakdown_groups_by_extension() {
        let archives = vec![
            archive("a", "", 10, 0),
            archive("b", "", 20, 0),
            Archive {
                file: "/content/c.pdf".into(),
                ..archive("c", "", 30, 0)
            },
        ];
        let formats = format_breakdown(&archives);
        assert_eq!(formats[0].extension, "zip");
        assert_eq!(formats[0].archives, 2);
        assert_eq!(formats[0].bytes, 30);
        assert_eq!(formats[1].extension, "pdf");
        assert_eq!(formats[1].mime, "application/pdf");
    }

    #[test]
    fn format_breakdown_reports_a_dotless_path_as_no_extension() {
        // `Archive::extension()` (legacy's own bare `rsplit('.')`) would return this entire path as
        // the "extension" — a row no operator could read as a format.
        let archives = vec![Archive {
            file: "/home/koyomi/lanrurugi/content/".into(),
            ..archive("a", "", 10, 0)
        }];
        let formats = format_breakdown(&archives);
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0].extension, "");
        assert_eq!(formats[0].mime, "application/octet-stream");
    }

    #[test]
    fn preference_weight_favours_finished_recent_highly_rated_works() {
        let now = 1_800_000_000;
        let day = 86_400;
        let good = read_archive("good", "artist:x,rating:5", 100, 100, now - day);
        let meh = read_archive("meh", "artist:x,rating:1", 1, 100, now - 300 * day);
        let unread = archive("unread", "artist:x", 0, 100);
        let weights = preference_weights(
            &[good.clone(), meh.clone(), unread.clone()],
            now,
            &HashMap::new(),
            &HashMap::new(),
        );
        let w = |a: &Archive| weights.get(a.id.as_str()).copied().unwrap_or(0.0);
        assert!(
            w(&good) > w(&meh),
            "finished/recent/5-star ({}) should beat barely-opened/old/1-star ({})",
            w(&good),
            w(&meh)
        );
        assert_eq!(
            w(&unread),
            0.0,
            "an unread archive is not a preference signal"
        );
    }

    #[test]
    fn preference_weight_rewards_bookmarks_and_rereads() {
        let now = 1_800_000_000;
        let base = read_archive("x", "", 50, 100, now);
        let plain = preference_weights(
            std::slice::from_ref(&base),
            now,
            &HashMap::new(),
            &HashMap::new(),
        );
        let boosted = preference_weights(
            std::slice::from_ref(&base),
            now,
            &HashMap::from([("x".to_string(), 40u64)]),
            &HashMap::from([("x".to_string(), 4u64)]),
        );
        assert!(boosted["x"] > plain["x"]);
    }

    #[test]
    fn preference_weight_treats_a_missing_timestamp_as_old_not_current() {
        let now = 1_800_000_000;
        let undated = read_archive("undated", "", 100, 100, 0);
        let fresh = read_archive("fresh", "", 100, 100, now);
        let weights = preference_weights(
            &[undated.clone(), fresh.clone()],
            now,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(weights["fresh"] > weights["undated"]);
    }

    #[test]
    fn namespace_weights_ignore_record_keeping_tags_and_count_once_per_archive() {
        let now = 1_800_000_000;
        let archives = vec![read_archive(
            "a",
            "artist:alpha,artist:beta,date_added:1700000000,rating:4,tag:school",
            10,
            10,
            now,
        )];
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let namespaces = namespace_weights(&archives, &weights, &excluded());
        let names: Vec<&str> = namespaces.iter().map(|n| n.namespace.as_str()).collect();
        assert!(names.contains(&"artist") && names.contains(&"tag"));
        assert!(!names.contains(&"date_added") && !names.contains(&"rating"));

        let artist = namespaces.iter().find(|n| n.namespace == "artist").unwrap();
        let tag = namespaces.iter().find(|n| n.namespace == "tag").unwrap();
        // Two `artist:` tags on one archive must not outweigh one namespace's single tag.
        assert_eq!(artist.weight, tag.weight);
        assert_eq!(artist.archives, 1);

        let by_ns = tags_for_namespaces(
            &archives,
            &weights,
            &excluded(),
            &["artist".to_string(), "tag".to_string()],
        );
        let artist_tags = by_ns.iter().find(|n| n.namespace == "artist").unwrap();
        assert_eq!(artist_tags.tags.len(), 2);
        assert!(artist_tags.tags.iter().all(|t| t.archives == 1));
    }

    #[test]
    fn tags_for_namespaces_follows_the_roster_and_skips_absent_ones() {
        let now = 1_800_000_000;
        let archives = vec![read_archive(
            "a",
            "language:chinese,artist:alpha,circle:beta,cosplayer:somebody,tag:school",
            10,
            10,
            now,
        )];
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let roster: Vec<String> = HIGHLIGHTED_NAMESPACES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let highlights = tags_for_namespaces(&archives, &weights, &excluded(), &roster);
        let order: Vec<&str> = highlights.iter().map(|n| n.namespace.as_str()).collect();
        assert_eq!(
            order,
            vec!["language", "artist", "circle", "cosplayer"],
            "roster order, not weight order"
        );

        // A namespace the library does not have is simply absent, not an empty row.
        let missing =
            tags_for_namespaces(&archives, &weights, &excluded(), &["parody".to_string()]);
        assert!(missing.is_empty());

        // The roster's own tags carry through.
        let language = highlights
            .iter()
            .find(|n| n.namespace == "language")
            .unwrap();
        assert_eq!(language.tags[0].text, "chinese");
    }

    #[test]
    fn a_promoted_axis_below_the_namespace_cap_still_gets_its_own_list() {
        // Regression: `namespace_weights` used to truncate to `MAX_NAMESPACES` itself, so the roster
        // could only find an axis that also made the chart's top twelve — a `circle` tag on one read
        // archive (the real shape of this library) was silently dropped despite being promoted.
        let now = 1_800_000_000;
        // The circle-tagged archive is old (so its preference weight is small) while the filler
        // namespaces are freshly read, which is what puts circle *below* the chart's cap: with equal
        // weights the alphabetical tie-break would let it in and the test would prove nothing.
        let mut archives = vec![read_archive(
            "old",
            "language:chinese,artist:alpha,circle:beta",
            10,
            10,
            now - 400 * 86_400,
        )];
        for i in 0..(MAX_NAMESPACES + 4) {
            archives.push(read_archive(
                &format!("heavy{i}"),
                &format!("heavy{i}:tag{i}"),
                10,
                10,
                now,
            ));
        }
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let excluded = excluded();

        let chart: Vec<String> = namespace_weights(&archives, &weights, &excluded)
            .into_iter()
            .take(MAX_NAMESPACES)
            .map(|n| n.namespace)
            .collect();
        assert!(
            !chart.contains(&"circle".to_string()),
            "the fixture must keep circle outside the chart's cap for this to test anything"
        );

        let roster: Vec<String> = HIGHLIGHTED_NAMESPACES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let highlights = tags_for_namespaces(&archives, &weights, &excluded, &roster);
        assert!(
            highlights.iter().any(|n| n.namespace == "circle"),
            "a promoted axis is listed however far below the cap it ranks"
        );
        let circle = highlights.iter().find(|n| n.namespace == "circle").unwrap();
        assert_eq!(circle.tags[0].text, "beta");
    }

    #[test]
    fn ingest_source_key_splits_uploads_by_credential_only() {
        assert_eq!(
            ingest_source_key("upload", ActorKind::Session),
            "upload_browser"
        );
        assert_eq!(
            ingest_source_key("upload", ActorKind::Token),
            "upload_token"
        );
        // Every other combination keeps its own path row untouched — a download by a token is
        // still just a download for this chart's purposes.
        assert_eq!(ingest_source_key("download", ActorKind::Token), "download");
        assert_eq!(
            ingest_source_key("download", ActorKind::Session),
            "download"
        );
        assert_eq!(ingest_source_key("scanner", ActorKind::System), "scanner");
        // An upload by an actor that is neither (a `System`/`Anonymous` record) stays its own row
        // instead of being folded into a credential bucket it may not belong to.
        assert_eq!(ingest_source_key("upload", ActorKind::System), "upload");
        assert_eq!(ingest_source_key("upload", ActorKind::Anonymous), "upload");
        assert_eq!(
            ingest_source_key("something-new", ActorKind::System),
            "unknown"
        );
    }

    #[test]
    fn source_rows_zero_fill_the_fixed_taxonomy_and_keep_unlisted_keys() {
        let rows = source_rows(HashMap::from([
            ("upload_token".to_string(), 1u64),
            ("download".to_string(), 31u64),
            ("surprise".to_string(), 2u64),
        ]));
        let names: Vec<&str> = rows.iter().map(|r| r.source.as_str()).collect();
        // Fixed rows first, in their own display order, present even at zero.
        assert_eq!(&names[..4], INGEST_SOURCE_ROWS);
        assert_eq!(rows[0].archives, 31);
        assert_eq!(
            rows[1].archives, 0,
            "scanner never ran, and still gets a row"
        );
        assert_eq!(rows[2].archives, 0, "browser upload never ran either");
        assert_eq!(rows[3].archives, 1);
        // An unrecognized key is appended rather than dropped from the total.
        assert_eq!(names[4], "surprise");
        assert_eq!(rows.iter().map(|r| r.archives).sum::<u64>(), 34);
    }

    #[test]
    fn tags_for_namespaces_drops_values_that_are_not_the_axis_they_sit_in() {
        let now = 1_800_000_000;
        let archives = vec![read_archive(
            "a",
            "language:translated,language:chinese,artist:alpha",
            10,
            10,
            now,
        )];
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let rows = tags_for_namespaces(
            &archives,
            &weights,
            &excluded(),
            &["language".to_string(), "artist".to_string()],
        );
        let language = rows.iter().find(|n| n.namespace == "language").unwrap();
        let texts: Vec<&str> = language.tags.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, vec!["chinese"], "`translated` is not a language");
        // Anywhere else the same word is just a tag value and stays.
        let archives = vec![read_archive("b", "tag:translated", 10, 10, now)];
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let rows = tags_for_namespaces(&archives, &weights, &excluded(), &["tag".to_string()]);
        assert_eq!(rows[0].tags.len(), 1);
    }

    #[test]
    fn rating_tiers_count_only_read_archives_and_keep_unrated_in_slot_zero() {
        let now = 1_800_000_000;
        let archives = vec![
            read_archive("five", "rating:5", 10, 10, now),
            read_archive("five2", "rating:5", 10, 10, now),
            read_archive("unrated", "", 10, 10, now),
            Archive {
                tags: "rating:5".to_string(),
                ..archive("unread", "", 0, 0)
            },
        ];
        let weights = preference_weights(&archives, now, &HashMap::new(), &HashMap::new());
        let tiers = rating_tiers(&archives, &weights);
        assert_eq!(tiers.len(), 6);
        assert_eq!(
            tiers[5].archives, 2,
            "the unread 5-star archive is not a reading preference"
        );
        assert_eq!(tiers[0].archives, 1);
        assert_eq!(tiers.iter().map(|t| t.archives).sum::<u64>(), 3);
    }

    #[test]
    fn progress_buckets_classify_the_boundaries() {
        let mk = |id: &str, page: u32, pages: u32, read: bool| Archive {
            lastreadpage: page,
            lastreadtime: if read { 1 } else { 0 },
            ..archive(id, "", 0, pages)
        };
        // Every 20-point boundary is tested on both sides, since each band is inclusive of its own
        // upper bound (a 20%-read archive is `1_20`, not `21_40`) — the one off-by-one that would
        // otherwise only show up as a suspicious-looking histogram.
        let archives = vec![
            mk("unread", 0, 100, false),
            mk("just-started", 1, 100, true),
            mk("first-max", 20, 100, true),
            mk("second", 21, 100, true),
            mk("second-max", 40, 100, true),
            mk("third", 41, 100, true),
            mk("third-max", 60, 100, true),
            mk("fourth", 61, 100, true),
            mk("fourth-max", 80, 100, true),
            mk("fifth", 81, 100, true),
            mk("fifth-max", 99, 100, true),
            mk("finished", 100, 100, true),
            mk("over", 120, 100, true),
        ];
        let buckets = progress_buckets(&archives);
        let get = |key: &str| buckets.iter().find(|b| b.key == key).unwrap().archives;
        assert_eq!(get("unread"), 1);
        assert_eq!(get("1_20"), 2);
        assert_eq!(get("21_40"), 2);
        assert_eq!(get("41_60"), 2);
        assert_eq!(get("61_80"), 2);
        assert_eq!(get("81_99"), 2);
        assert_eq!(get("finished"), 2);
        assert_eq!(buckets.len(), 7);
        assert_eq!(buckets.iter().map(|b| b.archives).sum::<u64>(), 13);
    }

    #[test]
    fn rating_distribution_counts_unrated_and_out_of_range_in_slot_zero() {
        let archives = vec![
            read_archive("a", "rating:5", 1, 1, 1),
            read_archive("b", "rating:5", 1, 1, 1),
            read_archive("c", "rating:0", 1, 1, 1),
            read_archive("d", "", 1, 1, 1),
        ];
        let ratings = rating_distribution(&archives);
        assert_eq!(ratings.len(), 6);
        assert_eq!(ratings[5].archives, 2);
        assert_eq!(ratings[0].archives, 2);
        assert_eq!(ratings.iter().map(|r| r.archives).sum::<u64>(), 4);
    }

    #[test]
    fn totals_split_unread_reading_and_finished() {
        let archives = vec![
            archive("unread", "", 10, 100),
            read_archive("reading", "", 50, 100, 1),
            read_archive("finished", "", 100, 100, 1),
            read_archive("over", "rating:3", 120, 100, 1),
        ];
        let totals = totals(&archives, &[], 3, 99, &HashMap::new());
        assert_eq!(totals.archives, 4);
        assert_eq!(totals.unread, 1);
        assert_eq!(totals.reading, 1);
        assert_eq!(totals.finished, 2);
        assert_eq!(totals.rated, 1);
        assert_eq!(totals.tankoubons, 3);
        assert_eq!(totals.pages, 400);
        assert_eq!(totals.pages_read, 99);
        // Only the unread archive above carries a size — `read_archive` builds its fixtures at 0.
        assert_eq!(totals.storage_bytes, 10);
        assert_eq!(totals.average_bytes, 2);
    }

    #[test]
    fn day_range_is_dense_and_ordered() {
        let days = day_range("2026-03-02", 3);
        assert_eq!(days, vec!["2026-02-28", "2026-03-01", "2026-03-02"]);
        // A nonsense end date yields nothing rather than panicking (only reachable if the system
        // clock is before the Unix epoch).
        assert!(day_range("nonsense", 3).is_empty());
    }

    #[test]
    fn dense_days_covers_every_calendar_day_inclusive_and_ordered() {
        let from = chrono::NaiveDate::from_ymd_opt(2026, 3, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp() as u64;
        let to = from + 2 * 86_400;
        assert_eq!(
            dense_days(from, to),
            vec!["2026-03-01", "2026-03-02", "2026-03-03"]
        );
        // A single day is one point, not zero.
        assert_eq!(dense_days(from, from), vec!["2026-03-01"]);
    }

    #[test]
    fn clamp_window_keeps_the_newest_days_and_never_leaves_the_data() {
        let now = 1_800_000_000u64; // 2027-01-15
        let day = 86_400u64;
        let long_ago = now - 3000 * day;

        // No request at all: the whole available range, truncated to the cap from the *newest* end.
        let (from, to) = clamp_window(None, None, long_ago, now);
        assert_eq!(to, start_of_day(now));
        assert_eq!(
            (to - from) / day,
            TIMELINE_MAX_DAYS,
            "a wider-than-cap request loses its oldest days, not its newest"
        );

        // A request inside the data is honoured exactly.
        let (from, to) = clamp_window(Some(now - 30 * day), Some(now - day), long_ago, now);
        assert_eq!((to - from) / day, 29);
        assert_eq!(to, start_of_day(now - day));

        // A request reaching outside the data is pulled back to it, and a reversed request cannot
        // produce a negative span.
        let (from, to) = clamp_window(Some(0), Some(now + 90 * day), long_ago, now);
        assert_eq!(to, start_of_day(now));
        assert_eq!(from, start_of_day(now - TIMELINE_MAX_DAYS * day));
        let (from, to) = clamp_window(Some(now), Some(now - 10 * day), long_ago, now);
        assert!(from <= to);
    }

    #[test]
    fn day_range_still_zero_fills_from_a_parsed_end_date() {
        let days = day_range("2026-03-02", 3);
        assert_eq!(days, vec!["2026-02-28", "2026-03-01", "2026-03-02"]);
    }

    #[test]
    fn recent_ingests_and_reads_are_ordered_newest_first() {
        let archives = vec![
            archive("old", "date_added:1000", 1, 1),
            archive("new", "date_added:3000", 1, 1),
            archive("mid", "date_added:2000", 1, 1),
        ];
        let recent = recent_ingests(&archives);
        assert_eq!(
            recent.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["new", "mid", "old"]
        );

        let reads = vec![
            read_archive("r1", "", 1, 1, 100),
            read_archive("r2", "", 1, 1, 300),
        ];
        assert_eq!(recent_reads(&reads)[0].id, "r2");
    }

    #[test]
    fn most_reread_ranks_by_progress_update_count() {
        let archives = vec![
            read_archive("a", "", 1, 1, 1),
            read_archive("b", "", 1, 1, 1),
            archive("never", "", 0, 0),
        ];
        let freq = HashMap::from([("a".to_string(), 5u64), ("b".to_string(), 50u64)]);
        let rows = most_reread(&archives, &freq);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "b");
        assert_eq!(rows[0].reads, 50);
    }
}
