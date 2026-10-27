# Quickstart: Validating Sharing

## Prerequisites

- **Instance A** (the sharer) is the dev container (`mise run dev-up`). Instance B (the importer)
  is a second instance with its own Redis:
  - for same-machine runs, a second compose project with different ports;
  - for the network scenarios, a second machine.
- **Library**: A has at least one tankoubon with 3 members, and one loose archive.
- **UDP port**: `share_udp_port` is published on A for direct-route tests. Leave it unpublished for
  the NAT scenario.
- **Logging in**: log in as administrator on both instances. Recipient-only checks use a private
  window with no session.

API examples assume A at `http://localhost:3000`. Contract details are in `contracts/`.

## Scenario 1: tankoubon download (US3)

1. In A's library, right-click the tankoubon and choose **Download**.
2. Unzip the result. Expect 3 entries named `01 - …`, `02 - …`, `03 - …` in tankoubon order, each
   byte-identical to its library file:
   - compare with `sha256sum` against `GET /api/archives/{id}/download`;
   - for an archive with a `.patch.zip`, compare against the merged download.
3. Move one member's file away and download again. Expect 2 entries plus a `MISSING.txt` that
   names the missing member.
4. Download a tankoubon totalling more than 4 GiB. Expect bytes to start arriving within 2 s, and
   no growth in `<temp>`.
5. Request the same URL as a `token_guest` and as a guest visitor. Expect `403`/`404`.

## Scenario 2: create, browse, limit, revoke (US1, US2)

1. Share the tankoubon plus the loose archive with permissions read and download, and limit 2.
   Expect a code, a link and a QR code.
2. Open the link in a private window. Expect both items, with the tankoubon's members in order.
   Reading pages works.
3. Turn guest mode off, and put one item in a non-guest category. Repeat step 2: it still works
   (FR-013a).
4. In the same window, request `/api/archives/<any other id>/thumbnail` with and without `?rt=`.
   Expect anonymous behaviour both times (US1-10).
5. Download from two different private windows, then a third. Expect the third to get
   `exhausted`, while browsing still works.
6. Start two downloads at the same moment with one remaining. Expect exactly one success
   (FR-015).
7. Open Settings → Sharing. Expect 2 claims with their addresses and times.
8. Remove the read permission. A page request now gets `403 permission_denied`, and the share page
   hides Read.
9. Revoke the share. Every request (page, thumbnail, download, card) now gets `not_found` within
   5 s (SC-009).
10. Check that the response headers of `/s/…` and `/api/share/r/*` include
    `Referrer-Policy: no-referrer`.
11. Change `share_public_url` to a new domain. The `lrrg:` text form handed out before the change
    still imports on another instance (FR-010a). The old link stops working, and the share dialog
    said so next to it.
12. Try to create a short (8-character) code without an expiry, or with an expiry beyond 7 days.
    Expect it to be refused.

## Scenario 3: IP restrictions (US1-7, FR-004)

1. Set `ip_allow` to a range that excludes your test client. Expect a `not_found` page with no
   titles in the response body.
2. Clear `ip_allow`, and set `ip_first_n = 1`. Download from client X, then browse from client Y.
   Expect Y to be refused and X to keep working.

## Scenario 4: import with QR and camera (US4)

1. Create a share on A with permission import only.
2. On B, open Upload → **Import share**:
   - paste a screenshot of A's QR code. The code field fills in, and the browser's network panel
     shows no upload of the image;
   - on a phone over plain HTTP: expect "Take photo" instead of live scan, and the photo decodes;
   - over HTTPS: live scan reads the code within 3 s, and the camera indicator turns off after the
     hit.
3. The preview shows items, the total size and the "already in library" state. Confirm.
4. In B's queue, expect **one** row for the import under "Share import", the same height as a
   download row, showing archives done/total, bytes, speed and time left. Expand it to see the
   per-archive list.
   - Pause the import mid-transfer, then resume it.
   - Restart B's container mid-transfer: transfers continue without re-downloading verified bytes
     (compare bytes received with the file size).
   - While the import runs, queue an ordinary plugin download on B. Expect it to start immediately,
     and library pages to stay responsive on both instances (SC-014).
   - Share 200+ archives that were never prepared. Expect the import's first archives to start
     transferring within seconds, while A's Jobs console shows the fingerprinting job progressing
     in the background.
5. When the import is done, expect:
   - each archive's title and tags equal A's;
   - one new tankoubon with the same name and member order;
   - every file's `sha256sum` equal to A's.
6. Import the same code again. Expect nothing to be transferred, and a second tankoubon to be
   created from the existing archives. A's share still shows 2 claims.
7. On A, revoke the share during an import on B. Expect B's items to fail with `revoked` within
   30 s, and completed archives to remain.

## Scenario 5: no public address (US5)

**Setup**: A sits behind NAT with its UDP port not published, and no `share_public_url`. B is on
a different network.

1. Import A's share on B. Expect it to succeed. A's log shows the connection path (direct after a
   hole punch, or relay).
2. Restart A's router or change A's public IP. The same code still resolves.
3. Run B on the same LAN as A. Expect the transfer to use the LAN address, with throughput of at
   least 80% of a plain `scp` between the two machines (SC-007).
4. Set an IP restriction while only a relay path is possible: block UDP between A and B. Expect
   the import to be refused with `address_unverifiable`.
5. Set `share_public_services = 0` on A:
   - a bare code from a different network fails with an "unreachable" message;
   - the same code from the same LAN imports via mDNS;
   - a link containing `share_public_url` works when A is reachable at that URL;
   - `lrrg:<code>?via=…` from the share dialog imports when A is directly reachable.
6. Stop A. B reports "sharer currently unreachable", which is different from the response to a
   mistyped code.
7. **Nodes, with public services off on A and B** (FR-048):
   - C has a public address and enables "Offer rendezvous" and "Offer relay";
   - add C's `lrrg-node:` string to A and B;
   - B imports A's bare code, resolved through C and relayed through C if needed;
   - captures on A and B show no connection to any host other than A, B and C;
   - C's Redis holds only the opaque `LANRURUGI_SHARE_RDV:*` packet.
8. **No third-party data (FR-047)**: with public services on, decode the packet a pkarr relay
   returns for the code. It holds only `endpoint_id` and `relay_urls`. The manifest, outboards and
   helper list are fetched from A's endpoint id only. Settings → Sharing lists every outside
   service in use, with reachability.
9. UDP blocked on B's network, with only TCP 443 out allowed: an import still completes over the
   relay (FR-046).
10. Run `scripts/share-traffic-check.sh` over the captures. Every flow is classified as ordinary
   encrypted web traffic, none as BitTorrent or P2P, and no plaintext `lanrurugi`, `lrrg` or `iroh`
   appears (SC-016).

## Scenario 6: embeds (US6)

1. With embedding off, `/embed/s/<code>` and `/api/share/r/card` return a 404 card with no
   content.
2. Turn embedding on. Paste both snippets into a static HTML page served from another origin.
   Expect both cards to render the same way, in light and dark mode, at 280 px wide.
3. Compare three shares: read-only, download-only and import-only. Each card shows only its own
   actions, and the import-only card leads with the code and QR.
4. Add an IP restriction. The card now shows only "restricted share". Revoke the share: the card
   shows that it is unavailable.

## Scenario 7: helpers (US7)

1. On A, turn on "Allow other instances to help deliver" for the share; with it off, B's
   **Help deliver** checkbox does not appear and registration is refused. B has finished
   importing and enables **Help deliver**. Throttle A's upload, for example with
   `tc` on A's interface.
2. Import on C. Expect C's log to show ranges fetched from B and from A, and the import to finish
   at least 1.5× faster than an A-only run (SC-011).
3. Corrupt B's copy so B serves a bad range: use a test build flag that flips one byte. Expect C
   to drop B, re-fetch that range from A, and finish with a byte-identical file.
4. Revoke on A. Expect B to refuse C's requests within 30 s. On B, Settings → Sharing → Helping
   deliver shows the share as ended, "revoked by sharer", with the revoke time.
5. On a fresh share helped by B: set A's download limit to the number used. B shows "paused ·
   downloads used up". Raise the limit: B returns to active without any action on B.
6. Remove one archive from the share on A. B shows "1 archive no longer shared", and stops serving
   only that archive.
7. Stop A for over 15 minutes. B shows "sharer offline · last reached …". Start A: B returns to its
   previous state.
8. On B, set the helping upload cap to 1 MB/s and max importers to 1. Expect C's fetches from B to
   stay under 1 MB/s, and a second importer D to wait for C.

## Scenario 8: upload limits and schedule (FR-041–FR-044)

1. On A, set the own-share upload cap to 2 MB/s. Import on B: B's speed stays at or under 2 MB/s,
   within 5% over any 10 s (SC-015).
2. Set an alternative own-share rate of 0, with a schedule window that starts 2 minutes from now.
   When the window starts, B's queue row reads "sharer paused uploads until …" within a minute,
   and its progress is kept. Online reading of the share still works.
3. When the window ends, B continues on its own without re-fetching anything.
4. Turn on "Use alternative limits now". The settings status line shows "alternative · manual".
   Turn it off, and the status line shows normal.
5. Over HTTP, a recipient file download during the pause gets `503` with `Retry-After`.

## Scenario 9: network environments (FR-050–FR-054, SC-018)

Run `scripts/share-network-matrix.sh`. For each environment, a small share is imported between two
instances in that environment, and the route used is recorded.

| # | Environment | Expected route |
|---|---|---|
| 1 | VPS with its public IP not on its interface, plus `share_external_addrs` | direct |
| 2 | home NAT with UPnP / without UPnP | direct (mapped) / direct after hole punch |
| 3 | double NAT on both sides | relay; with a node-only VPS node configured, through that node only |
| 4 | UDP blocked | relay over TCP 443 |
| 5 | HTTP proxy only, with "proxy only" on | relay through the proxy; no packet bypasses the proxy |
| 6 | SOCKS5 proxy only | relay through the SOCKS5 bridge; no packet bypasses the proxy |
| 7 | IPv6-only | direct or relay over IPv6 |
| 8 | Docker bridge with the UDP port published | direct |
| 9 | TLS-intercepting system proxy, its CA in the system trust store | relay through the proxy |
| 10 | isolated LAN, public services off | LAN direct via mDNS |
| 11 | WireGuard pair | direct over the VPN addresses |
| 12 | UPnP router (miniupnpd) / manual UDP port forward | direct (mapped); settings show "Mapped via UPnP · …" / the detected external address |
| 13 | frp UDP proxy on a VPS, with its `ip:port` in `share_external_addrs` | direct through the frp address |
| 14 | relay role in `proxy` mode behind an frp HTTP proxy or Cloudflare Tunnel, with `share_self_relay_url` set, and public services off | relay through the instance's own tunnel; no public relay contacted |
| 15 | importer behind its user's own proxy, **no LANrurugi proxy setting**: `ALL_PROXY=socks5://…` / `HTTPS_PROXY` / mihomo TUN with fake-IP (UDP relayed) / the same with UDP dropped | relay through the proxy, or direct through the proxy's UDP; transfer starts within 30 s. A share with an IP allow-list sees the proxy exit address |

Also run `lanrurugi-server share-node` on a VPS. Check that it prints its `lrrg-node:` string,
uses under 50 MB of RSS, and serves case 3 with no other outside host contacted.

## Automated checks

- **Rust unit tests** (single crate, through `mise run test-crate -- <crate>`):
  - code normalisation, both code lengths and `lrrg:` text-form parsing;
  - derivation vectors;
  - claim script concurrency, against the Redis test lock;
  - ticket expiry and signature;
  - bao range verification and resume;
  - tankoubon zip layout, collision suffixes and `MISSING.txt`;
  - route-policy tests for every new path and subject.
- **Frontend unit tests** (Vitest): code input parsing, permission-dependent card layout, and
  camera fallback selection.
- **Playwright**: the Scenario 2 browser flow, and the Scenario 4 import modal with a QR image
  fixture.
