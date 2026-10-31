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

use std::collections::HashMap;

use lanrurugi_plugin::protocol::DiscoveredCandidate;
use lanrurugi_storage::subscriptions::SnapshotEntry;

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
    }
}

/// Compares `candidates` against `previous`.
///
/// `still_matches` answers, for one candidate, whether the subscription's own filters still accept
/// it — passed in rather than evaluated here so this stays independent of the matcher.
///
/// `authoritative` gates absence bookkeeping entirely: a failed, inconclusive, or
/// challenge-blocked check saw a smaller world than the one asked about, so nothing may be counted
/// as missing from it (FR-012e). Works it *did* return are still recorded, since those sightings are
/// real.
pub fn diff(
    candidates: &[DiscoveredCandidate],
    previous: &HashMap<String, SnapshotEntry>,
    authoritative: bool,
    now: i64,
    still_matches: &dyn Fn(&DiscoveredCandidate) -> bool,
) -> SnapshotDiff {
    let mut out = SnapshotDiff::default();

    for candidate in candidates {
        let prior = previous.get(&candidate.source);
        let matches = still_matches(candidate);

        let mut title_was = std::collections::HashMap::new();
        if let Some(prior) = prior {
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

        out.to_save.push(entry_for(
            candidate,
            prior,
            now,
            !matches,
            // Kept only while unacknowledged; a title that has stopped moving should not keep
            // advertising a change forever.
            if title_was.is_empty() {
                prior
                    .map(|p| p.title_changed_from.clone())
                    .unwrap_or_default()
            } else {
                title_was.clone()
            },
        ));
    }

    if !authoritative {
        return out;
    }

    let present: std::collections::HashSet<&str> =
        candidates.iter().map(|c| c.source.as_str()).collect();

    for (source, prior) in previous {
        if present.contains(source.as_str()) {
            continue;
        }
        // Already settled as removed; counting further absences would re-report it every check.
        if prior.disappeared_at.is_some() {
            continue;
        }

        let missing_count = prior.missing_count + 1;
        let crossed = missing_count >= MISSING_THRESHOLD;
        if crossed {
            out.removed.push(source.clone());
        }
        out.to_save.push(SnapshotEntry {
            missing_count,
            disappeared_at: crossed.then_some(now),
            ..prior.clone()
        });
    }

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
}
