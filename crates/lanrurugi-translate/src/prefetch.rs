//! Sliding-window look-ahead scheduling and its budget cap (T039/T043/T058; FR-011/FR-013/FR-020).
//!
//! Look-ahead exists so turning the page doesn't wait on translation. Two properties are
//! load-bearing:
//!
//! - **Budget-capped for metered backends** (FR-013, and the constitution's cost-aware-defaults
//!   rule): automatic look-ahead must never quietly spend past a user's limit. A locally-hosted
//!   backend has no marginal cost and is not capped here.
//! - **Per-page failure isolation** (FR-020): the scheduler holds no shared mutable state that
//!   could let one page's failure cascade. Each page's outcome is recorded independently, and a
//!   failed page is simply marked failed — it never poisons the window.

use std::collections::{HashMap, HashSet};

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::entities::PageNumber;

use crate::adapter::TranslationError;
use crate::budget::UsageSnapshot;

/// Per-page look-ahead state.
#[derive(Debug, Clone, PartialEq)]
pub enum PageState {
    /// Queued but not started.
    Queued,
    /// Currently being translated.
    InFlight,
    /// Translated and ready.
    Ready,
    /// Failed — the reader shows the original page with a per-page indicator (FR-019).
    Failed(String),
    /// Skipped because the budget was exhausted (FR-013). Distinct from a failure: nothing went
    /// wrong, the user's own cap was reached.
    BudgetExhausted,
}

/// Which pages a look-ahead window should cover.
///
/// Forward-only: readers move forward, and translating backwards would spend budget on pages the
/// user has already passed.
pub fn window_pages(current: PageNumber, lookahead: u32, total_pages: u32) -> Vec<PageNumber> {
    if total_pages == 0 || lookahead == 0 {
        return Vec::new();
    }
    let start = current.get().saturating_add(1);
    let end = current.get().saturating_add(lookahead).min(total_pages);

    (start..=end).map(PageNumber).collect()
}

/// Tracks look-ahead for one reading session.
///
/// Scoped to a single archive: abandoning it on navigate-away (FR-015) is just dropping this
/// value, and no state outlives the session to leak into the next one.
#[derive(Debug, Default)]
pub struct PrefetchScheduler {
    states: HashMap<(String, u32), PageState>,
    in_flight: HashSet<(String, u32)>,
}

impl PrefetchScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(archive_id: &ArchiveId, page: PageNumber) -> (String, u32) {
        (archive_id.as_str().to_string(), page.get())
    }

    pub fn state(&self, archive_id: &ArchiveId, page: PageNumber) -> Option<&PageState> {
        self.states.get(&Self::key(archive_id, page))
    }

    /// Pages in the window that still need work.
    ///
    /// A page that is already ready, in flight, or previously failed is not re-queued: retrying a
    /// failure automatically would re-bill for something already known to be broken, and the user
    /// can still read the original page meanwhile (FR-019).
    pub fn pages_to_schedule(
        &self,
        archive_id: &ArchiveId,
        current: PageNumber,
        lookahead: u32,
        total_pages: u32,
    ) -> Vec<PageNumber> {
        window_pages(current, lookahead, total_pages)
            .into_iter()
            .filter(|page| !self.states.contains_key(&Self::key(archive_id, *page)))
            .collect()
    }

    /// Whether metered look-ahead may proceed.
    ///
    /// `is_metered` is false for a locally-hosted backend, which has no cost to cap (research.md
    /// §10). Only *automatic* look-ahead is gated: a page the user is actually looking at is an
    /// explicit action, not background spend (FR-013/SC-005).
    pub fn may_prefetch(is_metered: bool, usage: &UsageSnapshot) -> bool {
        !is_metered || !usage.is_exhausted()
    }

    pub fn mark_queued(&mut self, archive_id: &ArchiveId, page: PageNumber) {
        self.states
            .insert(Self::key(archive_id, page), PageState::Queued);
    }

    pub fn mark_in_flight(&mut self, archive_id: &ArchiveId, page: PageNumber) {
        let key = Self::key(archive_id, page);
        self.in_flight.insert(key.clone());
        self.states.insert(key, PageState::InFlight);
    }

    pub fn mark_ready(&mut self, archive_id: &ArchiveId, page: PageNumber) {
        let key = Self::key(archive_id, page);
        self.in_flight.remove(&key);
        self.states.insert(key, PageState::Ready);
    }

    /// Records a page-level failure. Isolated by construction: this touches only this page's entry
    /// (FR-020).
    pub fn mark_failed(
        &mut self,
        archive_id: &ArchiveId,
        page: PageNumber,
        error: &TranslationError,
    ) {
        let key = Self::key(archive_id, page);
        self.in_flight.remove(&key);
        self.states
            .insert(key, PageState::Failed(error.kind().to_string()));
    }

    pub fn mark_budget_exhausted(&mut self, archive_id: &ArchiveId, page: PageNumber) {
        self.states
            .insert(Self::key(archive_id, page), PageState::BudgetExhausted);
    }

    /// Abandons everything in flight (FR-015) — called when the reader closes or navigates away, so
    /// look-ahead can't keep accruing cost for pages nobody will see.
    pub fn abandon_all(&mut self) {
        self.in_flight.clear();
        self.states
            .retain(|_, state| !matches!(state, PageState::InFlight | PageState::Queued));
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive() -> ArchiveId {
        ArchiveId::from("abc")
    }

    fn usage(today: u64, limit: Option<u64>) -> UsageSnapshot {
        UsageSnapshot {
            consumption_today: today,
            limit,
            ..Default::default()
        }
    }

    #[test]
    fn the_window_looks_forward_only() {
        let pages = window_pages(PageNumber(5), 3, 100);
        assert_eq!(pages, vec![PageNumber(6), PageNumber(7), PageNumber(8)]);
    }

    #[test]
    fn the_window_is_clamped_to_the_last_page() {
        let pages = window_pages(PageNumber(9), 5, 10);
        assert_eq!(pages, vec![PageNumber(10)]);
    }

    #[test]
    fn no_window_on_the_last_page_or_with_lookahead_disabled() {
        assert!(window_pages(PageNumber(10), 3, 10).is_empty());
        assert!(window_pages(PageNumber(1), 0, 10).is_empty());
        assert!(window_pages(PageNumber(1), 3, 0).is_empty());
    }

    #[test]
    fn a_metered_backend_stops_prefetching_at_the_limit() {
        assert!(!PrefetchScheduler::may_prefetch(
            true,
            &usage(100, Some(100))
        ));
        assert!(PrefetchScheduler::may_prefetch(true, &usage(99, Some(100))));
    }

    #[test]
    fn a_local_backend_is_never_budget_capped() {
        // No marginal cost, so nothing to cap (research.md §10).
        assert!(PrefetchScheduler::may_prefetch(
            false,
            &usage(10_000, Some(1))
        ));
    }

    #[test]
    fn already_handled_pages_are_not_rescheduled() {
        let mut s = PrefetchScheduler::new();
        s.mark_ready(&archive(), PageNumber(6));
        s.mark_failed(
            &archive(),
            PageNumber(7),
            &TranslationError::Unreachable("x".into()),
        );

        let todo = s.pages_to_schedule(&archive(), PageNumber(5), 3, 100);

        assert_eq!(todo, vec![PageNumber(8)], "ready and failed pages stay put");
    }

    #[test]
    fn one_pages_failure_does_not_affect_another() {
        // FR-020's isolation, asserted directly.
        let mut s = PrefetchScheduler::new();
        s.mark_ready(&archive(), PageNumber(6));
        s.mark_failed(&archive(), PageNumber(7), &TranslationError::RateLimited);

        assert_eq!(s.state(&archive(), PageNumber(6)), Some(&PageState::Ready));
        assert!(matches!(
            s.state(&archive(), PageNumber(7)),
            Some(PageState::Failed(_))
        ));
    }

    #[test]
    fn failures_in_one_archive_do_not_affect_another() {
        let mut s = PrefetchScheduler::new();
        let other = ArchiveId::from("other");
        s.mark_failed(&archive(), PageNumber(1), &TranslationError::AuthFailed);
        s.mark_ready(&other, PageNumber(1));

        assert_eq!(s.state(&other, PageNumber(1)), Some(&PageState::Ready));
    }

    #[test]
    fn abandoning_clears_in_flight_work_but_keeps_finished_results() {
        let mut s = PrefetchScheduler::new();
        s.mark_ready(&archive(), PageNumber(6));
        s.mark_in_flight(&archive(), PageNumber(7));
        s.mark_queued(&archive(), PageNumber(8));

        s.abandon_all();

        assert_eq!(s.in_flight_count(), 0);
        assert_eq!(s.state(&archive(), PageNumber(6)), Some(&PageState::Ready));
        assert_eq!(s.state(&archive(), PageNumber(7)), None);
        assert_eq!(s.state(&archive(), PageNumber(8)), None);
    }

    #[test]
    fn budget_exhaustion_is_distinct_from_failure() {
        let mut s = PrefetchScheduler::new();
        s.mark_budget_exhausted(&archive(), PageNumber(6));
        assert_eq!(
            s.state(&archive(), PageNumber(6)),
            Some(&PageState::BudgetExhausted)
        );
    }
}
