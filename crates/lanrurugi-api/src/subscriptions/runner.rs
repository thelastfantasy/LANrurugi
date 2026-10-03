//! Running one check cycle end to end.
//!
//! The order here is deliberate and worth reading before changing anything:
//!
//! 1. Ask the source what it has.
//! 2. Decide whether that answer is authoritative (it is not, if the sign-in was missing).
//! 3. Filter, dedup, and act.
//! 4. **Only then**, and only for an authoritative cycle, record what was seen.
//!
//! Step 4's condition is the single most consequential line in this module. A degraded check saw a
//! smaller world than the one asked about; recording its candidates as "seen" would mark every work
//! it had no access to as handled, hiding them permanently with no error anywhere to reveal it.

use lanrurugi_plugin::protocol::{DiscoveredCandidate, DiscoveryResult};
use lanrurugi_storage::subscriptions::{
    CandidateRecord, CandidateVerdict, CheckCycle, CycleOutcome, Subscription,
};

use super::matcher;

/// What one cycle decided, before any of it is persisted. Separated from the persistence step so the
/// decision logic can be tested without Redis.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleDecision {
    pub outcome: CycleOutcome,
    pub records: Vec<CandidateRecord>,
    /// Sources to queue or present for approval, in listing order.
    pub actionable: Vec<String>,
    /// Sources that may be recorded as seen. **Empty whenever the cycle is not authoritative** —
    /// see this module's header.
    pub to_mark_seen: Vec<String>,
}

/// Context the decision needs from the outside world, gathered before deciding so the decision
/// itself stays pure.
pub struct CycleContext<'a> {
    /// Sources this subscription has already handled in an earlier conclusive cycle.
    pub already_seen: &'a [String],
    /// Sources already held in the library, normalised the same way candidates are.
    pub already_held: &'a [String],
    /// Sources the user discarded from the reservation list — not to be re-reserved (FR-016).
    pub discarded: &'a [String],
    /// Library categories per already-held source, for the excluded-category rule.
    pub categories_for: &'a dyn Fn(&str) -> Vec<String>,
}

/// Turns a discovery result into a decision. Pure: no I/O, no clock, no Redis.
pub fn decide(
    subscription: &Subscription,
    result: &DiscoveryResult,
    ctx: &CycleContext<'_>,
) -> CycleDecision {
    // A source that could not be read at all tells us nothing about what exists.
    if let Some(err) = &result.error {
        return CycleDecision {
            outcome: CycleOutcome::Failed {
                reason: err.error_code.clone(),
            },
            records: Vec::new(),
            actionable: Vec::new(),
            to_mark_seen: Vec::new(),
        };
    }

    let candidates: &[DiscoveredCandidate] = result.candidates.as_deref().unwrap_or(&[]);
    let mut records = Vec::with_capacity(candidates.len());
    let mut actionable = Vec::new();
    let mut seen_candidates = Vec::new();

    for candidate in candidates {
        let source = candidate.source.clone();
        seen_candidates.push(source.clone());

        let verdict = if ctx.already_held.iter().any(|h| h == &source) {
            CandidateVerdict::AlreadyHeld
        } else if ctx.already_seen.iter().any(|s| s == &source)
            || ctx.discarded.iter().any(|d| d == &source)
        {
            // A discarded work counts as settled: the user already said no. Re-offering it every
            // cycle would make the discard meaningless.
            CandidateVerdict::AlreadySeen
        } else {
            let categories = (ctx.categories_for)(&source);
            match matcher::evaluate(candidate, &subscription.filters, &categories) {
                Some(rule) => CandidateVerdict::Rejected {
                    rule: rule.as_key(),
                },
                None => {
                    actionable.push(source.clone());
                    if subscription.auto_download {
                        CandidateVerdict::Queued
                    } else {
                        CandidateVerdict::AwaitingApproval
                    }
                }
            }
        };

        records.push(CandidateRecord {
            source_url: source,
            title: candidate.title.clone(),
            verdict,
        });
    }

    // The guard. A degraded listing is a real answer to a smaller question — useful enough to act on
    // what it *did* return, never complete enough to conclude anything about what it did not.
    let outcome = if result.degraded {
        CycleOutcome::Inconclusive {
            reason: result
                .error
                .as_ref()
                .map(|e| e.error_code.clone())
                .unwrap_or_else(|| "not signed in".to_string()),
        }
    } else {
        CycleOutcome::Completed
    };

    let to_mark_seen = if outcome.is_authoritative() {
        seen_candidates
    } else {
        Vec::new()
    };

    CycleDecision {
        outcome,
        records,
        actionable,
        to_mark_seen,
    }
}

/// Assembles the persisted record of a cycle.
pub fn build_cycle(
    id: String,
    subscription_id: String,
    started_at: i64,
    finished_at: i64,
    decision: &CycleDecision,
) -> CheckCycle {
    CheckCycle {
        id,
        subscription_id,
        started_at,
        finished_at,
        outcome: decision.outcome.clone(),
        candidates_seen: decision.records.len() as u32,
        candidates: decision.records.clone(),
    }
}

// ── Running a real check ────────────────────────────────────────────────────────────────────────

use crate::AppState;
use lanrurugi_storage::download_queue::{NewQueueItem, QueueItemOrigin};
use lanrurugi_storage::subscriptions::{ReservationEntry, ReservationStatus, SubscriptionState};

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Runs one check for one subscription: ask the source, decide, act, record.
///
/// Best-effort throughout. A subscription that cannot be checked right now is checked again next
/// time; nothing here is allowed to abort the scheduler or leave the subscription wedged.
pub async fn run_check(state: &AppState, subscription_id: &str) {
    let Ok(Some(mut subscription)) = state.subscriptions.get(subscription_id).await else {
        return;
    };

    let started_at = now_secs();

    // Discovery runs with whatever signed-in state the source declared it needs, exactly like a
    // metadata or download call — subscriptions introduce no second credential path.
    let args = match state.plugins.plugin_info(&subscription.source).await {
        Ok(info) => {
            let base = serde_json::to_value(&subscription.criteria).unwrap_or_default();
            crate::plugins::with_login_cookies(state, &info, base).await
        }
        Err(e) => {
            tracing::warn!(%subscription_id, error = %e, "subscription source is no longer installed");
            return;
        }
    };

    let result = match state.plugins.discover(&subscription.source, args).await {
        Ok(Some(result)) => result,
        Ok(None) => {
            tracing::warn!(
                %subscription_id,
                source = %subscription.source,
                "source no longer offers discovery; skipping this check"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(%subscription_id, error = %e, "discovery call failed");
            return;
        }
    };

    // Gather what the decision needs, then decide purely.
    let seen = state
        .subscriptions
        .seen_all(subscription_id)
        .await
        .unwrap_or_default();
    let reservations = state
        .subscriptions
        .list_reservations()
        .await
        .unwrap_or_default();
    let settled = super::reservations::settled_sources(&reservations, subscription_id);
    let held = held_sources(state).await;

    let decision = {
        let no_categories = |_: &str| Vec::<String>::new();
        let ctx = CycleContext {
            already_seen: &seen,
            already_held: &held,
            discarded: &settled,
            categories_for: &no_categories,
        };
        decide(&subscription, &result, &ctx)
    };

    // Queue (or hold for approval) whatever survived.
    for source in &decision.actionable {
        if !subscription.auto_download {
            // Nothing is spent until the user approves. The candidate's verdict already records it
            // as awaiting approval, which is what the UI reads.
            continue;
        }
        let queued = state
            .download_queue
            .add(NewQueueItem {
                origin: QueueItemOrigin::Download,
                url: source.clone(),
                plugin_namespace: subscription.source.clone(),
                file_size: None,
                category: subscription.target_category.clone(),
                auto_fetch_metadata: subscription.enrich_metadata,
                overwrite_on_duplicate: false,
                // Enters the queue the same way a manually added URL does, so it inherits the
                // existing start/stop/retry behaviour rather than getting a path of its own.
                state: lanrurugi_storage::download_queue::DownloadQueueState::Queued,
            })
            .await;
        if let Err(e) = queued {
            // Could not even enqueue — reserve it so the work is not silently lost.
            let entry = ReservationEntry {
                id: uuid::Uuid::new_v4().to_string(),
                subscription_id: subscription_id.to_string(),
                source_url: source.clone(),
                reason: super::reservations::ReservationReason::DownloadFailed(e.to_string())
                    .as_key(),
                status: ReservationStatus::Waiting,
                created_at: now_secs(),
            };
            let _ = state.subscriptions.save_reservation(&entry).await;
        }
    }

    // The guard, enforced at the one place it matters: only an authoritative cycle may record works
    // as seen. `decide` already returns an empty list otherwise, so this is belt and braces.
    if decision.outcome.is_authoritative() && !decision.to_mark_seen.is_empty() {
        let _ = state
            .subscriptions
            .mark_seen(subscription_id, &decision.to_mark_seen)
            .await;
    }

    let cycle = build_cycle(
        uuid::Uuid::new_v4().to_string(),
        subscription_id.to_string(),
        started_at,
        now_secs(),
        &decision,
    );
    let _ = state.subscriptions.record_cycle(&cycle).await;

    // `last_checked_at` advances even for a degraded or failed cycle: the source *was* contacted,
    // and retrying immediately would hammer a source that is already struggling. The next scheduled
    // check will try again.
    subscription.last_checked_at = Some(now_secs());
    let _ = state.subscriptions.save(&subscription).await;

    let _ = state
        .subscriptions
        .prune_reservations(super::scheduler::RESERVATION_LIMIT)
        .await;
}

/// Normalised source URLs already in the library, so a work held under any of them is not offered
/// again.
async fn held_sources(state: &AppState) -> Vec<String> {
    let Ok(archives) = state.repos.archives.list_all().await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for archive in archives {
        for tag in archive.tags.split(',') {
            if let Some(source) = tag.trim().strip_prefix("source:") {
                out.push(crate::plugins::trim_url(source));
            }
        }
    }
    out
}

/// The scheduler loop: wake periodically, check whatever is due.
///
/// One task owns every start, which is what keeps FR-012's non-overlap a local question. A
/// subscription still mid-check is simply skipped this tick rather than coordinated with.
pub async fn scheduler_loop(state: AppState) {
    let mut ticker =
        tokio::time::interval(std::time::Duration::from_secs(super::scheduler::TICK_SECS));
    loop {
        ticker.tick().await;
        let Ok(subscriptions) = state.subscriptions.list_all().await else {
            continue;
        };
        let now = now_secs();
        for subscription in subscriptions {
            if subscription.state != SubscriptionState::Enabled || !subscription.is_due(now) {
                continue;
            }
            let Some(guard) = state.subscriptions_in_flight.claim(&subscription.id).await else {
                // Still running from a previous tick. Normal on a slow source, not a fault.
                continue;
            };
            let state = state.clone();
            let id = subscription.id.clone();
            tokio::spawn(async move {
                run_check(&state, &id).await;
                drop(guard);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::subscriptions::{CreditPolicy, Criteria, Filters, SubscriptionState};

    fn sub(auto: bool, filters: Filters) -> Subscription {
        Subscription {
            id: "s1".into(),
            name: "n".into(),
            source: "discovery/example".into(),
            criteria: Criteria::default(),
            filters,
            interval_secs: 3600,
            target_category: None,
            enrich_metadata: true,
            auto_download: auto,
            credit_policy: CreditPolicy::default(),
            state: SubscriptionState::Enabled,
            last_checked_at: None,
            created_at: 0,
        }
    }

    fn cand(source: &str, tags: &[&str]) -> DiscoveredCandidate {
        DiscoveredCandidate {
            source: source.into(),
            title: None,
            posted_at: None,
            rating: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn result(candidates: Vec<DiscoveredCandidate>, degraded: bool) -> DiscoveryResult {
        DiscoveryResult {
            candidates: Some(candidates),
            degraded,
            error: None,
        }
    }

    fn ctx<'a>(
        seen: &'a [String],
        held: &'a [String],
        discarded: &'a [String],
        cats: &'a dyn Fn(&str) -> Vec<String>,
    ) -> CycleContext<'a> {
        CycleContext {
            already_seen: seen,
            already_held: held,
            discarded,
            categories_for: cats,
        }
    }

    const NO_CATS: &dyn Fn(&str) -> Vec<String> = &|_: &str| Vec::new();

    #[test]
    fn a_clean_candidate_becomes_actionable() {
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("a", &[])], false),
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert_eq!(d.actionable, vec!["a".to_string()]);
        assert_eq!(d.records[0].verdict, CandidateVerdict::Queued);
    }

    /// FR-007a's default: without auto-download, nothing is queued — it waits for approval, and so
    /// spends no credit.
    #[test]
    fn without_auto_download_candidates_await_approval_instead_of_queueing() {
        let d = decide(
            &sub(false, Filters::default()),
            &result(vec![cand("a", &[])], false),
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::AwaitingApproval);
        assert_eq!(
            d.actionable,
            vec!["a".to_string()],
            "still actionable, just not yet spent"
        );
    }

    #[test]
    fn an_already_held_work_is_not_offered_again() {
        let held = vec!["a".to_string()];
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("a", &[])], false),
            &ctx(&[], &held, &[], NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::AlreadyHeld);
        assert!(d.actionable.is_empty());
    }

    #[test]
    fn a_discarded_work_is_not_re_offered() {
        let discarded = vec!["a".to_string()];
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("a", &[])], false),
            &ctx(&[], &[], &discarded, NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::AlreadySeen);
        assert!(
            d.actionable.is_empty(),
            "the user already said no to this one"
        );
    }

    #[test]
    fn a_rejected_candidate_carries_the_rule_that_rejected_it() {
        let f = Filters {
            excluded_tags: vec!["bad".into()],
            ..Default::default()
        };
        let d = decide(
            &sub(true, f),
            &result(vec![cand("a", &["bad"])], false),
            &ctx(&[], &[], &[], NO_CATS),
        );
        match &d.records[0].verdict {
            CandidateVerdict::Rejected { rule } => assert!(rule.contains("excluded_tag")),
            other => panic!("expected a rejection naming its rule, got {other:?}"),
        }
    }

    /// **The guard.** A degraded cycle may act on what it saw, but must never record anything as
    /// seen — otherwise works it had no access to are marked handled and hidden forever.
    #[test]
    fn a_degraded_cycle_marks_nothing_as_seen() {
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("a", &[]), cand("b", &[])], true),
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert!(
            matches!(d.outcome, CycleOutcome::Inconclusive { .. }),
            "a degraded listing is inconclusive, not completed"
        );
        assert!(
            d.to_mark_seen.is_empty(),
            "recording these as seen would permanently hide every work this check could not access"
        );
        assert_eq!(d.actionable.len(), 2, "what it did see is still actionable");
    }

    #[test]
    fn a_conclusive_cycle_marks_everything_it_saw() {
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("a", &[]), cand("b", &[])], false),
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert_eq!(d.outcome, CycleOutcome::Completed);
        assert_eq!(d.to_mark_seen.len(), 2);
    }

    #[test]
    fn a_failed_discovery_decides_nothing_at_all() {
        let failed = DiscoveryResult {
            candidates: None,
            degraded: false,
            error: Some(lanrurugi_plugin::protocol::PluginError {
                error_code: "source unreachable".into(),
                data: None,
            }),
        };
        let d = decide(
            &sub(true, Filters::default()),
            &failed,
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert!(matches!(d.outcome, CycleOutcome::Failed { .. }));
        assert!(d.to_mark_seen.is_empty());
        assert!(d.actionable.is_empty());
    }
}
