# Tasks: Direct Download Domain

**Input**: Design documents from `/specs/021-direct-download-domain/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Required. The validator is pure and gets a unit-test matrix; the host boundary already has tests and
gains the `http://`/Host cases this feature adds.

**Organization**: the code change is small (one validator, one warning path), the rest is the shipped reference
configuration and the documentation that makes the deployment supportable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel
- **[Story]**: US1–US5 as in spec.md

## Path Conventions

Rust under `crates/`, frontend under `apps/frontend/src/`, docs under `docs/`.

---

## Phase 1: Setup

- [ ] T001 [P] Add `DownloadUrlError` (`Shape`, `HasCredentials`, `HasQueryOrFragment`, `SchemeMismatch`) and
  `validate_download_url(text: &str, page_origin: Option<&str>) -> Result<String, DownloadUrlError>` in
  `crates/lanrurugi-api/src/settings.rs` (or a `download_url.rs` module beside it), reusing the same `url::Url`
  parser `share_settings.rs` already uses. Empty is valid (feature off) and returns an empty string.

---

## Phase 2: User Story 3 — The address is validated against the page scheme (Priority: P2, but first: it is the only code change)

**Goal**: an `http://` download address is refused when the page origin is HTTPS, accepted when it is HTTP, and the
shape checks reject credentials/query/fragment.

- [ ] T002 [US3] Call `validate_download_url` from `put_settings` in `crates/lanrurugi-api/src/settings.rs` for the
  `direct_download_url` key, passing the page origin (`share_settings`' `share_public_url` scheme+host, `None` when
  unset). Keep the existing `validate_setting_field` shape check as the type gate; the new check owns semantics.
  A rejection returns `400` with the field name and an actionable message (contracts/settings.md).
- [ ] T003 [US3] Add the FR-004 re-check: when `share_public_url` changes and the stored download address becomes
  invalid, include a warning in the `PUT /api/settings` response (`warnings: [..]`) and expose the same warning from
  `GET /api/settings` so the field can show it after a reload. Saving the page origin is never refused.
- [ ] T004 [US3] Frontend: render the field-level error and the page-origin warning in
  `apps/frontend/src/pages/Settings/SettingsPage.tsx` (same error surface the other fields use), with i18n keys in
  `apps/frontend/src/i18n/locales/{en,zh,ja}.json`.
- [ ] T005 [US3] Tests: `validate_download_url` matrix — `https` ok; `http` + HTTPS origin → `SchemeMismatch`;
  `http` + HTTP origin ok; `http` + unknown origin ok; `user:pass@host` → `HasCredentials`; `?x=1`/`#f` →
  `HasQueryOrFragment`; no scheme/`ftp://` → `Shape`; trailing slash trimmed; empty ok.

---

## Phase 3: User Story 4 — A copy-pasteable reverse-proxy reference (Priority: P2)

**Goal**: a real nginx configuration in the repository that reproduces both hosts, with the edge allow-list, the
real-client-address handling, the download-host path boundary and the streaming settings.

- [ ] T006 [US4] Add `docs/public/nginx/lanrurugi-dual-domain.conf` following review §15.3 and
  research.md's decisions: default server with `ssl_reject_handshake on`; page server allows only Cloudflare's
  ranges, sets `real_ip_header CF-Connecting-IP` + `real_ip_recursive on`, forwards `X-Forwarded-For
  $remote_addr` (overwriting, not appending) and `CF-Connecting-IP $remote_addr`; download server matches the
  download host name, serves only `/api/dl/` and `/api/share/r/` (everything else `404`), sets
  `proxy_set_header CF-Connecting-IP ""`, `proxy_buffering off`, `proxy_read_timeout 600s`,
  `proxy_request_buffering off`, `limit_conn`, and disables caching for those paths. Comment every non-obvious
  line with what breaks without it.
- [ ] T007 [US4] Test the reference: `nginx -t` against the file (with placeholder certs) inside the dev container
  or a scratch container; then put a dev instance behind it and run quickstart B (Host-boundary probes and a forged
  `CF-Connecting-IP`). Record the exact commands and outputs in this file's implementation record.
- [ ] T008 [US4] Keep `docs/src/content/docs/guide/reverse-proxy.md` as the prose entry point and link the new file
  from it; do not duplicate the config in the prose page.

---

## Phase 4: User Story 5 — The documentation states the constraints honestly (Priority: P3)

- [ ] T009 [US5] Fix `docs/src/content/docs/guide/sharing.md` (and `en/guide/sharing.md`)'s stale claim that a
  multi-GB archive times out at the CDN: replace it with the accurate statement (524 is a late first byte, the
  app streams repacks, the 100 MB figure is an upload limit) and link the reference configuration.
- [ ] T010 [US5] Add the deployment constraints to both languages: A/AAAA rather than a CNAME to the proxied
  record, IPv6 in parallel, no HTTP-01 for a grey-clouded host (DNS-01), CGNAT makes the arrangement impossible, the
  download host exposes the origin IP, and proxies must preserve `Host`. Place it where the dual-domain section
  already lives.
- [ ] T011 [US5] `mise run docs-build`, then grep `docs/dist` for the stale sentence to prove it is gone and for
  the new config's filename to prove it shipped. `docs/dist` itself stays untracked.

---

## Phase 5: User Story 1 & 2 — Verify the already-built path (Priority: P1)

These stories are already implemented; the tasks are proof, not construction. They exist so a later change cannot
regress them silently.

- [ ] T012 [US1] Quickstart A: with `direct_download_url` set to a non-proxied host, download a multi-GB archive and
  a freshly created tankoubon zip; record time-to-first-byte and total bytes.
- [ ] T013 [US2] Quickstart B: in a fresh browser profile with no cookies, download from a share page; confirm the
  bytes come from the download host, the link expires, and `/`, `/api/info`, `/api/archives` on the download host
  are 404.

---

- [ ] T013a [US1] Assert FR-010 (a hash-named or ordinarily named archive downloads under its title) and FR-014
  (page images and thumbnails are requested from the page host, not the download host) with a request-log check on
  the dev instance: no thumbnail/page URL appears on the download host during a browse-and-read session.
- [ ] T013b [US1] Assert FR-015: set and clear `direct_download_url` via `POST /api/settings` and confirm a download
  link uses the new base without restarting the container.

## Phase 6: Polish

- [ ] T014 [P] `docs/comparison-with-lanraragi.md` / `README.md`: only if either claims something this feature
  changes (the download host being a private arrangement, or "no reverse-proxy reference").
- [ ] T015 Gates: `mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-api`,
  `mise run test-crate -- lanrurugi-api`, `tsc --noEmit`, `eslint src`, `mise run docs-build`.

---

## Dependencies & Execution Order

1. T001 → T002/T003 → T005 (validator first).
2. T004 after T003's response shape exists.
3. T006 → T007 (config before testing it); T008 after T006.
4. T009/T010 → T011.
5. T012/T013 can run any time after the dev instance is rebuilt; they do not block code.
6. T015 last.

## Parallel Opportunities

T001/T006/T009 are independent (different files). T005 with T004 once T002/T003 land.

## Implementation Strategy

Land T001–T005 first (the only behavioural change), verify with unit tests, then the reference config and docs,
then run the two quickstarts against the dev instance and record the numbers.

## Notes

- Do not add a second setting key or a per-share override: one download host, one TTL, as research.md decided.
- The warning on a page-origin change must never block the save — the operator may be mid-migration.
- The nginx file is a *reference*, not a template engine: placeholders must be obvious and documented at the top.
