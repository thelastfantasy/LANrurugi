//! Decides whether an incoming download is a newer or older revision of something already in the
//! library, from the `version_history` a download plugin reported (issue #107).
//!
//! The judgment lives here, host-side, rather than in each plugin: a plugin only has to enumerate
//! what revisions its site says exist, which is mechanical I/O code. Working out which of them the
//! library already holds, and in which direction, is one algorithm with real edge cases — so it is
//! implemented and tested once here and shared by every plugin, including AI-generated ones
//! (`specs/006-ai-plugin-wizard/`), instead of being reinvented per plugin at varying quality.
//!
//! Every comparison is plain string equality over already-canonicalized sources (the plugin's own
//! `canonicalizeSource`, else the host's generic `trim_url`) — see `plugin-sdk.ts`'s contract on
//! that function for why both sides must normalize identically.

use lanrurugi_plugin::protocol::VersionHistoryEntry;

/// One already-catalogued archive's canonicalized `source:` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CataloguedSource {
    pub archive_id: String,
    /// Already run through the same canonicalization as every `version_history` entry.
    pub source: String,
}

/// How an incoming download relates to whatever the library already holds from the same series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionRelation {
    /// The incoming download is a newer revision than `archive_id`.
    NewerThan {
        archive_id: String,
        /// How many revisions separate the two, per the reported history (1 = directly adjacent).
        hops: usize,
    },
    /// The incoming download is an older revision than `archive_id`.
    OlderThan { archive_id: String, hops: usize },
    /// Nothing in the library appears in this history — an ordinary new archive.
    Unrelated,
}

/// Locates `downloaded_source` within `history`, finds whichever catalogued archive sits nearest to
/// it in that same history, and reports the direction between them.
///
/// `history` need not be sorted; entries are ordered by `posted_at` here. Entries whose `posted_at`
/// does not parse are dropped rather than failing the whole comparison — a single malformed
/// timestamp should degrade to "we know less about this series", not block the download.
///
/// When several catalogued archives appear in the same history (the library legitimately holds two
/// revisions of one series), the one *nearest* the incoming download wins: it is the most specific
/// thing to compare against, and it is what decides whether this download is a step forward or
/// backward from the library's current position.
///
/// Ties — one catalogued archive equally distant on each side — resolve to the **older** one, so the
/// download reports as [`RevisionRelation::NewerThan`]. The comparison below is strictly `<`, so the
/// earlier-indexed (older) candidate is kept; that is a deliberate choice, not an accident of
/// iteration order. It favors the less destructive branch: the newer-side policies at worst replace
/// an archive this download genuinely supersedes, whereas treating the download as older can block
/// it outright (`relative_older_policy: block`) over a relationship that is symmetric anyway.
pub fn classify(
    downloaded_source: &str,
    history: &[VersionHistoryEntry],
    catalogued: &[CataloguedSource],
) -> RevisionRelation {
    let mut ordered: Vec<(&str, i64)> = history
        .iter()
        .filter_map(|e| parse_posted_at(&e.posted_at).map(|ts| (e.source.as_str(), ts)))
        .collect();
    // Stable sort keeps a deterministic order for entries sharing a timestamp, so two revisions
    // posted in the same second never flip direction between runs.
    ordered.sort_by_key(|(_, ts)| *ts);

    let Some(self_index) = ordered.iter().position(|(s, _)| *s == downloaded_source) else {
        // The plugin did not include this download itself in the history it reported, so there is
        // no anchor to measure direction from.
        return RevisionRelation::Unrelated;
    };

    let mut nearest: Option<(usize, &CataloguedSource)> = None;
    for (index, (source, _)) in ordered.iter().enumerate() {
        if index == self_index {
            continue;
        }
        let Some(hit) = catalogued.iter().find(|c| c.source == *source) else {
            continue;
        };
        let distance = index.abs_diff(self_index);
        if nearest.is_none_or(|(best, _)| distance < best.abs_diff(self_index)) {
            nearest = Some((index, hit));
        }
    }

    match nearest {
        None => RevisionRelation::Unrelated,
        Some((index, hit)) if index < self_index => RevisionRelation::NewerThan {
            archive_id: hit.archive_id.clone(),
            hops: self_index - index,
        },
        Some((index, hit)) => RevisionRelation::OlderThan {
            archive_id: hit.archive_id.clone(),
            hops: index - self_index,
        },
    }
}

/// Parses an ISO 8601 timestamp into epoch seconds. Accepts the offset-bearing and `Z` forms
/// `chrono` handles natively, plus a bare `YYYY-MM-DDTHH:MM:SS` (read as UTC) — real plugins
/// converting from a unix-seconds field produce one of these.
fn parse_posted_at(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp());
    }
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str, posted_at: &str) -> VersionHistoryEntry {
        VersionHistoryEntry {
            source: source.to_string(),
            posted_at: posted_at.to_string(),
        }
    }

    fn catalogued(archive_id: &str, source: &str) -> CataloguedSource {
        CataloguedSource {
            archive_id: archive_id.to_string(),
            source: source.to_string(),
        }
    }

    /// a → b → c, oldest to newest.
    fn three_revision_history() -> Vec<VersionHistoryEntry> {
        vec![
            entry("e-hentai.org/g/1/a", "2026-01-01T00:00:00Z"),
            entry("e-hentai.org/g/2/b", "2026-02-01T00:00:00Z"),
            entry("e-hentai.org/g/3/c", "2026-03-01T00:00:00Z"),
        ]
    }

    #[test]
    fn reports_newer_when_the_library_holds_an_earlier_revision() {
        let relation = classify(
            "e-hentai.org/g/3/c",
            &three_revision_history(),
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 2,
            }
        );
    }

    #[test]
    fn reports_older_when_the_library_holds_a_later_revision() {
        let relation = classify(
            "e-hentai.org/g/1/a",
            &three_revision_history(),
            &[catalogued("arc-c", "e-hentai.org/g/3/c")],
        );
        assert_eq!(
            relation,
            RevisionRelation::OlderThan {
                archive_id: "arc-c".to_string(),
                hops: 2,
            }
        );
    }

    #[test]
    fn unsorted_history_is_ordered_by_posted_at_not_array_order() {
        let mut history = three_revision_history();
        history.reverse();
        let relation = classify(
            "e-hentai.org/g/3/c",
            &history,
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 2,
            }
        );
    }

    #[test]
    fn picks_the_nearest_catalogued_revision_when_several_match() {
        // Library holds both a and b; the download is c, which is adjacent to b.
        let relation = classify(
            "e-hentai.org/g/3/c",
            &three_revision_history(),
            &[
                catalogued("arc-a", "e-hentai.org/g/1/a"),
                catalogued("arc-b", "e-hentai.org/g/2/b"),
            ],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-b".to_string(),
                hops: 1,
            }
        );
    }

    /// The shape that slipped through in production (issue #107 follow-up): the library holds a
    /// revision *between* the downloaded one and the chain tip. `classify` handles it correctly —
    /// the real defect was `ehentai.ts` never putting that middle revision in `version_history` at
    /// all, because it walked outward from the seed instead of backtracking from the tip. This
    /// pins the host-side half so a future change can't break it too.
    #[test]
    fn a_catalogued_middle_revision_is_found_between_the_download_and_the_tip() {
        let history = vec![
            entry("e-hentai.org/g/1/a", "2026-01-01T00:00:00Z"),
            // Downloaded.
            entry("e-hentai.org/g/2/b", "2026-02-01T00:00:00Z"),
            // In the library — neither the download's parent nor the chain tip.
            entry("e-hentai.org/g/3/c", "2026-03-01T00:00:00Z"),
            entry("e-hentai.org/g/4/d", "2026-04-01T00:00:00Z"),
        ];
        let relation = classify(
            "e-hentai.org/g/2/b",
            &history,
            &[catalogued("arc-c", "e-hentai.org/g/3/c")],
        );
        assert_eq!(
            relation,
            RevisionRelation::OlderThan {
                archive_id: "arc-c".to_string(),
                hops: 1,
            },
            "a download must be judged older than a catalogued revision that sits between it and the tip"
        );
    }

    #[test]
    fn nearest_match_can_be_in_the_older_direction() {
        // Download is b; library holds a (1 hop back) and nothing after.
        let relation = classify(
            "e-hentai.org/g/2/b",
            &three_revision_history(),
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 1,
            }
        );
    }

    /// Equidistant candidates on both sides resolve to the older one (reported as `NewerThan`) —
    /// see [`classify`]'s own docs for why that side is the safe one to pick.
    #[test]
    fn an_equidistant_tie_resolves_to_the_older_candidate() {
        // Download is b, with a one hop back and c one hop forward — both catalogued.
        let relation = classify(
            "e-hentai.org/g/2/b",
            &three_revision_history(),
            &[
                catalogued("arc-a", "e-hentai.org/g/1/a"),
                catalogued("arc-c", "e-hentai.org/g/3/c"),
            ],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 1,
            }
        );
    }

    /// The tie-break must not depend on which order the catalogue happens to arrive in.
    #[test]
    fn the_tie_break_is_independent_of_catalogue_order() {
        let reversed = classify(
            "e-hentai.org/g/2/b",
            &three_revision_history(),
            &[
                catalogued("arc-c", "e-hentai.org/g/3/c"),
                catalogued("arc-a", "e-hentai.org/g/1/a"),
            ],
        );
        assert_eq!(
            reversed,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 1,
            }
        );
    }

    #[test]
    fn unrelated_when_nothing_in_the_library_appears_in_the_history() {
        let relation = classify(
            "e-hentai.org/g/3/c",
            &three_revision_history(),
            &[catalogued("arc-x", "e-hentai.org/g/99/zzz")],
        );
        assert_eq!(relation, RevisionRelation::Unrelated);
    }

    #[test]
    fn unrelated_when_the_library_is_empty() {
        let relation = classify("e-hentai.org/g/3/c", &three_revision_history(), &[]);
        assert_eq!(relation, RevisionRelation::Unrelated);
    }

    #[test]
    fn unrelated_when_the_plugin_omitted_the_download_itself_from_the_history() {
        let relation = classify(
            "e-hentai.org/g/77/missing",
            &three_revision_history(),
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(relation, RevisionRelation::Unrelated);
    }

    #[test]
    fn a_catalogued_archive_matching_the_download_itself_is_not_a_revision_relation() {
        // Re-downloading the exact same revision is a plain duplicate, handled by the existing
        // content-hash/filename path — not a newer/older judgment.
        let relation = classify(
            "e-hentai.org/g/2/b",
            &three_revision_history(),
            &[catalogued("arc-b", "e-hentai.org/g/2/b")],
        );
        assert_eq!(relation, RevisionRelation::Unrelated);
    }

    #[test]
    fn an_empty_history_yields_no_relation() {
        let relation = classify(
            "e-hentai.org/g/3/c",
            &[],
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(relation, RevisionRelation::Unrelated);
    }

    #[test]
    fn entries_with_unparseable_timestamps_are_skipped_not_fatal() {
        let history = vec![
            entry("e-hentai.org/g/1/a", "2026-01-01T00:00:00Z"),
            entry("e-hentai.org/g/2/b", "not a timestamp"),
            entry("e-hentai.org/g/3/c", "2026-03-01T00:00:00Z"),
        ];
        // b drops out, so c is 1 hop from a rather than 2.
        let relation = classify(
            "e-hentai.org/g/3/c",
            &history,
            &[catalogued("arc-a", "e-hentai.org/g/1/a")],
        );
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 1,
            }
        );
    }

    #[test]
    fn accepts_offset_bearing_and_bare_iso_timestamps() {
        let history = vec![
            entry("a", "2026-01-01T09:00:00+09:00"),
            entry("b", "2026-01-01T00:00:01"),
        ];
        // 09:00+09:00 is 00:00:00Z, so `a` precedes `b` by one second.
        let relation = classify("b", &history, &[catalogued("arc-a", "a")]);
        assert_eq!(
            relation,
            RevisionRelation::NewerThan {
                archive_id: "arc-a".to_string(),
                hops: 1,
            }
        );
    }
}
