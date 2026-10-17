//! The subscription orchestrator (issue #55, `specs/010-subscription-orchestrator/`).
//!
//! A standing-rule engine: it periodically asks a source extension "what is new that matches this?",
//! filters the answers against the user's rules, and hands survivors to the download queue that
//! already exists. It holds no site knowledge of its own — that lives in extensions — and opens no
//! second download path.
//!
//! Three things shape every decision in here:
//!
//! 1. **Discovery is new.** Existing extension entry points act on one already-known item. Asking a
//!    site what is new is the only genuinely new capability this feature adds.
//! 2. **A missing sign-in shrinks the world silently.** Many sources answer a signed-out search with
//!    fewer results rather than an error, so such a check *succeeds* while under-reporting. It is
//!    therefore recorded as inconclusive and never allowed to mark works as seen.
//! 3. **Unattended spending is the real risk.** Matched works wait for confirmation unless the user
//!    turned that off for a given subscription.

pub mod ai_condition;
pub mod api;
pub mod matcher;
pub mod reservations;
pub mod runner;
pub mod scheduler;
pub mod snapshot;

/// Layers a subscription's own [`ConflictPolicy`] onto the download plugin's resolved
/// `overwrite_on_duplicate` — the value frozen onto the queue item at enqueue time.
///
/// `AutoRename`/`Discard` force it *off* rather than leaving the plugin's own answer in place: the
/// plugin option and the global `replacedupe` both resolve byte-level collisions silently, so with
/// either of them on, a collision never becomes a staged conflict and the policy would never run.
/// A subscription that asked to rename or drop is asking for exactly that decision point.
pub fn overwrite_under_conflict_policy(
    policy: lanrurugi_storage::subscriptions::ConflictPolicy,
    plugin_resolved: bool,
) -> bool {
    use lanrurugi_storage::subscriptions::ConflictPolicy;
    match policy {
        ConflictPolicy::Overwrite => true,
        ConflictPolicy::AutoRename | ConflictPolicy::Discard => false,
        ConflictPolicy::Ask => plugin_resolved,
    }
}

#[cfg(test)]
mod tests {
    use lanrurugi_storage::subscriptions::ConflictPolicy;

    /// `Overwrite` forces the overwrite on; `AutoRename`/`Discard` force it *off*, or the plugin's
    /// own option (and the global `replacedupe`) would resolve the collision silently and the
    /// policy would never see a conflict to act on. `Ask` leaves the plugin's answer alone.
    #[test]
    fn a_conflict_policy_decides_whether_a_collision_reaches_it() {
        assert!(super::overwrite_under_conflict_policy(
            ConflictPolicy::Overwrite,
            false
        ));
        assert!(!super::overwrite_under_conflict_policy(
            ConflictPolicy::AutoRename,
            true
        ));
        assert!(!super::overwrite_under_conflict_policy(
            ConflictPolicy::Discard,
            true
        ));
        assert!(super::overwrite_under_conflict_policy(
            ConflictPolicy::Ask,
            true
        ));
        assert!(!super::overwrite_under_conflict_policy(
            ConflictPolicy::Ask,
            false
        ));
    }
}
