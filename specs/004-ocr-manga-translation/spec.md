# Feature Specification: On-Page Manga Translation (Phase 2)

**Feature Branch**: `004-ocr-manga-translation`

**Created**: 2026-07-06

**Status**: Draft

**Input**: User description: "Phase 2 for LANrurugi: on-page manga translation. Detect text regions on manga pages via batched OCR with geometric line/paragraph merging, translate detected text through a user-selected LLM backend (an OpenAI-compatible provider, Anthropic, or a locally-hosted model such as Ollama), and render the translated text back onto the page as an overlay rendered client-side in the browser. Include a volume-level font-matching cache so the system does not need to run an expensive font-recognition model on every single text block ... Include sliding-window prefetch so upcoming pages are translated ahead of the reader, with look-ahead aggressiveness capped by cost for metered cloud backends. Cloud LLM calls must be proxied through the backend server so provider credentials never reach the browser; calls to a locally-hosted model originate from the browser itself ... This phase is deliberately independent of and must not block Phase 1 ..."

**Relationship to Phase 1**: This feature depends on the library/reader delivered by
`specs/001-lanrurugi-full-rewrite` (Phase 1) being in place — it adds an optional layer on top of
reading, it does not replace or modify how archives are browsed, catalogued, or read without
translation enabled. Per constitution Principle VI, this feature's planning and delivery timeline
is independent of, and must not reopen or block, Phase 1. Phase 1's spec already reserved
placeholder User Stories 9–10 for this work at a summary level; this document is the detailed,
standalone specification for it, superseding those placeholders' level of detail (Phase 1's
spec.md is unchanged and still accurately describes this feature at the summary level it needs).
Prior exploratory technical thinking that informed this spec lives in
`specs/001-lanrurugi-full-rewrite/phase2-design-notes.md`.

## Clarifications

### Session 2026-07-06

- Q: The spec never states how the translation *target* language is chosen. → A: The user
  explicitly selects a target language, presented as a separate option alongside (not merged
  into) the interface-language setting from Phase 1's User Story 7, in the same settings screen.
  If the user has not explicitly set a target language, it falls back to the browser's own
  language setting rather than the interface language.
- Q: What exactly does "Translation Cache Entry" cache, and should it vary by backend/target
  language? → A: There are two distinct caches, not one. (1) The **Volume Font Pattern** cache
  holds the recognized body-text font(s) for a volume so the font classifier doesn't need to
  re-run per block; its voting sample explicitly **excludes cover page(s)**, since covers
  typically use stylized title/logo lettering unrepresentative of the body text font(s). (2) The
  **Translation Cache Entry** cache holds the translated output for a page — potentially a fully
  pre-rendered image with the translation already burned in — produced by background
  pre-processing of upcoming pages while the user reads (the look-ahead prefetch from User Story
  3, running concurrently rather than blocking the reader). Because a change in backend or target
  language changes the output, cache entries are keyed by (page, target language, backend); a
  change to either invalidates reuse of a prior entry rather than silently serving stale output.
- Q: The usage budget requirement (now FR-013, numbered FR-011 at the time of this question) has
  no visibility requirement — can the user see current consumption, or only find out when the
  limit is hit? → A: Users MUST be able to view current consumption
  against their budget at any time, broken down by current page, current archive, today, and the
  current week, and the system SHOULD provide a chart-style visualization of this breakdown over
  time (a "nice to have," not a hard requirement).
- Q: What does the reader show for a page reached before its look-ahead translation has finished
  (not failed, just not yet ready)? → A: Show the original untranslated page immediately, together
  with a loading indicator that does not obscure the original content (or obscures it minimally),
  then seamlessly replace it with the translated version once ready — no blocking wait and no
  indicator-free "it just quietly changes later" behavior either.
- Q: Where does a backend selection get stored, and does it carry across the user's devices? → A:
  It depends on the backend category. A locally-hosted backend selection (e.g. pointing at
  `127.0.0.1`) is stored client-side (per device/browser), since it is only meaningful on the
  device it was configured on and would be actively wrong if it silently applied on a different
  device. A cloud-hosted/API-key backend selection is stored server-side (consistent with
  Principle V's credential handling) and applies across all of the user's devices. On any given
  device, a client-side (local) selection takes precedence over the server-stored default — i.e.
  if a device has its own local backend configured, that device uses it instead of the
  account-wide cloud default, without needing to change the server-side setting to do so.
- Q: Which target languages are in scope? → A: Left-to-right (LTR) languages only for this phase.
  Right-to-left (RTL) language layout (e.g. Arabic, Hebrew) is explicitly out of scope and MAY be
  added later without being a breaking change to this spec.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Read a page with on-page translation (Priority: P1)

A user reading an archive in a language they don't read enables translation and picks which
translation service to use — a cloud-hosted provider or a model running on their own device — so
the page's dialogue and text appear translated, without leaving the reader or breaking their
reading flow.

**Why this priority**: This is the core value of the entire feature; every other story refines
this one.

**Independent Test**: Enable translation, select a backend, open an archive, and confirm the
page's text renders translated in place, in the same reading view used without translation.

**Acceptance Scenarios**:

1. **Given** translation is enabled with a backend selected, **When** the user opens a page
   containing dialogue/text, **Then** the translated text appears positioned over the original
   text regions.
2. **Given** translation is disabled, **When** the user reads any archive, **Then** the experience
   is identical to reading without this feature installed at all (no added latency, no altered
   UI).
3. **Given** a cloud translation backend is selected, **When** pages are translated, **Then** the
   user's credentials for that backend are never visible to, retrievable from, or stored in the
   browser.
4. **Given** the user has not explicitly set a target language, **When** a page is translated,
   **Then** it translates into the browser's own language setting; **Given** the user has
   explicitly set a target language (separately from the Phase 1 interface-language setting),
   **When** a page is translated, **Then** it translates into that explicitly chosen language.

---

### User Story 2 - Translated text looks like it belongs on the page (Priority: P2)

While reading with translation enabled, the translated text renders in a font visually consistent
with the surrounding artwork/lettering style of that volume, rather than a single generic font
pasted over every page regardless of context.

**Why this priority**: Directly affects immersion and readability; without it, translation works
but looks visibly "bolted on." It refines User Story 1 rather than being required for translation
to be minimally useful.

**Independent Test**: Read several consecutive pages of the same volume with translation enabled
and confirm rendered text uses a small, consistent set of fonts appropriate to that volume (e.g.
dialogue vs. sound-effect text visually distinguished), not one single font for every text block
regardless of its original style.

**Acceptance Scenarios**:

1. **Given** several pages of the same volume have been read with translation enabled, **When**
   the system has processed enough text to be confident about the volume's dominant lettering
   styles, **Then** subsequent pages render translated text using a small, consistent set of
   matched fonts rather than reprocessing font style from scratch on every block.
2. **Given** a text block whose visual style is a clear outlier from the volume's established
   pattern (e.g. a flashback page lettered differently), **When** it is translated, **Then** its
   rendered font is re-evaluated for that block specifically rather than forced into the
   volume's normal pattern, and this outlier does not measurably degrade font matching quality on
   later, normal pages of the same volume.

---

### User Story 3 - Reading stays fast while translation is on (Priority: P2)

While reading with translation enabled, upcoming pages are translated ahead of time so the user
doesn't wait when turning the page, while automatic look-ahead activity against a paid backend
stays within a budget the user controls.

**Why this priority**: Refines the core experience (User Story 1) so translation doesn't feel
like it's slowing down reading, but reading remains functional (just not prefetched) without it.

**Independent Test**: Read forward through several pages with translation and look-ahead enabled
and confirm translated pages are ready by the time the user reaches them; separately, confirm that
with a metered backend selected, look-ahead activity respects a configured limit.

**Acceptance Scenarios**:

1. **Given** translation and look-ahead are enabled, **When** the user turns to the next page
   within the look-ahead window, **Then** the translated version is already available with no
   visible delay.
2. **Given** a metered (paid) translation backend is selected, **When** the user is reading,
   **Then** automatic look-ahead activity does not exceed the user's configured usage budget
   without explicit confirmation.
3. **Given** the user closes the reader or navigates away, **When** look-ahead translations were
   still in flight, **Then** those in-flight requests are abandoned rather than continuing to
   accrue cost or work in the background indefinitely.
4. **Given** the user reaches a page faster than its look-ahead translation could complete,
   **When** that page is displayed, **Then** the original untranslated content appears immediately
   with a loading indicator that does not obscure it (or obscures it minimally), and the
   translated version replaces it seamlessly once ready.
5. **Given** a metered backend is selected, **When** the user checks their usage at any time,
   **Then** they can see consumption broken down by current page, current archive, today, and the
   current week.

---

### User Story 4 - Using a locally-hosted model works without installing extra software (Priority: P3)

A user who runs a translation model on their own device sets it up so the reader can reach it
directly, without needing to download and run a separate companion program, in the common case.

**Why this priority**: Meaningfully improves adoption of the "run it yourself, free" option, but
the feature is still usable via a cloud backend (User Story 1) if this path doesn't work for a
given user's setup.

**Independent Test**: Configure a locally-hosted model per the documented zero-extra-install path,
enable translation with it selected, and confirm pages translate using that local model. Then
simulate that path being blocked and confirm the guidance shown leads to a working alternative
(guided fallback) without needing to consult external documentation.

**Acceptance Scenarios**:

1. **Given** a locally-hosted model is configured per the standard documented settings (no
   additional program installed), **When** translation is enabled with it selected, **Then**
   pages translate successfully.
2. **Given** the connection to the locally-hosted model cannot be established (e.g. due to
   browser network-access restrictions), **When** the user attempts to use it, **Then** the
   interface shows a clear, actionable explanation and a documented next step, rather than a
   silent failure or a generic error.

---

### User Story 5 - Translation failures never take down reading (Priority: P3)

If a translation backend is slow, unreachable, or returns an error, the user can still read the
untranslated page immediately, and is told clearly what went wrong for that page, without the
issue affecting other pages or the rest of the application.

**Why this priority**: A safety net for the whole feature — protects the reading experience
(Phase 1's core value) from being degraded by a Phase 2 feature going wrong, consistent with the
non-blocking relationship between the two phases.

**Independent Test**: Point the selected backend at an unreachable/erroring endpoint, open a page,
and confirm the original (untranslated) page is still readable immediately, with a clear
per-page error indicator, and that reading other pages/archives is unaffected.

**Acceptance Scenarios**:

1. **Given** the selected translation backend is unreachable, **When** the user opens a page,
   **Then** the original page is shown immediately with a clear indication that translation is
   unavailable for it, not a blocked or broken page.
2. **Given** one page's translation fails, **When** the user continues reading, **Then**
   subsequent pages and other archives are unaffected.

---

### Edge Cases

- What happens when the same page is translated twice in a row (e.g. re-opened later)? Cached
  results should be reused rather than re-translating and re-billing a metered backend for
  identical work.
- What happens when a manga page has no detectable text at all? The page displays normally with
  no translation overlay and no error state.
- What happens when translation is enabled but no backend has been configured/selected yet? The
  user is guided to configuration rather than seeing pages silently fail to translate.
- What happens when a user switches backends mid-session? In-flight requests to the previous
  backend are not mixed with the new one; the change takes effect from the next translation
  request onward.
- What happens when the font-matching cache's "locked" font set turns out to be wrong for an
  entire volume (e.g. locked too early on unrepresentative early pages)? The user can reset the
  volume's font cache and let it re-vote, rather than being stuck with a bad match for the whole
  volume.
- What happens when a locally-hosted model is reachable but responds with a malformed or empty
  translation? That page is treated the same as a translation failure (User Story 5), not shown
  as if it were successfully translated with blank/garbled text.
- What happens when a user reaches a page before its look-ahead translation has finished (not
  failed, just not ready yet)? The original page displays immediately with a non-obscuring (or
  minimally obscuring) loading indicator, then the translated version replaces it seamlessly once
  ready — this is distinct from the failure case above.
- What happens when the user switches target language mid-session? Cached translations keyed to
  the previous language are not reused; new pages translate into the newly selected language.

## Requirements *(mandatory)*

### Functional Requirements

**Core translation (supports User Story 1)**

- **FR-001**: System MUST allow a user to enable or disable on-page translation independently of
  all other reading functionality.
- **FR-002**: System MUST allow a user to select which category of translation backend to use,
  offering at least one cloud-hosted option and one locally-hosted option.
- **FR-003**: A locally-hosted backend selection MUST be stored client-side (per device/browser),
  since it is only meaningful on the device it was configured on. A cloud-hosted/API-key backend
  selection MUST be stored server-side and apply across the user's devices. On a device with its
  own locally-hosted backend selection configured, that selection MUST take precedence over the
  server-stored default for that device.
- **FR-004**: System MUST allow a user to explicitly select a target language for translation,
  presented as a distinct setting from Phase 1's interface-language setting (US7) though reachable
  from the same settings screen; if the user has not explicitly set one, the target language MUST
  default to the browser's own language setting. Only left-to-right (LTR) target languages are in
  scope for this phase; right-to-left (RTL) languages are excluded.
- **FR-005**: System MUST render translated text positioned over the corresponding original text
  regions on the page being read.
- **FR-006**: When a cloud-hosted backend is used, the system MUST hold the user's credentials for
  that backend server-side only, and MUST NOT expose or store them in the browser.
- **FR-007**: Disabling translation MUST fully restore the reading experience to its
  pre-translation behavior and performance, with no residual latency or resource cost.

**Visual/font fidelity (supports User Story 2)**

- **FR-008**: System MUST render translated text using a font drawn from a small set matched to
  the volume's established body-text lettering style, once enough of that volume has been
  processed to establish that style with confidence, rather than a single fixed font applied
  uniformly regardless of context. Cover page(s) MUST be excluded from the sample used to
  establish this style, since covers typically use stylized title/logo lettering unrepresentative
  of body-text dialogue.
- **FR-009**: System MUST be able to identify individual text blocks whose style is a clear
  outlier from the volume's established pattern and handle them without allowing that occurrence
  to degrade the established pattern's accuracy for subsequent, normal text blocks in the same
  volume.
- **FR-010**: Users MUST be able to reset a volume's established font pattern and have it be
  re-established from scratch, in case it was set incorrectly.

**Performance/prefetch (supports User Story 3)**

- **FR-011**: System MUST translate a configurable number of upcoming pages ahead of the reader's
  current position while reading, so turning the page does not require waiting for translation to
  complete, in the common case.
- **FR-012**: When a page is reached before its look-ahead translation has completed (not
  failed, simply not yet ready), the system MUST display that page's original, untranslated
  content immediately together with a loading indicator that does not obscure — or minimally
  obscures — that content, then seamlessly replace it with the translated version once ready.
- **FR-013**: System MUST let users limit automatic look-ahead translation activity for
  metered/paid backends to avoid unexpected usage charges.
- **FR-014**: System MUST let users view their current usage/consumption against their configured
  usage budget for a metered backend at any time, not only when the budget is reached, broken down
  at minimum by current page, current archive, today, and the current week. The system SHOULD
  additionally provide a chart-style visualization of this breakdown over time.
- **FR-015**: In-flight look-ahead translation requests MUST be abandoned, not left running
  indefinitely, when the user navigates away before they complete.
- **FR-016**: A page's translation result MUST be cached, keyed by the combination of page,
  target language, and backend, and reused on subsequent views under that same combination rather
  than being re-requested from the translation backend every time; a change to the target
  language or backend MUST NOT reuse a cache entry produced under a different combination.

**Local-model connectivity (supports User Story 4)**

- **FR-017**: System MUST support a locally-hosted translation backend reachable directly from the
  user's browser without requiring the user to install a separate companion application, for the
  common/documented configuration case.
- **FR-018**: When the browser cannot establish a connection to a configured locally-hosted
  backend, the system MUST present a clear, actionable explanation and next step, rather than a
  silent or generic failure.

**Resilience (supports User Story 5)**

- **FR-019**: If translation cannot be completed for a given page (backend unreachable, error, or
  malformed/empty response), the system MUST still display that page's original, untranslated
  content immediately, with a clear per-page indication that translation is unavailable.
- **FR-020**: A translation failure for one page or one archive MUST NOT affect the ability to
  read other pages or other archives.
- **FR-021**: System MUST allow a user to enable translation without having pre-configured a
  backend, and MUST guide the user to configuration rather than silently failing in that state.

### Key Entities

- **Translation Backend Selection**: A user's choice of translation provider category (cloud vs.
  locally-hosted) and its non-secret connection details; any credential material is a protected
  reference, never a visible/exportable attribute (mirrors `specs/001-lanrurugi-full-rewrite`'s
  Key Entities of the same name). Storage location and cross-device portability differ by
  category: locally-hosted selections are client-side/per-device, cloud selections are
  server-side/account-wide, and a device's local selection takes precedence when present (FR-003).
- **Target Language Preference**: The user's explicitly-selected translation target language
  (limited to LTR languages this phase), or an unset state that falls back to the browser's own
  language setting; distinct from Phase 1's interface-language setting even though configured from
  the same settings screen (FR-004).
- **Detected Text Region**: A recognized, merged block of text on a page, with its position and
  the text it contains, used as the unit of translation and of font-style matching.
- **Volume Font Pattern**: The established, small set of body-text fonts associated with a given
  volume once enough non-cover pages have been processed with confidence, plus the ability to
  reset it (FR-008, FR-010). Cover page(s) are excluded from the sample that establishes it.
- **Translation Cache Entry**: A cached translation result for a specific (page, target language,
  backend) combination — potentially a fully pre-rendered image with the translation burned in —
  populated by background look-ahead prefetching while the user reads, and reused on subsequent
  views under the same combination (FR-016).
- **Usage Budget**: A user-configured limit on automatic look-ahead translation activity for a
  metered backend, together with the current consumption tracked against it — broken down by
  current page, current archive, today, and the current week — and visible to the user at any
  time (FR-013, FR-014).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can enable translation, select a backend, and see translated text on a page
  within the same session, without needing external documentation beyond in-app guidance.
- **SC-002**: A user who never enables translation observes no measurable difference in reading
  performance compared to a build without this feature at all.
- **SC-003**: Across a representative volume, translated text uses a small, consistent set of
  matched fonts for at least 90% of text blocks after the volume's font pattern has been
  established, rather than one generic font applied everywhere.
- **SC-004**: With look-ahead enabled, at least the configured look-ahead window of upcoming pages
  is translated and ready before the user reaches them, in at least 95% of ordinary forward-reading
  sessions.
- **SC-005**: A user-set usage budget for a metered backend is never exceeded without the user's
  explicit confirmation, and the user can check current consumption against that budget at any
  time without needing to wait for a warning or hit the limit.
- **SC-006**: A user following the documented zero-extra-install configuration for a locally-hosted
  backend can get it working without installing any additional program, in the common case.
- **SC-007**: When a translation backend fails or is unreachable, the affected page remains
  readable in its original form within the same time it would normally take to open any other
  page (i.e. failure adds no perceptible delay to reading).

## Assumptions

- This feature is additive to, and depends on, the library/reader delivered in
  `specs/001-lanrurugi-full-rewrite`; it does not change how archives are browsed, catalogued, or
  read when translation is off.
- Translation is off by default and is a global, user-controlled setting (not per-archive), unless
  a user's own workflow calls for finer control — finer-grained (per-archive) control MAY be added
  later without being a breaking change to this spec.
- "Cloud-hosted backend" and "locally-hosted backend" denote categories of user choice; the
  specific set of supported providers may expand over time without constituting a scope change to
  this specification (consistent with `specs/001-lanrurugi-full-rewrite`'s Assumptions).
- The specific OCR/font-matching/translation techniques used to satisfy these requirements are
  implementation decisions for planning, not prescribed here; the requirements are behavioral
  (what the user experiences), not algorithmic.
- Per constitution Principle VI, this feature's planning and delivery timeline is independent of
  Phase 1 and must not reopen or delay it.
- Right-to-left (RTL) target-language layout is out of scope for this phase (FR-004); it MAY be
  added later as a separate, non-breaking increment once there is design familiarity with RTL
  text layout to do it properly.
