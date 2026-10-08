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
    /// Sources this cycle rejected by a rule, which a later cycle may therefore consider again (their
    /// rating/tags can change). Empty for a non-authoritative cycle, like `to_mark_seen`.
    pub to_mark_rejected: Vec<String>,
    /// Sources that had been rejected earlier and are now handled (or held) — their reconsideration
    /// record is done with.
    pub reconsidered: Vec<String>,
}

/// Context the decision needs from the outside world, gathered before deciding so the decision
/// itself stays pure.
pub struct CycleContext<'a> {
    /// Sources this subscription has already handled in an earlier conclusive cycle.
    pub already_seen: &'a [String],
    /// The subset of `already_seen` that was turned away by a rule rather than handled, and is
    /// therefore worth re-deciding: a rating that was too low rises, a missing tag gets added. Kept
    /// separate, because re-deciding a work the user already answered (downloaded, approved,
    /// discarded) would re-offer what they have settled.
    pub reconsider: &'a [String],
    /// Answers "is this candidate an *older* revision of a work the library already holds, and if so
    /// which one is newer" — read from the revision chains stored with those archives, so it costs no
    /// requests. Supplied rather than computed here to keep `decide` free of state.
    ///
    /// This is what stops a listing's A/B/C parent-child trio from queueing three downloads: only the
    /// newest is offered, and the other two are recorded as settled rather than downloaded and then
    /// refused by the download stage.
    pub newer_held_revision: &'a dyn Fn(&str) -> Option<(String, String)>,
    /// The archive a held source belongs to, for `AlreadyHeld`'s own carried id.
    pub held_archive: &'a dyn Fn(&str) -> Option<String>,
    /// Sources already held in the library, normalised the same way candidates are.
    pub already_held: &'a [String],
    /// Sources the user discarded from the reservation list — not to be re-reserved (FR-016).
    pub discarded: &'a [String],
    /// Library categories per already-held source, for the excluded-category rule.
    pub categories_for: &'a dyn Fn(&str) -> Vec<String>,
    /// Unix seconds, supplied rather than read so `decide` stays clock-free and testable.
    pub now: i64,
    /// Normalises a tag before comparison, so a rule written in one script or width still matches a
    /// listing that spells the same tag differently. Supplied rather than built here to keep `decide`
    /// free of state.
    pub fold: &'a dyn Fn(&str) -> String,
}

/// Builds the "older revision of something held?" lookup from the chains stored with the library's
/// archives.
///
/// Pure once the inputs are gathered, which is what lets `decide` stay free of Redis: the chains and
/// the canonical sources of everything held are read once per cycle, then answering a candidate is a
/// scan over a handful of lists.
pub fn revision_lookup(
    held: &[(String, String)],
    chains: Vec<Vec<(String, String)>>,
) -> impl Fn(&str) -> Option<(String, String)> + '_ {
    move |candidate: &str| {
        let chain = chains
            .iter()
            .find(|c| c.iter().any(|(source, _)| source == candidate))?;
        // Held members of the same family, newest first by the only field that orders a chain.
        let mut members: Vec<(&str, &str, &str)> = chain
            .iter()
            .filter_map(|(source, posted)| {
                held.iter()
                    .find(|(h, _)| h == source)
                    .map(|(_, archive_id)| (source.as_str(), posted.as_str(), archive_id.as_str()))
            })
            .collect();
        if members.is_empty() {
            return None;
        }
        members.sort_by(|a, b| b.1.cmp(a.1));
        let (newest_source, newest_posted, newest_archive) = members[0];
        let candidate_posted = chain
            .iter()
            .find(|(source, _)| source == candidate)
            .map(|(_, posted)| posted.as_str())
            .unwrap_or_default();
        // ISO 8601 compares lexicographically, which is why the plugin reports it in that shape.
        (candidate_posted < newest_posted)
            .then(|| (newest_source.to_string(), newest_archive.to_string()))
    }
}

/// How many listing pages a check may read.
///
/// Four rather than one because a work edited to carry a subscription's tag enters the result set at
/// its original publication date — mid-list, never on page one. Four rather than unbounded because a
/// tag search can run to tens of thousands of works, and reading all of it on every check is both slow
/// and a good way to be rate-limited.
const MAX_LISTING_PAGES: u32 = 4;

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
            to_mark_rejected: Vec::new(),
            reconsidered: Vec::new(),
        };
    }

    let candidates: &[DiscoveredCandidate] = result.candidates.as_deref().unwrap_or(&[]);
    let mut records = Vec::with_capacity(candidates.len());
    let mut actionable = Vec::new();
    let mut seen_candidates = Vec::new();

    let mut to_mark_rejected = Vec::new();
    let mut reconsidered = Vec::new();

    for candidate in candidates {
        let source = candidate.source.clone();
        let seen = ctx.already_seen.iter().any(|s| s == &source);
        // Only a work that was *rejected* is decided afresh; one that was handled is not revisited.
        let redecide = seen && ctx.reconsider.iter().any(|r| r == &source);

        let verdict = if ctx.already_held.iter().any(|h| h == &source) {
            if redecide {
                reconsidered.push(source.clone());
            }
            // The id of the copy that is actually here, so the row links to it rather than back out
            // to the site this listing came from. Empty only for an archive whose tags carry no
            // `source:` entry, which is also the only way it could not have matched in the first
            // place.
            CandidateVerdict::AlreadyHeld {
                archive_id: (ctx.held_archive)(&source).unwrap_or_default(),
            }
        } else if let Some((newer_source, newer_archive_id)) = (!seen
            && !ctx.discarded.iter().any(|d| d == &source))
        .then(|| (ctx.newer_held_revision)(&source))
        .flatten()
        {
            // Settled, not "seen by a rule": the user already has this work's newer revision, so
            // there is nothing here to reconsider — only a newer revision than that would be news.
            CandidateVerdict::Superseded {
                newer_source,
                newer_archive_id,
            }
        } else if (seen && !redecide) || ctx.discarded.iter().any(|d| d == &source) {
            // A discarded work counts as settled: the user already said no. Re-offering it every
            // cycle would make the discard meaningless.
            CandidateVerdict::AlreadySeen
        } else {
            let categories = (ctx.categories_for)(&source);
            match matcher::evaluate_outcome(
                candidate,
                &subscription.filters,
                &categories,
                ctx.now,
                ctx.fold,
            ) {
                matcher::Outcome::Rejected(rule) => {
                    if redecide {
                        // Already recorded as rejected once: repeating it would put the same row in
                        // every cycle's history forever, which is exactly why rejections used to be
                        // final. The work stays in the reconsiderable set.
                        CandidateVerdict::AlreadySeen
                    } else {
                        to_mark_rejected.push(source.clone());
                        CandidateVerdict::Rejected {
                            rule: rule.as_key(),
                        }
                    }
                }
                // Pulled back out of `seen_candidates`: recording it as seen would make the waiting
                // period permanent, since a seen work is never looked at again.
                matcher::Outcome::TooSoon => CandidateVerdict::TooSoon,
                matcher::Outcome::Accepted => {
                    actionable.push(source.clone());
                    // Whatever it was rejected for before no longer applies; it is handled now.
                    if redecide {
                        reconsidered.push(source.clone());
                    }
                    if subscription.auto_download {
                        CandidateVerdict::Queued
                    } else {
                        CandidateVerdict::AwaitingApproval
                    }
                }
            }
        };

        // Recorded as seen only once a verdict is final. A work still inside its waiting period is
        // deliberately left out: marking it seen would make the wait permanent, since a seen work is
        // never reconsidered.
        if verdict != CandidateVerdict::TooSoon {
            seen_candidates.push(source.clone());
        }

        records.push(CandidateRecord {
            source_url: source,
            title: candidate.title.clone(),
            uploader: candidate.uploader.clone(),
            rating: candidate.rating,
            posted_at: candidate
                .posted_at
                .as_deref()
                .and_then(matcher::parse_posted_at),
            tags: candidate.tags.clone(),
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

    let authoritative = outcome.is_authoritative();
    let to_mark_seen = if authoritative {
        seen_candidates
    } else {
        Vec::new()
    };
    let (to_mark_rejected, reconsidered) = if authoritative {
        (to_mark_rejected, reconsidered)
    } else {
        (Vec::new(), Vec::new())
    };

    CycleDecision {
        outcome,
        records,
        actionable,
        to_mark_seen,
        to_mark_rejected,
        reconsidered,
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
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::download_queue::{NewQueueItem, QueueItemOrigin};
use lanrurugi_storage::subscriptions::{
    ReservationEntry, ReservationStatus, SourceChangePolicy, SourceRemovalPolicy, SubscriptionState,
};

/// Every revision chain the library has stored, one entry per archive that carries one.
///
/// Read through the pipelined batch accessor: a library-sized set of `GET`s is one round trip, and
/// this runs once per cycle rather than once per candidate.
async fn stored_revision_chains(state: &AppState) -> Vec<Vec<(String, String)>> {
    let Ok(archives) = state.repos.archives.list_all().await else {
        return Vec::new();
    };
    let ids: Vec<lanrurugi_core::ids::ArchiveId> = archives.iter().map(|a| a.id.clone()).collect();
    match state.repos.archives.version_histories_for(&ids).await {
        Ok(rows) => rows.into_iter().map(|(_, chain)| chain).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "could not read stored revision chains");
            Vec::new()
        }
    }
}

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
/// What a check *would* do, without doing any of it.
///
/// Runs the real discovery call and the real [`decide`], then throws the decision away: nothing is
/// queued, nothing is recorded as seen, no snapshot is written, no cycle is logged. Sharing `decide`
/// is the point — a preview computed by a separate code path would eventually disagree with the check
/// it claims to preview, and the disagreement would show up as a subscription that behaves unlike its
/// own preview.
///
/// Works on a disabled subscription too: "what would this rule actually catch" is a question asked
/// precisely while a subscription is switched off and being tuned.
/// Decides over candidates already in hand, without contacting the source.
///
/// Exists because a subscription's conditions are evaluated here, not at the source: only `criteria`
/// shapes the request, so changing a rule should re-judge what was already fetched rather than fetch
/// it again. The alternative — re-implementing the condition tree in the browser — would be a second
/// evaluator that eventually disagrees with the real one.
pub async fn preview_decide(
    state: &AppState,
    subscription: &Subscription,
    result: &lanrurugi_plugin::protocol::DiscoveryResult,
) -> CycleDecision {
    let seen = state
        .subscriptions
        .seen_all(&subscription.id)
        .await
        .unwrap_or_default();
    // Same context the real cycle uses, so the preview cannot promise a download the check would not
    // make (or hide one it would).
    let reconsider = state
        .subscriptions
        .rejected_all(&subscription.id)
        .await
        .unwrap_or_default();
    let reservations = state
        .subscriptions
        .list_reservations()
        .await
        .unwrap_or_default();
    let settled = super::reservations::settled_sources(&reservations, &subscription.id);
    let held = held_sources(state).await;
    let chains = stored_revision_chains(state).await;
    let held_names: Vec<String> = held.iter().map(|(source, _)| source.clone()).collect();
    let archive_for: std::collections::HashMap<&str, &str> = held
        .iter()
        .map(|(source, id)| (source.as_str(), id.as_str()))
        .collect();
    let held_archive = |source: &str| archive_for.get(source).map(|id| (*id).to_string());
    let revision = revision_lookup(&held, chains);

    let no_categories = |_: &str| Vec::<String>::new();
    let fold = |t: &str| state.equivalence.fold(t).into_owned();
    decide(
        subscription,
        result,
        &CycleContext {
            already_seen: &seen,
            reconsider: &reconsider,
            already_held: &held_names,
            held_archive: &held_archive,
            newer_held_revision: &revision,
            discarded: &settled,
            categories_for: &no_categories,
            now: now_secs(),
            fold: &fold,
        },
    )
}

/// Fetches from the source, then decides. The listing it read is returned alongside so a later
/// re-judge can reuse it.
pub async fn preview_check_raw(
    state: &AppState,
    subscription: &Subscription,
) -> Result<(CycleDecision, lanrurugi_plugin::protocol::DiscoveryResult), String> {
    let result = fetch_listing(state, subscription).await?;
    let decision = preview_decide(state, subscription, &result).await;
    Ok((decision, result))
}

/// Folds hand-entered cookies and headers into the same keys a login plugin's result goes into.
///
/// Merged rather than replacing: a source may need a plugin-supplied session *and* one header the
/// plugin knows nothing about. The manual values win on a conflict, since they are the more specific
/// statement of intent — someone typed them for this subscription.
fn apply_manual_credentials(
    args: &mut serde_json::Value,
    credentials: &lanrurugi_storage::subscriptions::Credentials,
) {
    if credentials.is_empty() {
        return;
    }
    let Some(obj) = args.as_object_mut() else {
        return;
    };

    if !credentials.cookies.trim().is_empty() {
        let existing = obj
            .get("user_agent_cookies")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let merged = if existing.is_empty() {
            credentials.cookies.trim().to_string()
        } else {
            format!("{existing}; {}", credentials.cookies.trim())
        };
        obj.insert("user_agent_cookies".into(), serde_json::json!(merged));
    }

    if !credentials.headers.trim().is_empty() {
        let mut headers = obj
            .get("user_agent_headers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        // One `Name: value` per line, which is how they are read off a browser's network panel.
        for line in credentials.headers.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            if !name.is_empty() {
                headers.insert(name.to_string(), serde_json::json!(value));
            }
        }
        obj.insert("user_agent_headers".into(), serde_json::json!(headers));
    }
}

async fn fetch_listing(
    state: &AppState,
    subscription: &Subscription,
) -> Result<lanrurugi_plugin::protocol::DiscoveryResult, String> {
    let info = state
        .plugins
        .plugin_info(&subscription.source)
        .await
        .map_err(|e| e.to_string())?;

    let mut base = serde_json::to_value(&subscription.criteria).unwrap_or_default();
    if let Some(obj) = base.as_object_mut() {
        obj.insert("max_pages".into(), serde_json::json!(MAX_LISTING_PAGES));
    }
    let mut args = crate::plugins::with_login_cookies(state, &info, base).await;
    apply_manual_credentials(&mut args, &subscription.credentials);

    // `None` means the extension has no `discover` at all — distinct from a discovery that ran and
    // found nothing, and worth saying so rather than showing an empty preview.
    let Some(result) = state
        .plugins
        .discover(&subscription.source, args)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Err("this source no longer offers discovery".into());
    };

    Ok(result)
}

pub async fn run_check(state: &AppState, subscription_id: &str) {
    let Ok(Some(mut subscription)) = state.subscriptions.get(subscription_id).await else {
        return;
    };

    let started_at = now_secs();

    // Discovery runs with whatever signed-in state the source declared it needs, exactly like a
    // metadata or download call — subscriptions introduce no second credential path.
    let args = match state.plugins.plugin_info(&subscription.source).await {
        Ok(info) => {
            let mut base = serde_json::to_value(&subscription.criteria).unwrap_or_default();
            // A ceiling the host sets, not the user: reading one page misses works that entered the
            // result set by being edited (they sort by original publication date, so they land
            // mid-list), while walking a whole tag search means tens of thousands of requests.
            if let Some(obj) = base.as_object_mut() {
                obj.insert("max_pages".into(), serde_json::json!(MAX_LISTING_PAGES));
            }
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
    let reconsider = state
        .subscriptions
        .rejected_all(subscription_id)
        .await
        .unwrap_or_default();
    // The library's stored revision chains, read in one round trip: every "is this candidate an
    // older revision of something we hold?" below is then answered without touching the network.
    let chains = stored_revision_chains(state).await;
    let reservations = state
        .subscriptions
        .list_reservations()
        .await
        .unwrap_or_default();
    let settled = super::reservations::settled_sources(&reservations, subscription_id);
    let held = held_sources(state).await;

    let held_names: Vec<String> = held.iter().map(|(source, _)| source.clone()).collect();
    let archive_for: std::collections::HashMap<&str, &str> = held
        .iter()
        .map(|(source, id)| (source.as_str(), id.as_str()))
        .collect();
    let held_archive = |source: &str| archive_for.get(source).map(|id| (*id).to_string());
    let revision = revision_lookup(&held, chains);
    let decision = {
        let no_categories = |_: &str| Vec::<String>::new();
        let fold = |t: &str| state.equivalence.fold(t).into_owned();
        let ctx = CycleContext {
            already_seen: &seen,
            reconsider: &reconsider,
            already_held: &held_names,
            held_archive: &held_archive,
            newer_held_revision: &revision,
            discarded: &settled,
            categories_for: &no_categories,
            now: now_secs(),
            fold: &fold,
        };
        decide(&subscription, &result, &ctx)
    };

    // Queue (or hold for approval) whatever survived.
    // Already-pending items are skipped so repeated checks don't pile up duplicates of the same
    // unanswered question.
    let pending_already = state
        .subscriptions
        .pending_sources(subscription_id)
        .await
        .unwrap_or_default();

    for source in &decision.actionable {
        if !subscription.auto_download {
            // Nothing is spent until the user approves. Recorded as its own addressable entry
            // rather than left in the cycle log: that log is bounded by age, so an unanswered
            // question would quietly expire out of it.
            if pending_already.iter().any(|p| p == source) {
                continue;
            }
            // Carried over from the cycle record rather than left empty: the approval list is
            // where the user decides, and deciding from a bare gid URL is not deciding.
            let title = decision
                .records
                .iter()
                .find(|r| &r.source_url == source)
                .map(|r| r.title.clone())
                .unwrap_or_default();
            let entry = lanrurugi_storage::subscriptions::PendingApproval {
                id: uuid::Uuid::new_v4().to_string(),
                subscription_id: subscription_id.to_string(),
                source_url: source.clone(),
                title,
                created_at: now_secs(),
            };
            let _ = state.subscriptions.save_pending(&entry).await;
            continue;
        }
        // The subscription's own `source` is the *discovery* plugin, which has no `execDownload` —
        // the download plugin that owns this URL has to be resolved separately, exactly as the
        // Upload page resolves it client-side for a manual add. Everything the download-plugin
        // settings page governs (domain concurrency/rate rules, revision policies, plugin
        // parameters) is keyed off this namespace, so resolving it here is also what makes those
        // settings apply to a subscription's downloads.
        let Some((download_namespace, _)) =
            crate::plugins::resolve_download_plugin_for_url(state, source).await
        else {
            // Matched, but nothing installed can download it. Recorded as a reservation rather than
            // dropped, and (via `settled_sources`) not re-offered every cycle.
            let entry = ReservationEntry {
                id: uuid::Uuid::new_v4().to_string(),
                subscription_id: subscription_id.to_string(),
                source_url: source.clone(),
                reason: super::reservations::ReservationReason::DownloadFailed(
                    "no installed download plugin matches this source URL".to_string(),
                )
                .as_key(),
                status: ReservationStatus::Waiting,
                created_at: now_secs(),
            };
            let _ = state.subscriptions.save_reservation(&entry).await;
            continue;
        };
        let overwrite_on_duplicate = super::overwrite_under_conflict_policy(
            subscription.conflict_policy,
            crate::plugins::resolve_queue_overwrite_on_duplicate(state, &download_namespace).await,
        );

        let queued = state
            .download_queue
            .add(NewQueueItem {
                origin: QueueItemOrigin::Download,
                // Traces the resulting archive back to the rule responsible (FR-022), and lets the
                // delete flow offer to stop this subscription re-fetching it.
                subscription_id: Some(subscription_id.to_string()),
                url: source.clone(),
                plugin_namespace: download_namespace,
                file_size: None,
                category: subscription.target_category.clone(),
                metadata_tags: subscription.metadata_tags.clone(),
                auto_fetch_metadata: subscription.enrich_metadata,
                overwrite_on_duplicate,
                conflict_policy: subscription.conflict_policy,
                // Enters the queue the same way a manually added URL does, so it inherits the
                // existing start/stop/retry behaviour rather than getting a path of its own.
                state: lanrurugi_storage::download_queue::DownloadQueueState::Queued,
            })
            .await;
        match queued {
            Ok(item) => {
                // "Download automatically" means started, not merely enqueued: nothing would ever
                // pick a `Queued` item up on its own (only the upload page's own Start buttons call
                // into `start_one`). See `start_item_from_subscription`'s own docs.
                //
                // Recorded *after* the attempt so the entry carries what actually happened: a start
                // that failed leaves the item queued and startable, which is worth seeing in the
                // audit log rather than being described as a success.
                let started = crate::download_queue::start_item_from_subscription(state, &item.id)
                    .await
                    .map_err(|e| {
                        // Left queued and startable: the upload page's own Start button still
                        // works, so this is a warning rather than a lost work item.
                        tracing::warn!(%subscription_id, item = %item.id, error = %e, "subscription download could not be started automatically");
                        e
                    });
                crate::activity::record_manual(
                    state,
                    None,
                    action_types::AUTO_DOWNLOAD,
                    ActivityTarget {
                        id: Some(subscription_id.to_string()),
                        label: Some(source.clone()),
                        kind: Some("subscription".to_string()),
                    },
                    match &started {
                        Ok(_) => Outcome::Success,
                        // `started`'s own `Err` is consumed below; the reason is cloned here so the
                        // record still says why.
                        Err(reason) => Outcome::Failure {
                            reason: reason.clone(),
                        },
                    },
                    None,
                    Some(serde_json::json!({
                        "subscription_id": subscription_id,
                        "subscription": subscription.name,
                        "queue_item_id": item.id,
                        "plugin_namespace": item.plugin_namespace,
                        "url": item.url,
                        "job_id": started.ok(),
                    })),
                )
                .await;
            }
            Err(e) => {
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
    }

    // The guard, enforced at the one place it matters: only an authoritative cycle may record works
    // as seen. `decide` already returns an empty list otherwise, so this is belt and braces.
    if decision.outcome.is_authoritative() {
        // A rejection is remembered as *reconsiderable* rather than settled, and one that has since
        // been taken stops being a question.
        if let Err(e) = state
            .subscriptions
            .mark_rejected(subscription_id, &decision.to_mark_rejected)
            .await
        {
            tracing::warn!(%subscription_id, error = %e, "could not record reconsiderable works");
        }
        if let Err(e) = state
            .subscriptions
            .forget_rejected(subscription_id, &decision.reconsidered)
            .await
        {
            tracing::warn!(%subscription_id, error = %e, "could not clear reconsidered works");
        }
    }
    if decision.outcome.is_authoritative() && !decision.to_mark_seen.is_empty() {
        let _ = state
            .subscriptions
            .mark_seen(subscription_id, &decision.to_mark_seen)
            .await;
    }

    apply_snapshot_diff(state, &subscription, &result, &decision).await;

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
/// Diffs this listing against the snapshot it last produced, persists the result, and carries out
/// whatever the subscription's own policies ask for (FR-012b–e, issue #111).
///
/// Separated from `decide` because this is all I/O and policy, while `decide` stays pure.
async fn apply_snapshot_diff(
    state: &AppState,
    subscription: &Subscription,
    result: &lanrurugi_plugin::protocol::DiscoveryResult,
    decision: &CycleDecision,
) {
    let candidates = result.candidates.as_deref().unwrap_or(&[]);
    let previous = state
        .subscriptions
        .snapshot(&subscription.id)
        .await
        .unwrap_or_default();

    let fold = |t: &str| state.equivalence.fold(t).into_owned();
    let still_matches = |c: &lanrurugi_plugin::protocol::DiscoveredCandidate| {
        // Only the subscription's own filters: whether it is *held* is a separate question, and a
        // held work that stopped matching is still worth telling the user about.
        // The same evaluation the decision itself uses, so "still matches" and "matched" can never
        // diverge. A premature candidate counts as still matching: it has not stopped qualifying, it
        // simply has not finished qualifying.
        !matches!(
            matcher::evaluate_outcome(c, &subscription.filters, &[], now_secs(), &fold),
            matcher::Outcome::Rejected(_)
        )
    };

    let diff = super::snapshot::diff(
        candidates,
        &previous,
        decision.outcome.is_authoritative(),
        now_secs(),
        &still_matches,
    );

    if let Err(e) = state
        .subscriptions
        .save_snapshot_entries(&subscription.id, &diff.to_save)
        .await
    {
        tracing::warn!(subscription_id = %subscription.id, error = %e, "failed to save listing snapshot");
    }

    for removed in &diff.removed {
        // Withdrawing an unanswered question regardless of policy: the work is gone from the source,
        // so asking the user whether to download it is asking something already settled. Only the
        // *held* archive is policy-governed, and no policy touches it.
        if let Ok(pending) = state.subscriptions.list_pending().await {
            for entry in pending
                .iter()
                .filter(|p| p.subscription_id == subscription.id && &p.source_url == removed)
            {
                let _ = state.subscriptions.delete_pending(&entry.id).await;
            }
        }
        match subscription.on_source_removed {
            SourceRemovalPolicy::Ignore => {}
            SourceRemovalPolicy::MarkOnly | SourceRemovalPolicy::Notify => {
                // The mark itself already lives in the snapshot entry; this is the operator-visible
                // trace. Nothing is deleted from the library under any policy (FR-012d).
                tracing::info!(
                    subscription_id = %subscription.id,
                    source = %removed,
                    "work no longer listed at the source"
                );
            }
        }
    }

    if subscription.on_source_changed == SourceChangePolicy::RefetchMetadata {
        let held = held_archive_ids(state).await;
        for change in &diff.changed {
            let Some(archive_id) = held.get(&change.source) else {
                continue;
            };
            // Re-runs the metadata capability rather than copying snapshot values across: a listing
            // truncates its tag list, so writing those tags onto the archive would delete real ones.
            // That path also keeps the existing merge-never-delete rule.
            let summary =
                crate::plugins::run_enabled_metadata_plugins_on_archive(state, archive_id).await;
            tracing::info!(
                subscription_id = %subscription.id,
                source = %change.source,
                %archive_id,
                added_tags = summary.added_tags,
                "re-enriched metadata after a source-side change"
            );
        }
    }
}

/// Canonical source URL to the id of the archive holding it, from the `source:` tag every downloaded
/// archive carries.
async fn held_archive_ids(state: &AppState) -> std::collections::HashMap<String, String> {
    let Ok(archives) = state.repos.archives.list_all().await else {
        return std::collections::HashMap::new();
    };
    let mut out = std::collections::HashMap::new();
    for archive in archives {
        for tag in archive.tags.split(',') {
            if let Some(source) = tag.trim().strip_prefix("source:") {
                out.insert(crate::plugins::trim_url(source), archive.id.0.clone());
            }
        }
    }
    out
}

async fn held_sources(state: &AppState) -> Vec<(String, String)> {
    let Ok(archives) = state.repos.archives.list_all().await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for archive in archives {
        for tag in archive.tags.split(',') {
            if let Some(source) = tag.trim().strip_prefix("source:") {
                out.push((crate::plugins::trim_url(source), archive.id.to_string()));
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
    // The queue sweep is not a per-tick job: it walks the whole queue, and the thing it bounds is
    // measured in days. Once an hour is already far more often than the setting can change anything.
    let mut last_sweep: Option<std::time::Instant> = None;
    loop {
        ticker.tick().await;
        if last_sweep.is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(3600)) {
            last_sweep = Some(std::time::Instant::now());
            sweep_finished_queue(&state).await;
        }
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

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Deletes finished download-queue rows older than the configured retention.
///
/// Lives here because this is the one long-running loop the API already owns, and because the setting
/// it reads (`download_queue_retention_days`) is about this orchestrator's own queue growth. `0` means
/// the user wants the whole log kept, so nothing is swept at all.
async fn sweep_finished_queue(state: &AppState) {
    let days = crate::settings::read_queue_retention_days(state).await;
    if days <= 0 {
        return;
    }
    let cutoff_ms = now_epoch_ms() - days * 24 * 60 * 60 * 1000;
    match state.download_queue.prune_finished_before(cutoff_ms).await {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, days, "swept finished download-queue entries"),
        Err(e) => tracing::warn!(error = %e, "could not sweep the download queue"),
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
            metadata_tags: Vec::new(),
            enrich_metadata: true,
            auto_download: auto,
            credit_policy: CreditPolicy::default(),
            conflict_policy: Default::default(),
            credentials: Default::default(),
            settings_changed_at: None,
            on_source_changed: Default::default(),
            on_source_removed: Default::default(),
            state: SubscriptionState::Enabled,
            last_checked_at: None,
            created_at: 0,
        }
    }

    fn cand(source: &str, tags: &[&str]) -> DiscoveredCandidate {
        DiscoveredCandidate {
            source: source.into(),
            title: std::collections::HashMap::new(),
            posted_at: None,
            rating: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            category: None,
            uploader: None,
            pages: None,
        }
    }

    fn result(candidates: Vec<DiscoveredCandidate>, degraded: bool) -> DiscoveryResult {
        DiscoveryResult {
            candidates: Some(candidates),
            degraded,
            error: None,
        }
    }

    /// A context with nothing reconsiderable — the common case, and what every pre-existing test
    /// below means unless it says otherwise (`reconsidering` adds the other set).
    fn ctx<'a>(
        seen: &'a [String],
        held: &'a [String],
        discarded: &'a [String],
        cats: &'a dyn Fn(&str) -> Vec<String>,
    ) -> CycleContext<'a> {
        ctx_with_reconsider(seen, &[], held, discarded, cats)
    }

    /// The default `held_archive` answer for tests: nothing is looked up (the id only decides where a
    /// row links, which no decision depends on).
    fn no_archive(_: &str) -> Option<String> {
        None
    }

    fn ctx_with_reconsider<'a>(
        seen: &'a [String],
        reconsider: &'a [String],
        held: &'a [String],
        discarded: &'a [String],
        cats: &'a dyn Fn(&str) -> Vec<String>,
    ) -> CycleContext<'a> {
        CycleContext {
            already_seen: seen,
            reconsider,
            already_held: held,
            held_archive: &no_archive,
            newer_held_revision: &|_: &str| None,
            discarded,
            categories_for: cats,
            fold: &|t: &str| t.to_string(),
            // Fixed so age-based rules are deterministic in tests; well past any fixture's posted_at.
            now: 4_000_000_000,
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

    /// The whole point of the reconsiderable set: a work a rule turned away is decided afresh on a
    /// later cycle, so a rating that rose or a tag that was added can still get it downloaded. Without
    /// this it stays in the seen set forever and the user never learns it was there.
    #[test]
    fn a_work_rejected_earlier_is_downloaded_once_it_qualifies() {
        let filters = Filters {
            minimum_rating: Some(4.0),
            ..Filters::default()
        };
        // Rejected at 3 stars: recorded, and remembered as reconsiderable rather than settled.
        let mut low = cand("a", &[]);
        low.rating = Some(3.0);
        let d = decide(
            &sub(true, filters.clone()),
            &result(vec![low], false),
            &ctx(&[], &[], &[], NO_CATS),
        );
        assert!(matches!(
            d.records[0].verdict,
            CandidateVerdict::Rejected { .. }
        ));
        assert_eq!(d.to_mark_rejected, vec!["a".to_string()]);
        assert!(d.reconsidered.is_empty());

        // Next cycle it is at 5 stars. It is in both the seen set and the reconsiderable set, so it
        // is decided again — and now downloaded.
        let mut high = cand("a", &[]);
        high.rating = Some(5.0);
        let seen = vec!["a".to_string()];
        let d = decide(
            &sub(true, filters),
            &result(vec![high], false),
            &ctx_with_reconsider(&seen, &seen, &[], &[], NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::Queued);
        assert_eq!(d.actionable, vec!["a".to_string()]);
        assert_eq!(
            d.reconsidered,
            vec!["a".to_string()],
            "it is handled now, so it stops being a question"
        );
    }

    /// Still not good enough is still not a rejection *recorded again*: the same row in every cycle's
    /// history is what made rejections permanent in the first place.
    #[test]
    fn a_reconsidered_work_that_still_fails_reads_as_already_seen_not_rejected() {
        let filters = Filters {
            minimum_rating: Some(4.0),
            ..Filters::default()
        };
        let mut low = cand("a", &[]);
        low.rating = Some(2.0);
        let seen = vec!["a".to_string()];
        let d = decide(
            &sub(true, filters),
            &result(vec![low], false),
            &ctx_with_reconsider(&seen, &seen, &[], &[], NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::AlreadySeen);
        assert!(d.to_mark_rejected.is_empty(), "already on record");
        assert!(d.reconsidered.is_empty(), "still a question for next time");
    }

    /// A work the user *answered* is not re-decided: re-offering an approved, downloaded, or
    /// discarded work would be asking again what they already said.
    #[test]
    fn a_handled_work_is_never_reconsidered_even_if_it_would_now_qualify() {
        let filters = Filters {
            minimum_rating: Some(4.0),
            ..Filters::default()
        };
        let mut high = cand("a", &[]);
        high.rating = Some(5.0);
        let seen = vec!["a".to_string()];
        // In the seen set, *not* in the reconsiderable one.
        let d = decide(
            &sub(true, filters),
            &result(vec![high], false),
            &ctx(&seen, &[], &[], NO_CATS),
        );
        assert_eq!(d.records[0].verdict, CandidateVerdict::AlreadySeen);
        assert!(d.actionable.is_empty());
    }

    /// The A/B/C case, at the level it can be decided without any download: the stored chain says the
    /// library holds C, and A/B are older revisions of it.
    #[test]
    fn an_older_revision_of_a_held_work_is_not_queued_at_all() {
        let held = vec![("e-hentai.org/g/3/cccccccccc".to_string(), "ccc".to_string())];
        let chains = vec![vec![
            (
                "e-hentai.org/g/1/aaaaaaaaaa".to_string(),
                "2026-10-01T00:00:00Z".to_string(),
            ),
            (
                "e-hentai.org/g/2/bbbbbbbbbb".to_string(),
                "2026-10-02T00:00:00Z".to_string(),
            ),
            (
                "e-hentai.org/g/3/cccccccccc".to_string(),
                "2026-10-03T00:00:00Z".to_string(),
            ),
        ]];
        let revision = revision_lookup(&held, chains);
        let held_names: Vec<String> = held.iter().map(|(s, _)| s.clone()).collect();
        let ctx = CycleContext {
            held_archive: &|source: &str| {
                held.iter()
                    .find(|(s, _)| s == source)
                    .map(|(_, id)| id.clone())
            },
            newer_held_revision: &revision,
            ..ctx_with_reconsider(&[], &[], &held_names, &[], NO_CATS)
        };

        let d = decide(
            &sub(true, Filters::default()),
            &result(
                vec![
                    cand("e-hentai.org/g/1/aaaaaaaaaa", &[]),
                    cand("e-hentai.org/g/2/bbbbbbbbbb", &[]),
                ],
                false,
            ),
            &ctx,
        );

        assert!(
            d.actionable.is_empty(),
            "neither old revision is downloaded"
        );
        for record in &d.records {
            match &record.verdict {
                CandidateVerdict::Superseded {
                    newer_source,
                    newer_archive_id,
                } => {
                    assert_eq!(newer_source, "e-hentai.org/g/3/cccccccccc");
                    assert_eq!(
                        newer_archive_id, "ccc",
                        "the row links to the copy that is actually here"
                    );
                }
                other => panic!("expected Superseded, got {other:?}"),
            }
        }
        assert_eq!(
            d.to_mark_seen.len(),
            2,
            "settled: the library's newer copy answers for them from now on"
        );
        assert!(d.to_mark_rejected.is_empty(), "not a rule rejection");
    }

    /// A *newer* revision than anything held is still news — the download stage's own supersede
    /// policy takes it from there.
    #[test]
    fn a_newer_revision_than_the_library_holds_is_still_queued() {
        let held = vec![("e-hentai.org/g/1/aaaaaaaaaa".to_string(), "aaa".to_string())];
        let chains = vec![vec![
            (
                "e-hentai.org/g/1/aaaaaaaaaa".to_string(),
                "2026-10-01T00:00:00Z".to_string(),
            ),
            (
                "e-hentai.org/g/2/bbbbbbbbbb".to_string(),
                "2026-10-09T00:00:00Z".to_string(),
            ),
        ]];
        let revision = revision_lookup(&held, chains);
        let held_names: Vec<String> = held.iter().map(|(s, _)| s.clone()).collect();
        let ctx = CycleContext {
            held_archive: &|source: &str| {
                held.iter()
                    .find(|(s, _)| s == source)
                    .map(|(_, id)| id.clone())
            },
            newer_held_revision: &revision,
            ..ctx_with_reconsider(&[], &[], &held_names, &[], NO_CATS)
        };

        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("e-hentai.org/g/2/bbbbbbbbbb", &[])], false),
            &ctx,
        );

        assert_eq!(
            d.actionable,
            vec!["e-hentai.org/g/2/bbbbbbbbbb".to_string()]
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
        assert_eq!(
            d.records[0].verdict,
            CandidateVerdict::AlreadyHeld {
                archive_id: String::new()
            }
        );
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
    /// FR-009b: a newer revision carries its own source URL, so it must not be mistaken for the
    /// revision already held. Deduplication here is by URL; deciding which revision supersedes which
    /// belongs to `download_manager::version_history::classify`, at download time, where the series
    /// history is actually available. Asserting it here pins the division of labour: if this ever
    /// starts reporting AlreadyHeld, subscriptions have gone blind to updates.
    #[test]
    fn a_newer_revision_is_not_mistaken_for_the_revision_already_held() {
        let held = vec!["e-hentai.org/g/100/aaaaaaaaaa".to_string()];
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("e-hentai.org/g/200/bbbbbbbbbb", &[])], false),
            &ctx(&[], &held, &[], NO_CATS),
        );
        assert!(
            !matches!(d.records[0].verdict, CandidateVerdict::AlreadyHeld { .. }),
            "got {:?}",
            d.records[0].verdict
        );
    }

    /// FR-016: the user already said no. Re-offering it every cycle would make the discard
    /// meaningless.
    #[test]
    fn a_discarded_work_is_not_offered_again() {
        let discarded = vec!["e-hentai.org/g/1/a".to_string()];
        let d = decide(
            &sub(true, Filters::default()),
            &result(vec![cand("e-hentai.org/g/1/a", &[])], false),
            &ctx(&[], &[], &discarded, NO_CATS),
        );
        assert!(matches!(
            d.records[0].verdict,
            CandidateVerdict::AlreadySeen
        ));
        assert!(d.actionable.is_empty());
    }
}
