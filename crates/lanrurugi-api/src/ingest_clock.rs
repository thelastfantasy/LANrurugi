//! Carries a download's own identity — its job and its queue row — into code that has neither.
//!
//! The download task owns the `job_id`, but the ingest half of a download runs through
//! `download_manager`'s catalogue/rescan path, which is shared with subscriptions and uploads and
//! takes no job id. Threading one through every one of those signature would make an unrelated
//! caller carry a download-only concept. A task-local clock is the same trick
//! [`crate::trace_context`] uses: the download task installs it around its own body, and any
//! `mark` in that task's call tree lands on the right job. Code running outside a clock (every
//! other ingest path) gets a no-op.

use std::future::Future;

use lanrurugi_core::jobs::JobRegistry;

tokio::task_local! {
    static CLOCK: Clock;
}

#[derive(Clone)]
struct Clock {
    jobs: JobRegistry,
    job_id: String,
    /// The queue row this download belongs to, when there is one. Cataloguing uses it to read the
    /// metadata preview taken at start time; the `/download_url` path has no row, hence `None`.
    queue_item_id: Option<String>,
}

/// Runs `future` with `job_id` as the current task's phase clock. Nested calls replace the clock
/// for the inner future only, which is what makes a re-entrant download (a subscription's own
/// download calling into `ingest`) attribute to its own job rather than the outer one.
pub async fn with_clock<F: Future>(
    future: F,
    jobs: &JobRegistry,
    job_id: &str,
    queue_item_id: Option<String>,
) -> F::Output {
    CLOCK
        .scope(
            Clock {
                jobs: jobs.clone(),
                job_id: job_id.to_string(),
                queue_item_id,
            },
            future,
        )
        .await
}

/// The queue row this task's download belongs to, if it has one and this task installed a clock.
pub fn queue_item_id() -> Option<String> {
    CLOCK
        .try_with(|clock| clock.queue_item_id.clone())
        .ok()
        .flatten()
}

/// Closes the current phase and opens `name` — a boundary means "the previous phase ended exactly
/// now", so this is one call rather than an `end` the next phase's `begin` might race with. A no-op
/// when this task has no clock (see the module docs).
pub async fn mark(name: &str) {
    let Ok(clock) = CLOCK.try_with(Clone::clone) else {
        return;
    };
    clock.jobs.end_phase(&clock.job_id).await;
    clock.jobs.begin_phase(&clock.job_id, name).await;
    // The trace is keyed by the same job id, so a boundary marks both timelines at once and the
    // two cannot drift apart (029 contract T2).
    crate::diagnostics::download_trace::phase(&clock.job_id, name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::jobs::{PHASE_INGEST, PHASE_PUBLISH};

    #[tokio::test]
    async fn mark_outside_a_clock_is_a_no_op() {
        // No panic, no job created — every other ingest path runs through here.
        mark(PHASE_PUBLISH).await;
    }

    #[tokio::test]
    async fn mark_lands_on_the_installed_jobs_timeline() {
        let jobs = JobRegistry::new();
        let id = jobs.create("download_url").await;
        jobs.mark_active(&id).await;
        jobs.begin_phase(&id, PHASE_INGEST).await;

        with_clock(mark(PHASE_PUBLISH), &jobs, &id, None).await;

        let phases = jobs.get(&id).await.unwrap().phases;
        let names: Vec<&str> = phases.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec![PHASE_INGEST, PHASE_PUBLISH]);
        assert_eq!(phases[0].ended_at, Some(phases[1].started_at));
        assert_eq!(phases[1].ended_at, None, "publish ends when the job does");
    }

    #[tokio::test]
    async fn a_nested_clock_attributes_to_the_inner_job() {
        let jobs = JobRegistry::new();
        let outer = jobs.create("download_url").await;
        let inner = jobs.create("download_url").await;
        jobs.begin_phase(&outer, PHASE_INGEST).await;
        jobs.begin_phase(&inner, PHASE_INGEST).await;

        with_clock(
            async {
                mark(PHASE_PUBLISH).await;
                with_clock(
                    mark(PHASE_PUBLISH),
                    &jobs,
                    &inner,
                    Some("q-inner".to_string()),
                )
                .await;
            },
            &jobs,
            &outer,
            None,
        )
        .await;

        let outer_names: Vec<String> = jobs
            .get(&outer)
            .await
            .unwrap()
            .phases
            .into_iter()
            .map(|p| p.name)
            .collect();
        let inner_names: Vec<String> = jobs
            .get(&inner)
            .await
            .unwrap()
            .phases
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(outer_names, vec![PHASE_INGEST, PHASE_PUBLISH]);
        assert_eq!(
            inner_names,
            vec![PHASE_INGEST, PHASE_PUBLISH],
            "the inner download's own ingest must not be reported on the outer job"
        );
    }
}
