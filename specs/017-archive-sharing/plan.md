# Implementation Plan: Archive & Tankoubon Sharing Across Instances

**Branch**: `017-archive-sharing` | **Date**: 2026-10-24 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/017-archive-sharing/spec.md`

## Summary

Administrators wrap archives and tankoubons in a **share** with permissions (import / read /
download), a download limit, IP restrictions and an optional expiry. They hand it on as a
10-character code (`lrrg:K7QM9-XRT2H`, magnet-style), a QR code and, when the sharer has a
public address, a link. Codes never contain an address, so domain or IP changes never invalidate
them.

**Browser recipients** use new recipient-only routes. The share credential is their only
authority: they need no session and no guest mode, and get nothing outside the share.

**Other instances** import over **iroh**: QUIC dialled by public key, with hole punching, relay
fallback and mDNS on the LAN.

**All connection and transfer information comes from the sharer itself** (FR-047): addresses,
public URL, manifest, piece fingerprints, tickets and the helper list. Outside services hold only
an encrypted **knock record** under a key derived from the code, containing the sharer's node id
and relays. It goes to pkarr HTTPS relays and/or LANrurugi rendezvous **nodes**, which any
reachable instance can offer to its friends, so a circle of instances needs no third party at all.
There is no web gateway.

Archives transfer as BLAKE3-verified
**bao-tree** ranges: resumable, from several sources at once, with **tickets** the sharer signs so
that helpers obey revocation.

Each import is **one** compact entry in the download queue, with aggregate progress and an
expandable per-archive list. It runs at a bounded concurrency outside the per-plugin pools, so it
never blocks other downloads. The sharer fingerprints shared archives in a throttled background
job, so a first import starts before a large share is fully prepared. Imports then rebuild
tankoubons with their tags and metadata.

Alongside: streamed **tankoubon zip download**; embeddable cards (iframe and `<lrrg-share>`)
whose layout follows the share's permissions, served by the sharer's own instance; upload limits
with a BT-style alternative-speed schedule; and a Settings → Sharing section.

## Technical Context

**Language/Version**: Rust 1.99 (workspace toolchain; iroh needs ≥ 1.91). TypeScript / React 19.

**Primary Dependencies (new)**:

| Area | Dependencies |
|---|---|
| Transport | `iroh` 1.4 (features `portmapper`), `iroh-mdns-address-lookup` 0.6, `iroh-relay` 1.4 (feature `server`, for the relay role) |
| Code resolution | `pkarr` 8.1 |
| Verified ranges | `bao-tree` 0.17 |
| Code derivation | `argon2` 0.6 |
| Record encryption | `chacha20poly1305` 0.11 |
| Wire encoding | `postcard` 1.1 |
| Proxies | `tokio-socks` (SOCKS5 bridge for iroh); `system-configuration` (macOS only, the system-proxy read); reqwest feature `socks` |
| Frontend | `barcode-detector` 3.2 (lazy `zxing-wasm`), `lean-qr` 2.7 |

Also new as direct dependencies: `blake3`, `ipnet`, `hkdf` and `ed25519-dalek` (all already transitive through iroh or pkarr).

The versions above are starting candidates. Setup pins the latest stable release of each, checked
live at implementation time (constitution, "Dependencies default to the latest stable release").
Reused from the workspace: `zip` 8.6, `sha2`, `hmac`, `axum`, `redis`.

**Storage**: Redis only (data-model.md). Files live under `<data_dir>/share/` (outboards) and
`<temp>/share/` (partials, merged-patch copies), counted against `tempmaxsize`.

**Testing**: single-crate Rust tests through `mise run test-crate` (detached), Vitest, and
Playwright for the recipient page and import modal. Network scenarios are manual, per
quickstart.md.

**Target Platform**: the Linux Docker image (primary), native packages (015), and modern
browsers.

**Project Type**: web service + SPA in the existing monorepo, plus one extra Vite build entry
(the embed card and `<lrrg-share>` bundle) from the same frontend package.

**Performance Goals**:

- LAN transfer ≥ 80% of a plain copy (SC-007);
- a tankoubon download's first byte < 2 s (SC-012);
- revocation stops new requests within 5 s and all sources within 30 s (SC-009);
- a running import leaves other downloads and page response times unaffected, within 10%
  (SC-014);
- helpers give a ≥ 1.5× speedup (SC-011).

**Constraints**:

- one extra UDP port;
- no new database (constitution: Redis only), which is why `iroh-blobs`' redb store is not used;
- hashing goes through `parallel_map` with the ~30% core cap;
- no long-lived work outside the single process.

**Scale/Scope**: shares of up to hundreds of archives; thousands of shares per instance;
multi-GB tankoubons.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Verdict |
| --- | --- | --- |
| I. Legacy data compatibility | No legacy key read differently or rewritten | **Pass**: every key is new and `LANRURUGI_`-prefixed. `LRR_CONFIG` only gains `share_*` fields. Archive IDs are untouched (BLAKE3 is a separate fingerprint, not an identity change) |
| II. API contract fidelity | Legacy endpoints unchanged | **Pass**: every route is new (`/api/shares*`, `/api/share/r/*`, `/api/tankoubons/{id}/download`, `/s/*`, `/embed/*`) |
| III. Single process, real concurrency | No extra long-lived processes; CPU work off the reactor and parallel | **Pass**: the iroh endpoint, knock-record republisher and helper heartbeats are Tokio tasks in the server. BLAKE3/outboard hashing batches through `parallel_map` with the core cap. The zip stream runs in `spawn_blocking` |
| IV. Plugin sandbox | — | Not touched. Imports are a built-in queue origin, not a plugin |
| V. Secrets & trust boundaries | Secrets server-side; outside network use deliberate and switchable | **Pass**: the node secret and code material stay in Redis. Every outside service is configurable, and public ones can be switched off (FR-027). They hold only the encrypted knock record and relay only ciphertext (FR-047); codes cannot be derived from it |
| VI. Phased scope | Phase 2 not smuggled in | **Pass**: Phase 1 addendum, additive |
| VII. Frontend discipline | common-ui controls, theme-aware colours, user-verified UI | **Pass with obligation**: in-app surfaces use common-ui and per-theme classes in all five theme files. The embed card's own CSS is exempt, being shown on foreign pages (research §12). Visual sign-off is the user's |
| Tech constraints: single monorepo, one frontend app | No new repo or app | **Pass**: the embed bundle is a second Vite entry of `apps/frontend`, sharing its components. The one new crate joins the workspace |
| Tech constraints: Redis only | No embedded store | **Pass**: `iroh-blobs`' redb store was deliberately rejected for `bao-tree` plus files (research §5) |
| Tech constraints: newtype ids | Domain ids are newtypes | **Pass**: `ShareId`, `ClaimId` and `ShareImportId` |

**Gate result: pass.**

**Post-design re-check**: still pass. Two points are made explicit by the design:

- FR-028 was tightened in the spec. An IP-restricted share is refused when only a relay path
  exists, rather than being evaluated against a relay address.
- Nothing about a share is served from outside the sharing instance (FR-047). The earlier web
  gateway was dropped for that reason, which also removed the riskiest piece (iroh in wasm).

## Project Structure

### Documentation (this feature)

```text
specs/017-archive-sharing/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── http-api.md
│   ├── share-protocol.md
│   └── ui.md
├── checklists/requirements.md
└── tasks.md            # /speckit-tasks
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-share-proto/          # NEW — protocol types, code derivation, knock record, tickets
│   └── src/{code.rs, derive.rs, record.rs, messages.rs, ticket.rs, framing.rs}
├── lanrurugi-core/src/ids.rs       # + ShareId, ClaimId, ShareImportId
├── lanrurugi-storage/src/
│   ├── keys.rs                     # + LANRURUGI_SHARE_* constructors
│   ├── shares.rs                   # NEW — Share/claim/token/helper repos, claim Lua script
│   └── download_queue.rs           # + QueueItemOrigin::ShareImport, share_import field
├── lanrurugi-api/
│   ├── policy/route_policy.csv     # + admin / recipient / tankoubon-download lines
│   └── src/
│       ├── shares.rs               # NEW — /api/shares* admin + settings + imports endpoints
│       ├── share_recipient.rs      # NEW — /api/share/r/* + card + rate limiting
│       ├── share_net/              # NEW — iroh endpoint, protocol server, pkarr publisher,
│       │   ├── mod.rs              #        mDNS resolve, ticket issue/verify, helper role
│       │   ├── server.rs
│       │   ├── publish.rs          #        knock-record publish/resolve to all rendezvous
│       │   ├── nodes.rs            #        rendezvous role, embedded iroh-relay role, service status
│       │   ├── limits.rs           #        upload limits + alternative-speed schedule
│       │   ├── proxy.rs            #        follow the user's proxy, SOCKS bridge, optional override
│       │   ├── blobs.rs            #        BLAKE3 fingerprint + outboard cache, background prepare job
│       │   └── import.rs           #        multi-source range fetcher, resume, ingest + rebuild
│       ├── tankoubons.rs           # + GET /tankoubons/{id}/download (streamed zip)
│       ├── tankoubon_zip.rs        # NEW — zip stream writer (stored, ZIP64, naming, MISSING.txt)
│       ├── download_queue.rs       # + start path for ShareImport items
│       ├── settings.rs             # + share_* fields (admin-only)
│       └── state.rs                # + ShareNet handle in AppState
├── lanrurugi-backup/src/           # + LANRURUGI_SHARE_NODE_SECRET in backup/restore
└── lanrurugi-server/src/main.rs    # + ShareNet start-up, `share-node` subcommand

scripts/share-network-matrix.sh     # NEW — quickstart Scenario 9
scripts/share-traffic-check.sh      # NEW — nDPI classification of captures (SC-016)

apps/frontend/
├── embed-card.html                 # NEW — iframe card entry
├── vite.config.ts                  # + embed card entry and share.js bundle
└── src/
    ├── share/                      # NEW
    │   ├── transport.ts            #   ShareTransport over the instance's HTTP recipient routes
    │   ├── SharePage.tsx           #   /s/:code recipient page
    │   ├── ShareReader.tsx         #   reader wrapper over a transport
    │   ├── ShareCard.tsx           #   embed card (permission-dependent layout)
    │   ├── embed-element.ts        #   <lrrg-share> custom element
    │   ├── ShareDialog.tsx         #   create / add-to-existing
    │   ├── ShareResult.tsx         #   code + QR + snippets
    │   ├── ImportShareModal.tsx    #   code / paste / file / camera / photo + preview
    │   ├── qr.ts                   #   lean-qr render, barcode-detector decode (lazy)
    │   └── code.ts                 #   normalisation + lrrg: text form (mirrors share-proto)
    ├── pages/Library/ArchiveContextMenu.tsx   # + Share…, tankoubon Download
    ├── pages/Reader/…              # + Share…, download menu (archive / whole tankoubon)
    ├── pages/Settings/SharingSection.tsx      # NEW
    ├── pages/Upload/…              # + Import share button, ShareImportRow (one row, expandable)
    └── i18n/{zh,en,ja}             # + share.* keys

apps/frontend/public/legacy/themes/{g,ex,modern,modern_red,modern_clear}.css  # + share status classes
docs/ (011 site)                    # + sharing guide page (no deployment details)
Dockerfile, compose*.yaml           # + EXPOSE/publish share UDP port
```

**Structure Decision**: one new crate is needed.

- The protocol types and code derivation get their own crate, so they are testable without the
  server and reusable by any later client.
- Everything server-side stays in `lanrurugi-api` and `lanrurugi-storage`, as other features do.
- The frontend gains one source folder (`src/share/`) and one extra build entry for the embed
  bundle.

## Delivery order (maps to spec priorities)

1. **US3 tankoubon download**: independent, smallest, and needed by US1's download permission.
2. **US1 + US2**: shares, recipient routes, claims, settings section, entry points, QR
   generation. Instance-hosted only; no iroh yet.
3. **US4**: iroh endpoint, pkarr, mDNS, protocol server, upload limits, import queue origin,
   verified transfer and resume, import modal with QR decode and camera. The traffic-profile
   checks that depend on iroh internals run as soon as the endpoint exists.
4. **US5**: NAT paths, public-services-off mode, rendezvous and relay roles on LANrurugi
   instances, node list, outside-service status, proxies, tunnels, node-only mode, the network
   matrix and traffic-profile verification.
5. **US6 embeds**: card plus custom element on the instance.
6. **US7 helpers**: register, heartbeat, tickets to helpers, multi-source fetch.

## Complexity Tracking

| Addition | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| One new crate (`share-proto`) | Protocol types, derivation and tickets testable in isolation | Folding them into `lanrurugi-api` makes the derivation vectors and ticket tests depend on the whole server |
| A second Vite entry (embed bundle) | `<lrrg-share>` and the iframe card load on foreign pages | The main SPA bundle is far too large to embed |
| A UDP port | iroh QUIC | HTTP-only transport cannot traverse NAT |
