//! Diffing a listing against what it last showed (FR-012b–e, issue #111).
//!
//! Pure: no I/O, no clock, no Redis. The caller gathers the previous snapshot and supplies `now`,
//! so every rule here is directly testable.
//!
//! The reason this exists at all: a tag-or-keyword result set changes in three ways, and only one is
//! chronological. A work may be newly published (appears at the top), an *existing* work may be
//! edited to carry the tag (appears mid-list, with a `posted_at` possibly years old), or a work may
//! lose the tag entirely (disappears). Deciding what to act on by "newer than last time" handles the
//! first and silently misses the second — hence membership, never a high-water mark.

use std::collections::{HashMap, HashSet};

use lanrurugi_plugin::protocol::DiscoveredCandidate;
use lanrurugi_storage::subscriptions::{
    CandidateRecord, CandidateVerdict, ChangedField, SnapshotEntry, ValueChange, VerdictRun,
};

/// How many consecutive checks a work must be absent from before it counts as removed.
///
/// Not one: a single throttled or partial listing would otherwise mark a whole page of works at once
/// (FR-012e). Three gives a source two chances to answer properly before anything is recorded.
pub const MISSING_THRESHOLD: u32 = 3;

/// A work that is still listed but whose title or tags moved.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangedWork {
    pub source: String,
    /// Set when the title moved; carries the titles the snapshot held before.
    pub title_was: std::collections::HashMap<String, String>,
    pub tags_changed: bool,
    /// It is still listed, but its current tags no longer satisfy the subscription's filters.
    pub no_longer_matching: bool,
}

/// The outcome of comparing a listing against the snapshot it last produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapshotDiff {
    /// Entries to persist — every work this listing showed, plus absence bookkeeping for those it
    /// did not.
    pub to_save: Vec<SnapshotEntry>,
    /// Works whose title or tags moved while remaining listed.
    pub changed: Vec<ChangedWork>,
    /// Works that crossed [`MISSING_THRESHOLD`] on this check. Reported once, on the crossing.
    pub removed: Vec<String>,
    /// Entries nothing needs any more, to delete from the snapshot.
    pub to_remove: Vec<String>,
}

/// Builds a snapshot entry for a work this listing showed.
///
/// `previous` carries forward `first_seen_at` — the one field a later sighting must not reset, since
/// "when did we first see this" is exactly what distinguishes a long-published work that only now
/// entered the result set.
fn entry_for(
    candidate: &DiscoveredCandidate,
    previous: Option<&SnapshotEntry>,
    now: i64,
    no_longer_matching: bool,
    title_changed_from: std::collections::HashMap<String, String>,
) -> SnapshotEntry {
    SnapshotEntry {
        source: candidate.source.clone(),
        title: candidate.title.clone(),
        posted_at: candidate.posted_at.clone(),
        tags: candidate.tags.clone(),
        rating: candidate.rating,
        category: candidate.category.clone(),
        uploader: candidate.uploader.clone(),
        pages: candidate.pages,
        first_seen_at: previous.map_or(now, |p| p.first_seen_at),
        last_seen_at: now,
        // Seeing it again clears any absence recorded earlier: a work that came back was not removed.
        disappeared_at: None,
        missing_count: 0,
        no_longer_matching,
        title_changed_from,
        data_version: previous.map_or(1, |p| p.data_version),
        runs: previous.map(|p| p.runs.clone()).unwrap_or_default(),
        changes: previous.map(|p| p.changes.clone()).unwrap_or_default(),
        rating_logged: previous.and_then(|p| p.rating_logged),
        last_verdict_at: previous.and_then(|p| p.last_verdict_at),
    }
}

/// When an entry the listing no longer shows may be dropped from the snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PruneRule {
    /// A day window: once it was published before this (Unix seconds).
    PostedBefore(i64),
    /// A page window, which has no age bound: once it was last listed before this.
    LastSeenBefore(i64),
    Never,
}

/// What one cycle adds to the snapshot beyond the listing itself.
pub struct DiffContext<'a> {
    /// See [`diff`].
    pub authoritative: bool,
    pub now: i64,
    pub still_matches: &'a dyn Fn(&DiscoveredCandidate) -> bool,
    /// This cycle's number. `None` records no history (no runs, no changes).
    pub seq: Option<u64>,
    /// The verdict this cycle gave each source it decided.
    pub verdicts: &'a HashMap<String, CandidateVerdict>,
    /// The source plugin's current `data_version`.
    pub data_version: u32,
    /// Sources re-read by `refreshSources` rather than seen in the listing. Their sighting does not
    /// count as being listed, or a work that has left the listing would never read as removed.
    pub refreshed: &'a HashSet<String>,
    /// Out of the subscription's window before this (Unix seconds): a work missing from the listing
    /// for that reason has not been removed from the source.
    pub window_cutoff: Option<i64>,
    /// The oldest cycle number still kept; history older than it is dropped.
    pub oldest_kept_seq: u64,
    pub prune: PruneRule,
    /// Rejected works still reconsiderable. Never pruned: the next check needs their entry.
    pub reconsider: &'a HashSet<String>,
}

/// Compares `candidates` against `previous`, recording no history. Tests only: a real cycle goes
/// through [`diff_with`], which this calls with an empty context.
///
/// `still_matches` answers, for one candidate, whether the subscription's own filters still accept
/// it — passed in rather than evaluated here so this stays independent of the matcher.
///
/// `authoritative` gates absence bookkeeping entirely: a failed, inconclusive, or
/// challenge-blocked check saw a smaller world than the one asked about, so nothing may be counted
/// as missing from it (FR-012e). Works it *did* return are still recorded, since those sightings are
/// real.
#[cfg(test)]
pub fn diff(
    candidates: &[DiscoveredCandidate],
    previous: &HashMap<String, SnapshotEntry>,
    authoritative: bool,
    now: i64,
    still_matches: &dyn Fn(&DiscoveredCandidate) -> bool,
) -> SnapshotDiff {
    diff_with(
        candidates,
        previous,
        &DiffContext {
            authoritative,
            now,
            still_matches,
            seq: None,
            verdicts: &HashMap::new(),
            data_version: 1,
            refreshed: &HashSet::new(),
            window_cutoff: None,
            oldest_kept_seq: 0,
            prune: PruneRule::Never,
            reconsider: &HashSet::new(),
        },
    )
}

/// Ratings are compared for history at 0.1 — see `SnapshotEntry::rating_logged`.
fn rating_step(rating: Option<f32>) -> Option<f32> {
    rating.map(|r| (r * 10.0).round() / 10.0)
}

/// [`diff`], also recording this cycle's verdicts and pruning what no longer needs keeping.
pub fn diff_with(
    candidates: &[DiscoveredCandidate],
    previous: &HashMap<String, SnapshotEntry>,
    ctx: &DiffContext<'_>,
) -> SnapshotDiff {
    let mut out = SnapshotDiff::default();

    for candidate in candidates {
        let prior = previous.get(&candidate.source);
        // A refresh has no listing row, so it carries no `origin` title. Keeping the listed titles
        // underneath stops every switch between listing and refresh reading as a title change.
        let merged;
        let candidate = match prior {
            Some(prior) if ctx.refreshed.contains(&candidate.source) => {
                let mut c = candidate.clone();
                for (lang, title) in &prior.title {
                    c.title.entry(lang.clone()).or_insert_with(|| title.clone());
                }
                merged = c;
                &merged
            }
            _ => candidate,
        };
        let matches = (ctx.still_matches)(candidate);
        // An entry written under another data version describes the same work through different
        // fields; comparing the two would report a change on every work at once.
        let comparable = prior.filter(|p| p.data_version == ctx.data_version);

        let mut title_was = std::collections::HashMap::new();
        if let Some(prior) = comparable {
            // Compared as sets: a listing may reorder tags between reads without anything having
            // actually changed, and reporting that as a change every check would make the signal
            // useless.
            let before: std::collections::BTreeSet<&String> = prior.tags.iter().collect();
            let after: std::collections::BTreeSet<&String> = candidate.tags.iter().collect();
            let tags_changed = before != after;

            // Compared whole: a work whose Japanese title appeared where only an English one was
            // known has changed, even though neither title was replaced.
            if !candidate.title.is_empty() && prior.title != candidate.title {
                title_was = prior.title.clone();
            }

            if tags_changed || !title_was.is_empty() || prior.no_longer_matching != !matches {
                out.changed.push(ChangedWork {
                    source: candidate.source.clone(),
                    title_was: title_was.clone(),
                    tags_changed,
                    no_longer_matching: !matches,
                });
            }
        }

        let mut entry = entry_for(
            candidate,
            prior,
            ctx.now,
            !matches,
            // Kept only while unacknowledged; a title that has stopped moving should not keep
            // advertising a change forever.
            if title_was.is_empty() {
                comparable
                    .map(|p| p.title_changed_from.clone())
                    .unwrap_or_default()
            } else {
                title_was.clone()
            },
        );
        entry.data_version = ctx.data_version;
        if let (Some(prior), true) = (prior, ctx.refreshed.contains(&candidate.source)) {
            entry.last_seen_at = prior.last_seen_at;
            entry.missing_count = prior.missing_count;
            entry.disappeared_at = prior.disappeared_at;
        }
        record_history(&mut entry, comparable, candidate, ctx);
        out.to_save.push(entry);
    }

    if !ctx.authoritative {
        return out;
    }

    let present: std::collections::HashSet<&str> =
        candidates.iter().map(|c| c.source.as_str()).collect();

    for (source, prior) in previous {
        if present.contains(source.as_str()) {
            continue;
        }
        if prunable(prior, ctx) {
            out.to_remove.push(source.clone());
            continue;
        }
        // Already settled as removed; counting further absences would re-report it every check.
        if prior.disappeared_at.is_some() {
            continue;
        }
        // Aged out of the window: the listing stops before it, which says nothing about the source.
        if let (Some(cutoff), Some(posted)) = (ctx.window_cutoff, posted_secs(prior))
            && posted < cutoff
        {
            continue;
        }

        let missing_count = prior.missing_count + 1;
        let crossed = missing_count >= MISSING_THRESHOLD;
        if crossed {
            out.removed.push(source.clone());
        }
        out.to_save.push(SnapshotEntry {
            missing_count,
            disappeared_at: crossed.then_some(ctx.now),
            ..prior.clone()
        });
    }

    out
}

fn posted_secs(entry: &SnapshotEntry) -> Option<i64> {
    entry
        .posted_at
        .as_deref()
        .and_then(super::matcher::parse_posted_at)
}

/// Whether an entry the listing did not show can be dropped: out of the window, no longer
/// reconsiderable, not referenced by any kept cycle, and not part-way through absence counting.
fn prunable(entry: &SnapshotEntry, ctx: &DiffContext<'_>) -> bool {
    let out_of_window = match ctx.prune {
        PruneRule::PostedBefore(cutoff) => posted_secs(entry).is_some_and(|p| p < cutoff),
        PruneRule::LastSeenBefore(cutoff) => entry.last_seen_at < cutoff,
        PruneRule::Never => false,
    };
    out_of_window
        && !ctx.reconsider.contains(&entry.source)
        && entry
            .runs
            .last()
            .is_none_or(|run| run.to_seq < ctx.oldest_kept_seq)
        && (entry.missing_count == 0 || entry.disappeared_at.is_some())
}

/// Extends `entry`'s verdict runs with this cycle and records which fields changed since `prior`.
fn record_history(
    entry: &mut SnapshotEntry,
    prior: Option<&SnapshotEntry>,
    candidate: &DiscoveredCandidate,
    ctx: &DiffContext<'_>,
) {
    let step = rating_step(candidate.rating);
    let Some(seq) = ctx.seq else {
        entry.rating_logged = step;
        return;
    };

    if let Some(verdict) = ctx.verdicts.get(&candidate.source) {
        match entry.runs.last_mut() {
            Some(run) if &run.verdict == verdict && run.to_seq + 1 == seq => run.to_seq = seq,
            Some(run) if run.to_seq >= seq => {}
            _ => entry.runs.push(VerdictRun {
                verdict: verdict.clone(),
                from_seq: seq,
                to_seq: seq,
            }),
        }
        entry.last_verdict_at = Some(ctx.now);
    }

    match prior {
        Some(prior) => {
            let mut push = |field, before| {
                entry.changes.push(ValueChange { seq, field, before });
            };
            if !candidate.title.is_empty() && prior.title != candidate.title {
                push(ChangedField::Title, serde_json::json!(prior.title));
            }
            let before: HashSet<&String> = prior.tags.iter().collect();
            let after: HashSet<&String> = candidate.tags.iter().collect();
            if before != after {
                let mut added: Vec<&String> = after.difference(&before).copied().collect();
                let mut removed: Vec<&String> = before.difference(&after).copied().collect();
                added.sort();
                removed.sort();
                push(
                    ChangedField::Tags,
                    serde_json::json!({ "added": added, "removed": removed }),
                );
            }
            // Only what a history row shows: pages and category are not part of one.
            if prior.uploader != candidate.uploader {
                push(ChangedField::Uploader, serde_json::json!(prior.uploader));
            }
            // Entries written before `rating_logged` existed start from their stored rating.
            let logged = prior.rating_logged.or_else(|| rating_step(prior.rating));
            if logged != step {
                push(ChangedField::Rating, serde_json::json!(logged));
            }
            entry.rating_logged = step;
        }
        None => entry.rating_logged = step,
    }

    entry.runs.retain(|run| run.to_seq >= ctx.oldest_kept_seq);
    entry
        .changes
        .retain(|change| change.seq > ctx.oldest_kept_seq);
}

/// The record this entry had in cycle `seq`, rebuilt from its runs and changes. `None` when the
/// work was not in that cycle.
pub fn record_at(entry: &SnapshotEntry, seq: u64) -> Option<CandidateRecord> {
    let run = entry
        .runs
        .iter()
        .find(|run| run.from_seq <= seq && seq <= run.to_seq)?;
    let mut title = entry.title.clone();
    let mut tags: Vec<String> = entry.tags.clone();
    let mut rating = entry.rating;
    let mut uploader = entry.uploader.clone();
    // Newest first, undoing every change made after `seq`.
    for change in entry.changes.iter().rev().filter(|c| c.seq > seq) {
        match change.field {
            ChangedField::Title => {
                title = serde_json::from_value(change.before.clone()).unwrap_or_default();
            }
            ChangedField::Tags => {
                let list = |key: &str| -> Vec<String> {
                    serde_json::from_value(change.before[key].clone()).unwrap_or_default()
                };
                let added = list("added");
                tags.retain(|t| !added.contains(t));
                tags.extend(list("removed"));
            }
            ChangedField::Rating => rating = change.before.as_f64().map(|r| r as f32),
            ChangedField::Uploader => {
                uploader = serde_json::from_value(change.before.clone()).unwrap_or_default();
            }
            ChangedField::Pages | ChangedField::Category => {}
        }
    }
    Some(CandidateRecord {
        source_url: entry.source.clone(),
        title,
        uploader,
        rating,
        posted_at: posted_secs(entry),
        tags,
        verdict: run.verdict.clone(),
    })
}

/// Every work's record in cycle `seq`, newest publication first.
pub fn records_at(entries: &HashMap<String, SnapshotEntry>, seq: u64) -> Vec<CandidateRecord> {
    let mut out: Vec<CandidateRecord> = entries
        .values()
        .filter_map(|entry| record_at(entry, seq))
        .collect();
    out.sort_by(|a, b| {
        b.posted_at
            .cmp(&a.posted_at)
            .then_with(|| a.source_url.cmp(&b.source_url))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(source: &str, title: Option<&str>, tags: &[&str]) -> DiscoveredCandidate {
        DiscoveredCandidate {
            source: source.into(),
            title: title.map_or_else(std::collections::HashMap::new, |t: &str| {
                std::collections::HashMap::from([("origin".to_string(), t.to_string())])
            }),
            posted_at: None,
            rating: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            category: None,
            uploader: None,
            pages: None,
            size: None,
        }
    }

    fn prior(source: &str, title: Option<&str>, tags: &[&str], first_seen: i64) -> SnapshotEntry {
        SnapshotEntry {
            source: source.into(),
            title: title.map_or_else(std::collections::HashMap::new, |t: &str| {
                std::collections::HashMap::from([("origin".to_string(), t.to_string())])
            }),
            posted_at: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            rating: None,
            category: None,
            uploader: None,
            pages: None,
            first_seen_at: first_seen,
            last_seen_at: first_seen,
            disappeared_at: None,
            missing_count: 0,
            no_longer_matching: false,
            title_changed_from: std::collections::HashMap::new(),
            data_version: 1,
            runs: Vec::new(),
            changes: Vec::new(),
            rating_logged: None,
            last_verdict_at: None,
        }
    }

    fn snap(entries: Vec<SnapshotEntry>) -> HashMap<String, SnapshotEntry> {
        entries.into_iter().map(|e| (e.source.clone(), e)).collect()
    }

    const MATCHES: &dyn Fn(&DiscoveredCandidate) -> bool = &|_| true;

    /// The case a high-water mark gets wrong: an old work edited to carry the tag today. It has no
    /// prior entry, so it is new regardless of how old its `posted_at` is.
    #[test]
    fn a_work_absent_from_the_snapshot_is_new_however_old_it_is() {
        let mut old = cand("g/1", Some("t"), &["a"]);
        old.posted_at = Some("2024-01-01 00:00".into());
        let d = diff(&[old], &snap(vec![]), true, 100, MATCHES);
        assert_eq!(d.to_save.len(), 1);
        assert_eq!(
            d.to_save[0].first_seen_at, 100,
            "first seen is now, not posted_at"
        );
        assert!(
            d.changed.is_empty(),
            "a work we never saw has not *changed*"
        );
    }

    #[test]
    fn a_reordered_tag_list_is_not_a_change() {
        let before = snap(vec![prior("g/1", Some("t"), &["b", "a"], 10)]);
        let d = diff(
            &[cand("g/1", Some("t"), &["a", "b"])],
            &before,
            true,
            100,
            MATCHES,
        );
        assert!(d.changed.is_empty(), "same set, different order");
    }

    #[test]
    fn a_changed_tag_set_is_reported_and_first_seen_is_preserved() {
        let before = snap(vec![prior("g/1", Some("t"), &["a"], 10)]);
        let d = diff(
            &[cand("g/1", Some("t"), &["a", "b"])],
            &before,
            true,
            100,
            MATCHES,
        );
        assert_eq!(d.changed.len(), 1);
        assert!(d.changed[0].tags_changed);
        assert!(d.changed[0].title_was.is_empty());
        assert_eq!(
            d.to_save[0].first_seen_at, 10,
            "not reset by a later sighting"
        );
        assert_eq!(d.to_save[0].last_seen_at, 100);
    }

    #[test]
    fn a_changed_title_carries_the_previous_value_rather_than_replacing_it_silently() {
        let before = snap(vec![prior("g/1", Some("old"), &["a"], 10)]);
        let d = diff(
            &[cand("g/1", Some("new"), &["a"])],
            &before,
            true,
            100,
            MATCHES,
        );
        assert_eq!(
            d.changed[0].title_was.get("origin").map(String::as_str),
            Some("old")
        );
        assert_eq!(
            d.to_save[0].title.get("origin").map(String::as_str),
            Some("new")
        );
        assert_eq!(
            d.to_save[0]
                .title_changed_from
                .get("origin")
                .map(String::as_str),
            Some("old")
        );
    }

    /// Still listed, but its tags no longer satisfy the filters — distinct from having disappeared.
    #[test]
    fn a_work_that_stopped_matching_is_flagged_not_removed() {
        let before = snap(vec![prior("g/1", Some("t"), &["a"], 10)]);
        let never: &dyn Fn(&DiscoveredCandidate) -> bool = &|_| false;
        let d = diff(&[cand("g/1", Some("t"), &["a"])], &before, true, 100, never);
        assert!(d.removed.is_empty());
        assert!(d.to_save[0].no_longer_matching);
        assert!(d.changed[0].no_longer_matching);
    }

    /// FR-012e: one absence is not evidence. A single partial listing must not mark a page of works.
    #[test]
    fn one_absence_counts_but_does_not_remove() {
        let before = snap(vec![prior("g/1", Some("t"), &["a"], 10)]);
        let d = diff(&[], &before, true, 100, MATCHES);
        assert!(d.removed.is_empty(), "not on the first absence");
        assert_eq!(d.to_save[0].missing_count, 1);
        assert_eq!(d.to_save[0].disappeared_at, None);
    }

    #[test]
    fn removal_is_recorded_once_the_threshold_is_crossed() {
        let mut p = prior("g/1", Some("t"), &["a"], 10);
        p.missing_count = MISSING_THRESHOLD - 1;
        let d = diff(&[], &snap(vec![p]), true, 100, MATCHES);
        assert_eq!(d.removed, vec!["g/1".to_string()]);
        assert_eq!(d.to_save[0].disappeared_at, Some(100));
    }

    /// Reported on the crossing only — otherwise every later check would re-report the same removal.
    #[test]
    fn an_already_removed_work_is_not_reported_again() {
        let mut p = prior("g/1", Some("t"), &["a"], 10);
        p.missing_count = MISSING_THRESHOLD;
        p.disappeared_at = Some(50);
        let d = diff(&[], &snap(vec![p]), true, 100, MATCHES);
        assert!(d.removed.is_empty());
        assert!(
            d.to_save.is_empty(),
            "nothing to rewrite for a settled entry"
        );
    }

    /// The guard that matters most: a check that could not see properly must not conclude anything
    /// about absence. Its own sightings are still real and are recorded.
    #[test]
    fn a_non_authoritative_check_never_counts_an_absence() {
        let before = snap(vec![prior("g/1", Some("t"), &["a"], 10)]);
        let d = diff(
            &[cand("g/2", Some("u"), &["b"])],
            &before,
            false,
            100,
            MATCHES,
        );
        assert!(d.removed.is_empty());
        assert_eq!(d.to_save.len(), 1, "only the work it did see");
        assert_eq!(d.to_save[0].source, "g/2");
    }

    /// A work that comes back was not removed after all.
    #[test]
    fn reappearing_clears_the_absence_record() {
        let mut p = prior("g/1", Some("t"), &["a"], 10);
        p.missing_count = 2;
        let d = diff(
            &[cand("g/1", Some("t"), &["a"])],
            &snap(vec![p]),
            true,
            100,
            MATCHES,
        );
        assert_eq!(d.to_save[0].missing_count, 0);
        assert_eq!(d.to_save[0].disappeared_at, None);
    }

    // ── Verdict runs and value changes ──────────────────────────────────────────────────────────

    fn history_ctx<'a>(
        seq: u64,
        verdicts: &'a HashMap<String, CandidateVerdict>,
        data_version: u32,
        empty: &'a HashSet<String>,
    ) -> DiffContext<'a> {
        DiffContext {
            authoritative: true,
            now: 1_000 + seq as i64,
            still_matches: MATCHES,
            seq: Some(seq),
            verdicts,
            data_version,
            refreshed: empty,
            window_cutoff: None,
            oldest_kept_seq: 1,
            prune: PruneRule::Never,
            reconsider: empty,
        }
    }

    fn verdicts(source: &str, verdict: CandidateVerdict) -> HashMap<String, CandidateVerdict> {
        HashMap::from([(source.to_string(), verdict)])
    }

    /// Runs one cycle and returns the saved entry for `c`.
    fn cycle(
        previous: Option<SnapshotEntry>,
        c: &DiscoveredCandidate,
        seq: u64,
        verdict: CandidateVerdict,
    ) -> SnapshotEntry {
        let empty = HashSet::new();
        let v = verdicts(&c.source, verdict);
        let previous = snap(previous.into_iter().collect());
        let d = diff_with(
            std::slice::from_ref(c),
            &previous,
            &history_ctx(seq, &v, 1, &empty),
        );
        d.to_save
            .into_iter()
            .find(|e| e.source == c.source)
            .unwrap()
    }

    #[test]
    fn an_unchanged_verdict_extends_one_run_and_a_new_one_starts_another() {
        let c = cand("a", Some("A"), &["x"]);
        let e = cycle(None, &c, 1, CandidateVerdict::AlreadySeen);
        let e = cycle(Some(e), &c, 2, CandidateVerdict::AlreadySeen);
        let e = cycle(Some(e), &c, 3, CandidateVerdict::Queued);
        assert_eq!(
            e.runs,
            vec![
                VerdictRun {
                    verdict: CandidateVerdict::AlreadySeen,
                    from_seq: 1,
                    to_seq: 2
                },
                VerdictRun {
                    verdict: CandidateVerdict::Queued,
                    from_seq: 3,
                    to_seq: 3
                },
            ]
        );
        // A cycle that did not see it breaks the run, so a gap never reads as "seen throughout".
        let e = cycle(Some(e), &c, 5, CandidateVerdict::Queued);
        assert_eq!(e.runs.len(), 3);
    }

    #[test]
    fn an_older_cycle_is_rebuilt_with_the_values_it_saw() {
        let mut c = cand("a", Some("Old title"), &["keep", "gone"]);
        c.rating = Some(3.52);
        c.uploader = Some("u1".into());
        let e = cycle(None, &c, 1, CandidateVerdict::AlreadySeen);

        let mut c2 = cand("a", Some("New title"), &["keep", "new"]);
        c2.rating = Some(4.01);
        c2.uploader = Some("u2".into());
        let e = cycle(Some(e), &c2, 2, CandidateVerdict::Queued);

        let then = record_at(&e, 1).unwrap();
        assert_eq!(
            then.title.get("origin").map(String::as_str),
            Some("Old title")
        );
        let mut tags = then.tags.clone();
        tags.sort();
        assert_eq!(tags, vec!["gone".to_string(), "keep".to_string()]);
        // History keeps ratings at 0.1; the latest cycle shows the exact value.
        assert_eq!(then.rating, Some(3.5));
        assert_eq!(then.uploader.as_deref(), Some("u1"));
        assert_eq!(then.verdict, CandidateVerdict::AlreadySeen);

        let now = record_at(&e, 2).unwrap();
        assert_eq!(now.rating, Some(4.01));
        assert_eq!(now.verdict, CandidateVerdict::Queued);
        assert!(
            record_at(&e, 3).is_none(),
            "not in a cycle that did not see it"
        );
    }

    #[test]
    fn rating_moves_below_a_tenth_are_not_recorded() {
        let mut c = cand("a", None, &[]);
        c.rating = Some(4.01);
        let mut e = cycle(None, &c, 1, CandidateVerdict::AlreadySeen);
        for (seq, r) in [(2, 4.03), (3, 3.98), (4, 4.04)] {
            c.rating = Some(r);
            e = cycle(Some(e), &c, seq, CandidateVerdict::AlreadySeen);
        }
        assert!(e.changes.is_empty(), "{:?}", e.changes);
        // Drift that adds up to a step is recorded, against the last logged value.
        c.rating = Some(4.06);
        let e = cycle(Some(e), &c, 5, CandidateVerdict::AlreadySeen);
        assert_eq!(e.changes.len(), 1);
        assert_eq!(e.rating_logged, Some(4.1));
    }

    #[test]
    fn history_older_than_the_kept_cycles_is_dropped() {
        let c = cand("a", Some("A"), &[]);
        let empty = HashSet::new();
        let v = verdicts("a", CandidateVerdict::AlreadySeen);
        let mut e = cycle(None, &c, 1, CandidateVerdict::Queued);
        e = cycle(Some(e), &c, 2, CandidateVerdict::AlreadySeen);
        let mut ctx = history_ctx(10, &v, 1, &empty);
        ctx.oldest_kept_seq = 5;
        let d = diff_with(std::slice::from_ref(&c), &snap(vec![e]), &ctx);
        assert_eq!(
            d.to_save[0].runs,
            vec![VerdictRun {
                verdict: CandidateVerdict::AlreadySeen,
                from_seq: 10,
                to_seq: 10
            }]
        );
    }

    #[test]
    fn another_data_version_updates_without_reporting_a_change() {
        let mut before = prior("a", Some("A"), &["x"], 10);
        before.data_version = 1;
        let empty = HashSet::new();
        let v = verdicts("a", CandidateVerdict::AlreadySeen);
        let c = cand("a", Some("A (full)"), &["x", "y", "z"]);
        let d = diff_with(
            std::slice::from_ref(&c),
            &snap(vec![before]),
            &history_ctx(1, &v, 2, &empty),
        );
        assert!(d.changed.is_empty());
        let saved = &d.to_save[0];
        assert_eq!(saved.data_version, 2);
        assert!(saved.changes.is_empty());
        assert!(saved.title_changed_from.is_empty());
        assert_eq!(saved.first_seen_at, 10);
    }

    #[test]
    fn a_refreshed_work_does_not_count_as_listed() {
        let mut before = prior("a", None, &["x"], 10);
        before.last_seen_at = 50;
        before.missing_count = 2;
        let refreshed = HashSet::from(["a".to_string()]);
        let empty = HashSet::new();
        let v = verdicts("a", CandidateVerdict::AlreadySeen);
        let mut ctx = history_ctx(3, &v, 1, &empty);
        ctx.refreshed = &refreshed;
        let mut c = cand("a", None, &["x"]);
        c.rating = Some(4.2);
        let d = diff_with(std::slice::from_ref(&c), &snap(vec![before]), &ctx);
        let saved = &d.to_save[0];
        assert_eq!(saved.last_seen_at, 50);
        assert_eq!(saved.missing_count, 2);
        assert_eq!(saved.rating, Some(4.2));
    }

    #[test]
    fn a_work_that_aged_out_of_the_window_is_not_counted_missing() {
        let mut old = prior("old", None, &[], 10);
        old.posted_at = Some("100".into());
        let (v, empty) = (HashMap::new(), HashSet::new());
        let mut ctx = history_ctx(3, &v, 1, &empty);
        ctx.window_cutoff = Some(1_000);
        let d = diff_with(&[], &snap(vec![old]), &ctx);
        assert!(d.to_save.is_empty() && d.removed.is_empty());
    }

    #[test]
    fn pruning_keeps_whatever_is_still_needed() {
        let entry = |source: &str, posted: &str, last_run: Option<u64>| {
            let mut e = prior(source, None, &[], 10);
            e.posted_at = Some(posted.into());
            if let Some(to) = last_run {
                e.runs.push(VerdictRun {
                    verdict: CandidateVerdict::AlreadySeen,
                    from_seq: to,
                    to_seq: to,
                });
            }
            e
        };
        let reconsider = HashSet::from(["rejected".to_string()]);
        let empty_v = HashMap::new();
        let mut ctx = history_ctx(60, &empty_v, 1, &reconsider);
        ctx.reconsider = &reconsider;
        let empty = HashSet::new();
        ctx.refreshed = &empty;
        ctx.oldest_kept_seq = 11;
        ctx.window_cutoff = Some(1_000);
        ctx.prune = PruneRule::PostedBefore(1_000);
        let mut counting = entry("counting", "100", None);
        counting.missing_count = 1;
        let d = diff_with(
            &[],
            &snap(vec![
                entry("gone", "100", Some(5)),
                entry("in-window", "5000", None),
                entry("rejected", "100", None),
                entry("in-history", "100", Some(20)),
                counting,
            ]),
            &ctx,
        );
        assert_eq!(d.to_remove, vec!["gone".to_string()]);
    }

    #[test]
    fn a_refresh_keeps_the_listed_titles() {
        let mut before = prior("a", Some("Listed"), &[], 10);
        before.title.insert("ja".into(), "原題".into());
        let refreshed = HashSet::from(["a".to_string()]);
        let empty = HashSet::new();
        let v = verdicts("a", CandidateVerdict::AlreadySeen);
        let mut ctx = history_ctx(3, &v, 1, &empty);
        ctx.refreshed = &refreshed;
        let mut c = cand("a", None, &[]);
        c.title = HashMap::from([("ja".to_string(), "原題".to_string())]);
        let d = diff_with(std::slice::from_ref(&c), &snap(vec![before]), &ctx);
        assert!(d.changed.is_empty());
        assert!(d.to_save[0].changes.is_empty());
        assert_eq!(
            d.to_save[0].title.get("origin").map(String::as_str),
            Some("Listed")
        );
    }

    #[test]
    fn several_changes_to_one_field_unwind_in_order() {
        let c1 = cand("a", None, &["a", "b"]);
        let c2 = cand("a", None, &["b", "c"]);
        let c3 = cand("a", None, &["c", "d", "a"]);
        let mut e = cycle(None, &c1, 1, CandidateVerdict::AlreadySeen);
        e = cycle(Some(e), &c2, 2, CandidateVerdict::AlreadySeen);
        e = cycle(Some(e), &c3, 3, CandidateVerdict::AlreadySeen);
        let tags_at = |seq| {
            let mut t = record_at(&e, seq).unwrap().tags;
            t.sort();
            t
        };
        assert_eq!(tags_at(1), vec!["a", "b"]);
        assert_eq!(tags_at(2), vec!["b", "c"]);
        assert_eq!(tags_at(3), vec!["a", "c", "d"]);
    }

    #[test]
    fn a_rebuilt_cycle_lists_each_work_once_newest_first() {
        let mut older = cand("older", None, &[]);
        older.posted_at = Some("100".into());
        let mut newer = cand("newer", None, &[]);
        newer.posted_at = Some("200".into());
        let a = cycle(None, &older, 1, CandidateVerdict::AlreadySeen);
        let b = cycle(None, &newer, 1, CandidateVerdict::Queued);
        let rows = records_at(&snap(vec![a, b]), 1);
        let sources: Vec<&str> = rows.iter().map(|r| r.source_url.as_str()).collect();
        assert_eq!(sources, vec!["newer", "older"]);
    }
}
