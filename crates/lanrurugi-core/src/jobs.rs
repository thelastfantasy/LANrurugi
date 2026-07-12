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
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl JobStatus {
    fn new(id: String, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            state: JobState::Queued,
            progress: 0.0,
            result: None,
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

    pub async fn finish(&self, id: &str, result: serde_json::Value) {
        if let Some(job) = self.jobs.write().await.get_mut(id) {
            job.state = JobState::Finished;
            job.progress = 1.0;
            job.result = Some(result);
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
}
