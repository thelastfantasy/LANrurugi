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

pub mod api;
pub mod matcher;
pub mod reservations;
pub mod runner;
pub mod scheduler;
