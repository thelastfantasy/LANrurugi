# Implementation Plan: Direct Download Domain

**Branch**: `021-direct-download-domain` | **Date**: 2026-11-08 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/021-direct-download-domain/spec.md`

## Summary

The dual-domain design is already implemented in the server: `direct_download_url`, the downloads-only Host policy,
signed 15-minute links, CORS/exposed headers on share range endpoints, Cloudflare-edge-only trust for
`CF-Connecting-IP`, and streaming repacks. This feature turns that arrangement into a supported deployment rather than
a private one:

1. validate the download address against the deployment's own scheme at save time (an `http://` address under an
   HTTPS page origin is refused with the fix in the message);
2. ship a complete reverse-proxy reference for both hosts, as a file the docs site serves and the guide links;
3. correct the guide (both languages) and rebuild the offline documentation copy.

No storage, no schema, no new endpoint. The server change is one validator with tests; everything else is
documentation and packaging.

## Technical Context

**Language/Version**: Rust 2021 (workspace toolchain), TypeScript 5.x + React 19 (setting field only), Astro/Starlight
for docs

**Primary Dependencies**: Axum settings handlers (`crates/lanrurugi-api/src/settings.rs`), the existing
`direct_download_url` plumbing, nginx as the documented reverse proxy (no code dependency)

**Storage**: Redis settings only (existing `direct_download_url` key); nothing new persisted

**Testing**: `mise run check-crate -- lanrurugi-api` plus unit tests for the validator (table-driven: page scheme ×
download scheme × URL shape); `mise run docs-build` to prove the offline copy builds; manual nginx `-t` on the shipped
reference

**Target Platform**: Linux server behind Cloudflare's orange cloud; the download host is a grey-cloud name with A/AAAA
records

**Project Type**: web service + documentation site

**Performance Goals**: validation is a pure function on one string, O(1); nothing is added to the request path of a
download

**Constraints**: A LAN deployment with an HTTP page origin and an HTTP download host must keep working; no new setting
key; the reverse-proxy reference must be valid for the nginx the project already documents in `reverse-proxy.md`

**Scale/Scope**: one download host per deployment; one validator; one configuration file; two guide pages

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How this plan satisfies it |
|---|---|---|
| I. Legacy data & user-trust compatibility | PASS | No existing setting changes meaning; the validator only refuses values that could not have worked from a browser anyway. A stored `http://` value under an HTTPS page is flagged, not rewritten. |
| II. API contract fidelity | PASS | `POST /api/settings` keeps its shape; it gains one validation error with a message. No new endpoint. |
| III. Single-process, resource-conscious, genuinely concurrent | PASS | No new task or process; the reference config's `proxy_buffering off` is what keeps transfers streaming. |
| IV. Plugin extensibility | PASS (untouched) | — |
| V. Secrets & network trust boundaries | PASS | The feature strengthens the trust boundary: the download host never trusts `CF-Connecting-IP`, the page host only trusts it from edge ranges, and the reference shows the header stripping. |
| VI. Phased scope discipline | PASS | Deployment/documentation work for a Phase 1 feature. |
| VII. Frontend discipline & legacy fidelity | PASS | The setting's inline error follows the existing settings-field pattern; strings go through i18next (en/zh/ja). |

No violations; Complexity Tracking stays empty.

## Project Structure

### Documentation (this feature)

```text
specs/021-direct-download-domain/
├── plan.md
├── research.md          # scheme rule, proxy choice, doc shape
├── data-model.md        # the setting + validation state (small)
├── quickstart.md        # nginx on a scratch host + probes
├── contracts/
│   └── settings.md      # validation contract and error message
└── checklists/requirements.md
```

### Source Code (repository root)

```text
crates/lanrurugi-api/src/
├── settings.rs                 # validate_direct_download_url(): absolute, scheme vs page origin, no creds/fragment/query
├── settings/tests.rs (or inline #[cfg(test)])
└── download_host.rs            # unchanged; referenced by the reference config

apps/frontend/src/pages/Settings/SettingsPage.tsx   # surface the validation error next to the field
apps/frontend/src/i18n/locales/{en,zh,ja}.json      # message for the scheme mismatch

docs/public/nginx/lanrurugi-dual-domain.conf        # NEW — served verbatim by the docs site
docs/src/content/docs/guide/sharing.md              # link + corrected constraints (zh)
docs/src/content/docs/en/guide/sharing.md           # link + corrected constraints (en)
docs/src/content/docs/guide/reverse-proxy.md        # pointer to the dual-domain reference
docs/dist/                                          # rebuilt locally for verification (gitignored; image build regenerates)
```

**Structure Decision**: The validator lives beside the other settings validators in `settings.rs`; the reference
configuration is a static asset under `docs/public/` so the public site and the offline copy both serve the exact file
that was tested, instead of an untested code block in prose.

## Complexity Tracking

None.

## Phase 0 — Research (see [research.md](./research.md))

Decisions: reject `http://` only when the page origin is HTTPS (and re-check on origin change); permit it for HTTP
origins; the reference configuration is nginx because that is the proxy `reverse-proxy.md` already documents; the file
ships under `docs/public/nginx/` so it is served, versioned and testable.

## Phase 1 — Design (see [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md))

One setting and one validator; the reference configuration's host matrix (page host vs download host: allowed paths,
trusted headers, buffering, timeouts, connection limits).

## Post-Design Constitution Re-check

Unchanged and still PASS. The design adds one pure validator, one static configuration file and documentation updates.
