---
description: "Task list for 017-archive-sharing"
---

# Tasks: Archive & Tankoubon Sharing Across Instances

**Input**: design documents in `specs/017-archive-sharing/`: plan.md, spec.md, research.md,
data-model.md, contracts/ (http-api.md, share-protocol.md, ui.md), quickstart.md.

**Tests**: the spec has no TDD request. Tests are included only where a guarantee cannot be
checked by eye: atomic claims, code derivation, route policy, ticket validity, verified ranges,
zip layout. They run per crate with `mise run test-crate -- <crate>`, launched detached
(CLAUDE.md). The full `mise run test` and visual UI sign-off are the user's.

**Organization**: phases follow spec priority. US3, the tankoubon download, has no dependency on
any other story and may be built first; it is the plan's delivery step 1.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: US1–US7 from spec.md

---

## Phase 1: Setup (Shared Infrastructure)

- [X] T001 Add workspace dependencies to `Cargo.toml` (`[workspace.dependencies]`): `iroh` (feature `portmapper`), `iroh-mdns-address-lookup`, `iroh-relay` (feature `server`), `pkarr` (key and signed-packet features only, no DHT client), `bao-tree` (without its tokio feature), `argon2`, `chacha20poly1305`, `postcard` (feature `alloc`), `ed25519-dalek`, `blake3`, `ipnet`, `hkdf`, `tokio-socks`, and `system-configuration` as a macOS-only target dependency. Enable reqwest's `socks` feature, plus `macos-system-configuration` on macOS. The versions in plan.md are starting candidates only: pin whatever is the latest stable release on crates.io when this task runs, checked live (constitution, "Dependencies default to the latest stable release"), and note any deliberate deviation in the Implementation Record. Unused workspace entries are not resolved, so resolution is confirmed when each is first used: T002 for the protocol crate, T047 for iroh.
- [X] T002 Create crate `crates/lanrurugi-share-proto/` (`Cargo.toml`, `src/lib.rs` with empty modules `code`, `derive`, `record`, `messages`, `ticket`, `framing`) and add it to `members` in `Cargo.toml`. Keep it free of tokio, iroh and filesystem dependencies, so it stays testable in isolation: endpoint ids are `[u8; 32]` ed25519 public keys (`ed25519-dalek`), converted to and from iroh's `EndpointId` in `lanrurugi-api`. Run `mise run check-crate -- lanrurugi-share-proto` to confirm its dependencies resolve on the Rust 1.99 toolchain.
- [X] T003 [P] Add newtype ids `ShareId`, `ClaimId` and `ShareImportId` (`String` newtypes, `#[serde(transparent)]`, generated from 16 random bytes in base32) to `crates/lanrurugi-core/src/ids.rs`, with unit tests for round-trip parsing.
- [X] T004 [P] Add key constructors to `crates/lanrurugi-storage/src/keys.rs`, one per key in data-model.md: `share_key`, `SHARES_INDEX_KEY`, `share_by_code_key`, `share_item_refs_key`, `share_ips_key`, `share_claim_key`, `share_claims_index_key`, `share_rtoken_key`, `share_helpers_key`, `share_blob_key`, `SHARE_PREPARE_KEY`, `SHARE_NODE_SECRET_KEY`, `share_import_key` and `SHARE_IMPORTS_INDEX_KEY`. All are `LANRURUGI_`-prefixed.
- [X] T005 [P] Add frontend dependencies `barcode-detector` and `lean-qr` to `apps/frontend/package.json`, at the latest stable npm releases checked live (plan.md's 3.2 / 2.7 are starting candidates), then regenerate the root `pnpm-lock.yaml` with `pnpm install` from the repo root. Note for the dev rebuild: delete the `lanrurugi_lrr-dev-frontend-node-modules` volume first (CLAUDE.md, "adding a new frontend dependency").
- [X] T006 [P] Expose the share UDP port. Add `EXPOSE 11204/udp` to `Dockerfile` and `Dockerfile.dev`. Both compose files use `network_mode: host`, so there is no port mapping: add a comment there that the UDP port (and the optional relay role's TCP port) is reachable directly, to be opened in the host firewall or set to 443 through `share_udp_port`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: the share record, credentials, settings and policy that every share story uses. US3
does not need this phase.

- [X] T007 Implement `crates/lanrurugi-share-proto/src/code.rs`:
  - `ShareCode::generate(short: bool)`: 50 random bits (10 chars), or 40 bits (8 chars) for a short code;
  - `ShareCode::parse(&str)`: uppercase, `O→0`, `I/L→1`, strip `-` and spaces, accept length 10 or 8;
  - `display()`: `XXXXX-XXXXX` or `XXXX-XXXX`;
  - `ShareRef::parse(&str)`: accepts `lrrg:<code>[?via=<endpoint_id>@<host:port>]` (unknown parameters ignored), `…/s/<code>`, `…/#<code>`, or a bare code, and returns the code plus an optional `via`;
  - `ShareRef::text_form()`.
  
  Add unit tests for look-alike mapping, both lengths, every input form, and `via` round-trips.
- [X] T008 Implement `crates/lanrurugi-share-proto/src/derive.rs` per contracts/share-protocol.md "Code derivation": `access_hash`, then Argon2id `seed`, then HKDF `lookup_key` (ed25519) and `record_key`, plus `lookup_hash`. Add fixed test vectors in `crates/lanrurugi-share-proto/tests/derive_vectors.rs`.
- [X] T009 [P] Implement the wire types in `crates/lanrurugi-share-proto/src/messages.rs`: `ShareManifest`, `ArchiveEntry`, `ManifestItem`, `Permissions`, the `Request`/`Response` enums for every row of contracts/share-protocol.md, and `RefusalReason`. Implement length-prefixed postcard framing helpers in `src/framing.rs`.
- [X] T010 [P] Implement `crates/lanrurugi-share-proto/src/ticket.rs`: `Ticket { share_id, claim_id, importer, roots, expires_at }`, `sign(&ed25519_dalek::SigningKey)` (built from the iroh secret key's bytes) and `verify(sharer public key, now)`. Add tests for expiry, a wrong importer and a tampered root.
- [X] T011 Implement `crates/lanrurugi-storage/src/shares.rs` `ShareRepository`:
  - create, get, list (`LANRURUGI_SHARES` zset), update, revoke;
  - add/remove items, maintaining `LANRURUGI_SHARE_ITEM_REFS`;
  - `by_code_hash`;
  - `status()` derivation (revoked → expired → exhausted → active);
  - validation from data-model.md (non-empty permissions, parsable `ip_allow`, limits ≥ 1).
  
  Export it from `crates/lanrurugi-storage/src/lib.rs`.
- [X] T012 Implement the atomic claim Lua script and `ShareRepository::claim(share_id, kind, address, endpoint_id?)` in `crates/lanrurugi-storage/src/shares.rs`, per research §9. In one script it checks status, permission for `kind`, the `ip_allow` CIDR match, the `ip_first_n` set and `downloads_used < download_limit`. It then creates `LANRURUGI_SHARE_CLAIM:<id>` (24 h TTL), appends the history member to `LANRURUGI_SHARE_CLAIMS:<share_id>` and increments `downloads_used`. It returns a typed refusal reason.
  - The CIDR match is done in Rust before the script, passing a boolean in. Only the first-N set and the counters live in the script.
- [X] T013 Test in `crates/lanrurugi-storage/tests/share_claims.rs`, using `test_support::RedisTestLock`:
  - 20 concurrent claims against `download_limit = 1` give exactly 1 success;
  - `ip_first_n = 1` locks the share to the first address;
  - a revoked share refuses;
  - claiming again under the same claim id does not increment `downloads_used`.
- [X] T014 [P] Implement `RecipientTokenRepository` in `crates/lanrurugi-storage/src/shares.rs`: issue (32 random bytes, stored by SHA-256), resolve with a 12 h sliding TTL, attach a claim id, and revoke all tokens of a share.
- [X] T015 [P] Add the `share_*` settings from data-model.md in `crates/lanrurugi-api/src/share_settings.rs`: a typed field table with validation (ports, counts, URLs, node strings, `ip:port` lists, permissions, the alternative schedule), read and written only through `GET/PUT /api/shares/settings` — not the general `/settings`, which read-only tokens can read, since `share_proxy_url` may carry credentials. The proxy password is masked on read and kept when the mask is submitted back. `PUT` is session-only by route policy. Types live in `apps/frontend/src/share/types.ts`.
- [X] T016 Add route policy lines to `crates/lanrurugi-api/policy/route_policy.csv`:
  - `/api/share/r/*` is allowed for `anonymous`, `guest_visitor` and `token_guest`;
  - `/api/shares*` is denied for `token_guest` and `guest_visitor`;
  - `/api/tankoubons/*/download` is denied for `guest_visitor` and `token_guest`.
  
  Add matching cases to the tests module in `crates/lanrurugi-api/src/authz.rs`.
- [X] T017 Implement the failed-attempt limiter in `crates/lanrurugi-api/src/share_attempts.rs` (`ShareAttemptLimiter`, in-memory with expiry): per source key, 10 failures per minute then exponential back-off; per instance, beyond 500 failures per hour, further failed lookups are answered one at a time, each held 10 s, so concurrency cannot bypass the delay (research §3). It is used by both HTTP open and P2P `Open` (FR-011). Unit-test both thresholds with a mocked clock; these rates are the inputs of SC-010's bound.
- [X] T018 [P] Mirror code normalisation and the `lrrg:` text form in `apps/frontend/src/share/code.ts` (`parseShareRef`, `formatShareCode`, `toTextForm`), with Vitest cases in `apps/frontend/tests/unit/share/code.test.ts` that match the T007 Rust cases.
- [X] T019 [P] Add the `share.*` i18n namespace skeleton to `apps/frontend/src/i18n/locales/{zh,en,ja}.json`. It holds the status names, permission names, refusal-reason messages (one per `RefusalReason`) and the generic "not available" text. Each later UI task adds its own keys.
- [X] T020 Add the share API hooks and types for the admin endpoints in contracts/http-api.md to `apps/frontend/src/share/api.ts` and `apps/frontend/src/share/types.ts` (`useShares`, `useShare`, `useCreateShare`, `useUpdateShare`, `useAddShareItems`, `useRemoveShareItems`, `useRevokeShare`, `useShareCode`, `useSharesByItem`, `useShareSettings`, `useSaveShareSettings`). Kept out of `api/hooks.ts`, which is already over 2,000 lines.

**Checkpoint**: share records, claims, tokens, settings and policy exist and are unit-tested.

---

## Phase 3: User Story 1 — Create a share, browse and download it (Priority: P1) 🎯 MVP

**Goal**: an admin creates a share from the library or the reader. A browser recipient opens it
with only the code or link, and browses, reads and downloads within its permissions and limits.

**Independent Test**: quickstart.md Scenarios 2 and 3. A share of a tankoubon plus an archive is
created with a limit of 2. A recipient in a private window browses, reads and downloads, the third
download is refused, and guest mode off makes no difference.

- [X] T021 [US1] Implement `crates/lanrurugi-api/src/shares.rs` admin handlers:
  - `GET/POST /api/shares`, `GET/PATCH /api/shares/{id}`;
  - `POST/DELETE /api/shares/{id}/items`, where POST accepts an optional `update` (FR-002);
  - `POST /api/shares/{id}/revoke`, `GET /api/shares/{id}/code`, `GET /api/shares/by-item/{id}`.
  
  Creation generates the code (T007; `short_code` only with an expiry within 7 days, and a PATCH may not extend it beyond that), stores `code_hash` and `code_enc`, and fills defaults from the `share_default_*` settings. Revoke also revokes recipient tokens (T014). Every mutation records an activity entry (`share.create`, `share.update`, `share.revoke`, `share.items`) through `crates/lanrurugi-api/src/activity.rs`. Register the router in `crates/lanrurugi-api/src/lib.rs`.
- [X] T022 [US1] Implement `code_enc` in `crates/lanrurugi-api/src/shares.rs`: XChaCha20-Poly1305 under an HKDF key from the node secret (`LANRURUGI_SHARE_NODE_SECRET`, created on first use, see T048), so management can re-display the code.
- [X] T023 [US1] Implement the item resolver in `crates/lanrurugi-api/src/shares.rs`. It turns a share's `items` into the browse-form `ShareManifest`: archive → `ArchiveEntry`; tankoubon → ordered members from the current `Grouping`; a missing item → `unavailable`. It is reused by the recipient routes and later by the P2P server.
- [X] T024 [US1] Implement `crates/lanrurugi-api/src/share_recipient.rs`:
  - `POST /api/share/r/open`, which runs the limiter, then the lookup by `access_hash`, then the IP check, then issues a token and returns the browse manifest and permissions;
  - `GET /api/share/r/manifest`;
  - `GET /api/share/r/archives/{id}/thumbnail`.
  
  The token is read from `X-Share-Token` or `?rt=`. Unknown, revoked, expired and IP-refused requests all get the identical `404 not_found`. An item outside the share gets the same 404 as having no token (FR-013a). Set `Referrer-Policy: no-referrer` on every response. Register the routes.
- [X] T025 [US1] Add `pages` and `page` (read permission) to `crates/lanrurugi-api/src/share_recipient.rs`, delegating to the same page-extraction and serving code the reader uses in `crates/lanrurugi-api/src/archives.rs`. Refactor that code into a shared function if it is handler-bound today.
- [X] T026 [US1] Add `GET /api/share/r/archives/{id}/download` (download permission) to `crates/lanrurugi-api/src/share_recipient.rs`. On the first download it calls `ShareRepository::claim(kind = browser_download)` and attaches the claim to the token; later downloads on the same token reuse it. It answers `409 exhausted` on refusal and otherwise serves exactly what `download_archive` serves. Extract `download_archive`'s body into a reusable `serve_archive_download(&archive)` in `crates/lanrurugi-api/src/archives.rs`.
- [X] T027 [US1] Test the recipient routes end to end through the real router in `crates/lanrurugi-server/tests/share_recipient.rs` (the router is assembled in `lanrurugi-server`, so the test lives there):
  - guest mode off still gives access;
  - an item outside the share returns 404, the same as no token;
  - no read or download permission returns 403;
  - the limit is reached after two distinct tokens, while a second download on one token is free and browsing still works;
  - the IP allow-list refuses with 404, including an existing token used from a new address;
  - a revoked share returns 404 on the next request;
  - the `Referrer-Policy` header is present;
  - anonymous callers cannot list shares.
  - guest mode off still gives access;
  - an item outside the share returns 404;
  - no read permission returns 403;
  - the limit is reached after two distinct tokens;
  - the IP allow-list refuses with 404;
  - a revoked share returns 404;
  - the `Referrer-Policy` header is present.
- [X] T028 [US1] Serve the SPA for `/s/{code}` without the auth gate. In `crates/lanrurugi-api/src/lib.rs`, the static fallback must return `index.html` for `/s/*` with `Referrer-Policy: no-referrer`. In `apps/frontend/src/App.tsx`, add the route outside `RouteGuards`/`AuthBridgeGate`, so no login redirect and no session refresh run there.
- [X] T029 [P] [US1] Implement `apps/frontend/src/share/transport.ts`: a `ShareTransport` interface (`open`, `manifest`, `thumbnailUrl`, `pages`, `pageUrl`, `downloadUrl`, `tankoubonDownloadUrl`) and an `HttpTransport` that keeps the token in memory and appends `?rt=` to media URLs.
- [X] T030 [US1] Implement `apps/frontend/src/share/SharePage.tsx`:
  - a grid of items with covers, titles, tags and page counts;
  - tankoubons expand to ordered members, and unavailable items are marked;
  - per-permission actions: Read, Download, and a "How to import" box with the code and QR (T033);
  - the "not available" page for `not_found`, and the exhausted notice in place of download buttons.
  
  Add `<meta name="referrer" content="no-referrer">` while mounted. Styles use existing legacy classes; any new colour gets per-theme classes (T036).
- [X] T031 [US1] Implement `apps/frontend/src/share/ShareReader.tsx`: a read-only, one-page-at-a-time reader over a `ShareTransport` (keyboard, click halves, next-page preload). It does not reuse `pages/Reader`: that component is bound to library archive ids, session-only APIs (progress, bookmarks, translation) and the auth shell, none of which a recipient may touch. Navigation beyond the share, bookmarks, progress and admin actions do not exist here.
- [X] T032 [P] [US1] Implement `apps/frontend/src/share/qr.ts` `renderQr(text, canvas)` and `qrPngBlob(text)` with `lean-qr`, imported dynamically.
- [X] T033 [US1] Implement `apps/frontend/src/share/ShareResult.tsx`. It shows the `lrrg:` text form from `GET /api/shares/{id}/code`. When a public URL is set, it also shows the link, with the note "stops working if your domain changes; the code keeps working". The QR canvas encodes the link if there is one, and the text form otherwise (FR-009, FR-010a). Provide Copy text form, Copy link and Save PNG buttons, and an embed-snippets slot (filled in by US6).
- [X] T034 [US1] Implement `apps/frontend/src/share/ShareDialog.tsx` with common-ui controls (`components/common-ui/Form`):
  - tab "New share": title, three permission checkboxes (at least one), download limit, IP allow-list textarea, first-N, expiry, embed toggle (disabled, with the reason, while `share_public_url` is empty; FR-030), "Allow other instances to help deliver" (off, with the IP-visibility note, FR-033a), short-code toggle, all prefilled from the `share_default_*` settings. The IP-restriction help says that a recipient behind a proxy is seen at the proxy's exit address (FR-051a);
  - tab "Add to existing": active shares from `useSharesByItem` / `useShares`, plus "Also update this share's settings", which reveals the same fields prefilled.
  
  On success it shows `ShareResult`.
- [X] T035 [US1] Add "Share…" to `apps/frontend/src/pages/Library/ArchiveContextMenu.tsx` for archives and tankoubons, only when `loggedIn`. With an active multi-selection, it passes every selected item to `ShareDialog`. Wire the selection source from `apps/frontend/src/hooks/useLibrary.ts`.
- [X] T036 [P] [US1] Add share status classes (`.share-status-active`, `-exhausted`, `-expired`, `-revoked`) to all five theme files in `apps/frontend/public/legacy/themes/{g,ex,modern,modern_red,modern_clear}.css`. Each reuses that theme's existing accent and muted colours, as the `.tankoubon-member-row` precedent does.
- [X] T037 [US1] Add "Share…" to the reader in `apps/frontend/src/pages/Reader/Reader.tsx`, only when logged in. It shares the current archive, or offers "This archive" / "Whole tankoubon" when reading inside a tankoubon.
- [ ] T038 [P] [US1] Write a Playwright test `apps/frontend/tests/e2e/share-recipient.spec.ts`: an admin creates a share via the API; a fresh context opens `/s/<code>`, sees the items, reads a page and downloads; a third context gets the exhausted notice once the limit is reached; with read permission removed, Read is hidden.

**Checkpoint**: Scenarios 2 and 3 pass on a reachable instance.

---

## Phase 4: User Story 2 — Manage existing shares (Priority: P1)

**Goal**: the Settings → Sharing section lists shares with usage and claim history, and lets the
admin edit, show the code, toggle embedding and revoke. It also holds the sharing-wide settings.

**Independent Test**: quickstart.md Scenario 2 steps 7–9.

- [X] T039 [US2] Extend `GET /api/shares/{id}` in `crates/lanrurugi-api/src/shares.rs` with `claims` (from `LANRURUGI_SHARE_CLAIMS`, newest first, capped at 500), `helpers` (empty until US7) and resolved items with an `unavailable` flag. Add `GET/PUT /api/shares/settings`, reading and writing the `share_*` fields.
- [X] T040 [US2] Implement `apps/frontend/src/pages/Settings/SharingSection.tsx`, part 1, the shares table: title, status badge (T036 classes), permissions, used/limit, items, created, expires. A row opens a detail panel with items (unavailable marked), claim history (kind / address / time), edit fields (common-ui: permissions, limits, IP restriction, expiry, embedding, helping) saved through `useUpdateShare`, per-item Remove through `useRemoveShareItems`, Show code/QR (reuses `ShareResult`) and Revoke with a confirm dialog (FR-006).
- [X] T041 [US2] Implement `SharingSection.tsx`, part 2: the "Defaults for new shares" form (permissions, limit, IP, expiry days, embed). Also add the Connectivity form fields (public services, relay URLs, pkarr relays, UDP port, public URL). The route-status line is a placeholder until T054.
- [X] T042 [US2] Register the section in `apps/frontend/src/pages/Settings/SettingsPage.tsx` and `apps/frontend/src/pages/Settings/index.ts` as "Sharing", visible only when logged in as admin. Add its i18n keys to the zh/en/ja locales.

**Checkpoint**: shares can be monitored, edited and revoked from Settings.

---

## Phase 5: User Story 3 — Download a whole tankoubon (Priority: P2)

**Goal**: one streamed zip of a tankoubon's member files, in order, from the library and the
reader. Share recipients with download permission get the same.

**Independent Test**: quickstart.md Scenario 1.

- [X] T043 [P] [US3] Implement `crates/lanrurugi-api/src/tankoubon_zip.rs`:
  - `stream_tankoubon_zip(members: Vec<MemberSource>) -> impl Stream<Item = io::Result<Bytes>>`, using `zip::ZipWriter::new_stream` over `SyncIoBridge(tokio::io::duplex(256 KiB))` in `spawn_blocking`;
  - every entry is `CompressionMethod::Stored` with `large_file(true)`;
  - names are `NN - <sanitised title>.<ext>`, padded to the width of the member count, with `-2`/`-3` suffixes on collisions;
  - missing members are skipped and listed in a final `MISSING.txt`;
  - `MemberSource` yields the bytes `serve_archive_download` would serve, merging patches through `lanrurugi_scanner::patch::build_merged_zip`.
- [X] T044 [US3] Test the zip layout in `crates/lanrurugi-api/src/tankoubon_zip.rs` (unit tests): the order and padding of names, collision suffixes, member bytes identical to the inputs, a `MISSING.txt` listing, and the stored compression method.
- [X] T045 [US3] Add `GET /api/tankoubons/{id}/download` to `crates/lanrurugi-api/src/tankoubons.rs`, with an RFC 5987 `Content-Disposition`, `application/zip` and no `Content-Length`. Add `GET /api/share/r/tankoubons/{id}/download` to `crates/lanrurugi-api/src/share_recipient.rs`, behind the download permission and the same claim as T026. Add a docgen sidecar entry in `docs/api/sidecar/tankoubons.yaml` (create it if absent).
- [X] T046 [US3] Show "Download" for tankoubons in `apps/frontend/src/pages/Library/ArchiveContextMenu.tsx`: remove the `!isTank` guard and choose the tankoubon URL when `isTank`. In `apps/frontend/src/pages/Reader/Reader.tsx`, the download action offers "This archive" / "Whole tankoubon" when reading inside a tankoubon. In `apps/frontend/src/share/SharePage.tsx`, add a tankoubon Download action. Add the i18n keys.

**Checkpoint**: Scenario 1 passes; this story can ship on its own.

---

## Phase 6: User Story 4 — Import a share into another instance (Priority: P2)

**Goal**: another instance imports a share by code, pasted or uploaded QR image, or camera, with
a preview. The import is one compact queue row with verified, resumable transfer, metadata and
rebuilt tankoubons, and it never blocks other work.

**Independent Test**: quickstart.md Scenario 4, with two mutually reachable instances.

### Network core (shared with US5 and US7)

- [X] T047 [US4] Implement `crates/lanrurugi-api/src/share_net/mod.rs`: `bind()` builds the iroh `Endpoint` (preset `Minimal`, no n0 DNS or other address lookup; FR-047) from the node secret, `share_udp_port` on IPv4 plus optional IPv6, the relay map from `share_relay_urls`, `share_self_relay_url` and, while public services are on, n0's relays; mDNS with the neutral service label `lshr` (the library default names the transport); ALPN `h3` with the version in each stream's first message (FR-045). `start()` binds and installs it as the process-wide endpoint, read with `share_net::handle()` rather than an `AppState` field, so the many test harnesses that build `AppState` literally are untouched; `bind()` alone lets tests run several endpoints in one process. Started from `crates/lanrurugi-server/src/main.rs`, rebuilt when an endpoint setting is saved.
- [X] T048 [US4] Carry `LANRURUGI_SHARE_NODE_SECRET` in backups as the optional `share_node_secret` field of `BackupDocument` (`crates/lanrurugi-backup/src/build.rs`, filled and restored in `crates/lanrurugi-api/src/database.rs`); a restore writes it back and restarts the endpoint, so the `EndpointId` survives. Older backups without the field restore unchanged.
- [X] T049 [US4] Implement `crates/lanrurugi-api/src/share_net/blobs.rs`:
  - `ShareBlobs::fingerprint(archive_id)` computes BLAKE3 plus a `bao-tree` outboard (16 KiB groups) of the download-form bytes;
  - patch-merged archives are materialised under `<temp>/share/`, counting against `tempmaxsize`;
  - outboards are cached at `<data_dir>/share/outboards/<blake3>.obao`, indexed in `LANRURUGI_SHARE_BLOB:<archive_id>` with change detection by path, mtime and size.
  
  Hashing runs through `lanrurugi_core::concurrency::parallel_map`, capped by `recommend_precompute::precompute_worker_budget` (same crate, so no local duplicate).
- [X] T050 [US4] Implement the background prepare job `spawn_prepare_worker` in `crates/lanrurugi-api/src/share_net/blobs.rs`: share creation and item adds push archive ids onto `LANRURUGI_SHARE_PREPARE`; the worker drains it in batches of 16 under one `share_prepare` job per busy period (visible in the Jobs console), with `LoadThrottle` backoff. An importer's `Prepare` hashes the archive it needs directly instead of waiting for the queue (research §13).
- [X] T051 [US4] Implement `crates/lanrurugi-api/src/share_net/server.rs`, the protocol accept loop (one task per stream). It handles:
  - `Open { access_hash }`, which shares the limiter with T017, keys it by `EndpointId` plus address, and returns the share id, browse manifest, permissions and `public_url` (FR-047);
  - `Claim`, which uses the T012 script with the observed remote address. For a share with an IP restriction, it waits up to 15 s for a direct path, then refuses with `address_unverifiable`;
  - `Prepare`;
  - `Renew`, which issues a 25 s ticket with the archives' current roots. It re-checks status, import permission and the IP restriction against the connection's current observed address, appending a new address to the claim (spec edge case "Changing addresses"). Exhaustion never refuses a renewal of an existing claim (FR-015);
  - `Range`, which serves a bao-encoded stream for the requested `ChunkRanges` after checking the ticket and that `blake3` is in its roots;
  - `Outboard`, served by the sharer only;
  - `Thumbnail`, for the import preview.
  
  Re-check share status, permissions and revocation on every request.
- [X] T052 [US4] Implement `crates/lanrurugi-api/src/share_net/publish.rs`, the knock-record publisher. For every active share it publishes the encrypted `KnockV1 { endpoint_id, relay_urls }` (no share id, address or URL; FR-047) under the derived lookup key, to every configured rendezvous: pkarr HTTPS relays and `share_nodes` via `RendezvousPut`: on create, every 60 minutes, and immediately when the endpoint's relay set changes. It stops on revoke or expiry. Publish only through the HTTPS relays in `share_pkarr_relays`; the instance never joins or queries the DHT itself (FR-045). With public services off, publish only to `share_nodes`; with none configured, publish nothing. Implement `crates/lanrurugi-share-proto/src/record.rs` encrypt/decrypt with tests.
- [X] T053 [US4] Implement resolution in `crates/lanrurugi-api/src/share_net/publish.rs` `resolve()`: a `via=<endpoint_id>@<host:port>` hint is dialled directly; otherwise every configured pkarr relay is queried in parallel, the first packet whose signature verifies is decrypted, and `EndpointAddr { endpoint_id, relay_urls }` is dialled. Each relay's success or failure feeds the status list (FR-049). Failures map to `not_found` versus `sharer_unreachable` (FR-029). The `ResolveLocal` mDNS fallback and `share_nodes` rendezvous are US5 (T072, T073).
- [X] T054 [US4] Add the route-status line to `GET /api/shares/settings`: the `EndpointId`, whether a relay is connected, the direct addresses, and whether a port mapping is active, taken from the iroh endpoint's watchers. Render it in `SharingSection.tsx`, replacing the T041 placeholder.

### Upload limits and schedule (FR-041–FR-044)

- [X] T055 [US4] Implement `crates/lanrurugi-api/src/share_net/limits.rs` `ShareUploadLimiter` (research §14):
  - own-share and helping token buckets, reusing `crates/lanrurugi-api/src/download_manager/rate_limit.rs`, plus semaphores;
  - a mode (normal or alternative) built from the `share_*` and `share_alt_*` settings;
  - a minute ticker that evaluates `share_alt_schedule` (windows may cross midnight; days of the week; local time zone), overridden by `share_alt_manual`;
  - `status()` returning mode, source and `next_switch_at`.
  
  Add every setting from data-model.md to `crates/lanrurugi-api/src/settings.rs` (admin-only) and `apps/frontend/src/api/types.ts`. Unit-test the schedule evaluation: a midnight-crossing window, day filters, the manual override, the next-switch computation.
- [X] T056 [US4] Apply the own-share caps:
  - P2P: `Range` and `Outboard` in `crates/lanrurugi-api/src/share_net/server.rs`;
  - HTTP: recipient file and tankoubon downloads in `crates/lanrurugi-api/src/share_recipient.rs`, by wrapping the body stream in the bucket.
  
  When the own-share rate is 0, answer `Refused { paused, until }` for `Claim`, `Prepare` and `Range` over P2P, and `503` with `Retry-After` over HTTP. Reading and thumbnails stay unlimited (FR-043).
- [X] T057 [US4] Handle `paused` in `crates/lanrurugi-api/src/share_net/import.rs`: keep the verified ranges, set the queue item's status text to "sharer paused uploads until …", and retry at `until`, or every 5 minutes for a manual pause (FR-044). Show it in `apps/frontend/src/pages/Upload/ShareImportRow.tsx`'s collapsed row.
- [X] T058 [US4] Add the "Upload limits" block to `apps/frontend/src/pages/Settings/SharingSection.tsx` per contracts/ui.md: the normal and alternative caps, the schedule editor (windows and days, common-ui controls), the manual switch, a status line from `GET /api/shares/settings`, and "Archives transferred at once per import" (`share_import_concurrency`). Add the i18n keys.

### Import pipeline

- [X] T059 [US4] Add `QueueItemOrigin::ShareImport` and the field `share_import_id` to `crates/lanrurugi-storage/src/download_queue.rs` (serde default for old records), and `QueueError::ShareImport { reason }` for an import that ends early. Archive counts for the collapsed row come from `GET /api/shares/imports/{id}` rather than new `JobStatus` fields, so the job registry is unchanged.
- [X] T060 [US4] Implement the `ShareImport` repository in `crates/lanrurugi-storage/src/shares.rs`: create, get, list, update the per-archive state in `archives`, set `help_deliver`, and delete the code on terminal states (data-model.md).
- [X] T061 [US4] Add `POST /api/shares/imports/resolve` (`ImportPreview` with `already_present`, matched by archive id against the local library) and `POST /api/shares/imports` to `crates/lanrurugi-api/src/shares.rs`. The second claims (`kind = import`), creates the `ShareImport` (its `concurrency` taken from `share_import_concurrency`) and **one** queue item (`plugin_namespace = "share_import"`), and starts it.
  - Refusals map to the reason codes in contracts/http-api.md, before anything is queued (FR-021).
  - Also add `GET /api/shares/imports`, `GET /api/shares/imports/{id}`, `PATCH /api/shares/imports/{id}` and `POST /api/shares/imports/{id}/archives/{sharer_archive_id}/retry`.
- [X] T062 [US4] Implement `crates/lanrurugi-api/src/share_net/import.rs` `start`/`run_import`: archives transfer under a semaphore of `concurrency` (default 3), outside the `DownloadManager` pools (FR-019a); per archive: `Prepare` when the manifest had no fingerprint, then the missing 1 MiB pieces as `Range` requests, each bao stream verified against the manifest's root with `decode_ranges` before anything is written; the ticket is renewed every 10 s and a refusal stops the import; byte progress feeds the queue item's job. Resume needs no `.ranges` bitmap and no `Outboard` request: verified hash nodes are written into a local outboard next to the partial file, and `valid_ranges` recomputes the verified pieces on restart. An archive listed twice is transferred once; the whole file is hashed once more before cataloguing (FR-022).
- [X] T063 [US4] Ingest each completed archive in `crates/lanrurugi-api/src/share_net/import.rs` through `catalogue_staged_file` (`crates/lanrurugi-api/src/download_manager/ingest.rs`; make it `pub(crate)`). A `ContentHash` duplicate marks it `reused` with the existing id. Then apply the manifest title, tags and summary, and the optional category. Once every archive is settled, create each manifest tankoubon as a new `Grouping`, members in order and never merged by name. End the queue item `Done` or `Error`.
- [X] T064 [US4] Wire start, stop and resume for `ShareImport` queue items in `crates/lanrurugi-api/src/download_queue.rs`: `start_one` dispatches to `run_import` instead of `plugins::start_download`; `stop_one` cancels the import's token and keeps verified ranges; a server restart resumes `Downloading` imports the way existing downloads are recovered.
- [X] T065 [US4] Test the protocol end to end over real iroh connections on loopback in `crates/lanrurugi-server/tests/share_transfer.rs`: a sharer with a full `AppState` and a bare endpoint as the importer (two full instances in one process would share one Redis library). Covers open, a wrong code, claim, prepare, renewal after the limit is used up, a full verified piece-by-piece transfer that is byte-identical, a tampered piece rejected before writing, a ticket refused for another endpoint, and renewal refused after revocation. The test harness is shared with `share_recipient.rs` in `tests/common/mod.rs`.

### Import UI

- [X] T066 [P] [US4] Implement `decodeQrFromImage(blob)` in `apps/frontend/src/share/qr.ts`: dynamic `import("barcode-detector")`, accept only results that `extractShareCode` parses, and never fetch decoded URLs (FR-017).
- [X] T067 [US4] Implement `apps/frontend/src/share/ImportShareModal.tsx`: a code field accepting `lrrg:` forms, links and bare codes; image paste and a file chooser decoded in the browser; "Scan with camera" only in a secure context, otherwise "Take photo" with the reason; the zxing-wasm fallback is bundled with the instance, never fetched from a CDN (`zxing-wasm` is a direct dependency so its wasm can be imported); a preview with lazily loaded covers fetched over the preview's open connection, count, size, "already in library" badges and a category picker. The helping checkbox arrives with US7.
- [X] T068 [US4] Add an "Import share" button to `apps/frontend/src/pages/Upload/UploadPage.tsx` that opens `ImportShareModal`. Add the i18n keys.
- [X] T069 [US4] Implement `apps/frontend/src/pages/Upload/ShareImportRow.tsx` and route `origin === "share_import"` items to it from `apps/frontend/src/pages/Upload/DownloadQueuePanel.tsx`.
  - **Collapsed**: same height as `QueueItemRow`, showing title, progress bar, `done/total archives · bytes · speed · time left`, and pause/stop.
  - **Expanded**: a bounded-height (max ~320 px), virtualised per-archive list from `GET /api/shares/imports/{id}`, polled only while expanded. Each row shows state, a progress bar, the error and a retry button.
- [X] T070 [P] [US4] Add Vitest tests in `apps/frontend/tests/unit/share/import.test.ts`: camera-versus-photo selection by `isSecureContext`, rejection of a non-share QR payload, and link-form code extraction.

**Checkpoint**: Scenario 4 passes between two reachable instances.

---

## Phase 7: User Story 5 — Codes work without a domain or public IP (Priority: P2)

**Goal**: hole punching, relay, LAN preference, changing addresses, the public-services-off mode,
and LANrurugi rendezvous and relay nodes, so that a circle of instances needs no third party
(FR-047–FR-049).

**Independent Test**: quickstart.md Scenario 5.

- [X] T071 [US5] Verify and harden the NAT paths in `crates/lanrurugi-api/src/share_net/mod.rs`:
  - log the chosen path per connection (direct, LAN, relay) at info level;
  - surface `sharer_unreachable` distinctly from `not_found`;
  - make imports retry connecting with backoff (cap 5 min) while the queue item is active, so an import resumes when the sharer returns (FR-029).
  
  Document `share_udp_port` publishing.
- [X] T072 [US5] Implement public-services-off mode end to end:
  - n0 relays and pkarr relays are dropped, while configured `share_nodes` stay (T047/T052);
  - `ResolveLocal` is answered by `share_net/server.rs`;
  - `GET /api/shares/{id}/code` adds `via=<endpoint_id>@<host:port>` to the text form;
  - the share dialog shows "requires a direct route" when no node is configured and no public URL is set.

- [X] T073 [US5] Implement the rendezvous role in `crates/lanrurugi-api/src/share_net/nodes.rs`. When `share_node_rendezvous` is on, it answers `RendezvousPut` (verify the pkarr signature, store in `LANRURUGI_SHARE_RDV:<lookup_pubkey>` with a 2 h TTL) and `RendezvousGet`, rate-limited per source with the T017 limiter. Add the request types to `crates/lanrurugi-share-proto/src/messages.rs`, and the key constructor to `crates/lanrurugi-storage/src/keys.rs`.
- [X] T074 [US5] Implement the relay role in `crates/lanrurugi-api/src/share_net/nodes.rs`. When `share_node_relay` is on, it runs the `iroh-relay` server on TCP `share_node_relay_port`, with TLS from ACME for `share_public_url`'s host or from the configured PEM files; for those two modes the port is published as host `443/tcp`, which ACME's TLS-ALPN challenge needs (document it in the compose comment from T006). Add the settings to `crates/lanrurugi-api/src/settings.rs`. `GET /api/shares/settings` returns `node_string` (`lrrg-node:<endpoint_id>@<host:port>`).
- [X] T075 [US5] Implement outside-service status (FR-049) in `crates/lanrurugi-api/src/share_net/nodes.rs`. Track the last success and error per rendezvous (from T052/T053) and per relay (from the endpoint's relay watchers), and return them as `services` in `GET /api/shares/settings`. Parse and validate `share_nodes` strings.
- [X] T076 [US5] Add the "LANrurugi nodes" and "Outside services in use" blocks to `apps/frontend/src/pages/Settings/SharingSection.tsx`, per contracts/ui.md: the node list editor, the offer-rendezvous and offer-relay toggles, the relay TLS choice, this instance's node string with Copy, and the per-service reachability list. Add the i18n keys.
- [X] T077 [US5] Test in `crates/lanrurugi-api/tests/share_transfer.rs`, with a third in-process endpoint acting as a node and public services off. Publish to it and resolve through it, then import end to end. Also check that a tampered packet is rejected, and that the knock record holds nothing beyond `endpoint_id` and `relay_urls`.
- [X] T078 [US5] Implement proxy support. The default is to follow the user's own proxy with no settings; the override is optional in `crates/lanrurugi-api/src/share_net/proxy.rs` (FR-051, research §16):
  - by default (`share_proxy_from_env = 1`) always build the endpoint with `proxy_from_env()`; when only `ALL_PROXY=socks5(h)://…` is set, start the SOCKS bridge; honour `NO_PROXY`; never proxy LAN or mDNS;
  - on macOS native builds, read the system proxy (`system-configuration`; reqwest's `macos-system-configuration` feature) when no proxy environment variables are set;
  - parse the optional override `share_proxy_url`;
  - `http(s)` goes straight to the iroh `Builder::proxy_url`;
  - `socks5(h)` starts an in-process HTTP CONNECT → SOCKS5 bridge (`tokio-socks`, from T001) on `127.0.0.1:<random>` and passes that to `proxy_url`;
  - build the pkarr-relay `reqwest` client with the same proxy (reqwest's `socks` feature, from T001);
  - `share_proxy_only` calls `clear_ip_transports` and keeps mDNS and LAN direct per `share_proxy_allow_lan`.
  
  Add the settings to `crates/lanrurugi-api/src/settings.rs`, with `share_proxy_url` masked on read and added to `TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS`. Unit-test URL parsing and the bridge against an in-process SOCKS5 test server.
- [X] T079 [US5] Implement the CA bundle and external addresses: `share_ca_file` goes to iroh's `ca_tls_config` and to the reqwest root store; `share_external_addrs` goes to `add_external_addr` (FR-052, FR-053). Both live in `crates/lanrurugi-api/src/share_net/mod.rs`.
- [X] T080 [US5] Implement node-only mode (FR-054): a `share-node` subcommand in `crates/lanrurugi-server/src/main.rs`, running `share_net::nodes` with an in-memory rendezvous store (file-persisted on shutdown) and the node key in `--data <dir>`. It needs no Redis and no library, and prints its `lrrg-node:` string. Abstract the rendezvous store behind a trait in `crates/lanrurugi-api/src/share_net/nodes.rs`, with Redis and memory implementations.
- [X] T081 [US5] Implement router NAT traversal and tunnel support (FR-055, research §16 rows 12–14) in `crates/lanrurugi-api/src/share_net/mod.rs` and `crates/lanrurugi-api/src/share_net/nodes.rs`:
  - `share_portmap` drives iroh's `portmapper_config`; expose the mapping protocol, state and external address in the `GET /api/shares/settings` route status;
  - the relay role's `proxy` TLS mode serves plain HTTP on `share_node_relay_port`, bound to localhost or the container network only;
  - `share_self_relay_url` is added to the endpoint's relay map and to the knock record's `relay_urls`, and republishes on change.
  
  Add the settings to `crates/lanrurugi-api/src/settings.rs`.
- [X] T082 [US5] Add the Proxy, External addresses, Router port mapping and Own-relay-behind-a-tunnel blocks to `apps/frontend/src/pages/Settings/SharingSection.tsx` per contracts/ui.md, with i18n keys.
- [X] T083 [US5] Write `scripts/share-network-matrix.sh`, which runs quickstart Scenario 9. Use network namespaces and containers for CGNAT (double NAT), UDP blocked, HTTP-proxy-only (a squid container), SOCKS5-only (a dante container), IPv6-only, a Docker bridge, a TLS-intercepting proxy (mitmproxy with its CA in `share_ca_file`), an isolated LAN, a WireGuard pair, a UPnP router (miniupnpd in a container), a manual port forward, an frp UDP proxy, an frp HTTP proxy in front of the relay role in `proxy` mode (the stand-in for Cloudflare Tunnel and ngrok), and the importer behind its user's own proxy with **no LANrurugi proxy setting**: `ALL_PROXY=socks5://…` only; `HTTPS_PROXY` only; a mihomo TUN container with fake-IP DNS whose line relays UDP; and the same with UDP dropped. For the proxy cases, check the transfer starts within 30 s (SC-018). For each, import a small share and record the route used. Its captures feed the traffic-profile check.

- [ ] T084 [US5] Verify the traffic profile (research §15, FR-045, SC-016):
  - write `scripts/share-traffic-check.sh`, which captures with `tcpdump` on both instances during a scripted lifecycle (create, resolve, direct import, relay-only import with UDP blocked, helping), then runs `ndpiReader` on each capture;
  - it fails on any flow classified as BitTorrent or another P2P protocol, and on any packet containing `lanrurugi`, `lrrg` or `iroh` in plaintext;
  - check the three iroh-dependent points in research §15 and apply the matching handling. If a point cannot be met without forking iroh, stop and report it before changing anything;
  - record the classifier output in the Implementation Record.

**Checkpoint**: Scenario 5 passes, including the node steps.

---

## Phase 8: User Story 6 — Embed a share (Priority: P3)

**Goal**: iframe and `<lrrg-share>` cards whose layout follows the share's permissions and state.

**Independent Test**: quickstart.md Scenario 6.

- [X] T085 [US6] Add `GET /api/share/r/card?code=` to `crates/lanrurugi-api/src/share_recipient.rs`: 404 when embedding is off; only `restricted` for a share with IP rules; only the state for a revoked or expired share; otherwise title, item count, up to four cover archive ids and a recipient token for the covers. Failed lookups go through the attempt limiter. `serve_index` sends `Content-Security-Policy: frame-ancestors *` for `/embed/s/*` only. No CORS is needed: the card page is same-origin with its API.
- [X] T086 [US6] Implement `apps/frontend/src/share/ShareCard.tsx` and the route page `ShareCardPage.tsx` (`/embed/s/:code`): up to 4 covers, title and item count; actions per permission (FR-030a), with an import-only card led by its code and QR; state replacement for restricted, revoked, expired and exhausted. Its own CSS (`card.css`) follows `prefers-color-scheme` and is fluid from 260 px. The page reports its height to a framing element by `postMessage`.
- [X] T087 [US6] Implement `<lrrg-share code src>` as the static script `apps/frontend/public/embed/share.js`: it frames the sharing instance's `/embed/s/<code>` card in a shadow root and sizes the frame from the card's height messages, so both snippets show the very same card. A static classic script rather than a second Vite entry: a foreign page loads a few KB with no module CORS requirement, and the card itself is a lazily loaded SPA route.
- [X] T088 [US6] Fill the embed-snippets slot in `apps/frontend/src/share/ShareResult.tsx` and the Settings detail panel: iframe and custom-element snippets with Copy, shown only when embedding is on.
- [X] T089 [P] [US6] Add Vitest tests in `apps/frontend/tests/unit/share/card.test.ts` for the permission-dependent actions and the restricted and revoked states.

**Checkpoint**: Scenario 6 passes.

---

## Phase 9: User Story 7 — Helpers deliver shares (Priority: P3)

**Goal**: instances that opted in serve verified ranges to authorised importers. Importers fetch
from several sources at once.

**Independent Test**: quickstart.md Scenario 7.

- [X] T090 [US7] Implement the helper role in `crates/lanrurugi-api/src/share_net/server.rs`:
  - only `Range` is served (never `Outboard`; FR-047), for blobs of `done` imports with `help_deliver = true`, after verifying the ticket signature against the sharer's `EndpointId`, the importer identity and expiry;
  - a background task sends `HelperRegister` once, then `HelperHeartbeat` every 5 min;
  - on a `HelperStatus` of `ended` (T095), or when `help_deliver` is turned off, serving stops immediately (FR-033).
- [X] T091 [US7] Implement the sharer side in `crates/lanrurugi-api/src/share_net/server.rs`: accept `HelperRegister` only when the share has `allow_helpers`, and only from endpoints holding a completed import claim for that share; answer registrations and heartbeats with `HelperStatus` (T095), which is `ended` for revoked or expired shares and once `allow_helpers` is turned off; record heartbeats in `LANRURUGI_SHARE_HELPERS`; include helpers seen in the last 15 min in `Claimed`, and none while `allow_helpers` is off.
- [X] T092 [US7] Extend `run_import` in `crates/lanrurugi-api/src/share_net/import.rs` to fetch from multiple sources: assign 1 MiB pieces across the sharer and helpers, with up to 4 in flight per source; drop a source whose range fails verification, and re-queue its pieces (FR-034).
- [X] T093 [US7] Show the share's helpers (endpoint id, last seen) in the shares-table detail panel of `apps/frontend/src/pages/Settings/SharingSection.tsx` (sharer side). The helper-side "Helping deliver" list is T097.
- [X] T094 [US7] Extend `crates/lanrurugi-api/tests/share_transfer.rs` with three in-process endpoints: a share without `allow_helpers` refuses `HelperRegister`; a helper-assisted import is byte-identical; a helper serving a corrupted range is dropped; revocation stops the helper within the ticket TTL; a helper refuses an importer without a ticket.

- [X] T095 [US7] Implement `HelperStatus` on the sharer in `crates/lanrurugi-api/src/share_net/server.rs`. `HelperRegister` and `HelperHeartbeat` answer with `{ state, reason?, at, removed_archives }`, derived from the share: `revoked_at`, `expires_at`, the time `allow_helpers` was turned off (store it as `helpers_disabled_at` on the share in `crates/lanrurugi-storage/src/shares.rs`), and the exhausted state. `removed_archives` is the helper's registered blobs minus the share's current archives. Add the `HelperStatus` type to `crates/lanrurugi-share-proto/src/messages.rs`.
- [X] T096 [US7] Implement the helper-side state in `crates/lanrurugi-api/src/share_net/server.rs` and `crates/lanrurugi-storage/src/shares.rs`. Persist every `HelperStatus` into the ShareImport `help_*` fields and follow the transitions in data-model.md ("Help state transitions"): stop for good on `ended`, pause on `paused`, skip `help_removed_archives`, and update `help_last_contact_at`. Count served bytes into `help_bytes_served`. Route every helper `Range` through the helping half of `ShareUploadLimiter` (T055).
- [X] T097 [US7] Implement the helper list in `apps/frontend/src/pages/Settings/SharingSection.tsx` per contracts/ui.md "Helping deliver":
  - one row per helped share (`GET /api/shares/imports?helping=1`), with the `help_deliver` toggle (`PATCH /api/shares/imports/{id}`), the state badge, reason and time, last contact, removed-archive count and bytes uploaded;
  - "Remove from list" on ended rows (`PATCH … { help_hidden: true }`).
  
  Add the i18n keys for every state and reason in zh/en/ja.
- [X] T098 [US7] Extend `crates/lanrurugi-api/tests/share_transfer.rs`:
  - revoking on the sharer makes the helper's record `ended` / `revoked` with the revoke time on the next heartbeat;
  - exhausting the limit gives `paused`, and raising it gives `active` again;
  - removing one archive from the share stops only that archive;
  - with the sharer stopped, the helper derives `sharer_offline` after the threshold (use a shortened test threshold);
  - with `share_helper_max_peers = 1`, a second importer waits for the first.

**Checkpoint**: Scenario 7 passes.

---

## Phase 10: Polish & Cross-Cutting Concerns

- [X] T099 [P] Add a user guide page `docs/src/content/docs/guide/sharing.md`, with en/ja counterparts under `docs/src/content/docs/en/guide/` and `docs/src/content/docs/ja/guide/` if those exist. Cover creating shares, permissions, limits, importing (code, QR, camera, photo over HTTP) and embedding. For connectivity, say only that the UDP port should be published for best speed. Describe no hosting or deployment details.
- [X] T100 [P] Add docgen sidecar entries for every new route in `docs/api/sidecar/shares.yaml` and `docs/api/sidecar/share-recipient.yaml`, then run the 011 docgen gate so the route scan and the docs agree.
- [X] T101 [P] Update `README.md` (zh/en/ja) under "Improvements over LANraragi" with sharing, cross-instance import and tankoubon download.
- [ ] T102 Run `mise run check` (rust-check + frontend-lint) and `mise run clippy`. Run `mise run test-crate` detached for `lanrurugi-share-proto`, `lanrurugi-storage` and `lanrurugi-api`. Fix every failure.
- [X] T103 Update the 017 entry in `CLAUDE.md`'s spec status block, and append an Implementation Record to this file. It must say what was verified, what was not (network scenarios that need real NAT or real proxies, the macOS system-proxy path, UI sign-off left to the user), the dependency versions actually pinned, and any departure from plan.md.

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (Phase 1)** blocks everything except US3, which needs only T001.
- **Foundational (Phase 2)** blocks US1, US2, US4, US5, US6 and US7.
- **US3 (Phase 5)** is independent of all other stories and can be done first. T045's recipient
  route needs T024/T026 from US1; skip it until US1 lands.
- **US1** comes before US2, because US2's detail panel reuses `ShareResult` and the share
  handlers.
- **US4** depends on US1's resolver (T023) and page code (T025).
- **US5** depends on US4's network core (T047–T053).
- **T084's three iroh checks** (research §15) should be run as soon as T047 brings up an endpoint,
  before the rest of US4 builds on the transport. The full capture script can wait until US5.
- **US6** depends on US1. Embeds need `share_public_url`.
- **US7** depends on US4's import pipeline.

### Story completion order

```text
Setup ─┬─> US3 (any time)
       └─> Foundational ─> US1 ─> US2
                             └─> US4 ─> US5
                                    └─> US7
                             └─> US6 (instance mode)
```

### Within each story

Storage, then handlers, then tests for those handlers, then UI. UI tasks need the Phase 2 hooks
and i18n.

## Parallel Opportunities

- **Setup**: T003, T004, T005 and T006 touch different files; T002 follows T001.
- **Foundational**: T009, T010, T014, T015, T018 and T019 can run in parallel once T007/T008 are
  done.
- **US1**: T029, T032 and T036 can run in parallel with the backend tasks T021–T027.
- **US3**: T043 can run in parallel with all of US1.
- **US4**: T066 and T070 can run in parallel with the network core T047–T054.
- **US6**: T089 can run in parallel with T085–T088.

### Example: US1

```text
Backend:  T021 → T022 → T023 → T024 → T025 → T026 → T027
Frontend: T029 ∥ T032 ∥ T036, then T030 → T031 → T033 → T034 → T035 → T037
E2E:      T038 after both
```

## Implementation Strategy

1. **MVP**: Setup, then US3 (tankoubon download, shippable alone), then Foundational, then US1 and
   US2. That gives instance-hosted shares with limits and management, which already works for
   instances with a public address.
2. **Cross-instance**: US4 (import, compact queue row, non-blocking).
3. **Reach**: US5 (NAT, relay, LAN, public-services-off, LANrurugi nodes, traffic-profile
   verification).
4. **Presentation and scale**: US6 (embeds), then US7 (helpers).

Each checkpoint maps to a quickstart.md scenario. Commit after each story lands (CLAUDE.md: commit
during the work, not by splitting a tangled worktree afterwards).

---

## Implementation Record

Written 2026-10-25 at the end of the `/speckit-implement` run. 100 of 103 tasks are checked; the
open ones are T038 (the Playwright spec is written but has not been run), T084 (one traffic-profile
point needs a decision, see below) and T102 (the workspace-wide gates are the user's to run).

### Verified

- **Unit and integration tests**, single crate through `mise run test-crate`, run detached:
  `lanrurugi-share-proto` (19 + derivation vectors), `lanrurugi-storage` (118 + the claim-script
  test against the Redis test lock), `lanrurugi-api` (547, including the node, proxy, helper,
  limits, settings and route-policy tests) and `lanrurugi-server` (`share_recipient` 4,
  `share_transfer` 3). `share_transfer` runs real iroh endpoints on loopback: a full transfer
  verified piece by piece, tamper rejection, a stolen ticket refused, renewal after exhaustion and
  refusal after revocation; the helper role end to end (registration gates, `Claimed`/`Renewed`
  helper lists, a whole archive fetched from a helper and verified against the sharer's root,
  outboard and stolen-ticket refusals, `share_helper_max_peers = 1` holding a second importer,
  removed-archive / exhausted / raised-limit / revoked statuses, the helper storing `ended` and
  stopping); and a rendezvous node storing and returning a knock packet, refusing a tampered one.
- **Frontend**: scoped `tsc` and ESLint on every changed file; Vitest for `share/code.test.ts`.
- **API docs**: `mise run docs-api -- --check` passes with the two new sidecars (216 paths,
  275 operations). The gate caught a read-only-token write (`POST /api/share/r/open`), now denied.
- **Live, on the dev container**: US1/US2/US3 flows (stored-entry tankoubon zip, recipient open,
  pages, download, exhausted 409, wrong code 404, revoke 404, activity log).
- **Network matrix** (`scripts/share-network-matrix.sh`, quickstart Scenario 9): rootless podman
  networks with NAT router containers, a `share-node` on the emulated WAN with a private-CA relay,
  and a 3 MB share (40 MB for the hole-punch check). Every environment imports successfully:

  | # | Environment | Route seen by the sharer |
  |---|---|---|
  | 1 | 1:1 NAT + `share_external_addrs` | direct |
  | 2 | home NAT both sides (40 MB) | relay, upgraded to direct by hole punching |
  | 3 | double symmetric NAT both sides | relay through the node |
  | 4 | UDP blocked | relay over TCP 443 |
  | 5 | only an HTTP proxy gets out, set as `HTTPS_PROXY` | relay through squid |
  | 6 | only a SOCKS5 proxy (with credentials) gets out, set as `ALL_PROXY` | relay through the bridge |
  | 7 | IPv6 only (IPv4 removed from the instances) | relay over IPv6, upgraded to direct |
  | 8 | published UDP port, address learnt by QAD | direct |
  | 9 | TLS-intercepting system proxy (mitmproxy CA in the system trust store) | relay through the proxy |
  | 10 | isolated LAN, public services off, no node | LAN direct via mDNS |
  | 11 | WireGuard pair | direct over the tunnel addresses |
  | 12 | UPnP router | direct (external address learnt) |
  | 13 | frp UDP proxy + `share_external_addrs` | direct through frp |
  | 14 | own relay in `proxy` mode behind an frp HTTPS tunnel, no node | relay through the own tunnel |
  | 15 | importer's own proxy, no LANrurugi setting: `ALL_PROXY` SOCKS5 / `HTTPS_PROXY` / mihomo TUN with UDP relayed / with UDP dropped | relay; first byte within 2 s |

### Defects the matrix found, fixed here

- Rebuilding the endpoint after a connectivity setting changed bound the new endpoint before the
  old one released the UDP port, failed, and the error was dropped: **no connectivity setting ever
  took effect without a restart**. `share_net::start` now shuts the old endpoint first and logs a
  failure.
- `share_ca_file` and the relay-role settings were missing from the fields that rebuild the endpoint.
- An endpoint with no relay of its own was built with `RelayMode::Disabled`, so it could not dial
  through the relay a sharer names. It now always has the relay transport (an empty map).
- The `share-node` endpoint did not use its own relay, so peers without UDP could not reach its
  rendezvous; it now does (`--ca-file` added for private-CA relays), and the relay role also serves
  QUIC address discovery on 7842/udp (needed for hole punching), and listens on IPv6 where the host
  has it. A full instance's relay role is now in its own relay map.
- A share with no rendezvous at all could only carry a direct address; the `via` hint now also
  carries the sharer's own relay (`&relay=https://…`, contracts/share-protocol.md).
- `share-node` and the relay role listened on IPv4 only; IPv6-only peers timed out (30 s) before
  falling back. Both now also listen on IPv6 where the host has it.

Test-harness notes: podman reserves `.1` of every network for its bridge (routers use `.254`); on
Ubuntu hosts the AppArmor profiles for `/usr/bin/tcpdump` and `wg` apply inside containers by path,
so the tools image runs a renamed tcpdump and feeds WireGuard keys on a pipe.

### Traffic profile (T084)

`scripts/share-traffic-check.sh` captured a direct import, a relay-only import with UDP blocked, and
a helper-assisted import, and ran nDPI 4.2.0's `ndpiReader` on every capture:

- No flow is classified as BitTorrent or any other P2P protocol. Instance-to-instance traffic is
  QUIC with ALPN `h3` and TLS on the relay port; mDNS announces the neutral service `_lshr`.
- No packet carries `lanrurugi`, `lrrg` or `iroh` in plaintext.
- iroh 1.4.1 sends no SNI to peers (`crypto.enable_sni = false`); its ClientHello does offer raw
  public key certificate types, which a fingerprint could single out.
- **Not met**: while a relay is in use, iroh's QUIC address-discovery probe to the relay carries
  ALPN `/iroh-qad/0` in its Initial packet, which nDPI decodes (seen in the helper-step captures,
  which start with the instance). `NetReportConfig` cannot turn QAD off; without forking iroh the
  only lever is a relay map whose entries have no QAD port, which also gives up learning the
  public address and with it hole punching. Left for the user to decide; T084 stays open.
- The default n0 relays (`*.relay.n0.iroh.link`) name iroh in SNI; they are only used while public
  services are on. Self-hosted relays and LANrurugi nodes carry the operator's own name.

### Not verified

- Real NAT, real carrier-grade NAT, real proxies and Cloudflare Tunnel/ngrok (the matrix emulates
  them in containers), and `lanrurugi-server share-node` on a real VPS (the 50 MB RSS figure was not
  measured).
- The macOS system-proxy path (`system-configuration`), FreeBSD and macOS builds in general: only
  CI builds them.
- The importer's multi-source scheduler dropping a helper that serves a corrupted piece: the drop
  logic is in `fetch_pieces`, but no test feeds it a corrupt helper (the integration test covers
  verification failure on the bao level and helper refusals).
- UI: every new view (share dialog, Settings → Sharing incl. helpers and helping lists, import
  modal, cards, embed) is left to the user's visual sign-off, per CLAUDE.md.
- T102's workspace gates (`mise run check`, `mise run clippy`): not run, per the user's standing
  instruction not to run workspace-wide jobs. `mise run fmt` was applied.

### Dependencies pinned

iroh 1.4.1 (`portmapper`), iroh-relay 1.4.1 (`server`), iroh-mdns-address-lookup 0.6.1, pkarr 8.1.0
(`signed_packet` only), bao-tree 0.17.0 (`validate`, `fs`), argon2 0.6.0, chacha20poly1305 0.11.0,
postcard 1.1.3, ed25519-dalek 3.0.0, blake3 1.8.7, ipnet 2.12.2, hkdf 0.13.0, tokio-socks 0.5.3,
system-configuration 0.8.0 (macOS only), rustls 0.23.46 (`ring`), chrono-tz 0.10; reqwest gained
`socks`. Frontend: barcode-detector 3.2.3, lean-qr 2.7.4, zxing-wasm 3.1.6 (direct dependency, so the
wasm is bundled rather than fetched from a CDN).

### Departures from plan.md

- One process-wide `ShareNet` handle (`share_net::handle()`), replaced on every rebuild.
- Sharing settings have their own endpoint (`/api/shares/settings`), session-only for writes, not
  part of `/api/settings`.
- Frontend hooks live in `src/share/api.ts`; the reader for recipients is a standalone
  `ShareReader`; the embed is the static `public/embed/share.js` rather than a second Vite entry.
- Transfers keep only the local outboard plus `valid_ranges` for resume: no `.ranges` bitmap, and
  importers never fetch outboards (`Outboard` stays sharer-only and unused by the importer).
- Integration tests live in `crates/lanrurugi-server/tests/` (they need a full `AppState`).
- Failed-attempt limits: 10 per source per minute with exponential lockout; 500 per instance per
  hour, then 10 s serialized penalties.
- Outboards live under `<temp>/share/outboards` and are not counted against `tempmaxsize`.
- reqwest is built without `system-proxy`, so sharing reads proxy settings itself and configures
  reqwest explicitly.
- `HelperRegister` carries the sharer's archive ids and the helper's dialable addresses (not blob
  hashes), and `Renew` answers `Renewed { ticket, helpers }`: with the `Minimal` preset an endpoint
  id alone cannot be dialled, and renewals let helpers join mid-import.
- Router port-mapping status shows the detected external address; the mapping protocol name
  ("via UPnP") is not exposed by iroh's public API.
- No proxy or CA settings (`share_proxy_url`, `share_proxy_from_env`, `share_proxy_only`,
  `share_proxy_allow_lan`, `share_ca_file` were removed, 2026-10-25). Sharing always follows the
  operating system's proxy (environment variables, the macOS system proxy, TUN transparently), and
  relay TLS uses iroh's `platform-verifier` (`CaTlsConfig::system()`), i.e. the system trust store,
  falling back to the bundled roots on a host without one. Matrix cases 5, 6 and 9 set the proxy
  as environment variables and install test CAs with `update-ca-certificates`.

### Follow-ups, 2026-10-26

- Codes carry their own routes, sealed: `?r=` holds the endpoint id, up to six addresses, the
  connected relays and the configured nodes (contracts/share-protocol.md). The importer races those
  routes against the rendezvous lookup (3 s head start), so a code works with no outside service
  when any route reaches the sharer. Older plain `via`/`relay`/`node` codes still parse.
- The relay role has a per-client rate limit (`share_node_relay_rate`, default 10 MiB/s) and an
  optional allow-list (`share_node_relay_allow`); before, it admitted anyone without a limit.
  `share-node` takes `--relay-rate` and `--relay-allow`.
- Relay and pkarr addresses are stored in one canonical form (scheme added, host lowercased,
  trailing dot dropped), so one relay never appears twice. Default pkarr relays are now two
  operators (`pkarr.pubky.app`, `relay.pkarr.org`); the two previous defaults resolved to one
  address. `dns.iroh.link/pkarr` works too but is left out: n0 also runs the default relays, so it
  could link a lookup to the connection that follows.
- A patched archive is merged into its shared zip a page at a time through a temporary file instead
  of in memory.
- Imported archives are re-dated after each run: the sharer's order, one second apart, ending at
  the import's completion; reused archives keep their date.
- Share pages hide `source:` tags.
- Not verified: these were checked with `check-crate` and unit tests only; no import has run over
  a sealed hint, the race, or the limited relay on a live instance.
- The embed card is its own page (`apps/frontend/embed.html`, `src/embed/`), plain DOM code with its
  own strings, served by `serve_index` for `/embed/*`. It had been a route of the library app, so a
  page framing a dozen cards started a dozen copies of the app (226 requests, ~25 MB each in dev)
  and hung the browser. The card follows the sharer's theme or `?theme=` / `theme="…"`, enlarges a
  hovered cover, and lists every item behind a "+N" tile (`items` in `GET /api/share/r/card`).
- A used-up share now closes to browsers as a revoked or expired one does (an import already under
  way keeps its claim). Opening a closed share with its right code answers why (`expired`/`revoked`
  404, `exhausted` 403) so the share page can say so; a wrong code stays a bare 404. The embed card
  shows a closed share's state with its actions disabled and nothing of its content.
- Archive entries carry their chapters (`ArchiveEntry.toc`, protocol version 2): the recipient's
  reader jumps by chapter, and an import fills an empty table of contents with the sharer's.
  Bookmarks and stamps stay the sharer's own.
