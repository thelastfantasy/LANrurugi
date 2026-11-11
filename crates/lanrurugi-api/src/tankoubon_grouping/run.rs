//! The detached side of `ai-group-suggestions`: the expensive pass runs on its own task, and an
//! HTTP request only starts it or attaches to the one already running.
//!
//! Before this split the whole computation lived inside the request future, which made the
//! endpoint's cost unbounded from the client's side: Cloudflare gives up on an origin after ~100s
//! and hyper drops the future when the connection goes, so the work was discarded *and* no cache
//! write happened, leaving the next click to redo all of it from zero. Confirmed live on a 6969-
//! archive library: `POST /tankoubons/ai-group-suggestions` logged at 05:46:25, nginx recorded the
//! client (Cloudflare) disconnecting with 499 at 05:48:30, and neither
//! `LANRURUGI_GROUP_SUGGESTIONS_CACHE` nor a single suggestion record existed afterwards.
//!
//! The shape is `download_queue`'s, which solves the same problem for downloads: state lives in
//! [`GroupingHub`], the SSE handler is only a subscriber, and a reconnecting client gets a full
//! snapshot and carries on. Two additions specific to this workload:
//!
//! - a run is keyed by the same fingerprint as the cache, so two clicks at once join one run
//!   instead of each starting a whole-library pass;
//! - a fingerprint change (an archive grouped, a suggestion ignored) supersedes the running one —
//!   its answer is already known to be stale, so letting it finish would only burn CPU.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use deadpool_redis::redis::AsyncCommands;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

use super::{
    enrich_suggestions, group_evidence, group_with_hooked, review_hints, tag_set, title_tokens_for,
    GroupSuggestion, GroupingSettings, Scorer, MIN_GROUP_SIZE, SUGGESTIONS_CACHE_KEY,
};
use crate::AppState;

/// Which stage the run is in, as a small integer so progress can be read without a lock.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Load,
    Embed,
    Group,
    Save,
}

impl Phase {
    fn code(self) -> u8 {
        match self {
            Phase::Load => 0,
            Phase::Embed => 1,
            Phase::Group => 2,
            Phase::Save => 3,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => Phase::Embed,
            2 => Phase::Group,
            _ => Phase::Load,
        }
    }

    /// The name the SSE `status` event and `GET /result` both report.
    fn name(self) -> &'static str {
        match self {
            Phase::Load => "load",
            Phase::Embed => "embed",
            Phase::Group => "group",
            Phase::Save => "save",
        }
    }
}

/// One in-flight computation. Every field the progress path touches is atomic — the grouping scan
/// reports from a rayon worker thread, so a lock here would serialize the hot loop.
pub(crate) struct RunHandle {
    pub run_id: String,
    pub fingerprint: String,
    pub started_at: i64,
    phase: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    /// Set from the async side, read by the blocking scan — `CancellationToken` is async-only, and
    /// the scan cannot await between rows.
    cancelled: AtomicBool,
    cancel: CancellationToken,
}

impl RunHandle {
    fn new(fingerprint: String) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let started_at = unix_now();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            run_id: format!("{started_at}-{seq}"),
            fingerprint,
            started_at,
            phase: AtomicU8::new(Phase::Load.code()),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            cancel: CancellationToken::new(),
        }
    }

    fn start_phase(&self, phase: Phase, total: u64) {
        self.phase.store(phase.code(), Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    fn set_progress(&self, done: u64, total: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total.max(1), Ordering::Relaxed);
    }

    fn phase(&self) -> Phase {
        Phase::from_code(self.phase.load(Ordering::Relaxed))
    }

    fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.cancel.cancel();
    }

    pub(crate) fn snapshot(&self) -> Value {
        json!({
            "run_id": self.run_id,
            "fingerprint": self.fingerprint,
            "phase": self.phase().name(),
            "done": self.done(),
            "total": self.total(),
            "started_at": self.started_at,
            "elapsed_secs": unix_now().saturating_sub(self.started_at),
        })
    }
}

/// Holds the one run in flight and fans its progress out to SSE subscribers.
pub struct GroupingHub {
    tx: broadcast::Sender<Value>,
    current: tokio::sync::Mutex<Option<Arc<RunHandle>>>,
}

impl Default for GroupingHub {
    fn default() -> Self {
        Self::new()
    }
}

impl GroupingHub {
    pub fn new() -> Self {
        // Deep enough that a client watching a multi-second scan through a slow connection is not
        // dropped to the `Lagged` path constantly; a lagging client just gets a fresh `full`.
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            current: tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.tx.subscribe()
    }

    /// The run in flight, if any.
    pub(crate) async fn current(&self) -> Option<Arc<RunHandle>> {
        self.current.lock().await.clone()
    }

    fn publish(&self, event: Value) {
        // No subscribers is the normal case (nobody has the drawer open) — not an error.
        let _ = self.tx.send(event);
    }

    fn publish_status(&self, handle: &RunHandle) {
        self.publish(json!({ "kind": "status", "status": handle.snapshot() }));
    }

    /// Joins the run already going for this fingerprint, or supersedes a stale one and starts a
    /// new one. Returns immediately; the work itself is on its own task.
    pub(crate) async fn start_or_join(
        self: &Arc<Self>,
        state: &AppState,
        candidates: CandidateSet,
    ) -> Arc<RunHandle> {
        let mut current = self.current.lock().await;
        if let Some(existing) = current.as_ref() {
            if existing.fingerprint == candidates.fingerprint && !existing.is_cancelled() {
                return existing.clone();
            }
            // Stale answer or abandoned run: stop spending CPU on it.
            existing.cancel();
        }
        let handle = Arc::new(RunHandle::new(candidates.fingerprint.clone()));
        *current = Some(handle.clone());
        drop(current);

        let hub = self.clone();
        let state = state.clone();
        let task_handle = handle.clone();
        tokio::spawn(async move {
            execute(state, hub, task_handle, candidates).await;
        });
        handle
    }

    async fn finish(&self, run_id: &str) {
        let mut current = self.current.lock().await;
        if current.as_ref().is_some_and(|r| r.run_id == run_id) {
            *current = None;
        }
    }
}

/// The persisted result. `suggestions` is kept as plain JSON: it is a transport shape (the handlers
/// hand it to the client unchanged), and typing it would force `Deserialize` onto
/// `GroupSuggestion` and everything it contains for no other reader.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CachedSuggestions {
    pub fingerprint: String,
    pub suggestions: Vec<Value>,
    /// `false` for the graph-only write that lands before `enrich_suggestions` runs — the
    /// suggestions are already usable, they just don't carry their `settled` review flags yet.
    #[serde(default = "default_true")]
    pub reviewed: bool,
    #[serde(default)]
    pub computed_at: i64,
    #[serde(default)]
    pub duration_ms: u64,
}

fn default_true() -> bool {
    true
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Everything a run (or a cache lookup) needs before the expensive part: the candidate ids, the
/// existing Tankoubons, the ignored set, and the fingerprint over all of it.
pub(crate) struct CandidateSet {
    pub ungrouped_ids: Vec<String>,
    pub tankoubons: Vec<lanrurugi_core::entities::Grouping>,
    pub ignored: HashSet<String>,
    pub fingerprint: String,
}

impl CandidateSet {
    /// Nothing to group: fewer than two loose archives and no Tankoubon to add one to.
    pub(crate) fn nothing_to_suggest(&self) -> bool {
        self.ungrouped_ids.is_empty()
            || (self.ungrouped_ids.len() < MIN_GROUP_SIZE && self.tankoubons.is_empty())
    }
}

/// Reads the candidate set. Cheap (two Redis reads and the Tankoubon list) — this is what lets a
/// request decide "cache hit", "join the run" or "start one" without doing any real work.
pub(crate) async fn candidate_set(state: &AppState) -> Result<CandidateSet, String> {
    let mut search_conn = state.redis.search.get().await.map_err(|e| e.to_string())?;
    // "Not currently a member of any Tankoubon" is exactly `LRR_TANKGROUPED`, minus the
    // Tankoubons' own `TANK_` aggregate entries (see that key's docs in `lanrurugi-search::keys`).
    let ungrouped_ids: Vec<String> = search_conn
        .smembers::<_, Vec<String>>(lanrurugi_search::keys::TANKGROUPED_KEY)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|id| !id.starts_with("TANK_"))
        .collect();
    let tankoubons = state
        .repos
        .groupings
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    let ignored = state
        .ignored_group_suggestions
        .ignored_fingerprints()
        .await
        .map_err(|e| e.to_string())?;
    let fingerprint = super::suggestions_fingerprint(
        &ungrouped_ids,
        &tankoubons,
        &ignored,
        &GroupingSettings::default(),
    );
    Ok(CandidateSet {
        ungrouped_ids,
        tankoubons,
        ignored,
        fingerprint,
    })
}

/// The cached result for this exact library state, if there is one.
pub(crate) async fn cached(state: &AppState, fingerprint: &str) -> Option<CachedSuggestions> {
    let mut conn = state.redis.config.get().await.ok()?;
    let raw: Option<String> = conn
        .get::<_, Option<String>>(SUGGESTIONS_CACHE_KEY)
        .await
        .ok()?;
    // An entry written before `reviewed`/`computed_at` existed still parses: both have defaults.
    let entry: CachedSuggestions = serde_json::from_str(&raw?).ok()?;
    (entry.fingerprint == fingerprint).then_some(entry)
}

/// The one payload `GET /result` and the SSE `full` snapshot share.
pub(crate) fn result_json(entry: Option<&CachedSuggestions>, running: Option<&RunHandle>) -> Value {
    // A running run wins over a cache entry for the same fingerprint: the early graph-only write
    // makes an entry visible while `enrich_suggestions` is still going, and a client told "ready"
    // then would never learn about the reviewed version.
    if let Some(handle) = running {
        let mut value = handle.snapshot();
        value["status"] = json!("running");
        if let Some(entry) = entry {
            value["suggestions"] = json!(entry.suggestions);
        }
        return value;
    }
    match entry {
        Some(entry) => json!({
            "status": "ready",
            "fingerprint": entry.fingerprint,
            "suggestions": entry.suggestions,
            "reviewed": entry.reviewed,
            "computed_at": entry.computed_at,
            "duration_ms": entry.duration_ms,
        }),
        None => json!({ "status": "idle" }),
    }
}

/// Drops suggestions whose exact combination the user already dismissed. Done at read time rather
/// than before the cache write so one cached run serves both the filtered and the "show ignored"
/// view — the fingerprint already covers the ignored set, so the two options can never disagree
/// about *which* library state they describe.
pub(crate) fn filter_ignored(mut suggestions: Vec<Value>, ignored: &HashSet<String>) -> Vec<Value> {
    suggestions.retain(|suggestion| {
        let archive_ids: Vec<String> = suggestion
            .get("archive_ids")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let existing = suggestion
            .get("existing_tankoubon_id")
            .and_then(|v| v.as_str());
        let fingerprint =
            lanrurugi_storage::ignored_group_suggestions::fingerprint(&archive_ids, existing);
        !ignored.contains(&fingerprint)
    });
    suggestions
}

/// Runs one computation to completion. Returns `None` when superseded (cancelled) part-way.
async fn run_once(
    state: &AppState,
    hub: &Arc<GroupingHub>,
    handle: &Arc<RunHandle>,
    candidates: &CandidateSet,
) -> Result<Option<CachedSuggestions>, RunError> {
    let started = Instant::now();
    let settings = GroupingSettings::default();

    // 1. Everything already known: title, tags, and the persisted vector when it still matches the
    // title. Fetching an archive is one Redis read; the vector cache is another.
    let mut tags_by_id: HashMap<String, String> = HashMap::new();
    let mut titles_by_id: HashMap<String, String> = HashMap::new();
    let mut vectors_by_id: HashMap<String, Vec<f32>> = HashMap::new();
    let mut missing: Vec<(String, String)> = Vec::new();
    let mut queued: HashSet<String> = HashSet::new();

    handle.start_phase(Phase::Load, candidates.ungrouped_ids.len() as u64);
    hub.publish_status(handle);
    for (i, id) in candidates.ungrouped_ids.iter().enumerate() {
        if handle.is_cancelled() {
            return Ok(None);
        }
        let Ok(Some(archive)) = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
        else {
            continue;
        };
        tags_by_id.insert(id.clone(), archive.tags.clone());
        titles_by_id.insert(id.clone(), archive.title.clone());
        match state.recommend_cache.get_vector(id).await {
            Ok(Some((cached_title, vector))) if cached_title == archive.title => {
                vectors_by_id.insert(id.clone(), vector);
            }
            _ => {
                if queued.insert(id.clone()) {
                    missing.push((id.clone(), archive.title));
                }
            }
        }
        if i.is_multiple_of(64) {
            handle.set_progress(i as u64, candidates.ungrouped_ids.len() as u64);
            hub.publish_status(handle);
        }
    }

    // 2. Each existing Tankoubon's synthetic node is derived from its own members (average vector,
    // joined tags, own name as the title) rather than stored, so it tracks member edits for free.
    let mut tankoubon_nodes: Vec<(String, String, Vec<String>)> = Vec::new();
    for tankoubon in &candidates.tankoubons {
        if tankoubon.archives.is_empty() {
            // No members, no synthetic node — nothing meaningful to suggest adding to.
            continue;
        }
        let mut members = Vec::new();
        for member_id in &tankoubon.archives {
            let Ok(Some(archive)) = state.repos.archives.get(member_id).await else {
                continue;
            };
            let member = member_id.as_str().to_string();
            tags_by_id
                .entry(member.clone())
                .or_insert_with(|| archive.tags.clone());
            titles_by_id
                .entry(member.clone())
                .or_insert_with(|| archive.title.clone());
            match state.recommend_cache.get_vector(&member).await {
                Ok(Some((cached_title, vector))) if cached_title == archive.title => {
                    vectors_by_id.insert(member.clone(), vector);
                }
                _ => {
                    if queued.insert(member.clone()) {
                        missing.push((member.clone(), archive.title));
                    }
                }
            }
            members.push(member);
        }
        tankoubon_nodes.push((
            tankoubon.tankid.as_str().to_string(),
            tankoubon.name.clone(),
            members,
        ));
    }

    // 3. Embed whatever has no usable cached vector. Sequential on purpose: `embed()` is an IPC
    // round trip to the worker's single serialized session, so fanning out gains nothing.
    handle.start_phase(Phase::Embed, missing.len() as u64);
    hub.publish_status(handle);
    let embedder = state.recommender.embed_worker_client();
    for (i, (id, title)) in missing.iter().enumerate() {
        if handle.is_cancelled() {
            return Ok(None);
        }
        let normalized = lanrurugi_recommend::recommend::normalize_title(title);
        if let Ok(vector) = embedder.embed(&normalized).await {
            let _ = state.recommend_cache.put_vector(id, title, &vector).await;
            vectors_by_id.insert(id.clone(), vector);
        }
        handle.set_progress(i as u64 + 1, missing.len() as u64);
        if i.is_multiple_of(8) {
            hub.publish_status(handle);
        }
    }

    for (node_id, name, members) in &tankoubon_nodes {
        let mut member_tags: Vec<String> = Vec::new();
        let mut member_vectors: Vec<&Vec<f32>> = Vec::new();
        for member in members {
            if let Some(tags) = tags_by_id.get(member) {
                member_tags.push(tags.clone());
            }
            if let Some(vector) = vectors_by_id.get(member) {
                member_vectors.push(vector);
            }
        }
        let Some(first) = member_vectors.first() else {
            continue;
        };
        let dims = first.len();
        let mut averaged = vec![0.0f32; dims];
        for vector in &member_vectors {
            for (i, x) in vector.iter().enumerate() {
                averaged[i] += x;
            }
        }
        let count = member_vectors.len() as f32;
        for x in &mut averaged {
            *x /= count;
        }
        vectors_by_id.insert(node_id.clone(), averaged);
        tags_by_id.insert(node_id.clone(), member_tags.join(","));
        // The Tankoubon's name stands in for a title: only its token set is used, and a fresh
        // Tankoubon is named after its first member, so it carries the same creator/series hint.
        titles_by_id.insert(node_id.clone(), name.clone());
    }

    // Nothing could be vectorized at all — an environment problem (missing worker binary, failed
    // model download), which must not look like "this library has no same-series pairs".
    let scored = candidates
        .ungrouped_ids
        .iter()
        .filter(|id| vectors_by_id.contains_key(*id))
        .count();
    if candidates.ungrouped_ids.len() >= MIN_GROUP_SIZE && scored == 0 {
        tracing::warn!(
            candidates = candidates.ungrouped_ids.len(),
            "ai_group_suggestions: no archive could be embedded; the local worker or model is unavailable"
        );
        return Err(RunError {
            code: "embed_unavailable",
            message: "The local embedding worker or its model is unavailable, so titles cannot be compared.".to_string(),
        });
    }

    // 4. The one expensive pass, off the reactor and cancellable between rows.
    handle.start_phase(
        Phase::Group,
        candidates.ungrouped_ids.len() as u64 + tankoubon_nodes.len() as u64,
    );
    hub.publish_status(handle);
    let all_ids: Vec<String> = candidates
        .ungrouped_ids
        .iter()
        .cloned()
        .chain(tankoubon_nodes.into_iter().map(|(id, _, _)| id))
        .collect();
    let cancel_flag = handle.clone();
    let hub_for_task = hub.clone();
    let handle_for_task = handle.clone();
    let task_ids = all_ids.clone();
    // `Arc` rather than a clone: 7k vectors of 1024 f32 is ~28 MB, and copies of that would be
    // both slower and a needless second peak. The maps stay readable here afterwards.
    let vectors_shared = Arc::new(vectors_by_id);
    let tags_shared = Arc::new(tags_by_id);
    let titles_shared = Arc::new(titles_by_id);
    let settings_shared = Arc::new(settings.clone());
    let titles_for_task = Arc::clone(&titles_shared);
    let tags_for_task = Arc::clone(&tags_shared);
    let settings_for_task = Arc::clone(&settings_shared);
    let vectors_for_task = Arc::clone(&vectors_shared);
    let groups = tokio::task::spawn_blocking(move || {
        // The borrowed views must be built inside the closure: `spawn_blocking` needs `'static`,
        // and a `HashMap<&str, _>` borrowing the owned maps from outside cannot satisfy that.
        let vector_refs: HashMap<&str, &[f32]> = vectors_for_task
            .iter()
            .map(|(id, v)| (id.as_str(), v.as_slice()))
            .collect();
        let tag_refs: HashMap<&str, HashSet<&str>> = tags_for_task
            .iter()
            .map(|(id, t)| (id.as_str(), tag_set(t)))
            .collect();
        let token_refs: HashMap<&str, HashSet<&str>> = titles_for_task
            .iter()
            .map(|(id, t)| (id.as_str(), title_tokens_for(t, &settings_for_task)))
            .collect();
        let scorer = Scorer::new(&vector_refs, &tag_refs, &token_refs, &settings_for_task);
        let progress = move |done: u64, total: u64| {
            handle_for_task.set_progress(done, total);
            hub_for_task.publish_status(&handle_for_task);
        };
        group_with_hooked(
            &task_ids,
            &scorer,
            Some(&cancel_flag.cancelled),
            Some(&progress),
        )
        .into_iter()
        .map(|group| {
            let evidence = group_evidence(&group, &scorer);
            let hints = review_hints(&group, &evidence, &scorer);
            (group, evidence, hints)
        })
        .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| RunError {
        code: "internal",
        message: e.to_string(),
    })?;

    if handle.is_cancelled() {
        return Ok(None);
    }

    // 5. Group -> suggestion shape. At most one existing-Tankoubon node per group, by
    // `tankoubon_pair_gate`'s own guarantee.
    let suggestions: Vec<GroupSuggestion> = groups
        .into_iter()
        .map(|(group, evidence, hints)| {
            let mut existing_tankoubon_id = None;
            let mut archive_ids = Vec::with_capacity(group.len());
            for id in group {
                if super::is_existing_tankoubon_node(&id) {
                    existing_tankoubon_id = Some(id);
                } else {
                    archive_ids.push(id);
                }
            }
            let suggestion_id = lanrurugi_storage::ignored_group_suggestions::suggestion_id(
                &archive_ids,
                existing_tankoubon_id.as_deref(),
            );
            GroupSuggestion {
                archive_ids,
                existing_tankoubon_id,
                suggestion_id,
                evidence,
                review: None,
                hints,
            }
        })
        .collect();

    // 6. Publish the graph-only result before the (cheap, but network-bound) record write, so a
    // crash between the two still leaves something usable behind.
    handle.start_phase(Phase::Save, 1);
    hub.publish_status(handle);
    let computed_at = unix_now();
    let graph_only = CachedSuggestions {
        fingerprint: candidates.fingerprint.clone(),
        suggestions: suggestions
            .iter()
            .map(|s| serde_json::to_value(s).unwrap_or(Value::Null))
            .collect(),
        reviewed: false,
        computed_at,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    if let Err(e) = write_cache(state, &graph_only).await {
        tracing::warn!(error = %e, "grouping suggestions could not be cached");
    }
    hub.publish(json!({
        "kind": "partial",
        "run_id": handle.run_id,
        "suggestions": graph_only.suggestions,
        "reviewed": false,
        "computed_at": computed_at,
    }));

    let enriched =
        enrich_suggestions(state, suggestions, &titles_shared, &tags_shared, &settings).await;

    let entry = CachedSuggestions {
        fingerprint: candidates.fingerprint.clone(),
        suggestions: enriched
            .iter()
            .map(|s| serde_json::to_value(s).unwrap_or(Value::Null))
            .collect(),
        reviewed: true,
        computed_at,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    if let Err(e) = write_cache(state, &entry).await {
        tracing::warn!(error = %e, "grouping suggestions could not be cached");
    }
    Ok(Some(entry))
}

async fn write_cache(state: &AppState, entry: &CachedSuggestions) -> Result<(), String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    let _: Result<(), _> = conn.set(SUGGESTIONS_CACHE_KEY, payload).await;
    Ok(())
}

struct RunError {
    code: &'static str,
    message: String,
}

async fn execute(
    state: AppState,
    hub: Arc<GroupingHub>,
    handle: Arc<RunHandle>,
    candidates: CandidateSet,
) {
    let outcome = run_once(&state, &hub, &handle, &candidates).await;
    match outcome {
        Ok(Some(entry)) => hub.publish(json!({
            "kind": "done",
            "run_id": handle.run_id,
            "suggestions": entry.suggestions,
            "reviewed": entry.reviewed,
            "computed_at": entry.computed_at,
            "duration_ms": entry.duration_ms,
        })),
        Ok(None) => hub.publish(json!({ "kind": "cancelled", "run_id": handle.run_id })),
        Err(error) => {
            tracing::warn!(error = %error.message, "ai_group_suggestions run failed");
            hub.publish(json!({
                "kind": "error",
                "run_id": handle.run_id,
                "code": error.code,
                "message": error.message,
            }));
        }
    }
    hub.finish(&handle.run_id).await;
}

/// `GET /tankoubons/ai-group-suggestions/stream` — the drawer's one live connection: a `full`
/// snapshot first, then `status`/`partial`/`done`/`error` events. Subscribing before taking the
/// snapshot is deliberate, same as `download_queue::queue_stream`: an event published in the gap
/// would otherwise be missed forever.
pub(crate) async fn stream(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<super::AiGroupSuggestionsQuery>,
) -> Response {
    let rx = state.grouping.subscribe();
    let initial = Event::default()
        .event("full")
        .data(snapshot(&state, query.include_ignored).await.to_string());

    let state_for_lag = state.clone();
    let include_ignored = query.include_ignored;
    let deltas = BroadcastStream::new(rx).filter_map(move |result| {
        let state = state_for_lag.clone();
        async move {
            match result {
                Ok(event) => Some(Ok::<_, std::convert::Infallible>(
                    Event::default()
                        .event(event_name(&event))
                        .data(event.to_string()),
                )),
                // Fell behind the buffer: resend a full snapshot rather than silently skipping
                // progress the client would otherwise never see.
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                    Some(Ok(Event::default()
                        .event("full")
                        .data(snapshot(&state, include_ignored).await.to_string())))
                }
            }
        }
    });

    let stream = futures_util::stream::once(async move { Ok(initial) }).chain(deltas);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn event_name(event: &Value) -> &'static str {
    match event.get("kind").and_then(|k| k.as_str()) {
        Some("status") => "status",
        Some("partial") => "partial",
        Some("done") => "done",
        Some("error") => "error",
        Some("cancelled") => "cancelled",
        _ => "message",
    }
}

/// The current state, as `GET /result` reports it.
pub(crate) async fn snapshot(state: &AppState, include_ignored: bool) -> Value {
    let candidates = match candidate_set(state).await {
        Ok(c) => c,
        Err(e) => return json!({ "status": "error", "message": e }),
    };
    if candidates.nothing_to_suggest() {
        return json!({ "status": "ready", "suggestions": [] });
    }
    let entry = cached(state, &candidates.fingerprint).await;
    let running = state
        .grouping
        .current()
        .await
        .filter(|r| r.fingerprint == candidates.fingerprint && !r.is_cancelled());
    let mut value = result_json(entry.as_ref(), running.as_deref());
    // Filtering is done here rather than before the cache write, so one entry serves both the
    // default view and "show ignored combinations".
    if !include_ignored {
        if let Some(suggestions) = value.get_mut("suggestions").and_then(|s| s.as_array_mut()) {
            *suggestions = filter_ignored(std::mem::take(suggestions), &candidates.ignored);
        }
    }
    value
}
