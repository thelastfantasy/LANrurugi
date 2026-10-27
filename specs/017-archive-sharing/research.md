# Research: Archive & Tankoubon Sharing Across Instances

All versions below were checked against crates.io / npm on 2026-10-24.

## 1. Transport between instances: iroh

**Decision**: use `iroh` 1.4 (MIT/Apache-2.0, n0-computer) as the only instance-to-instance
transport. Each instance gets one iroh endpoint with a persistent ed25519 identity, and speaks one
application protocol over it (ALPN `h3`, see §15 and `contracts/share-protocol.md`).

**Discovery is limited to what the sharer itself provides (FR-047)**: n0's DNS address discovery is
**not** used.

- An importer dials the sharer by `EndpointAddr { endpoint_id, relay_urls }`, taken from the knock
  record (§2).
- Direct addresses are then exchanged by the two endpoints themselves, end to end encrypted, while
  iroh upgrades the connection to a direct path. The relay forwards only ciphertext.
- On the LAN, the sharer's own mDNS announcements supply its local address.

**Rationale**: iroh covers FR-023–FR-026 with one dependency:

- endpoints are dialled by public key, not by address, so a changing address never invalidates
  anything (FR-010);
- it hole-punches through NAT, and falls back to relay servers only when that fails (FR-023);
- connections are QUIC with TLS end to end, so relays carry ciphertext only (FR-026);
- the default relays are public n0 servers, and `iroh-relay` is the same code for self-hosting,
  including embedded in a LANrurugi instance (§7, FR-048);
- local-network discovery comes from `iroh-mdns-address-lookup` 0.6, published by n0. Once a peer
  is known, iroh picks the lowest-latency path, so LAN is preferred automatically (FR-023, FR-024);
- the `portmapper` feature opens a UPnP/NAT-PMP mapping where the router allows it, improving the
  direct-route rate (SC-006).

**Operational note**: iroh binds one UDP port inside the container (`share_udp_port`, default
`11204`). The docs recommend publishing it on the host as `443/udp` (`-p 443:11204/udp`) when
nothing else on the host uses HTTP/3, so direct traffic sits where HTTP/3 normally does. Any host
port works; iroh learns the external port by itself. Without a published port, hole punching and
relay still work, at lower throughput.

**Alternatives considered**:

- **rust-libp2p** (QUIC, DCUtR, circuit-relay v2, mDNS, Kademlia): equivalent capability, but
  many more pieces to assemble and tune. Its public relays are IPFS-oriented and rate-limited.
- **WebRTC DataChannels** (`webrtc-rs`) with STUN/TURN: needs a signalling service and a TURN
  server that we would have to run. TURN carries the full traffic, so a default can't be offered
  for free.
- **Plain HTTP between instances**: fails the core requirement for instances without a public
  address.

## 2. The knock record: the only thing outside services hold (FR-010, FR-047)

**Decision**: the share code is 50 random bits (40 for an opt-in short code, see §3). The record
published for it is derived from the code itself:

1. `seed = Argon2id(code, salt = "lrrg-share-lookup-v1", m = 64 MiB, t = 3, p = 1)`, 32 bytes.
2. From `seed`, HKDF-SHA256 derives:
   - an ed25519 **lookup key** (`info = "lookup"`);
   - a XChaCha20-Poly1305 **record key** (`info = "record"`).
3. The sharer publishes a `pkarr` 8.x signed packet under the lookup key, to **every configured
   rendezvous**: pkarr HTTPS relays, which store it on the Mainline DHT for us, and LANrurugi
   rendezvous nodes (§7).
   - The instance itself never speaks the DHT protocol (§15).
   - The packet is one TXT entry, encrypted with the record key, holding **only** the
     `KnockV1 { endpoint_id, relay_urls }`. There is no share id, no address and no public URL.
4. Anyone holding the code derives the same keys, queries all configured rendezvous in parallel,
   and takes the first valid packet. The packet is signed by the lookup key, so a rendezvous cannot
   forge or alter it.
5. The importer dials `EndpointAddr { endpoint_id, relay_urls }`. Everything else comes from the
   sharer over that connection:
   - the share id (the importer proves the code with `access_hash`);
   - direct addresses;
   - the public URL;
   - the manifest, fingerprints, tickets and helper list.

**Republishing**: hourly while a share is active, and immediately when the sharer's relay set
changes. Address and domain changes need no republish, because none are in the record.

**Rationale**:

- What a third party holds is minimal and unreadable without the code. Recovering the code takes
  up to 2⁵⁰ memory-hard Argon2 evaluations (≈ 7 × 10⁵ core-years; still thousands of core-years
  for a 40-bit short code, which also expires within 7 days).
- Since the record never contains an address, codes survive any address or domain change
  (FR-010a).

**Alternatives considered**:

- **A project-run rendezvous**: excluded by FR-047.
- **Putting the `EndpointId` in the code**: not short (FR-008).
- **n0 DNS discovery**: publishes addresses in readable form, keyed by `EndpointId`, on a third
  party. Excluded by FR-047.

## 3. Code format, text form and guessing resistance (FR-008, FR-008a, FR-011, SC-010)

**Decision**:

- **Default code**: 10 Crockford-base32 characters (50 bits), shown as `K7QM9-XRT2H`.
- **Short code**: 8 characters (40 bits), shown as `K7QM-9XRT`. It is opt-in per share, and
  allowed only with `expires_at` no more than 7 days out.
- **No check character**: it would cost one character. Typos surface as a failed lookup instead.
- **Parsing**: case-insensitive. `O`→`0`, `I`/`L`→`1`, and dashes and spaces are ignored. The
  length tells a default code from a short code.
- **Text form**, magnet-style: `lrrg:<code>[?via=<endpoint_id>@<host:port>]`.
  - `via` is a direct-dial hint supplied by the sharer itself in the text it hands out. It is added
    only when public services are off, since otherwise the knock record lets the importer reach
    the sharer.
  - Unknown parameters are ignored, for forward compatibility.
- **Accepted input forms**: the parser accepts `lrrg:` forms, share links (`…/s/<code>`,
  `…/#<code>`) and bare codes.
- **Storage**: the sharer stores `SHA-256(code)` for lookups, plus `code_enc` for re-display. The
  plaintext code is never stored.

**Attempt limits**:

- per source, 10 failures per minute, then exponential back-off;
- per instance, 500 failures per hour. Beyond that, failed lookups are answered one at a time,
  each held for 10 s, so parallel connections cannot get around the delay. Valid codes are not
  queued, since they succeed immediately.

The worst case is therefore 500 + 360 = 860 guesses an hour, ≈ 7.5 × 10⁶ a year.

| Code | Space | 100 active shares | 1,000 active shares |
|---|---|---|---|
| 10 chars | 2⁵⁰ | ≈ 6.7 × 10⁻⁷ | ≈ 6.7 × 10⁻⁶ |
| 8 chars, expires ≤ 7 d | 2⁴⁰ | ≈ 1.3 × 10⁻⁵ (one week of exposure) | — |

SC-010 is stated for 100 active shares, which is generous for a home library.

**Why a prefix but QR codes hold links**:

- `lrrg:` makes a pasted code recognisable, and leaves room for hints, the way `magnet:` carries
  trackers.
- Phone camera apps and chat clients cannot open custom schemes, and `navigator.registerProtocolHandler`
  only accepts `web+…` schemes.
- So QR codes and clickable links carry https links (§7), while `lrrg:` is the form for plain
  text.

**Alternatives considered**:

- 13 characters with a check character (60 bits): safer than needed, and noticeably longer.
- A mandatory `web+lrrg:` scheme: longer than a link, and still not openable by phone cameras.

## 4. IP restrictions when the recipient's address is not observable (FR-028)

**Finding**: over a relayed iroh path the sharer does not see the recipient's address. n0 relays
do not forward it, and a recipient-reported address is not trustworthy.

**Decision**: never evaluate an IP restriction against a relay's address.

- For a share **with** an IP restriction, a P2P claim waits up to 15 s for iroh to upgrade the
  connection to a direct path. It is then evaluated against the observed address. If no direct
  path appears, the claim is refused with reason `address_unverifiable`.
- Shares **without** an IP restriction work on every route.

The spec's FR-028 is amended to say this.

**Alternatives considered**: trusting a self-reported address was rejected, because it makes the
restriction meaningless.

## 5. Verified, resumable, multi-source transfer: `bao-tree`

**Decision**:

- Each shared archive is identified by the BLAKE3 hash of the bytes a download would deliver (the
  file, or the merged zip when a `.patch.zip` sidecar exists, matching `download_archive`).
- Its BLAKE3 outboard is computed with `bao-tree` 0.17, using 16 KiB chunk groups (≈0.4% size
  overhead), and cached on disk.
- Transfers request chunk ranges and verify every range against the root hash as it streams
  (FR-022, FR-034).
- The importer keeps two files next to the partial archive: the outboard and a verified-range
  bitmap. Resume therefore never re-transfers verified data (SC-008).
- With several sources, the importer splits the file into 1 MiB pieces and keeps up to 4 requests
  in flight per source. A source that sends a failing range is dropped for that transfer (FR-034).
- **Fingerprints come only from the sharer** (FR-047). The root hashes arrive in the claim
  manifest, and the outboard (the piece hash tree) is fetched with `Outboard` from the sharer only.
  Helpers serve `Range` data only. Its bao encoding is checked against the sharer's outboard and
  root, so a helper cannot inject fingerprints.

**Alternatives considered**:

- **`iroh-blobs` 0.103**: provides all of this, but its `fs-store` keeps its own `redb` database and
  wants to own blob storage. This project keeps state in Redis and archives as plain files, so
  using the lower-level `bao-tree` directly avoids a second database.
- **SHA-256 over whole files**: cannot verify partial ranges.

**Outboard cache**: `<data_dir>/share/outboards/<blake3>.obao`. A Redis index maps
`archive_id → {blake3, size, mtime, path}`, so a changed file is detected and rehashed. Hashing
runs through `lanrurugi_core::concurrency::parallel_map` with the ~30% core cap from CLAUDE.md.
Merged-patch archives are materialised under `<temp>/share/` and count against `tempmaxsize`.

## 6. Who may serve: tickets signed by the sharer (FR-035, SC-009)

**Decision**: after a successful claim, the sharer issues a **ticket**, signed with its iroh
secret key, containing:

- `share_id`;
- `claim_id`;
- the importer's `EndpointId`;
- the share's root hashes;
- `expires_at = now + 25 s`.

The importer renews the ticket every 10 s while transferring. Helpers verify the signature
against the sharer's `EndpointId`, which they hold from their own import, and serve only to the
`EndpointId` named in the ticket.

When the share is revoked or expires, loses its import permission, or the importer's current
address is no longer allowed, renewal fails, and every source stops within 25 s. This meets
SC-009's 30 s. Exhaustion only refuses new claims: an existing claim keeps renewing (FR-015).

**Helper discovery**: an instance that opts in to helping registers with the sharer over the same
protocol. Claim responses include the current helper list. No content index is published anywhere
else.

## 7. No web gateway; LANrurugi nodes instead of third-party servers (FR-025, FR-047–FR-049)

**No public web gateway.** A gateway (an earlier draft had `share.lrrg.dev`) would hand out
connection information to browsers, which FR-047 excludes.

- Browser recipients open a share only on the sharer's own instance: `<share_public_url>/s/<code>`,
  or its LAN address.
- When the sharer has no public address, the share page and dialog offer the `lrrg:` text form
  and QR code for importing instead (FR-009, FR-010a).

**Links and domain changes**:

- Links name the sharer's own address, so a link on an old domain stops working. The dialog says
  so next to the link.
- Codes and `lrrg:` forms keep working across any change, because the knock record holds no
  address (§2).

**LANrurugi rendezvous and relay nodes (FR-048)**: an instance with a public address can enable
two roles.

- **Rendezvous role**: answers `RendezvousPut` and `RendezvousGet` on the share protocol
  (contracts/share-protocol.md).
  - It stores signed knock packets in `LANRURUGI_SHARE_RDV:<lookup_pubkey>` with a 2 h TTL, after
    verifying each packet's signature.
  - It is rate-limited per source.
  - It never speaks the DHT. Its traffic is the same `h3`-ALPN QUIC as all other sharing traffic,
    so no plaintext HTTP path is exposed (§15).
- **Relay role**: embeds `iroh-relay`'s server on its own TCP port (`share_node_relay_port`,
  default 11205), with TLS from a configured certificate pair or ACME for `share_public_url`'s
  host. In those two modes the port is published as host `443/tcp`, which ACME's TLS-ALPN
  challenge requires. Relay traffic is then ordinary TLS to the friend's own domain. The `proxy`
  mode for tunnels is in §16.

**Node list**: other instances add a node as an `lrrg-node:<endpoint_id>@<host:port>` string. The
node's settings page shows it, ready to copy.

- Configured nodes are used **alongside or instead of** public services, as the administrator
  chooses (FR-027).
- With public services off, configured nodes are still used, so a circle of friends with one
  reachable instance needs no third party at all.

**Redundancy and visibility (FR-049)**:

- Knock packets are published to every configured rendezvous.
- Lookups query them all in parallel and take the first valid answer.
- iroh is given every configured relay.
- Settings → Sharing lists every outside service in use, each with a reachability dot and the last
  success time.

**Defaults** (FR-027): public services on, which means pkarr's default HTTPS relays and n0's
public iroh relays, with no configured nodes. One switch turns public services off.

## 8. Turning public services off (FR-027)

**Decision**: the setting `share_public_services = 0` switches off the **public** services:

- n0's iroh relays;
- the pkarr relays.

What still works:

- configured LANrurugi nodes (§7);
- iroh direct connections;
- mDNS local discovery;
- links that carry `share_public_url`;
- `lrrg:<code>?via=<endpoint_id@host:port>` text forms. The `via` hint is supplied by the sharer
  inside the text it hands out. The share page shows "requires a direct route" when no node is
  configured.

To resolve a **bare** code in this mode, the importer asks each endpoint found by mDNS directly
(`ResolveLocal{lookup_hash}`), which handles same-LAN imports.

**Configurable values**:

- `share_nodes`: `lrrg-node:` strings for LANrurugi rendezvous and relay nodes;
- `share_relay_urls`: extra iroh relay URLs. When public services are on, n0's public relays are
  added;
- `share_pkarr_relays`: pkarr HTTPS relays. When public services are on and this is empty, pkarr's
  default public relays are used.

## 9. Recipient web access on the instance (FR-012–FR-015, FR-013a/b)

**Decision**:

- **Recipient endpoints**: new public routes under `/api/share/r/*`, allowed for `anonymous`,
  `guest_visitor` and `token_guest` in `route_policy.csv`. The handlers authenticate by share
  credential only, and never consult or create a library session (FR-013a).
- **Credential**:
  1. The SPA route `/s/:code` posts the code once to `POST /api/share/r/open`.
  2. On success it receives a **recipient token**: 32 random bytes, stored as
     `LANRURUGI_SHARE_RTOKEN:<sha256>` with a 12 h sliding TTL, scoped to that `share_id`.
  3. Every later request carries the token, as a header or, for `<img>`/download URLs, as
     `?rt=`.
  
  Token lookup also re-checks share status, permissions and IP, so revocation and permission
  changes apply on the next request.
- **Admin endpoints**: `/api/shares*`, allowed only for `session`/`token_admin`. A
  `token_guest` deny line covers its blanket GET allow (FR-007).
- **Referrer**: the `/s/*`, `/embed/*` and `/api/share/r/*` responses carry
  `Referrer-Policy: no-referrer`, and the SPA also sets the `<meta>` (FR-013b).
- **Claims**: one Redis Lua script performs the check-and-create atomically:
  - status, expiry and permission;
  - the IP allow-list and first-N set;
  - `used < limit`;
  - creating the claim and incrementing `used`.
  
  Two simultaneous last claims therefore yield exactly one success (FR-015).

## 10. Tankoubon download (US3, FR-036–FR-040)

**Decision**: the existing `zip` 8.6 workspace dependency's streaming writer
(`ZipWriter::new_stream`), driven in `spawn_blocking`.

- Each member is written as **Stored** (no re-compression) with ZIP64 enabled.
- The writer writes into a `SyncIoBridge` over a `tokio::io::duplex`, and the read half becomes
  the axum response body. Bytes leave as they are produced, and no temporary file exists
  (FR-038).
- Members are named `NN - <title>.<ext>`, zero-padded to the member count's width. A
  `-2`/`-3` suffix is added on name collisions.
- A missing member is skipped, and a `MISSING.txt` entry listing skipped members is written last
  (FR-039).
- Each member's bytes are what `download_archive` serves, including patch merging.

**Alternatives considered**: `async_zip` 0.0.19 is pre-1.0, and its ZIP64 streaming support is
less certain. A hand-written stored-zip writer is needless given `zip` is already a dependency.

## 11. Frontend libraries

- **QR decoding**: `barcode-detector` 3.2 (MIT). It uses the native `BarcodeDetector` when present
  and lazy-loads `zxing-wasm` otherwise. It is imported dynamically by the import modal only.
- **QR generation**: `lean-qr` 2.7 (MIT, ~69 KB unpacked, renders to canvas/SVG, used for the
  saved image). `uqr` was considered (smaller, but 0.1.x).
- **Camera**: `navigator.mediaDevices.getUserMedia({ video: { facingMode: "environment" } })` is
  used only when `window.isSecureContext`. Frames go to `BarcodeDetector.detect(video)`, and the
  stream's tracks are stopped on a hit or when the modal closes.
  - Fallback: `<input type="file" accept="image/*" capture="environment">`, which opens the
    device camera app over plain HTTP.

## 12. Embeds (US6, FR-030–FR-032)

**Decision**:

- **Availability**: only when the sharer has `share_public_url`. The card is served by the
  sharer's own instance (FR-030, FR-047).
- **Iframe**: `<share_public_url>/embed/s/<code>`.
  It renders a self-contained card page from the share viewer bundle.
- **Web Component**: `<lrrg-share code="…" src="…"></lrrg-share>`, loaded from
  `/embed/share.js`. It renders the same card inside a shadow root. The `src` attribute (the sharer's public URL) is required.
- **Card layout by permissions** (FR-030a):
  - `read` adds "Read";
  - `download` adds "Download";
  - `import` adds the code, its QR code and "Copy code".
  
  An import-only share renders the code as the primary element.
- **Theme**: the card follows `prefers-color-scheme`. Only the embed route answers with
  `Content-Security-Policy: frame-ancestors *`; every other page keeps its existing framing
  policy.
- The card's own colours live in the embed bundle's CSS, not in the five legacy theme files. The
  card is shown on foreign pages where no LANrurugi theme applies, so the theme-file rule in
  CLAUDE.md (for in-app UI) does not apply.

## 13. Not blocking the instance or the queue (FR-019a, SC-014)

**Runtime model**: everything long-running is a Tokio task or bounded blocking work.

- The iroh endpoint and protocol handlers are async. Each request is its own stream and its own
  task.
- The pkarr republisher and helper heartbeats are interval tasks.
- File reads for ranges use async file I/O. BLAKE3 hashing and outboard encoding run in
  `spawn_blocking` or `parallel_map` with the ~30% core cap.
- The tankoubon zip writer runs in `spawn_blocking`, with back-pressure from the `duplex` buffer
  (256 KiB), so a slow client never makes the server buffer a whole tankoubon.

**Receiving side**:

- An import is one queue item, and its archives transfer through the import's own semaphore
  (default 3).
- It does not touch the per-plugin `DownloadManager` job pools that other downloads wait on.
- Ingest reuses `catalogue_staged_file`, which already runs scanning and thumbnailing off the
  reactor, one archive at a time per import slot.

**Sharing side**: fingerprinting is the only expensive step, and is handled in three ways:

- Creating a share, or adding items to one, enqueues its archives in `LANRURUGI_SHARE_PREPARE`.
  One background job drains that queue with `LoadThrottle` backoff, like the other background
  jobs. It is visible in the Jobs console.
- A claim's manifest lists `blake3` for archives that are already prepared. Unprepared archives
  are marked `pending`.
- The importer sends `Prepare{archive_id}` for the archive it wants next. That moves the archive to
  the front of the queue and answers once it is fingerprinted. Transfer of prepared archives
  starts immediately, so a first import of a large share never waits for the whole share to be
  hashed.

**Alternatives considered**:

- Hashing on first claim: blocks the first importer for minutes on a large share.
- Hashing at share creation in the request: makes the share dialog slow.

## 14. Upload limits and the alternative-speed schedule (FR-041–FR-044)

**Decision**: one `ShareUploadLimiter` in `share_net`, holding two pairs of caps:

| Group | Rate cap | Concurrency cap | Applies to |
|---|---|---|---|
| Own shares | token bucket | semaphore | P2P `Range` and `Outboard` for this instance's own shares, plus HTTP recipient file and tankoubon downloads |
| Helping | token bucket | semaphore | P2P `Range` served as a helper (helpers never serve `Outboard`) |

**Limiter mechanics**:

- Token buckets reuse the `governor`-based limiter in `crates/lanrurugi-api/src/download_manager/rate_limit.rs`.
- A **mode** (normal or alternative) selects which configured values the buckets and semaphores
  use. Switching mode rebuilds them; transfers already holding a permit finish their current
  piece.
- A minute ticker evaluates the schedule, and the manual switch overrides it.
- Reading (`Page`, `Thumbnail`) and HTTP page requests are never limited (FR-043).

**Paused own-share uploads**: when the alternative own-share rate is 0, `Claim`, `Prepare` and
`Range` for own shares answer `Refused { reason: paused, until }`, where `until` is the end of the
current window, or absent for a manual pause.

- The importer marks its queue row "sharer paused uploads until …".
- It holds its verified ranges, and retries at `until`. For a manual pause it retries every
  5 minutes.
- Already-issued tickets stay valid, so helpers keep serving within their own caps (FR-044).
- HTTP recipient downloads get `503` with `Retry-After`.

**Rationale**: this is qBittorrent's model — alternative limits plus a scheduler plus a manual
toggle — which users of BT clients already understand. Keeping the two groups separate lets an
administrator stay generous to their own recipients while capping charity traffic, or the reverse.

## 15. Traffic profile: no protocol signatures (FR-045, FR-046, SC-016)

**Goal**: nothing a DPI classifier can match by content. It is measured, not assumed: a capture of
the whole lifecycle is fed to **nDPI** (the open-source classifier behind ntopng) in
`scripts/share-traffic-check.sh`.

**Decisions**:

| Traffic | Before | Now |
|---|---|---|
| Record publish/resolve | direct Mainline DHT, which is classified as BitTorrent | HTTPS to pkarr relays only (§2). No DHT from the instance, ever |
| Instance-to-instance | QUIC with ALPN `lrrg/share/1`, which is readable in the Initial packet | QUIC with ALPN `h3`. The application protocol is identified inside the encrypted stream by its first message, which carries the version |
| Direct port | `11204/udp` | still `11204` in the container; publishing it as host `443/udp` is recommended |
| Relay | HTTPS/WebSocket to the configured relay | unchanged: TLS on TCP 443, which also covers UDP-blocked networks (FR-046) |
| Address discovery | n0 DNS over HTTPS | removed. Addresses come from the sharer over the connection (§1, FR-047) |
| Rendezvous and relay on LANrurugi nodes | — | the same `h3` QUIC, and TLS on the friend's own domain (§7) |
| Local discovery | mDNS on the LAN | unchanged: link-local, never leaves the network |

**To verify first (T084), because they depend on iroh internals**:

1. **ClientHello**: whether iroh's TLS ClientHello carries distinctive fields, such as raw public
   key certificate types or a recognisable SNI.
2. **Path packets**: whether its path-validation and hole-punching packets are plain QUIC, or
   carry a recognisable magic. iroh 1.x does NAT traversal inside QUIC (`noq`), which suggests
   plain QUIC.
3. **Relay hostnames**: whether the default relay hostnames name iroh. They are visible as SNI,
   as with any HTTPS.

**Handling each one if it does not hold**:

1. **ClientHello**: configure iroh's TLS to present a standard certificate type and a neutral SNI,
   if its API allows that. Otherwise carry a small patch, with the user told before any fork.
2. **Path packets**: if a classifier can match them, report it before shipping and do not claim
   FR-045.
3. **Relay hostnames**: a hostname is not a protocol signature, but it reveals "iroh". Document
   self-hosting a relay under one's own name (`share_relay_urls`), and report what the default
   reveals.

**Out of reach, stated in the docs**: flow analysis (who talks to whom, when and how much), the
simultaneous-send pattern of hole punching, and the server names of outside services.

## 16. Network environments, proxies and VPS nodes (FR-050–FR-054, SC-018)

Checked against `iroh` 1.4.1's `endpoint::Builder` on docs.rs. It provides:

- `proxy_url` / `proxy_from_env`: an HTTP(S) proxy for all of iroh's HTTP(S) traffic, which is
  relay connections and net reports;
- `add_external_addr`;
- `clear_ip_transports`, which turns off direct UDP and leaves relay only;
- `ca_tls_config`, which sets trusted roots for non-iroh TLS;
- `relay_mode` and `portmapper_config`.

| # | Environment | How it works | Notes |
|---|---|---|---|
| 1 | Public IP / VPS | direct QUIC on the published UDP port | cloud 1:1 NAT: `share_external_addrs` → `add_external_addr` (FR-053). The VPS firewall must allow the UDP port |
| 2 | Home NAT | hole punching; UPnP/NAT-PMP mapping via `portmapper` | |
| 3 | CGNAT / symmetric NAT | relay; hole punching may still succeed one-sided | a LANrurugi node or self-run relay removes the third party |
| 4 | UDP blocked | relay over TLS on TCP 443 | FR-046 |
| 5 | HTTP/HTTPS proxy only | `proxy_url` for relays; `reqwest` proxy for pkarr relays; `clear_ip_transports` in proxy-only mode | relays must be reachable through the proxy (CONNECT to 443) |
| 6 | SOCKS5 proxy only | `reqwest` `socks5h://` for pkarr relays. For iroh, an in-process HTTP CONNECT → SOCKS5 bridge on `127.0.0.1:<random>` (`tokio-socks`), handed to `proxy_url` | SOCKS5 UDP ASSOCIATE is rarely available, so UDP is not attempted through SOCKS; proxy-only mode applies |
| 7 | IPv6-only / dual-stack | iroh binds both stacks; relays are dual-stack | NAT64 networks are covered when the relay has an AAAA record |
| 8 | Docker bridge / host | bridge: publish the UDP port (it is NAT, like case 2); host: like case 1 | |
| 9 | TLS-intercepting proxy | `share_ca_file` → `ca_tls_config` plus `reqwest` root store (FR-052) | iroh's own peer TLS is key-based and unaffected |
| 10 | Isolated LAN | mDNS plus direct; `lrrg:` with `via` or a LAN link | no outside service needed |
| 11 | VPN (Tailscale, WireGuard) | iroh uses every interface, so VPN addresses are tried directly | `share_external_addrs` can add the VPN address explicitly |
| 12 | Router NAT traversal: UPnP / NAT-PMP / PCP, or a manually forwarded port | `portmapper` requests a mapping through all three protocols (`share_portmap`, on by default). With a manual forward, iroh's address discovery learns the external `ip:port` by itself, or it can be set in `share_external_addrs` | the settings show the mapping state and external address (FR-055) |
| 13 | UDP-capable tunnel (frp `udp`, ZeroTier, WireGuard to a VPS) | forward the iroh UDP port, and put the tunnel's public `ip:port` in `share_external_addrs` | behaves like a direct route |
| 14 | HTTP/TCP-only tunnel (Cloudflare Tunnel, ngrok, frp `http`/`tcp`) | enable the relay role in **behind-proxy** mode (`share_node_relay_tls = proxy`: plain HTTP on a local port, with TLS terminated by the tunnel or reverse proxy), and set `share_self_relay_url` to the tunnel's public `https://` address. The endpoint adds that relay to its relay map and to its knock record | peers reach the instance over TCP through the user's own tunnel; no public relay is involved (FR-055) |
| 15 | The user's own proxy: env vars (HTTP / SOCKS5), TUN or transparent (fake-IP; UDP relayed or dropped), router-level | followed automatically: `proxy_from_env` + SOCKS bridge + reqwest env proxies; relay-path fallback for dropped UDP (see below) | no LANrurugi setting needed; the sharer sees the proxy exit address (FR-051a) |

**Following the user's own proxy (FR-051), the default with no settings**:

- **iroh**: does not read proxy variables unless told to, so the endpoint is always built with
  `proxy_from_env()`. If only `ALL_PROXY` with a `socks5(h)://` URL is set, the in-process
  CONNECT → SOCKS5 bridge is started and passed as `proxy_url`.
- **`NO_PROXY`**: honoured for relay hosts. LAN, link-local and mDNS traffic never goes to a
  proxy, because iroh proxies only its HTTP(S) relay connections, and direct UDP is never proxied.
- **reqwest clients** (pkarr relays): the workspace builds reqwest without its `system-proxy`
  feature, so it does not read proxy variables by itself. Sharing's client is given the same proxy
  chosen for iroh explicitly, with `NO_PROXY` applied; reqwest's `socks` feature carries
  `socks5(h)://`. Other features' clients are unaffected.
- **macOS native package**: the system proxy (System Settings → Network) is not visible as
  environment variables. The native build reads it through the `system-configuration` APIs, and
  uses it for both iroh and the pkarr client.
  FreeBSD and Linux packages rely on environment variables, as their desktop and session tools
  set them.
- **TUN, transparent and router proxies** (Clash/mihomo TUN, sing-box, OpenClash, Surge enhanced
  mode) need nothing from us beyond fallback:
  - relay hostnames resolve through the proxy's DNS, including fake-IP, and are proxied as
    ordinary TLS;
  - direct UDP to peers is either relayed by the proxy line, dropped, or blocked by the line's
    rules. In the last two cases iroh's path selection keeps the relay path. Hole punching through
    a proxy usually fails, because the peer sees the proxy exit, and that is expected;
  - most proxy rule sets send private ranges direct, so LAN imports stay direct.
- **Fallback timing**: iroh starts on the relay path and upgrades to direct when it can, so a
  dropped UDP path never blocks the start of a transfer. SC-018's 30-second start bound is checked
  in the matrix.
- **Address seen by the sharer**: a proxied peer's observed address is the proxy exit, and
  IP-based limits use it (FR-051a).

**Optional override settings**:

- `share_proxy_url` accepts `http://`, `https://`, `socks5://` and `socks5h://`, with optional
  `user:pass@`.
- `share_proxy_from_env` reads `HTTPS_PROXY`, `HTTP_PROXY` and `ALL_PROXY`.
- `share_proxy_only` makes the endpoint relay-only (`clear_ip_transports`), and makes every pkarr,
  rendezvous and relay connection go through the proxy. mDNS and LAN direct stay allowed, since
  they never leave the network; turning off "allow LAN direct" makes the instance strictly
  proxy-only.
- **Node addresses under a proxy**: a LANrurugi node is dialled by its `EndpointId` through the
  relay it offers, so rendezvous works through TCP proxies too.

**Tunnel notes**:

- The relay protocol is an HTTP upgrade to WebSocket, which Cloudflare Tunnel, ngrok and frp
  `http` all forward. An frp `tcp` proxy needs the tunnel to terminate TLS (`tls_enable`), or the
  relay to use its own certificate on the forwarded port (`share_node_relay_tls = acme|files`).
- The local plain-HTTP listener binds to `127.0.0.1` or the Docker network only, never a public
  interface.
- Traffic between the instance and the tunnel provider is the provider's own encrypted protocol.
  Peers see ordinary TLS to the tunnel's domain (§15).

**Node-only mode (FR-054)**: `lanrurugi-server share-node --data <dir>` runs only the iroh endpoint
with the rendezvous and relay roles.

- Rendezvous packets are kept in memory with a 2 h TTL, persisted best-effort to a file under
  `<dir>` on shutdown.
- The node key is stored in `<dir>`.
- It needs no Redis, no library and no frontend. It prints its `lrrg-node:` string at start-up.
- It ships in the same binary and image, and runs under 50 MB of RSS.

**Verification**: quickstart Scenario 9 runs each environment, with network namespaces and
containers for cases 3–9. It records the route used, and feeds the same captures to the
traffic-profile check (§15).

## 17. Settings and secrets

- **Iroh secret key**: generated on first start and stored in Redis
  (`LANRURUGI_SHARE_NODE_SECRET`). It is server-side only (Principle V) and is included in
  backups, so the `EndpointId` survives restores.
- **Sharing settings**: stored in `LRR_CONFIG` fields with a `share_` prefix, edited in the new
  Settings → Sharing section (FR-006a). See `data-model.md`.
