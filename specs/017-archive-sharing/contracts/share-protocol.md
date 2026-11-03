# P2P Protocol Contract: share protocol v1

This is the protocol between a sharing instance, importing instances, helpers and LANrurugi
rendezvous nodes. It runs over iroh QUIC connections (research §1) and has no plaintext
identifier (research §15).

Everything an importer learns about a share comes from the sharer over this protocol (FR-047).
Helpers supply only `Range` data.

- **Implementation**: the crate `crates/lanrurugi-share-proto`, which owns the message types,
  code derivation and framing.
- **Framing**: each request opens one bidirectional stream. The client writes one
  length-prefixed `postcard` request and finishes its send side. The server answers with one
  length-prefixed `postcard` response header, optionally followed by a raw byte body (bao stream
  or image bytes).
- **ALPN**: `h3`, so the Initial packet reads like HTTP/3 (FR-045). It does not name the
  application.
- **Versioning**: the first frame on every stream carries this build's `ShareEngine` — the
  `protocol` epoch and the `transport` generation (`iroh 1.4.1`) — in its own length-prefixed part of
  the frame, before the message. Both fields must match; a mismatch is answered with
  `Refused { reason: unsupported_version, engine } ` and the importer turns that into an HTTP 422
  whose `detail` names both engines. Because the engine is read before the payload is decoded, a
  peer whose message shapes are unknown still gets a clear refusal rather than a broken stream. A peer answers an unsupported
  version with `Refused { reason: unsupported_version, supported: [..] }`, and the importer turns
  that into an HTTP 422 whose `detail` names both sides' versions — a mismatch has to be actionable,
  not a generic failure. Version 2 added `ArchiveEntry.toc` (each archive's chapters,
  `{ page (1-based), name }`) and version 3 added `Opened.allow_helpers`; postcard cannot skip
  either, so older peers are refused rather than misread.
- **Compatibility policy**: exactly one engine version is served at a time, and any additive change to
  a postcard message bumps `protocol`. An iroh upgrade that changes the wire bumps `transport` (and
  `protocol` with it); `crates/lanrurugi-share-proto/tests/version.rs` fails if the constant drifts
  from the pinned `iroh`. Browsing, covers and reading do **not** use this protocol (they are
  plain HTTP JSON, which tolerates new fields in both directions), so a version gap only ever
  affects importing and helping to deliver — and it is reported as such.

## Code derivation

Input: `normalize(code)`, which uppercases, maps `O→0` and `I`/`L→1`, strips separators, and
requires 10 characters (or 8 for a short code). There is no check character.

**Text form**: `lrrg:<code>[?r=<sealed route hint>]`. Every code the sharer shows carries its own
routes, so it works with no rendezvous or relay reachable as long as one route still leads to the
sharer. `r` is `base64url(nonce ‖ XChaCha20-Poly1305(postcard(RouteHint)))` under
`HKDF-SHA256(salt "lrrg-share-hint-v1", code)`, AAD `lrrg-hint-v1`: the text shows no address,
though anyone holding the whole text form also holds the code and can open it. `RouteHint` holds
the sharer's endpoint id, up to six dialable addresses (configured external ones, then public, then
private), the relays it is connected to plus its own relay, and its configured LANrurugi nodes.
The importer dials those routes at once and, after a 3 s head start, also asks the rendezvous
(including the hint's nodes), for when the sharer's addresses have changed; the first connection
wins. A bare code works through the rendezvous alone. Codes from before the sealed hint carry
`via=<endpoint_id>@<addr>[,…]`, `relay=<https url>[,…]` and `node=<endpoint_id>@<host:port>[,…]`
in the clear; these are still read. Unknown query parameters are ignored.

| Output | Derivation | Purpose |
|---|---|---|
| `access_hash` | `SHA-256("lrrg-share-access-v1" ‖ code)` | sent in `Open`; stored by the sharer as `code_hash` |
| `seed` | `Argon2id(code, "lrrg-share-lookup-v1", m=64MiB, t=3, p=1)` | |
| `lookup_key` | `HKDF(seed, "lookup")` → ed25519 | pkarr record key |
| `record_key` | `HKDF(seed, "record")` → XChaCha20-Poly1305 key | record encryption |
| `lookup_hash` | `SHA-256(lookup_key.public)` | LAN `ResolveLocal` |

## Knock record (the only data held by outside services, FR-047)

- A pkarr signed packet under `lookup_key`. Its TXT entry holds
  `base64(nonce ‖ AEAD(record_key, postcard(KnockV1)))`.
- `KnockV1 { endpoint_id, relay_urls }`. There is no share id, no address and no URL.
- Publishing goes to every configured rendezvous: pkarr HTTPS relays (`PUT /<z32 key>`) and
  LANrurugi nodes (`RendezvousPut`). Lookups query all of them in parallel, and the first packet
  whose signature verifies wins.
- TTL 3600. The sharer republishes hourly while the share is active, and immediately when its
  relay set changes. It stops on revoke or expiry.

## Requests

| Request | Sent by | Response | Notes |
|---|---|---|---|
| `ResolveLocal { lookup_hash }` | importer → mDNS peers | `Found { share_id }` / `NotHere` | only when public services are off and no rendezvous answers (research §8) |
| `Open { access_hash }` | importer | `Opened { session, share_id, manifest(browse), permissions, public_url? }` or `Refused{reason}` | rate-limited per endpoint and address. `session` is the P2P equivalent of the HTTP recipient token |
| `Thumbnail { session, archive_id }` | importer (import preview) | header + image bytes | |
| `Claim { session, kind: Import }` | importer | `Claimed { claim_id, manifest(claim form, with blake3 and each archive's static category names), ticket, helpers: [HelperAddr] }` or `Refused{reason}` | atomic claim script. With an IP restriction, it waits up to 15 s for a direct path, then answers `address_unverifiable` |
| `Prepare { session, claim_id, archive_id }` | importer | `Prepared { blake3, size }` / `Refused{reason}` | moves the archive to the front of the sharer's fingerprint queue and answers when it is ready (research §13). Used for manifest entries marked `pending` |
| `Renew { claim_id }` | importer | `Renewed { ticket, helpers }` / `Refused{reason}` | every 10 s while transferring; `helpers` is the current helper list, so sources can join mid-import. Re-checks status, import permission and the IP restriction against the current observed address (a new address is appended to the claim). Exhaustion never refuses an existing claim (FR-015). The ticket lists the archives' current roots: a missing root means that archive changed and must restart via `Prepare`. A refusal means stop |
| `Range { ticket, blake3, ranges: ChunkRanges }` | importer → sharer or helper | header + bao-encoded stream, verified by the importer against the sharer's outboard and root | served by the sharer, or by helpers holding the blob. Ticket checks: signature, `expires_at`, importer = connection's remote `EndpointId`, `blake3 ∈ roots` |
| `Outboard { ticket, blake3 }` | importer → **sharer only** | header + outboard bytes | the piece hash tree. Helpers refuse this request (FR-047) |
| `HelperRegister { share_id, archives: [sharer archive id], relay_urls, addrs }` | helper → sharer | `HelperStatus` / `Refused{reason}` | the sharer accepts only when the share has `allow_helpers`, and only from endpoints that made an import claim for this share. `relay_urls`/`addrs` are passed on to importers in `Claimed.helpers` (`{ endpoint_id, relay_urls, addrs }`), since the endpoint runs no address-lookup service |
| `HelperHeartbeat { share_id }` | helper → sharer | `HelperStatus { state, reason?, at, removed_archives }` | `state` is `active`, `paused` (reason `exhausted`) or `ended` (reason `revoked`, `expired` or `helping_disabled`). `at` is when that state began. `removed_archives` lists sharer archive ids no longer in the share. On `ended` the helper stops serving for good (FR-035a). `HelperRegister` answers with the same `HelperStatus`. |
| `RendezvousPut { signed_packet }` | sharer → LANrurugi node | `Ok` / `Refused{reason}` | only on nodes with the rendezvous role. The node verifies the pkarr signature, then stores the packet for 2 h. Rate-limited per source |
| `RendezvousGet { lookup_pubkey }` | importer → LANrurugi node | `Packet { signed_packet }` / `NotHere` | rate-limited per source. The importer verifies the signature itself |

### Refusal reasons

`not_found`, `revoked`, `expired`, `exhausted`, `permission_denied`, `address_unverifiable`,
`rate_limited`, `paused { until? }`.

`paused` is sent only for own-share `Claim`, `Prepare` and `Range` while the alternative own-share
rate is 0 (research §14). The importer keeps its progress and retries at `until`.

- An IP-refused, unknown or wrong `access_hash` always yields `not_found` (FR-014).
- `permission_denied` is only returned after a successful `Open`.

## Guarantees

- **Chunking**: transfers are always chunked. `Range` requests address 1 MiB pieces, and the bao
  stream is verified in 16 KiB groups. Unverified bytes are never written, and resume fetches only
  pieces that are not yet verified (FR-022).

- **Encryption**: all bytes are end-to-end encrypted by iroh, so relays see only ciphertext
  (FR-026).
- **Verification**: every `Range` body is verified against `blake3` before it is written. A
  failing source is dropped for that transfer (FR-034).
- **Helpers**: a helper never serves without a valid ticket (FR-035), and never serves after its
  admin turns `help_deliver` off (FR-033).
- **Revocation**: tickets live 25 s and are renewed every 10 s, so revocation or a permission
  removal stops every source within 30 s (SC-009).
