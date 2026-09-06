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

### Session 2026-09-06

- Q: Should rendered translated text preserve the *original* text block's own visual
  attributes (font size, color, weight/boldness), rather than only the volume-level font-family
  matching User Story 2 already covers? → A: Yes — per-block visual-attribute fidelity is a
  distinct, additional requirement on top of the existing volume-level font-family matching, not a
  replacement for it. Researched against the most prominent open-source prior art in this space
  (`zyddnys/manga-image-translator`, GPL-3.0 — cloned locally for architecture reference only, per
  this project's existing Koharu precedent, never as a dependency or copied code): that project
  extracts per-character foreground/background color via a dedicated model output head trained
  alongside text recognition, but even it has never actually implemented automatic bold/weight
  detection — the field exists in its data model but is always left at its default. Given
  `manga-ocr` (this project's chosen recognition model, unlike that project's own) has no such
  color-output head and retraining/fine-tuning it is out of scope for this phase, LANrurugi
  estimates both color and boldness via a lightweight heuristic pass over each detected region's
  cropped pixels (not a full retrained model): dominant foreground/background color via clustering
  over the region's pixels (informed by `sift-ocr`'s published KMeans-based bubble/text color
  approach), and boldness via a stroke-width-to-glyph-height heuristic ratio. Per FR-008a below,
  any attribute the heuristic cannot estimate with confidence falls back independently — it does
  not block rendering of the other, successfully-estimated attributes for that same block.
- Q: Each text block's translation request is currently independent — no shared state carries a
  character name or term's translation from one block/page to the next, so the same name could be
  rendered differently across a volume. How should this be addressed? → A: A per-volume
  **Terminology Glossary** — a name/term → chosen-translation mapping maintained per volume (or
  per-archive if ungrouped, mirroring Volume Font Pattern's own scoping). The first time the LLM
  translates a given name/term, its output is captured into the glossary automatically and used
  immediately for subsequent requests — no user confirmation gate before a glossary entry takes
  effect, consistent with this feature's existing low-friction posture (FR-007). A user can edit or
  delete an individual glossary entry (to correct a wrong auto-captured translation) from the
  settings screen at any time, taking effect on the next translation request; no bulk/whole-glossary
  reset is provided (unlike Volume Font Pattern's FR-010) since a wrong entry is independent of the
  others and a bulk reset would needlessly discard already-correct entries.
- Q: Names/terms don't only recur verbatim — a character's given name (e.g. さゆき) may later be
  referred to by a nickname (e.g. さっちゃん) with no substring relationship to the original, and a
  Western-style full name introduced once (e.g. "Axxx Bxxx Cxxx") is often abbreviated later
  (initials "A.B.C." or "ABC") — while an unrelated all-caps acronym that was never introduced as a
  name at all shouldn't be force-matched into the glossary just because it looks similar. Can the
  glossary catch these variant forms too? → A: Plain substring matching alone cannot — nickname
  formation and initialism are semantic/contextual judgments, not something a fixed string rule can
  reliably generalize, and blindly matching on shape would also risk misfiring on real acronyms that
  were never a name. This is addressed as a hybrid, not by trying to extend the substring rule with
  more cases: (1) exact substring hits against known glossary source terms remain a fast, free,
  zero-LLM-judgment path used as-is when they occur; (2) additionally, the request's `context`
  includes the lightweight list of character/term names already known for that volume (names only,
  not full source→translation pairs) so the LLM itself can recognize a nickname or initialism as
  referring to an already-known name and reuse its established translation, rather than coining a
  new one — this leverages the LLM's own language understanding for exactly the judgment call
  substring matching cannot make, at the cost of one small, bounded (name-list-sized, not
  full-glossary-sized) context addition per request. A name/term genuinely not recognized as
  matching anything known (including a real, unrelated acronym) is still added to the glossary as a
  new entry, same as before.
- Q: Dialogue/narration tone (playful, serious, formal, etc.) is often distinct per character or
  per scene, and matters for translation quality just as much as name consistency — but unlike a
  name, tone isn't a discrete value that can be cached in a lookup table; the same character can
  shift tone scene to scene. Should this phase address tone consistency, and if so how? → A: Yes,
  as a further extension of the same `context` mechanism FR-007c already introduces, not a
  separate cacheable entity like the glossary. A translation request's `context` MUST also include
  the other already-translated text blocks on the same page (their source text and chosen
  translation) as reference material, giving the backend enough surrounding dialogue/narration to
  infer tone from — this is deliberately a *hint*, not a rule or a stored preference: there is no
  per-character "tone table" to maintain (tone is scene-dependent, not a fixed character
  attribute), no automatic detection of "this is playful vs. serious" is performed by LANrurugi
  itself, and no success criterion enforces a specific tonal outcome — that judgment is left
  entirely to the translation backend's own language understanding, the same way FR-007c already
  leaves name-variant recognition to it rather than encoding a matching rule.

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

**Translation consistency (extends User Story 1 — no dedicated user story; a cross-cutting
correctness requirement on the translation output itself)**

- **FR-007a**: System MUST maintain a per-volume (or per-archive if ungrouped) Terminology
  Glossary mapping recognized character names/terms to a chosen translation. The first time a
  name/term is translated, its translation MUST be captured into the glossary automatically, with
  no user confirmation gate before that entry takes effect for subsequent translation requests in
  the same volume.
- **FR-007b**: A subsequent translation request whose source text contains an exact-substring
  match against a known glossary source term MUST reuse that term's established translation
  rather than requesting a new translation for it.
- **FR-007c**: To catch name/term variants that are not exact substrings (nicknames, e.g. さゆき
  later referred to as さっちゃん; initialisms/abbreviations of a previously-introduced full name,
  e.g. "Axxx Bxxx Cxxx" later abbreviated "A.B.C."/"ABC"), a translation request MUST include the
  volume's already-known character/term names (names only, not their full translations) as
  context, so the translation backend can itself recognize such a variant as referring to an
  already-known name and reuse its established translation instead of coining a new one. A
  name/term not recognized as matching any known entry (including a genuine, unrelated acronym
  that coincidentally resembles an initialism) MUST still be added to the glossary as its own new
  entry, same as any other first-seen term.
- **FR-007d**: Users MUST be able to view, edit, and delete individual Terminology Glossary
  entries from the settings screen at any time; an edit or deletion MUST take effect starting with
  the next translation request. No bulk/whole-glossary reset is required (unlike Volume Font
  Pattern's FR-010), since an incorrect entry is independent of the others.
- **FR-007e**: A translation request MUST include the page's other already-translated text
  blocks (source text and chosen translation) as context, so the translation backend has enough
  surrounding dialogue/narration to infer and preserve tone (playful, serious, formal, etc.)
  consistent with the rest of the page. This is advisory context only — LANrurugi itself performs
  no tone classification, maintains no per-character tone preference, and enforces no specific
  tonal outcome; tone inference is left entirely to the translation backend's own judgment.

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
- **FR-008a**: In addition to the volume-level font-family matching of FR-008, the system MUST
  estimate each detected text block's own foreground color, background color, and boldness from
  its cropped pixels via a lightweight heuristic pass (not a retrained/fine-tuned recognition
  model — see Clarifications, Session 2026-09-06), and render the translated text using those
  per-block estimated attributes. Estimation of each of the three attributes (color, background
  color, boldness) is independent: if the heuristic cannot estimate one attribute for a given block
  with confidence, that attribute alone falls back to a safe default (the volume's established
  golden-set font's own default weight for boldness; a legible default color/background,
  e.g. matching the reader's existing untranslated-page contrast convention) — a low-confidence
  result on one attribute MUST NOT block rendering of the other, successfully-estimated attributes
  for that same block, and MUST NOT be treated as a translation failure (FR-019).
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

**Cross-cutting integration (no dedicated user story — extends Phase 1's existing backup/export
and Activity audit mechanisms, per constitution Principle I's data-loss-is-a-bug stance)**

- **FR-022**: The Terminology Glossary, translated `Detected Text Region` records, and Volume Font
  Pattern MUST be included in the library backup/export produced by Phase 1's existing backup
  mechanism (`specs/001-lanrurugi-full-rewrite`'s FR-008/FR-009), and MUST be restorable from it,
  so this data is not silently lost on a restore-from-backup. The re-derivable rendered-page cache
  (Translation Cache Entry) and point-in-time Usage Budget consumption counters are not required
  in the backup, since neither represents durable user-authored state.
- **FR-023**: User-initiated changes to the Terminology Glossary (entry capture, edit, deletion),
  Volume Font Pattern resets, and translation backend/target-language selection changes MUST be
  recorded in Phase 1's existing Activity audit log, using the same mechanism every other mutable
  entity in this project already uses, so a user can see what changed and when for this feature's
  data the same way they already can for archives, categories, and plugins. This feature's own LLM
  translation calls, and their token-usage/cache-hit/estimated-cost metrics specifically, are out
  of scope for FR-023 itself — that is a cross-project Activity capability gap tracked in GitHub
  issue #100 (LLM-call-specific audit fields, depends on #87's general audit log system), not
  something this spec defines its own parallel mechanism for.

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
  the text it contains, used as the unit of translation and of font-style matching. Also carries
  the block's own heuristically-estimated foreground color, background color, and boldness
  (FR-008a) — each independently nullable when the heuristic couldn't estimate it with confidence,
  distinct from and in addition to the volume-level font-family matched via Volume Font Pattern.
- **Volume Font Pattern**: The established, small set of body-text fonts associated with a given
  volume once enough non-cover pages have been processed with confidence, plus the ability to
  reset it (FR-008, FR-010). Cover page(s) are excluded from the sample that establishes it.
- **Terminology Glossary**: A per-volume (or per-archive if ungrouped) name/term → chosen
  translation mapping, auto-populated on first translation of each name/term and reused for
  exact-substring matches on subsequent requests; individually user-editable/deletable (FR-007a–d).
  Distinct from Volume Font Pattern (visual style) and Translation Cache Entry (page-level output
  cache) — this entity concerns translation *content* consistency, not rendering.
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
- **SC-003a**: Across a representative sample of text blocks, the heuristic color estimate
  (FR-008a) is not visibly wrong (a human reviewer would not describe the rendered color as
  mismatched against the original block) for at least 80% of blocks; blocks where color/boldness
  couldn't be estimated with confidence fall back per FR-008a rather than rendering an
  obviously-incorrect value. This is a lower confidence bar than SC-003's 90% deliberately — per
  research.md §13, this heuristic (unlike the font-family matching SC-003 measures) has no adopted
  external prior art to validate its accuracy against ahead of real usage.
- **SC-003b**: Across a representative volume containing at least one recurring character
  name/term, a name/term already present in the glossary (whether via exact match or a
  nickname/abbreviation the backend recognized per FR-007c) renders with the same translation
  every time it recurs, for at least 95% of recurrences — the higher confidence bar reflects that
  exact-substring reuse (FR-007b) is deterministic; the remaining tolerance accounts for
  variant-recognition (FR-007c) being a backend judgment call, not a guaranteed match.
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
