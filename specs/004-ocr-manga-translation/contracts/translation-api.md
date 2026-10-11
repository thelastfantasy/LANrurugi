# Contract: Translation REST API (Phase 2)

All paths here are **new and additive** — per constitution Principle II, none of these alter or
replace any path listed in `specs/001-lanrurugi-full-rewrite/contracts/rest-api.md`. Auth follows
the same API-key scheme Phase 1 already established.

## Settings

- `GET /translation/settings` — returns the server-stored defaults: cloud/API-key backend
  configuration (non-secret fields only — `credential_ref`, never the secret itself), target
  language preference. Does **not** return any `localStorage`-only local-backend override (that
  never leaves the requesting device by definition).
- `PUT /translation/settings` — updates the server-stored cloud/API-key backend configuration
  and/or target language preference (FR-003, FR-004).
- Local-backend configuration (the `localStorage` override, research.md §8) has **no server
  endpoint** — it is a client-only setting, by design.

## Usage

- `GET /translation/usage` — returns current consumption broken down by current page, current
  archive, today, and current week (FR-014), plus the configured limit, for the active
  metered/cloud provider. Returns an empty/zero result if no metered backend is configured (no
  usage to report for a locally-hosted backend).

## Per-page translation (server-composited / cloud-backend path)

- `GET /archives/{id}/page/{page}/translation?lang={target_language}` — returns the composited,
  translated page image for the given archive/page/target-language, using the currently
  server-configured cloud provider. Triggers on-demand OCR + translation + compositing if not
  already cached (`Translation Cache Entry`, server variant); served directly from cache
  otherwise.
  - Returns a `202`-style "not ready" response (not an error) if look-ahead hasn't reached this
    page yet and it's still processing, so the frontend can show the FR-012 non-blocking loading
    state rather than treating this as a failure.
  - Returns a distinct "translation unavailable for this page" response (FR-019) on genuine
    backend failure, never a blank/broken image.

## Detected text + font metadata (local-backend path — client composites itself)

- `GET /archives/{id}/page/{page}/text-regions` — returns `Detected Text Region` records (position,
  source text, and each region's independently-nullable `fg_color`/`bg_color`/`is_bold` per
  FR-008a/research.md §13 — no translation), the volume's current `Volume Font Pattern` golden
  set, and the same `context` (Terminology Glossary matches/names, same-page tone reference) the
  cloud path receives via T019b, so the local-backend path gets FR-007a–e's consistency benefit
  too (`contracts/client-compositing-cache.md`). The browser uses this, together with a translation
  it obtains directly from its own locally-hosted backend (never proxied through this server, per
  constitution Principle V), to composite the page client-side (research.md §6/§7), applying each
  region's own estimated attributes per `contracts/client-compositing-cache.md`.
- `POST /archives/{id}/page/{page}/translation/record` — the locally-hosted-backend path reports
  a translation it obtained directly (never proxied) back to the server so any new name/term it
  discovers is captured into the shared, server-stored Terminology Glossary (FR-007a); this is a
  lightweight "record the result," not a translation call — the server performs no LLM call here.

## Font pattern management

- `POST /volumes/{id}/font-pattern/reset` — clears a volume's `Volume Font Pattern` (`vote_pool`,
  `golden_set`, `meltdown_tally`) back to `unlocked` (FR-010).

## Terminology glossary management

- `GET /volumes/{id}/terminology-glossary` — returns the volume's `Terminology Glossary` entries
  (source term → translation) for display in settings (FR-007d).
- `PUT /volumes/{id}/terminology-glossary/{term}` — edits a single entry's translation (FR-007d).
- `DELETE /volumes/{id}/terminology-glossary/{term}` — removes a single entry (FR-007d). No
  bulk-clear endpoint, unlike font-pattern reset above — see data-model.md's Terminology Glossary
  section and research.md §14 for why.
