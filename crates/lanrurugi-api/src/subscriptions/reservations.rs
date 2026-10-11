//! The reservation list: matched works that could not be downloaded.
//!
//! Its reason for existing is that "nothing matched" and "everything failed" look identical from
//! the outside. Without this list a subscription that silently fails every cycle is
//! indistinguishable from one with nothing to find — so failures are recorded with their cause and
//! surfaced where the user is already working, beside the download queue.
//!
//! A discarded entry is **kept, not deleted**. Later cycles consult it so the same work is not
//! re-offered every time (FR-016); deleting it would make the user's decision evaporate and the
//! work would come back on the next check.

use lanrurugi_storage::subscriptions::{ReservationEntry, ReservationStatus};

/// Why a matched work could not be downloaded, in terms worth showing a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationReason {
    /// The source could not be reached at all.
    SourceUnreachable,
    /// The source reported the user is out of download credit.
    InsufficientCredit,
    /// Anything else the download path reported.
    DownloadFailed(String),
}

impl ReservationReason {
    /// A stable key the frontend maps to a translated string — never a raw English sentence, since
    /// this surfaces directly in the UI.
    pub fn as_key(&self) -> String {
        match self {
            ReservationReason::SourceUnreachable => "source_unreachable".to_string(),
            ReservationReason::InsufficientCredit => "insufficient_credit".to_string(),
            ReservationReason::DownloadFailed(detail) => format!("download_failed:{detail}"),
        }
    }

    /// Whether this reason means the user's download credit ran out — the signal a subscription's
    /// credit policy branches on (FR-019).
    ///
    /// Reactive by necessity: no source this project talks to exposes a balance to check in
    /// advance, so a failure is the only moment this is knowable.
    pub fn is_credit_exhaustion(&self) -> bool {
        matches!(self, ReservationReason::InsufficientCredit)
    }
}

/// Which reservations a later cycle should treat as settled, so it does not re-offer them.
///
/// Both waiting and discarded entries count: a waiting entry is already on the user's list (adding
/// it twice would be noise), and a discarded one was explicitly refused.
pub fn settled_sources(entries: &[ReservationEntry], subscription_id: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.subscription_id == subscription_id)
        .map(|e| e.source_url.clone())
        .collect()
}

/// Entries eligible for pruning once the list exceeds its cap, oldest first (FR-018).
///
/// Discarded entries are given up before waiting ones: a waiting entry is still a to-do the user
/// has not acted on, whereas a discarded one has already served its purpose of recording a "no".
/// Losing the latter costs only the possibility of re-offering a work the user once refused.
pub fn prune_order(entries: &[ReservationEntry]) -> Vec<&ReservationEntry> {
    let mut sorted: Vec<&ReservationEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| {
        let a_discarded = a.status == ReservationStatus::Discarded;
        let b_discarded = b.status == ReservationStatus::Discarded;
        b_discarded
            .cmp(&a_discarded)
            .then(a.created_at.cmp(&b.created_at))
    });
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        id: &str,
        sub: &str,
        source: &str,
        status: ReservationStatus,
        at: i64,
    ) -> ReservationEntry {
        ReservationEntry {
            id: id.into(),
            subscription_id: sub.into(),
            source_url: source.into(),
            reason: "r".into(),
            status,
            created_at: at,
        }
    }

    #[test]
    fn credit_exhaustion_is_recognised_only_for_that_reason() {
        assert!(ReservationReason::InsufficientCredit.is_credit_exhaustion());
        assert!(!ReservationReason::SourceUnreachable.is_credit_exhaustion());
        assert!(!ReservationReason::DownloadFailed("x".into()).is_credit_exhaustion());
    }

    #[test]
    fn reasons_produce_distinct_stable_keys() {
        let keys = [
            ReservationReason::SourceUnreachable.as_key(),
            ReservationReason::InsufficientCredit.as_key(),
            ReservationReason::DownloadFailed("x".into()).as_key(),
        ];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
    }

    /// Both waiting and discarded entries stop a work being re-offered — for different reasons, but
    /// with the same effect.
    #[test]
    fn settled_sources_covers_waiting_and_discarded_for_that_subscription_only() {
        let entries = vec![
            entry("1", "s1", "a", ReservationStatus::Waiting, 10),
            entry("2", "s1", "b", ReservationStatus::Discarded, 20),
            entry("3", "s2", "c", ReservationStatus::Waiting, 30),
        ];
        let mut got = settled_sources(&entries, "s1");
        got.sort();
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
    }

    /// Discarded entries are surrendered first: a waiting entry is still an unanswered to-do.
    #[test]
    fn pruning_gives_up_discarded_entries_before_waiting_ones() {
        let entries = vec![
            entry("old-waiting", "s", "a", ReservationStatus::Waiting, 1),
            entry("new-discarded", "s", "b", ReservationStatus::Discarded, 99),
        ];
        let order = prune_order(&entries);
        assert_eq!(
            order[0].id, "new-discarded",
            "a discarded entry goes before an older waiting one — the waiting one is still a to-do"
        );
    }

    #[test]
    fn within_the_same_status_the_oldest_goes_first() {
        let entries = vec![
            entry("newer", "s", "a", ReservationStatus::Waiting, 50),
            entry("older", "s", "b", ReservationStatus::Waiting, 10),
        ];
        let order = prune_order(&entries);
        assert_eq!(order[0].id, "older");
    }
}
