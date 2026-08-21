//! Generic job-status tracking, reused by the backup, rebuild-index, and bench job endpoints
//! (US5/US6/US8) instead of each reimplementing its own job tracking. Answers the legacy
//! `/minion/{jobid}`-shaped polling contract (`contracts/rest-api.md`) without a separate Minion
//! process (constitution Principle III).

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Active,
    Finished,
    Failed,
}

impl JobState {
    /// Terminal states are ones a job never leaves again (`Finished`/`Failed`); `Queued`/`Active`
    /// are still in flight and must not be cleared or evicted (FR-004, research.md §2).
    fn is_terminal(self) -> bool {
        matches!(self, JobState::Finished | JobState::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: String,
    pub name: String,
    pub state: JobState,
    /// 0.0-1.0, best-effort; jobs that can't report granular progress just jump 0 -> 1.
    pub progress: f32,
    /// Bytes transferred so far for a real download job (`specs/005-download-plugin-progress/
    /// data-model.md`'s `Download Job` extension), updated incrementally as the Rust-side
    /// streaming download proceeds. `None` — genuinely absent from JSON, not a `0`/`null`
    /// sentinel (`skip_serializing_if` below) — until the download has actually started
    /// transferring bytes, and for every non-download job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    /// Total expected size for a real download job, taken from the response's `Content-Length`
    /// when present. `None` when the server doesn't report a size (spec FR-002) — the frontend
    /// renders an indeterminate indicator in that case rather than treating `None` as zero. For a
    /// multi-resource download, this is the sum across all resources once each one's size becomes
    /// known (spec FR-003: one combined indicator per job, not per-resource).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// The rate limit actually in effect for this download job — the resolved `max_bytes_per_sec`
    /// for the download's first resource's hostname (`download_manager::domain_rules::resolve`,
    /// spec FR-009/FR-016), in bytes/second. `None` (genuinely absent from JSON, not `null` —
    /// mirroring `downloaded_bytes`/`total_bytes`'s own contract) for every non-download job, for a
    /// download with no matching rate-limit rule, and until the download has actually started. A
    /// `0`/`null` would be ambiguous with "unlimited", so absence means unlimited. The frontend
    /// renders this as a highlighted badge + tooltip next to the progress bar (issue #2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_bytes_per_sec: Option<u64>,
    /// The matching domain-rule pattern that produced `rate_limit_bytes_per_sec` — an exact hostname
    /// (`"cdn.example.com"`), a wildcard (`"*.example.com"`), or `"*"` for the general fallback
    /// (i.e. `download_manager::domain_rules::resolved_key`'s return value). `None` for non-download
    /// jobs and before the download starts. Lets the frontend's rate-limit tooltip show *which* rule
    /// is in effect, and is `None` (not `"*"`) when the first resource's URL had no parseable
    /// hostname at all, to avoid a misleading catch-all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_matched_pattern: Option<String>,
    pub result: Option<serde_json::Value>,
    /// `true` only when `finish()` was handed a `result` whose serialized size exceeded
    /// `MAX_JOB_RESULT_BYTES` — `result` itself is `None` in that case (the oversized value is
    /// dropped, not truncated/partially kept, since a partial JSON value would need per-shape
    /// truncation logic no generic job-result contract can express safely). `#[serde(default)]`
    /// so an older client deserializing a response from a build that didn't have this field yet
    /// (or vice versa) doesn't choke on its absence. Absent (`false`) is the overwhelmingly common
    /// case for every job type today, so this isn't `skip_serializing_if`-hidden the way the
    /// optional download-progress fields above are — those are "doesn't apply to this job type" by
    /// design, this is "something unusual happened", worth always being present enough that a
    /// caller checking for it doesn't need `?? false`.
    #[serde(default)]
    pub result_truncated: bool,
    pub error: Option<String>,
}

impl JobStatus {
    fn new(id: String, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            state: JobState::Queued,
            progress: 0.0,
            downloaded_bytes: None,
            total_bytes: None,
            rate_limit_bytes_per_sec: None,
            rate_limit_matched_pattern: None,
            result: None,
            result_truncated: false,
            error: None,
        }
    }
}

/// Outcome of a single-job clear, distinguishing the three cases the `DELETE /jobs/{id}` contract
/// needs to map to distinct status codes (200/404/409) — a plain `bool` can't tell "unknown ID"
/// apart from "still running", so `clear()` returns this and the handler translates it (T013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearOutcome {
    /// Job existed, was terminal, and has been removed → HTTP 200.
    Cleared,
    /// No job with this ID is tracked → HTTP 404.
    NotFound,
    /// Job exists but is still `Queued`/`Active` (not safe to drop mid-flight) → HTTP 409.
    NotTerminal,
}

/// Upper bound on how many jobs a single process keeps tracked in memory (research.md §2 / FR-006).
/// Generous for coarse admin actions (backups, thumbnail regen, duplicate scans, index rebuilds,
/// plugin runs, URL downloads) — even frequent manual use wouldn't realistically approach this
/// within one uptime. Tuned as a constant, not a user-facing setting (spec Assumptions).
const MAX_TRACKED_JOBS: usize = 500;

/// Upper bound (serialized JSON bytes) on a single job's own `result` field (issue #67):
/// `MAX_TRACKED_JOBS` only ever constrained the *count* of tracked jobs, not how large any one
/// job's own `result: Option<serde_json::Value>` could grow — a job whose caller hands `finish()`
/// a genuinely large payload (e.g. a full duplicate-scan group listing, rather than just a count)
/// could otherwise sit resident in memory for as long as that job stays tracked, times up to
/// `MAX_TRACKED_JOBS` of them. 1 MiB is generous for what every current caller actually reports
/// (counts, IDs, short summaries — `duplicates.rs`'s own scan job deliberately reports only
/// `groups_found`, not the groups themselves, precisely to stay well under this) while still
/// catching a future caller that accidentally dumps an unbounded collection in here.
const MAX_JOB_RESULT_BYTES: usize = 1024 * 1024;

/// In-process job registry. One instance is shared (via `Arc`) across the Axum app state and
/// every background task that reports progress.
#[derive(Debug, Default, Clone)]
pub struct JobRegistry {
    jobs: Arc<RwLock<HashMap<String, JobStatus>>>,
    /// Job IDs in creation order, oldest first (index 0) → newest last. `HashMap` doesn't preserve
    /// insertion order, so this parallel vector is what gives `list_all()` a stable most-recent-
    /// first ordering and `create()`'s eviction a well-defined "oldest terminal job" to pick
    /// (research.md §2). Kept in lockstep with `jobs` under the same write lock acquisitions.
    order: Arc<RwLock<Vec<String>>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new job under a fresh ID and returns it so the caller can start work. Enforces
    /// the `MAX_TRACKED_JOBS` retention bound: once the registry holds that many entries, the
    /// oldest job currently in a terminal state is evicted before the new one is inserted; if no
    /// terminal job exists to evict (all slots in-flight), the new job is inserted anyway rather
    /// than dropping tracking for a still-running job (research.md §2).
    pub async fn create(&self, name: &str) -> String {
        let id = Uuid::new_v4().to_string();
        let status = JobStatus::new(id.clone(), name);

        // Acquire both write locks in a fixed order (`jobs` then `order`) everywhere they're taken
        // together, so no two methods can deadlock on each other.
        let mut jobs = self.jobs.write().await;
        let mut order = self.order.write().await;

        if jobs.len() >= MAX_TRACKED_JOBS {
            // Evict the oldest terminal-state job. Iterating `order` (oldest first) and picking the
            // first terminal hit is exactly "oldest currently-finished/failed job" (research.md §2).
            let victim = order
                .iter()
                .find(|oid| jobs.get(*oid).is_some_and(|j| j.state.is_terminal()))
                .cloned();
            if let Some(victim) = victim {
                jobs.remove(&victim);
                order.retain(|o| *o != victim);
            }
            // No terminal victim → every slot is in-flight; insert anyway and temporarily exceed the
            // cap rather than lose visibility into a running job.
        }

        order.push(id.clone());
        jobs.insert(id.clone(), status);
        id
    }

    pub async fn mark_active(&self, id: &str) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.state = JobState::Active;
        }
    }

    pub async fn set_progress(&self, id: &str, progress: f32) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.progress = progress.clamp(0.0, 1.0);
        }
    }

    /// Sibling to [`Self::set_progress`] for a real byte-level download (`specs/
    /// 005-download-plugin-progress`'s US1) — called by the download-manager on each streamed
    /// chunk (or at a throttled interval, to avoid excessive lock contention on very fast
    /// transfers). Also keeps the plain `progress: f32` fraction in sync when `total` is known, so
    /// any existing non-download-aware UI still gets a sane fraction rather than being stuck at
    /// its last value.
    pub async fn set_download_progress(&self, id: &str, downloaded: u64, total: Option<u64>) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.downloaded_bytes = Some(downloaded);
            job.total_bytes = total;
            if let Some(total) = total.filter(|&t| t > 0) {
                job.progress = (downloaded as f32 / total as f32).clamp(0.0, 1.0);
            }
        }
    }

    /// Sibling to [`Self::set_download_progress`] recording the resolved rate limit (and the
    /// domain-rule pattern it came from) for a download job, called once when the download starts
    /// (`download_manager`'s `run_managed_downloads`, snapshotting FR-016's limit-at-start-time so a
    /// mid-download settings change doesn't retroactively alter the displayed value). Both fields
    /// stay `None` for every non-download job; a download whose resolved rule declared no
    /// `max_bytes_per_sec` still records the matched `pattern` (e.g. `"*"` catch-all with no cap),
    /// leaving `bytes_per_sec` as `None` to mean "unlimited" — matching `RateLimiterMap::throttle`'s
    /// own "absent cap = full speed" convention. The two args are recorded verbatim rather than
    /// coupled so the frontend can decide what to render.
    pub async fn set_rate_limit(
        &self,
        id: &str,
        bytes_per_sec: Option<u64>,
        matched_pattern: Option<String>,
    ) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.rate_limit_bytes_per_sec = bytes_per_sec;
            job.rate_limit_matched_pattern = matched_pattern;
        }
    }

    pub async fn finish(&self, id: &str, result: serde_json::Value) {
        // `serde_json::to_vec` (not `.to_string().len()`) to measure the actual serialized byte
        // size a JSON string produces, not a UTF-8-oblivious approximation — matters for a result
        // containing non-ASCII archive titles/tags, which are byte-cheaper as UTF-8 but not 1:1
        // with `.to_string()`'s own char count either way; this is the same "count what actually
        // gets stored/transmitted" measurement whichever direction it errs, just correct instead
        // of approximate.
        let size = serde_json::to_vec(&result).map(|bytes| bytes.len());
        let (result, result_truncated) = match size {
            Ok(n) if n > MAX_JOB_RESULT_BYTES => {
                tracing::warn!(
                    job_id = id,
                    size_bytes = n,
                    limit_bytes = MAX_JOB_RESULT_BYTES,
                    "job result exceeds MAX_JOB_RESULT_BYTES, dropping it"
                );
                (None, true)
            }
            _ => (Some(result), false),
        };
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.state = JobState::Finished;
            job.progress = 1.0;
            job.result = result;
            job.result_truncated = result_truncated;
        }
    }

    pub async fn fail(&self, id: &str, error: impl Into<String>) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.state = JobState::Failed;
            job.error = Some(error.into());
        }
    }

    pub async fn get(&self, id: &str) -> Option<JobStatus> {
        self.jobs.read().await.get(id).cloned()
    }

    /// Every tracked job, most-recently-created first (reverse of the creation-order index). Used
    /// by the job console's `GET /jobs` (T003).
    pub async fn list_all(&self) -> Vec<JobStatus> {
        let jobs = self.jobs.read().await;
        let order = self.order.read().await;
        order
            .iter()
            .rev()
            .filter_map(|id| jobs.get(id).cloned())
            .collect()
    }

    /// Removes one job by ID. Only terminal (`Finished`/`Failed`) jobs may be cleared — in-flight
    /// jobs (`Queued`/`Active`) are left untouched (FR-004). The three-valued outcome lets the
    /// caller distinguish "not tracked" from "still running" (T011/T013).
    pub async fn clear(&self, id: &str) -> ClearOutcome {
        let mut jobs = self.jobs.write().await;
        let state = match jobs.get(id) {
            None => return ClearOutcome::NotFound,
            Some(job) => job.state,
        };
        if !state.is_terminal() {
            return ClearOutcome::NotTerminal;
        }
        jobs.remove(id);
        let mut order = self.order.write().await;
        order.retain(|o| o != id);
        ClearOutcome::Cleared
    }

    /// Removes every job currently in a terminal state, returning how many were cleared. In-flight
    /// (`Queued`/`Active`) jobs are left untouched and continue to be tracked (FR-004, T012).
    pub async fn clear_finished(&self) -> usize {
        let mut jobs = self.jobs.write().await;
        let mut order = self.order.write().await;

        let terminal_ids: Vec<String> = jobs
            .iter()
            .filter(|(_, job)| job.state.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        let count = terminal_ids.len();
        for id in &terminal_ids {
            jobs.remove(id);
        }
        order.retain(|id| !terminal_ids.contains(id));
        count
    }

    /// Every job currently tracked whose `name` matches, most-recently-created last (insertion
    /// order isn't preserved by `HashMap`, so callers needing FIFO order should sort by `id`'s
    /// embedded UUID creation time via a wrapping timestamp if that's ever needed; Phase 1's
    /// `/minion/{jobname}/queue` only needs "does a job with this name exist and what's its
    /// state", not strict ordering).
    pub async fn by_name(&self, name: &str) -> Vec<JobStatus> {
        self.jobs
            .read()
            .await
            .values()
            .filter(|j| j.name == name)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn download_progress_fields_are_absent_until_set() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        let job = reg.get(&id).await.unwrap();
        let value = serde_json::to_value(&job).unwrap();
        assert!(
            value.get("downloaded_bytes").is_none(),
            "downloaded_bytes must be genuinely absent from JSON, not null, until a download \
             actually starts transferring bytes — got: {value}"
        );
        assert!(value.get("total_bytes").is_none());
    }

    #[tokio::test]
    async fn set_download_progress_populates_both_fields_and_keeps_progress_in_sync() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        reg.set_download_progress(&id, 50, Some(200)).await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(job.downloaded_bytes, Some(50));
        assert_eq!(job.total_bytes, Some(200));
        assert_eq!(job.progress, 0.25, "plain progress fraction stays in sync");

        let value = serde_json::to_value(&job).unwrap();
        assert_eq!(value["downloaded_bytes"], json!(50));
        assert_eq!(value["total_bytes"], json!(200));
    }

    #[tokio::test]
    async fn set_download_progress_with_no_total_leaves_progress_fraction_alone() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        reg.set_download_progress(&id, 12345, None).await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(job.downloaded_bytes, Some(12345));
        assert_eq!(job.total_bytes, None);
        assert_eq!(
            job.progress, 0.0,
            "no total means no sane fraction to compute — plain progress is left untouched"
        );

        let value = serde_json::to_value(&job).unwrap();
        assert!(
            value.get("total_bytes").is_none(),
            "an unknown total must stay genuinely absent from JSON (indeterminate progress), \
             not null — got: {value}"
        );
    }

    #[tokio::test]
    async fn rate_limit_fields_are_absent_until_set() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        let job = reg.get(&id).await.unwrap();
        let value = serde_json::to_value(&job).unwrap();
        assert!(
            value.get("rate_limit_bytes_per_sec").is_none(),
            "rate_limit_bytes_per_sec must be genuinely absent from JSON, not null, until the \
             download starts — got: {value}"
        );
        assert!(
            value.get("rate_limit_matched_pattern").is_none(),
            "rate_limit_matched_pattern must be genuinely absent from JSON, not null, until the \
             download starts — got: {value}"
        );
    }

    #[tokio::test]
    async fn set_rate_limit_populates_both_fields() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        reg.set_rate_limit(&id, Some(1_048_576), Some("cdn.example.com".to_string()))
            .await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(job.rate_limit_bytes_per_sec, Some(1_048_576));
        assert_eq!(
            job.rate_limit_matched_pattern.as_deref(),
            Some("cdn.example.com")
        );

        let value = serde_json::to_value(&job).unwrap();
        assert_eq!(value["rate_limit_bytes_per_sec"], json!(1_048_576));
        assert_eq!(
            value["rate_limit_matched_pattern"],
            json!("cdn.example.com")
        );
    }

    #[tokio::test]
    async fn set_rate_limit_with_none_bytes_per_sec_still_records_pattern() {
        let reg = JobRegistry::new();
        let id = reg.create("download_url").await;
        // A `"*"` catch-all rule that declared no `max_bytes_per_sec` — the pattern is meaningful
        // ("this hostname matched the fallback rule") even though no cap is in effect.
        reg.set_rate_limit(&id, None, Some("*".to_string())).await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(job.rate_limit_bytes_per_sec, None);
        assert_eq!(job.rate_limit_matched_pattern.as_deref(), Some("*"));

        let value = serde_json::to_value(&job).unwrap();
        assert!(
            value.get("rate_limit_bytes_per_sec").is_none(),
            "an absent cap must stay genuinely absent from JSON (unlimited), not null — got: {value}"
        );
        assert_eq!(value["rate_limit_matched_pattern"], json!("*"));
    }

    #[tokio::test]
    async fn non_download_job_never_has_rate_limit_fields() {
        let reg = JobRegistry::new();
        let id = reg.create("backup").await;
        reg.mark_active(&id).await;
        reg.finish(&id, json!({})).await;
        let job = reg.get(&id).await.unwrap();
        let value = serde_json::to_value(&job).unwrap();
        assert!(value.get("rate_limit_bytes_per_sec").is_none());
        assert!(value.get("rate_limit_matched_pattern").is_none());
    }

    #[tokio::test]
    async fn list_all_returns_most_recently_created_first() {
        let reg = JobRegistry::new();
        let a = reg.create("backup").await;
        let b = reg.create("restore").await;
        let c = reg.create("rebuild_index").await;

        let all: Vec<String> = reg.list_all().await.into_iter().map(|j| j.id).collect();
        assert_eq!(all, vec![c, b, a], "most-recently-created comes first");
    }

    #[tokio::test]
    async fn clear_removes_terminal_job_and_reports_cleared() {
        let reg = JobRegistry::new();
        let id = reg.create("backup").await;
        reg.finish(&id, json!({ "ok": 1 })).await;

        assert_eq!(reg.clear(&id).await, ClearOutcome::Cleared);
        assert!(reg.get(&id).await.is_none());
    }

    #[tokio::test]
    async fn clear_reports_not_found_for_unknown_id() {
        let reg = JobRegistry::new();
        assert_eq!(reg.clear("does-not-exist").await, ClearOutcome::NotFound);
    }

    #[tokio::test]
    async fn clear_rejects_queued_and_active_jobs_leaving_them_in_place() {
        let reg = JobRegistry::new();
        // Queued (freshly created).
        let queued = reg.create("backup").await;
        assert_eq!(reg.clear(&queued).await, ClearOutcome::NotTerminal);
        assert!(
            reg.get(&queued).await.is_some(),
            "queued job must remain tracked"
        );

        // Active.
        let active = reg.create("backup").await;
        reg.mark_active(&active).await;
        assert_eq!(reg.clear(&active).await, ClearOutcome::NotTerminal);
        assert!(
            reg.get(&active).await.is_some(),
            "active job must remain tracked"
        );
    }

    #[tokio::test]
    async fn clear_finished_removes_only_terminal_jobs_and_counts_them() {
        let reg = JobRegistry::new();
        let finished = reg.create("backup").await;
        reg.finish(&finished, json!({})).await;
        let failed = reg.create("backup").await;
        reg.fail(&failed, "boom").await;
        let active = reg.create("backup").await;
        reg.mark_active(&active).await;
        let queued = reg.create("backup").await;

        let removed = reg.clear_finished().await;
        assert_eq!(removed, 2, "only the finished + failed jobs are cleared");
        assert!(reg.get(&finished).await.is_none());
        assert!(reg.get(&failed).await.is_none());
        assert!(reg.get(&active).await.is_some(), "active job untouched");
        assert!(reg.get(&queued).await.is_some(), "queued job untouched");
    }

    #[tokio::test]
    async fn create_evicts_oldest_terminal_job_when_cap_reached() {
        let reg = JobRegistry::new();
        // Fill to the cap; the very first job is the oldest and is finished (terminal).
        let oldest = reg.create("backup").await;
        reg.finish(&oldest, json!({})).await;
        for _ in 0..(MAX_TRACKED_JOBS - 1) {
            let id = reg.create("backup").await;
            reg.finish(&id, json!({})).await;
        }
        assert_eq!(
            reg.list_all().await.len(),
            MAX_TRACKED_JOBS,
            "registry holds exactly the cap before overflow"
        );

        // One more create must evict the oldest terminal job (`oldest`) to make room.
        let newcomer = reg.create("backup").await;
        let all = reg.list_all().await;
        assert_eq!(
            all.len(),
            MAX_TRACKED_JOBS,
            "cap is maintained after eviction"
        );
        assert!(
            reg.get(&oldest).await.is_none(),
            "the oldest terminal job is the one evicted"
        );
        assert!(reg.get(&newcomer).await.is_some(), "the new job is present");
    }

    #[tokio::test]
    async fn create_does_not_evict_in_flight_job_to_make_room() {
        let reg = JobRegistry::new();
        // Fill to the cap with all-Active jobs — nothing is terminal, so there's nothing safe to
        // evict (research.md §2's "practically unreachable" all-in-flight case).
        for _ in 0..MAX_TRACKED_JOBS {
            let id = reg.create("backup").await;
            reg.mark_active(&id).await;
        }
        assert_eq!(reg.list_all().await.len(), MAX_TRACKED_JOBS);

        let extra = reg.create("backup").await;
        assert!(
            reg.get(&extra).await.is_some(),
            "new job is still inserted even with nothing to evict"
        );
        assert_eq!(
            reg.list_all().await.len(),
            MAX_TRACKED_JOBS + 1,
            "registry temporarily exceeds the cap rather than dropping an in-flight job"
        );
    }

    #[tokio::test]
    async fn finish_keeps_a_small_result_intact() {
        let reg = JobRegistry::new();
        let id = reg.create("find_duplicates").await;
        reg.finish(&id, json!({ "groups_found": 3 })).await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(job.result, Some(json!({ "groups_found": 3 })));
        assert!(!job.result_truncated);
    }

    #[tokio::test]
    async fn finish_drops_a_result_over_the_size_cap_and_flags_it_truncated() {
        let reg = JobRegistry::new();
        let id = reg.create("verify").await;
        // One giant string comfortably over MAX_JOB_RESULT_BYTES once JSON-serialized (quotes +
        // escaping overhead aside, this alone already exceeds it).
        let oversized = "x".repeat(MAX_JOB_RESULT_BYTES + 1);
        reg.finish(&id, json!({ "suspect_groups": oversized }))
            .await;

        let job = reg.get(&id).await.unwrap();
        assert_eq!(
            job.result, None,
            "oversized result must be dropped, not partially kept"
        );
        assert!(job.result_truncated);
        assert_eq!(
            job.state,
            JobState::Finished,
            "the job itself still finished successfully"
        );
    }

    #[test]
    fn result_truncated_defaults_to_false_when_absent_from_json() {
        // A response from a build predating this field (or a hand-constructed JSON blob in a
        // test/fixture) must still deserialize — `#[serde(default)]` is what makes that safe.
        let value = json!({
            "id": "abc",
            "name": "backup",
            "state": "finished",
            "progress": 1.0,
            "result": null,
            "error": null,
        });
        let job: JobStatus = serde_json::from_value(value).unwrap();
        assert!(!job.result_truncated);
    }
}
