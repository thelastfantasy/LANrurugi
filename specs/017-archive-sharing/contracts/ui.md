# UI Contract: Sharing

Rules for every new surface:

- Form controls come from `apps/frontend/src/components/common-ui/`.
- Colours beyond existing legacy classes get one class per theme file, in all five themes
  (CLAUDE.md).
- Strings go through i18n (zh/en/ja) with semantic keys under `share.*`.
- None of these surfaces are shown to non-administrators.

## Entry points (US1)

- **Library context menu** (`ArchiveContextMenu.tsx`): "Share…" for archives and tankoubons. With
  a multi-selection active, it acts on the whole selection. Also adds "Download" for tankoubons
  (US3), removing the `!isTank` guard.
- **Reader**:
  - a "Share…" action next to the existing download action;
  - when reading inside a tankoubon, download offers two entries: "This archive" and "Whole
    tankoubon".
- **Share dialog**:
  - tab "New share": title, permissions (three checkboxes, at least one), download limit, IP
    allow-list (its help says a recipient behind a proxy is seen at the proxy's exit address,
    FR-051a), first-N, expiry, embed toggle (disabled with a reason while no public URL is set,
    FR-030), an "Allow other instances to help deliver" toggle
    (off by default, with the note "participants can see each other's IP addresses"), and a "Short
    code (8 characters)" toggle that is enabled only when an expiry of 7 days or less is set;
  - tab "Add to existing": a list of active shares, plus "Also update this share's settings",
    which reveals the same fields prefilled.
  
  Defaults come from Settings → Sharing.
- **Result view**:
  - the text form `lrrg:XXXXX-XXXXX` (or `lrrg:XXXX-XXXX` for a short code), with Copy;
  - the link, only when a public URL is set, with the note "stops working if your domain
    changes; the code keeps working";
  - the QR code (canvas), encoding the link or else the text form, with Copy and Save PNG;
  - the embed snippets when embedding is on.

## Settings → Sharing section (US2, FR-005, FR-006, FR-006a)

1. **Shares table**:
   - columns: title, status badge, permissions, used/limit, items, created, expires;
   - a row opens a detail panel with items (unavailable ones marked), claim history
     (kind / address / time), helpers, edit fields, Show code/QR, embed snippets and Revoke (with
     confirmation).
2. **Helping deliver**:
   - One row per share this instance helps or has helped. Each row has:
     - the share title, and the `help_deliver` toggle;
     - a state badge: active, paused, sharer offline or ended, using the share status classes;
     - the reason and its time, for example "Revoked by sharer · 2026-10-24 21:13", "Downloads used
       up · since …" or "Sharer offline · last reached …";
     - "N archives no longer shared", when any were removed;
     - total uploaded while helping;
     - "Remove from list" on ended rows. It is a confirmation-free removal of the row only;
       imported archives stay in the library.
3. **Connectivity**:
   - public services on/off (configured LANrurugi nodes stay in use);
   - relay URLs;
   - pkarr relays;
   - UDP port;
   - public URL, with a status line showing the current routes (direct reachable, relay
     connected);
   - **LANrurugi nodes** (FR-048): a list of `lrrg-node:` strings to use; this instance's own
     "Offer rendezvous" and "Offer relay" toggles, the relay TLS choice, and its own node string
     with Copy;
   - **Proxy** (FR-051, FR-052): a status line showing the proxy currently followed ("Following
     ALL_PROXY · socks5://127.0.0.1:7890" / "No proxy detected"). Under an "Advanced" disclosure:
     an override URL, "Proxy only", "Allow LAN direct in proxy-only mode", and a CA bundle path;
   - **External addresses** (FR-053): a list of `ip:port`, for manual port forwards and UDP
     tunnels;
   - **Router port mapping** (FR-055): a UPnP / NAT-PMP / PCP toggle, and a status line ("Mapped
     via UPnP · 203.0.113.7:41641" / "No mapping");
   - **Own relay behind a tunnel** (FR-055): relay TLS mode `proxy`, the local port, and the public
     relay address (for example a Cloudflare Tunnel hostname), with a short note on what to point
     the tunnel at;
   - **Outside services in use** (FR-049): one row per rendezvous and relay, showing its source
     (public / node / custom), a reachability dot and the last success time.
4. **Upload limits** (FR-041, FR-042), laid out like a BT client's speed settings:
   - normal caps: own-share rate, own-share importers, helping rate, helping importers;
   - alternative caps, with the same four fields;
   - the schedule: an enable toggle, one or more from–to windows, and day-of-week checkboxes;
   - a manual "Use alternative limits now" switch;
   - a status line: "Normal limits in effect · switches to alternative at 01:00".
   - "Archives transferred at once per import" (receiving side, FR-019a).
5. **Defaults for new shares**.

## Upload page: Import share (US4)

- **Entry**: an "Import share" button next to the existing upload controls opens the modal.
- **Inputs**:
  - a code field; paste is normalised and accepts the `lrrg:` text form, a link or a bare code;
  - an image drop / paste / file chooser;
  - "Scan with camera". This is shown only when `isSecureContext` and `mediaDevices` exist.
    Otherwise "Take photo" is shown (`capture="environment"`), with a one-line reason (FR-016).
- **Camera view**: a live preview, closed and released on a hit or on dismissal (FR-017).
- **Preview step**:
  - items with covers;
  - archive count;
  - total size;
  - an "already in library" badge;
  - a category picker;
  - a "Help deliver this share" checkbox (default off), shown only when the share allows helping,
    with the note "other downloaders will see this instance's IP address". When the share has
    helpers, a line says the import may download from other instances, which will see this
    instance's IP address.
  
  Confirm queues the import. Refusals show their reason; `not_found` adds a hint that the sharer
  sees a proxy's exit address when this instance uses one (FR-051a).
- **Queue**: each import is **one row** in the existing queue panel, under a "Share import" group.
  - **Collapsed**: the row is the same height as a download row. It shows the share title, a
    progress bar, `12 / 300 archives · 3.2 / 48 GB · 25 MB/s · 30 min left`, and the usual
    pause/stop buttons. While the sharer has paused uploads, the speed part reads "sharer paused
    uploads until 08:00".
  - **Expanded**: a disclosure toggle opens a per-archive list in a bounded-height scrolling area,
    virtualised for hundreds of rows. Each row shows title, state, a progress bar while
    transferring, the error if failed, and a retry button on failed rows.
  - The expanded list is fetched only while expanded
    (`GET /api/shares/imports/{id}`, polled like the queue).

## Recipient share page `/s/:code` (US1)

- Outside the authenticated app shell: no library navigation, no login redirect.
- Shows a grid of items. Tankoubons expand to their ordered members.
- Actions per permission:
  - Read opens a share-scoped reader view (reuses the reader component with a share transport);
  - Download;
  - for import permission, a "How to import" box with the code and QR.
- States:
  - revoked, expired and unknown codes render one "not available" page;
  - an exhausted share shows a notice where the download buttons would be;
  - there is no gateway: this page exists only on the sharer's own instance (FR-047).

## Embed card (US6)

- **Same component** for the iframe page and the `<lrrg-share>` shadow root.
- **Content**:
  - up to 4 covers;
  - title;
  - item count;
  - actions per permission, where import-only makes the code and QR the main element.
- **States**: restricted, revoked, expired and exhausted states each replace the content.
- **Layout**: light/dark follows `prefers-color-scheme`. The card is fluid down to 280 px wide.
