//! Duplicate detection and review (spec 024).
//!
//! Replaces legacy's cover-SHA-1 grouping with a perceptual hash plus metadata evidence, keeps the
//! legacy `LRR_DUPLICATE_GROUPS` hash in sync, and exposes the group/compare/decision endpoints the
//! new `/duplicates` page uses. The legacy read endpoint (`GET /database/duplicates`) is unchanged.

pub mod compare_cache;
pub mod cover;
pub mod decisions;
pub mod detect;
pub mod model;
pub mod patching;
pub mod review;
pub mod scan;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::common::error;
use crate::AppState;
use model::{ArchiveMeta, DecisionRecord, GroupRecord};

const GROUPS_KEY: &str = "LANRURUGI_DUP_GROUPS";
const LEGACY_GROUPS_KEY: &str = "LRR_DUPLICATE_GROUPS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/database/duplicates",
            get(get_duplicates).delete(clear_duplicates),
        )
        .route("/database/duplicates/scan", post(scan_duplicates))
        .route("/database/duplicates/events", get(scan_events))
        .route("/database/duplicates/groups", get(list_groups))
        .route("/database/duplicates/groups/{key}", get(group_detail))
        .route("/database/duplicates/compare", get(compare_pair))
        .route(
            "/database/duplicates/compare/progress",
            get(compare_progress),
        )
        .route("/database/duplicates/patch", post(patching::create_patch))
        .route(
            "/database/duplicates/decisions",
            get(get_decisions).post(put_decisions),
        )
}

#[derive(Debug, Deserialize, Default)]
struct ScanParams {
    threshold: Option<u32>,
}

async fn scan_duplicates(
    State(state): State<AppState>,
    Query(params): Query<ScanParams>,
) -> Response {
    let threshold = params.threshold.unwrap_or(detect::DEFAULT_THRESHOLD);
    if threshold > detect::MAX_THRESHOLD {
        return error(
            StatusCode::BAD_REQUEST,
            "find_duplicates",
            "threshold must be between 0 and 7",
        );
    }
    let Some(job_id) = scan::start_scan(&state, threshold).await else {
        return error(
            StatusCode::CONFLICT,
            "find_duplicates",
            "a duplicate scan is already running",
        );
    };

    axum::Json(json!({"operation": "find_duplicates", "success": 1, "job": job_id})).into_response()
}

pub(crate) async fn run_scan(state: &AppState, threshold: u32, job: &str) -> Result<usize, String> {
    // A restore replaces the keyspace; stop before hashing covers into records that may not exist
    // afterwards.
    if state.restore_gate.closed() {
        return Err(lanrurugi_core::restore_gate::RESTORE_ABORTED_MESSAGE.to_string());
    }
    let archives = state
        .repos
        .archives
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    // Only catalogued archives with a real file are candidates; an unfinished ingest has no cover,
    // and one still in its metadata run is not offered for deletion (spec 025). A record whose
    // `file` is a directory (seen on a real library: empty name, path ending in `/`) has no file.
    let pending = crate::visibility::pending_ids(state).await;
    let mut metas: Vec<ArchiveMeta> = archives
        .iter()
        .filter(|a| has_archive_file(&a.file) && a.pagecount > 0)
        .filter(|a| !pending.contains(a.id.as_str()))
        .map(|a| ArchiveMeta {
            id: a.id.as_str().to_string(),
            title: a.title.clone(),
            tags: split_tags(&a.tags),
            pages: a.pagecount,
            size: a.arcsize,
        })
        .collect();
    let library_ids: HashSet<String> = archives.iter().map(|a| a.id.as_str().to_string()).collect();

    // Members are compared as the reader shows them: a `.patch.zip` adds pages.
    let files: HashMap<String, String> = archives
        .iter()
        .map(|a| (a.id.as_str().to_string(), a.file.clone()))
        .collect();
    let blake3s: HashMap<String, String> = archives
        .iter()
        .filter_map(|a| {
            Some((
                a.id.as_str().to_string(),
                a.blake3.clone()?.to_ascii_lowercase(),
            ))
        })
        .collect();
    let candidates: Vec<(String, String, Option<String>)> = metas
        .iter()
        .filter_map(|m| {
            files
                .get(&m.id)
                .map(|f| (m.id.clone(), f.clone(), blake3s.get(&m.id).cloned()))
        })
        .collect();
    let files_for_sweep = files.clone();
    let mut patch_states: HashMap<String, patching::PatchState> =
        lanrurugi_core::concurrency::run_blocking(move || {
            let swept = scan::sweep_patch_temps(&files_for_sweep);
            if swept > 0 {
                tracing::info!(swept, "removed leftover patch temp files");
            }
            candidates
                .iter()
                .filter_map(|(id, file, blake3)| {
                    patching::patch_state(std::path::Path::new(file), blake3.as_deref())
                        .map(|s| (id.clone(), s))
                })
                .collect()
        })
        .await
        .map_err(|e| e.to_string())?;
    for meta in metas.iter_mut() {
        if let Some(patch) = patch_states.get(&meta.id) {
            meta.pages += patch.pages;
        }
    }
    state.jobs.set_progress(job, 0.05).await;

    let ids: Vec<String> = metas.iter().map(|m| m.id.clone()).collect();
    let rejects = decisions::load_rejects(state).await;
    // Cached covers first, then the rest one by one. The groups found so far are published as they
    // grow, so the page can show them while the scan runs (it warns they may still change).
    let mut hashes = cover::cached(state, &ids).await;
    let missing: Vec<&String> = ids.iter().filter(|id| !hashes.contains_key(*id)).collect();
    let mut published: Option<Vec<String>> = None;
    scan::set_progress(scan::ScanPhase::Covers, 0, missing.len());
    publish_partial(state, &metas, &hashes, threshold, &rejects, &mut published).await?;
    let mut last_tick = Instant::now();
    let mut last_publish = Instant::now();
    let mut no_cover = 0usize;
    for (done, id) in missing.iter().enumerate() {
        if done.is_multiple_of(200) && state.restore_gate.closed() {
            return Err(lanrurugi_core::restore_gate::RESTORE_ABORTED_MESSAGE.to_string());
        }
        match cover::hash_and_store(state, id).await {
            Some(bytes) => {
                hashes.insert((*id).clone(), bytes);
            }
            None => no_cover += 1,
        }
        if last_tick.elapsed() >= PROGRESS_INTERVAL {
            scan::set_progress(scan::ScanPhase::Covers, done + 1, missing.len());
            let fraction = (done + 1) as f32 / missing.len() as f32;
            state.jobs.set_progress(job, 0.05 + 0.55 * fraction).await;
            if last_publish.elapsed() >= PARTIAL_PUBLISH_INTERVAL {
                publish_partial(state, &metas, &hashes, threshold, &rejects, &mut published)
                    .await?;
                last_publish = Instant::now();
            }
            notify_changed();
            last_tick = Instant::now();
        }
    }
    scan::store_no_cover(state, no_cover).await;
    if no_cover > 0 {
        tracing::info!(
            no_cover,
            "duplicate scan skipped archives without a cover thumbnail"
        );
    }
    state.jobs.set_progress(job, 0.6).await;

    scan::set_progress(scan::ScanPhase::Grouping, 0, 0);
    notify_changed();
    let metas_for_task = metas.clone();
    let hashes_for_task = hashes.clone();
    let rejects_for_task = rejects.clone();
    let mut groups = lanrurugi_core::concurrency::run_blocking(move || {
        detect::build_groups(
            &metas_for_task,
            &hashes_for_task,
            threshold,
            now_secs(),
            &rejects_for_task,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    let hashes_for_regroup = hashes.clone();
    groups = groups
        .iter()
        .flat_map(|group| detect::regroup_by_pairs(group, &hashes_for_regroup))
        .collect();
    scan::set_progress(scan::ScanPhase::Patches, 0, 0);
    notify_changed();
    attach_patch_info(&mut groups, &mut patch_states, &files, &blake3s).await?;

    store_groups(state, &groups).await?;
    notify_changed();
    // Semantic review: one model call per group without a cached verdict. `ensure_with` reuses one
    // loaded map for the whole pass and stores both the content-keyed verdict and the group-keyed
    // copy the read endpoints use.
    let mut reviews = review::load_all(state).await;
    for (done, group) in groups.iter().enumerate() {
        if done.is_multiple_of(200) && state.restore_gate.closed() {
            return Err(lanrurugi_core::restore_gate::RESTORE_ABORTED_MESSAGE.to_string());
        }
        if last_tick.elapsed() >= PROGRESS_INTERVAL {
            scan::set_progress(scan::ScanPhase::Reviewing, done, groups.len());
            state
                .jobs
                .set_progress(job, 0.6 + 0.35 * done as f32 / groups.len() as f32)
                .await;
            notify_changed();
            last_tick = Instant::now();
        }
        if group.patch_links.is_empty() {
            review::ensure_with(state, group, &metas, &mut reviews).await;
        } else {
            review::store_patched_verdict(state, group, &mut reviews).await;
        }
    }

    scan::set_progress(scan::ScanPhase::Cleanup, 0, 0);
    notify_changed();
    scan::prune(state, &library_ids, &groups, &metas).await;

    state.jobs.set_progress(job, 1.0).await;
    Ok(groups.len())
}

/// Whether an archive record points at a file: not empty, and not a directory path (a damaged
/// record seen on a real library had its library folder, ending in `/`, as its file).
fn has_archive_file(file: &str) -> bool {
    let file = file.trim();
    !file.is_empty() && !file.ends_with('/') && !file.ends_with(std::path::MAIN_SEPARATOR)
}

/// How often a running scan updates the progress it reports.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

/// How often a running scan republishes the groups found so far.
const PARTIAL_PUBLISH_INTERVAL: Duration = Duration::from_secs(2);

/// Groups the covers hashed so far and stores them, without the patch links or model reviews the
/// final pass adds.
async fn publish_partial(
    state: &AppState,
    metas: &[ArchiveMeta],
    hashes: &HashMap<String, [u8; 8]>,
    threshold: u32,
    rejects: &HashSet<(String, String)>,
    published: &mut Option<Vec<String>>,
) -> Result<(), String> {
    let metas = metas.to_vec();
    let hashes = hashes.clone();
    let rejects = rejects.clone();
    let groups = lanrurugi_core::concurrency::run_blocking(move || {
        detect::build_groups(&metas, &hashes, threshold, now_secs(), &rejects)
            .iter()
            .flat_map(|group| detect::regroup_by_pairs(group, &hashes))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    // Nothing to write or announce when the same groups came out again — the usual case on a
    // rescan, where nearly every cover hash is already cached.
    let mut keys: Vec<String> = groups.iter().map(|g| group_key(&g.members)).collect();
    keys.sort();
    if published.as_ref() == Some(&keys) {
        return Ok(());
    }
    store_groups(state, &groups).await?;
    *published = Some(keys);
    notify_changed();
    Ok(())
}

/// Replaces the stored groups (ours and legacy's) in one transaction, so a reader never sees the
/// set half-written.
async fn store_groups(state: &AppState, groups: &[GroupRecord]) -> Result<(), String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let mut pipe = deadpool_redis::redis::pipe();
    pipe.atomic().del(GROUPS_KEY).del(LEGACY_GROUPS_KEY);
    for group in groups {
        pipe.hset(
            GROUPS_KEY,
            group_key(&group.members),
            serde_json::to_string(group).unwrap_or_default(),
        );
        pipe.hset(
            LEGACY_GROUPS_KEY,
            format!("dupgp_{}", legacy_composite(&group.members)),
            serde_json::to_string(&group.members).unwrap_or_default(),
        );
    }
    pipe.query_async::<()>(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    GROUPS_REVISION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Bumped whenever the stored groups are replaced or cleared, so the page refetches them only when
/// they changed rather than on every progress update.
static GROUPS_REVISION: AtomicU64 = AtomicU64::new(0);

/// Wakes `GET /database/duplicates/events` subscribers whenever the stored groups or the scan's
/// state change.
fn scan_changes() -> &'static broadcast::Sender<()> {
    static TX: OnceLock<broadcast::Sender<()>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(16).0)
}

fn notify_changed() {
    let _ = scan_changes().send(());
}

/// Whether a duplicate scan is queued or running, and how far it is.
async fn scan_status(state: &AppState) -> (bool, f32) {
    state
        .jobs
        .by_name("find_duplicates")
        .await
        .iter()
        .find(|job| !job.state.is_terminal())
        .map_or((false, 1.0), |job| (true, job.progress))
}

/// The scan's state as the page reads it, in both the events and the groups listing.
async fn status_json(state: &AppState) -> serde_json::Value {
    let (scanning, progress) = scan_status(state).await;
    let phase = scanning.then(scan::progress).flatten();
    json!({
        "scanning": scanning,
        "progress": progress,
        "revision": GROUPS_REVISION.load(Ordering::Relaxed),
        "phase": phase.map(|p| p.phase),
        "done": phase.map_or(0, |p| p.done),
        "total": phase.map_or(0, |p| p.total),
        "interrupted": scan::interrupted(state, scanning).await,
        "no_cover": scan::no_cover(state).await,
    })
}

/// `GET /database/duplicates/events` — SSE: a `status` event (see `status_json`) on connect
/// and again whenever the stored groups or the scan's state change, so the page can refetch the
/// groups as a scan finds them.
async fn scan_events(State(state): State<AppState>) -> Response {
    let rx = scan_changes().subscribe();
    let first = futures_util::stream::once(async {});
    let changes = BroadcastStream::new(rx).map(|_| ());
    let stream = first.chain(changes).then(move |()| {
        let state = state.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event("status")
                    .data(status_json(&state).await.to_string()),
            )
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// A patch whose sources are still CRC32s, with the files and BLAKE3s of the other members of its
/// group — the only archives those CRC32s can still be matched against.
struct LegacyPatch {
    target: String,
    file: String,
    blake3: Option<String>,
    others: Vec<(String, String)>,
}

/// Records each member's patch page count and which members' patches came from another member,
/// by the members' BLAKE3s. A patch whose sources are still CRC32s is migrated first, which costs
/// the CRC32 of the other members of its groups (a full read of each) once.
async fn attach_patch_info(
    groups: &mut [GroupRecord],
    patch_states: &mut HashMap<String, patching::PatchState>,
    files: &HashMap<String, String>,
    blake3s: &HashMap<String, String>,
) -> Result<(), String> {
    let mut legacy: Vec<LegacyPatch> = Vec::new();
    for group in groups.iter() {
        for target in &group.members {
            if !patch_states
                .get(target)
                .is_some_and(|p| !p.legacy_sources.is_empty())
            {
                continue;
            }
            let Some(file) = files.get(target) else {
                continue;
            };
            let others: Vec<(String, String)> = group
                .members
                .iter()
                .filter(|m| *m != target)
                .filter_map(|m| Some((files.get(m)?.clone(), blake3s.get(m)?.clone())))
                .collect();
            legacy.push(LegacyPatch {
                target: target.clone(),
                file: file.clone(),
                blake3: blake3s.get(target).cloned(),
                others,
            });
        }
    }
    if !legacy.is_empty() {
        let migrated: Vec<(String, Vec<String>)> = lanrurugi_core::concurrency::run_blocking(move || {
            legacy
                .into_iter()
                .filter_map(|LegacyPatch { target, file, blake3, others }| {
                    let found: HashMap<String, String> = others
                        .into_iter()
                        .filter_map(|(path, hash)| {
                            let crc = lanrurugi_scanner::patch::crc32_of_file(std::path::Path::new(&path)).ok()?;
                            Some((crc.to_ascii_lowercase(), hash))
                        })
                        .collect();
                    match patching::migrate_legacy_sources(std::path::Path::new(&file), blake3.as_deref(), &found) {
                        Ok(sources) => Some((target, sources)),
                        Err(e) => {
                            tracing::warn!(id = %target, error = %e, "could not convert a patch's CRC32 sources");
                            None
                        }
                    }
                })
                .collect()
        })
        .await
        .map_err(|e| e.to_string())?;
        for (target, sources) in migrated {
            if let Some(state) = patch_states.get_mut(&target) {
                state.sources = sources;
                state.legacy_sources.clear();
            }
        }
    }

    for group in groups.iter_mut() {
        for target in &group.members {
            let Some(patch) = patch_states.get(target) else {
                continue;
            };
            if patch.pages > 0 {
                group.patch_pages.insert(target.clone(), patch.pages);
            }
            for source in &group.members {
                if source != target
                    && blake3s
                        .get(source)
                        .is_some_and(|hash| patch.sources.contains(hash))
                {
                    group.patch_links.push(model::PatchLink {
                        target: target.clone(),
                        source: source.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

pub fn group_key(members: &[String]) -> String {
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.join(",")
}

fn legacy_composite(members: &[String]) -> String {
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.iter().map(|id| &id[..10.min(id.len())]).collect()
}

async fn load_groups(state: &AppState) -> Vec<GroupRecord> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return Vec::new();
    };
    let raw: HashMap<String, String> = conn.hgetall(GROUPS_KEY).await.unwrap_or_default();
    let mut groups: Vec<GroupRecord> = raw
        .into_values()
        .filter_map(|v| serde_json::from_str(&v).ok())
        .collect();
    groups.sort_by(|a, b| a.members.first().cmp(&b.members.first()));
    groups
}

async fn list_groups(State(state): State<AppState>) -> Response {
    let groups = load_groups(&state).await;
    let decisions = decisions::load_all(&state).await;
    let reviews = review::load_all(&state).await;
    let mut views = Vec::with_capacity(groups.len());
    for group in &groups {
        let key = group_key(&group.members);
        let decision = decisions.get(&key).cloned().unwrap_or_default();
        let review = reviews.get(&key).cloned();
        views.push(group_view(&state, group, &key, &decision, review).await);
    }
    axum::Json(json!({
        "status": status_json(&state).await,
        "scanned_at": groups.first().map(|g| g.built_at).unwrap_or(0),
        "algorithm": detect::ALGORITHM,
        "chained_groups": groups.iter().filter(|g| g.chained).count(),
        "groups": views,
    }))
    .into_response()
}

async fn group_view(
    state: &AppState,
    group: &GroupRecord,
    key: &str,
    decision: &DecisionRecord,
    review: Option<model::ReviewVerdict>,
) -> serde_json::Value {
    let mut members = Vec::with_capacity(group.members.len());
    let mut pages_by_id = HashMap::new();
    let mut sizes_by_id = HashMap::new();
    for id in &group.members {
        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
            .ok()
            .flatten();
        let Some(archive) = archive else { continue };
        let patch_pages = group.patch_pages.get(id).copied().unwrap_or(0);
        let pages = archive.pagecount + patch_pages;
        pages_by_id.insert(id.clone(), pages);
        sizes_by_id.insert(id.clone(), archive.arcsize);
        let recommended = review
            .as_ref()
            .map(|r| r.keep.iter().any(|k| k == id))
            .unwrap_or(false);
        members.push(json!({
            "id": id,
            "title": archive.title,
            "size": archive.arcsize,
            "pages": pages,
            "patch_pages": patch_pages,
            "cover": format!("/api/archives/{id}/thumbnail"),
            "tags": split_tags(&archive.tags),
            "decision": decision.state(id),
            "recommended": recommended,
        }));
    }
    let best = detect::best_member(group, &pages_by_id, &sizes_by_id);
    if let Some(best_id) = best.as_deref() {
        if let Some(index) = members
            .iter()
            .position(|m| m.get("id").and_then(|v| v.as_str()) == Some(best_id))
        {
            if let Some(object) = members[index].as_object_mut() {
                object.insert("best".to_string(), serde_json::Value::Bool(true));
            }
        }
    }
    json!({
        "key": key,
        "resolved": decision.keeps_all(&group.members),
        "chained": group.chained,
        "middle": group.middle,
        "members": members,
        "pairs": group.pairs.iter().map(|p| json!({
            "a": p.a, "b": p.b, "similarity": p.similarity,
            "distance": p.distance, "title_similarity": p.title_similarity,
        })).collect::<Vec<_>>(),
        "review": review,
        "patch_links": group.patch_links,
    })
}

async fn group_detail(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    let groups = load_groups(&state).await;
    let Some(group) = groups.into_iter().find(|g| group_key(&g.members) == key) else {
        return error(StatusCode::NOT_FOUND, "duplicates", "group not found");
    };
    let decision = decisions::load_all(&state)
        .await
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let review = review::load_all(&state).await.get(&key).cloned();
    let mut view = group_view(&state, &group, &key, &decision, review).await;

    let matrix: Vec<Vec<f64>> = group
        .members
        .iter()
        .map(|a| {
            group
                .members
                .iter()
                .map(|b| {
                    if a == b {
                        1.0
                    } else {
                        group
                            .pairs
                            .iter()
                            .find(|p| (&p.a == a && &p.b == b) || (&p.a == b && &p.b == a))
                            .map(|p| p.similarity)
                            .unwrap_or(0.0)
                    }
                })
                .collect()
        })
        .collect();
    view["matrix"] = json!(matrix);
    axum::Json(view).into_response()
}

#[derive(Debug, Deserialize)]
struct CompareParams {
    a: String,
    b: String,
}

async fn compare_pair(
    State(state): State<AppState>,
    Query(params): Query<CompareParams>,
) -> Response {
    match review::compare(&state, &params.a, &params.b).await {
        Ok(value) => axum::Json(value).into_response(),
        Err((status, message)) => error(status, "duplicates", message),
    }
}

/// `GET /database/duplicates/compare/progress?a=&b=` — how many pages of each side the running
/// comparison of that pair has fingerprinted (`null` when it is not running).
async fn compare_progress(Query(params): Query<CompareParams>) -> Response {
    let key = compare_cache::progress_key(&params.a, &params.b);
    let body = compare_cache::current(&key).map(|[a, b]| {
        json!({
            "a": { "done": a.0, "total": a.1 },
            "b": { "done": b.0, "total": b.1 },
        })
    });
    axum::Json(json!({ "progress": body })).into_response()
}

#[derive(Debug, Deserialize)]
struct DecisionsQuery {
    group_key: String,
}

async fn get_decisions(
    State(state): State<AppState>,
    Query(query): Query<DecisionsQuery>,
) -> Response {
    let record = decisions::load_all(&state)
        .await
        .get(&query.group_key)
        .cloned()
        .unwrap_or_default();
    axum::Json(json!({
        "group_key": query.group_key,
        "decisions": record.decisions,
        "not_duplicate_pairs": record.not_duplicate,
        "updated_at": record.updated_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct DecisionsBody {
    group_key: String,
    #[serde(default)]
    decisions: HashMap<String, String>,
    #[serde(default)]
    not_duplicate_pairs: Vec<[String; 2]>,
}

async fn put_decisions(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<DecisionsBody>,
) -> Response {
    let groups = load_groups(&state).await;
    let Some(group) = groups
        .into_iter()
        .find(|g| group_key(&g.members) == body.group_key)
    else {
        return error(StatusCode::NOT_FOUND, "duplicates", "group not found");
    };
    let members: HashSet<&str> = group.members.iter().map(String::as_str).collect();
    for (id, state_value) in &body.decisions {
        if !members.contains(id.as_str()) {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "decision names an archive outside this group",
            );
        }
        if !matches!(state_value.as_str(), "keep" | "discard" | "undecided") {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "decision state must be keep, discard or undecided",
            );
        }
    }
    for [a, b] in &body.not_duplicate_pairs {
        if !members.contains(a.as_str()) || !members.contains(b.as_str()) {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "a not-duplicate pair names an archive outside this group",
            );
        }
    }
    let mut record = decisions::load_all(&state)
        .await
        .get(&body.group_key)
        .cloned()
        .unwrap_or_default();
    for (id, value) in body.decisions {
        if value == "undecided" {
            record.decisions.remove(&id);
        } else {
            record.decisions.insert(id, value);
        }
    }
    for pair in body.not_duplicate_pairs {
        if !record.rejects(&pair[0], &pair[1]) {
            record.not_duplicate.push(pair);
        }
    }
    record.updated_at = now_secs();
    if let Err(e) = decisions::save(&state, &body.group_key, &record).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "duplicates", e);
    }
    if let Err(e) = decisions::add_rejects(&state, &record.not_duplicate).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "duplicates", e);
    }
    axum::Json(json!({"operation": "duplicate_decisions", "success": 1})).into_response()
}

/// Legacy read endpoint: unchanged shape, still fed by `LRR_DUPLICATE_GROUPS`.
async fn get_duplicates(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_duplicates",
                e.to_string(),
            )
        }
    };
    let raw_groups: HashMap<String, String> =
        conn.hgetall(LEGACY_GROUPS_KEY).await.unwrap_or_default();
    let mut result = Vec::new();
    for (key, json_ids) in raw_groups {
        let Ok(ids) = serde_json::from_str::<Vec<String>>(&json_ids) else {
            continue;
        };
        let mut surviving = Vec::new();
        let mut archives = Vec::new();
        for id in &ids {
            match state
                .repos
                .archives
                .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
                .await
            {
                Ok(Some(archive)) => {
                    surviving.push(id.clone());
                    archives.push(json!({
                        "arcid": archive.id,
                        "title": archive.title,
                        "tags": archive.tags,
                        "size": archive.arcsize,
                        "extension": archive.extension(),
                        "group_key": key,
                    }));
                }
                _ => continue,
            }
        }
        if surviving.len() < 2 {
            let _: Result<(), _> = conn.hdel(LEGACY_GROUPS_KEY, &key).await;
            continue;
        }
        if surviving.len() != ids.len() {
            let value = serde_json::to_string(&surviving).unwrap_or_default();
            let _: Result<(), _> = conn.hset(LEGACY_GROUPS_KEY, &key, value).await;
        }
        result.push(archives);
    }
    axum::Json(result).into_response()
}

async fn clear_duplicates(State(state): State<AppState>) -> Response {
    let Ok(mut conn) = state.redis.config.get().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "clear_duplicates",
            "redis unavailable",
        );
    };
    for key in [
        GROUPS_KEY,
        LEGACY_GROUPS_KEY,
        cover::COVER_HASH_KEY,
        decisions::DECISIONS_KEY,
        review::REVIEW_KEY,
    ] {
        let _: Result<(), _> = conn.del(key).await;
    }
    GROUPS_REVISION.fetch_add(1, Ordering::Relaxed);
    notify_changed();
    axum::Json(json!({"operation": "clear_duplicates", "success": 1})).into_response()
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_key_is_sorted_and_stable() {
        let a = vec!["b".to_string(), "a".to_string()];
        let b = vec!["a".to_string(), "b".to_string()];
        assert_eq!(group_key(&a), "a,b");
        assert_eq!(group_key(&a), group_key(&b));
    }

    #[test]
    fn legacy_composite_uses_first_ten_chars() {
        let members = vec!["a".repeat(12), "b".repeat(12)];
        assert_eq!(
            legacy_composite(&members),
            format!("{}{}", "a".repeat(10), "b".repeat(10))
        );
    }
}
