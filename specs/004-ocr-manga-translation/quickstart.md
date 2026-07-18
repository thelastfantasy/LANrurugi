# Quickstart: Validating On-Page Manga Translation (Phase 2)

Prerequisites: a working Phase 1 LANrurugi instance (`specs/001-lanrurugi-full-rewrite`) with a
library already loaded. At least one cloud-hosted provider (API key) and one locally-hosted model
(e.g. Ollama running on the machine you'll browse from) configured for testing both paths.

## 1. Core translation (US1)

```
PUT /translation/settings   # select a cloud provider + target language
```
Open an archive containing dialogue/text.

**Expected**: translated text appears positioned over the original regions (SC-001). Disable
translation and confirm reading behaves identically to a build without this feature (SC-002).
Inspect network traffic to confirm no provider credential is ever sent to or stored in the
browser (FR-006).

## 2. Font fidelity (US2)

Read several consecutive pages of the same volume with translation enabled.

**Expected**: after the volume's `Volume Font Pattern` locks (data-model.md), subsequent pages
use a small, consistent set of matched fonts (SC-003), not one generic font. Include a
deliberately different-looking page (e.g. a flashback) and confirm it doesn't visibly disturb the
matched pattern on later, normal pages (research.md §4's separate meltdown tally).

## 3. Prefetch responsiveness (US3)

Read forward through several pages with look-ahead enabled.

**Expected**: pages within the look-ahead window show no visible delay (SC-004). Deliberately
turn pages faster than look-ahead can keep up and confirm the FR-012 behavior: original page
immediately, small non-obscuring loading indicator, seamless swap once ready — not a blocked page,
not a full-page spinner. Check `GET /translation/usage` and confirm the page/archive/day/week
breakdown (FR-014).

## 4. Locally-hosted backend (US4)

Configure a locally-hosted backend per the documented zero-extra-install path (research.md §9,
option 1). Enable translation with it selected.

**Expected**: pages translate using the local model, composited client-side (SC-006;
`contracts/client-compositing-cache.md`). Then block the connection (e.g. via browser devtools)
and confirm the FR-018 guided-fallback message appears rather than a generic error.

## 5. Resilience (US5)

Point the selected backend at an unreachable/erroring endpoint.

**Expected**: the original page remains readable immediately, with a clear per-page indication
that translation is unavailable (SC-007), and other pages/archives are unaffected.

## Non-goals for this quickstart

RTL target languages (out of scope this phase, research.md §11) and anything from Phase 1's own
`quickstart.md` (already validated separately) are not exercised here.
