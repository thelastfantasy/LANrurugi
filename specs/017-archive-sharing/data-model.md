# Data Model: Archive & Tankoubon Sharing

All state lives in Redis (no new database). Archive files stay where they are; share-specific
files live under `<data_dir>/share/` and `<temp>/share/`.

New key names follow the `LANRURUGI_` prefix convention. Each one gets a constructor in
`crates/lanrurugi-storage/src/keys.rs`. IDs are newtypes in `lanrurugi_core::ids` (constitution
1.8.0 newtype-ID rule):

- `ShareId`: 16 random bytes, rendered as base32;
- `ClaimId`;
- `ShareImportId`.

## Sharing instance

### Share — `LANRURUGI_SHARE:<share_id>` (hash)

| Field | Type | Notes |
|---|---|---|
| `id` | ShareId | |
| `title` | string? | default: first item's title |
| `code_hash` | hex `access_hash` of the code (contracts/share-protocol.md) | lookup key for incoming attempts, HTTP and P2P alike |
| `short_code` | bool | 8-character code; only valid while `expires_at ≤ created_at + 7 d`, and the expiry cannot later be extended beyond that |
| `code_enc` | XChaCha20-Poly1305 of the code under a key derived from the node secret | lets management re-display the code (FR-006). The plaintext code is never stored |
| `lookup_pubkey` | hex ed25519 | pkarr key derived from the code (research §2), for republishing |
| `record_key` | hex 32 bytes | XChaCha20 key derived from the code, needed to re-encrypt the record on republish. Kept in Redis, which is already the secret store's trust boundary (Principle V) |
| `items` | JSON `[ShareItem]` | ordered as added |
| `permissions` | JSON `{import, read, download}` | at least one true (FR-003) |
| `download_limit` | int? | `null` = unlimited |
| `downloads_used` | int | incremented only by the claim script |
| `ip_allow` | JSON `[string]` | IPs and CIDRs; empty = no allow-list |
| `ip_first_n` | int? | lock to the first N distinct addresses |
| `ip_seen` | — | kept in the separate set `LANRURUGI_SHARE_IPS:<share_id>` |
| `expires_at` | unix secs? | |
| `embed` | bool | default `false` (FR-030) |
| `allow_helpers` | bool | default `false` (FR-033). Turning it off makes heartbeats answer `HelperStatus` `ended` (`helping_disabled`) and empties the helper list in claims |
| `helpers_disabled_at` | unix secs? | when `allow_helpers` was last turned off; the `at` of that `HelperStatus` |
| `revoked_at` | unix secs? | terminal |
| `created_at` | unix secs | |
| `created_by` | actor label | same shape as activity records |

**Indexes**:

- `LANRURUGI_SHARES`: zset, `share_id` scored by `created_at`, for the management list.
- `LANRURUGI_SHARE_BY_CODE:<code_hash>`: string → `share_id`, for O(1) code lookup.
- `LANRURUGI_SHARE_ITEM_REFS:<archive_or_tank_id>`: set of `share_id`. It lets a deleted item be
  shown as unavailable, and lets the reader show "already in share X".

**Derived status**, never stored:

1. `revoked` if `revoked_at` is set;
2. otherwise `expired` if `expires_at` has passed;
3. otherwise `exhausted` if `download_limit` is set and `downloads_used >= download_limit`;
4. otherwise `active`.

`revoked` is terminal (FR-006). The others can change back when the limits are edited.

**Validation**:

- `permissions` must not all be false.
- `ip_allow` entries must parse as `IpAddr` or `IpNet`.
- `ip_first_n >= 1`.
- `short_code` requires `expires_at` within 7 days, and a PATCH may not move it later.
- `download_limit >= 1`.
- Items must exist at add time.

### ShareItem (inside `items`)

```text
{ kind: "archive", id: ArchiveId }
{ kind: "tankoubon", id: TankId }
```

Resolved at request time against the library:

- tankoubon members come from the current `Grouping`;
- a missing item is reported as `unavailable` (FR spec edge case, US2-4).

### Recipient claim — `LANRURUGI_SHARE_CLAIM:<claim_id>` (hash, TTL 24 h sliding)

| Field | Notes |
|---|---|
| `share_id` | |
| `kind` | `browser_download` or `import` |
| `addresses` | JSON list of observed addresses (a mobile address change appends) |
| `endpoint_id` | importer's iroh `EndpointId` (`import` only) |
| `started_at`, `last_active_at` | |

**Indexes**:

- `LANRURUGI_SHARE_CLAIMS:<share_id>`: zset by `started_at`. The management page reads it for
  "address and time of each download" (FR-005).
- That index outlives the claim hashes. Its entries carry `{claim_id, kind, first_address,
  started_at}` as JSON members, so history survives claim expiry.

**Creation**: only through the claim Lua script (research §9), which atomically checks:

- status and permission;
- IP rules, adding to `LANRURUGI_SHARE_IPS` when `ip_first_n` is set;
- the download limit.

It then creates the claim and increments `downloads_used`. Reading online never creates a claim
(FR-015).

### Recipient token — `LANRURUGI_SHARE_RTOKEN:<sha256(token)>` (string → JSON, TTL 12 h sliding)

`{share_id, claim_id?, address}`. It is issued by `POST /api/share/r/open`. `claim_id` is filled
in once the recipient starts a browser download.

### Helper registry — `LANRURUGI_SHARE_HELPERS:<share_id>` (zset)

Members are helper `EndpointId`s, scored by their last heartbeat. A helper heartbeats every
5 minutes while opted in. An entry older than 15 minutes is excluded from claim responses.

### Fingerprint preparation queue — `LANRURUGI_SHARE_PREPARE` (zset)

Archive ids awaiting fingerprinting, scored by enqueue time. A single background job drains it,
with `LoadThrottle` backoff, and is visible in the Jobs console. A `Prepare` request from an
importer moves its archive to the front (research §13).

### Archive content fingerprint — `LANRURUGI_SHARE_BLOB:<archive_id>` (hash)

| Field | Notes |
|---|---|
| `blake3` | root hash of the bytes a download delivers (research §5) |
| `size` | |
| `source_path`, `source_mtime`, `source_size` | detect change → rehash |
| `merged` | bool: bytes come from a patch-merged copy under `<temp>/share/` |

The outboard file is `<data_dir>/share/outboards/<blake3>.obao`.

### Node identity — `LANRURUGI_SHARE_NODE_SECRET` (string)

The iroh secret key. It is created once and included in backup/export.

### Settings — `LRR_CONFIG` fields (FR-006a, FR-027)

| Field | Default | Meaning |
|---|---|---|
| `share_public_services` | `1` | public services on/off: n0 iroh relays and pkarr relays (research §8). Configured nodes are unaffected |
| `share_nodes` | `""` | newline-separated `lrrg-node:<endpoint_id>@<host:port>` LANrurugi rendezvous and relay nodes (FR-048) |
| `share_relay_urls` | `""` | extra iroh relay URLs; n0's are added while public services are on |
| `share_pkarr_relays` | `""` | pkarr HTTPS relays; pkarr's defaults are used while public services are on and this is empty. The instance never speaks the DHT protocol (FR-045) |
| `share_node_rendezvous` | `0` | offer the rendezvous role to other instances (FR-048) |
| `share_node_relay` | `0` | offer the relay role (embedded `iroh-relay`) |
| `share_node_relay_tls` | `acme` | `acme` (for `share_public_url`'s host), `files`, or `proxy` (plain HTTP on a local port, behind a tunnel or reverse proxy that terminates TLS; FR-055) |
| `share_node_relay_port` | `11205` | TCP listen port of the relay role. With `acme`/`files`, publish it as host `443/tcp`; with `proxy`, it is bound to localhost or the container network only |
| `share_self_relay_url` | `""` | public `https://` address at which this instance's own relay is reachable, for example through Cloudflare Tunnel. Added to the endpoint's relay map and knock record |
| `share_portmap` | `1` | UPnP / NAT-PMP / PCP automatic port mapping on or off |
| `share_node_relay_cert` / `share_node_relay_key` | `""` | PEM paths when `files` |
| `share_node_relay_rate` | `10485760` | per-client relay rate in bytes/s; empty for none |
| `share_node_relay_allow` | `""` | endpoint ids or node strings the relay admits besides this instance; empty admits everyone |
| `share_external_addrs` | `""` | comma-separated `ip:port` the instance is reachable at (FR-053) |
| `share_udp_port` | `11204` | iroh bind port in the container; publishing it as host `443/udp` is recommended |
| `share_public_url` | `""` | when set, links and QR codes use `<url>/s/<code>`, and embeds are available. It is given to importers by the sharer over the connection, never published (FR-047) |
| `share_default_permissions` | `import,read,download` | |
| `share_default_download_limit` | `""` | |
| `share_default_ip_allow` | `""` | |
| `share_default_ip_first_n` | `""` | |
| `share_default_expiry_days` | `""` | |
| `share_default_embed` | `0` | |
| `share_default_allow_helpers` | `0` | |
| `share_upload_limit` | `""` | own shares: bytes/s; empty = unlimited (FR-041) |
| `share_max_peers` | `8` | own shares: importers served at once |
| `share_helper_upload_limit` | `""` | helping: bytes/s; empty = unlimited |
| `share_helper_max_peers` | `4` | helping: importers served at once |
| `share_alt_upload_limit` | `""` | alternative set (FR-042); `0` pauses own-share uploads |
| `share_alt_max_peers` | `""` | alternative set; empty = same as normal |
| `share_alt_helper_upload_limit` | `""` | alternative set; `0` pauses helping |
| `share_alt_helper_max_peers` | `""` | alternative set |
| `share_alt_schedule` | `""` | JSON `{ enabled, windows: [{ from: "HH:MM", to: "HH:MM" }], days: [1..7] }`. A window may cross midnight. Evaluated in the server's local time zone |
| `share_alt_manual` | `0` | force the alternative set on |
| `share_import_concurrency` | `3` | receiving side: archives transferred at once per import (FR-019a); copied into each new ShareImport's `concurrency` |

All of these are admin-only and are added to `TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS` alongside the
other network settings.

## Receiving instance

### ShareImport — `LANRURUGI_SHARE_IMPORT:<import_id>` (hash)

| Field | Notes |
|---|---|
| `code` | the share code. It is needed to reconnect after restart, and is deleted when the import completes or fails terminally |
| `sharer_endpoint_id` | |
| `share_id` | the sharer's id |
| `claim_id` | from the sharer |
| `manifest` | JSON `ShareManifest` as fetched at preview/confirm time |
| `category` | optional local category chosen in the dialog |
| `queue_item_id` | the single download-queue entry representing this import (FR-019) |
| `archives` | JSON list, one entry per manifest archive: `{sharer_archive_id, blake3, size, state: pending\|transferring\|ingesting\|done\|reused\|failed, bytes_done, error?, local_archive_id?}` |
| `concurrency` | archives transferred at once, from `share_import_concurrency` at creation (FR-019a) |
| `help_deliver` | bool, the opt-in (FR-033); editable later in Settings → Sharing |
| `help_state` | `inactive` / `active` / `paused` / `sharer_offline` / `ended` (FR-035a). `sharer_offline` is derived when `help_last_contact_at` is more than 15 min old and the state is not `ended` |
| `help_reason` | `exhausted` / `revoked` / `expired` / `helping_disabled`, when paused or ended |
| `help_state_at` | unix secs: the sharer-reported time of the change |
| `help_last_contact_at` | unix secs: the last successful heartbeat |
| `help_removed_archives` | JSON list of sharer archive ids no longer in the share (not served) |
| `help_bytes_served` | int, total uploaded while helping |
| `help_hidden` | bool. The administrator removed the entry from the helping list: the record stays for the import, but the row is hidden and helping is off |
| `state` | `transferring` → `rebuilding` → `done`, or `failed` (with reason) |

**Index**: `LANRURUGI_SHARE_IMPORTS` (zset by creation time).

### Download queue integration (existing `DownloadQueueItem`)

An import is **one** queue item, however many archives it has (FR-019):

- `origin` gains the variant `ShareImport`.
- New optional field `share_import: { import_id }`.
- `plugin_namespace` is the fixed placeholder `"share_import"`, used only for the Upload page
  grouping, like `local_upload`.
- `url` holds the display string `lrrg-share:<share title>`.
- The queue item's job reports **aggregate** progress through the existing `JobStatus`:
  `downloaded_bytes`/`total_bytes` across every archive, plus a new optional
  `items_done`/`items_total` pair.
- Per-archive state lives in the ShareImport's `archives` list and is served by
  `GET /api/shares/imports/{id}` for the expanded view.

Partial transfer files:

```text
<temp>/share/import/<import_id>/<blake3>.part    the data
<temp>/share/import/<import_id>/<blake3>.obao    the outboard
<temp>/share/import/<import_id>/<blake3>.ranges  verified-range bitmap, 1 bit per 16 KiB group
```

They count against `tempmaxsize` (existing temp-space accounting).

**Queue state machine**: the queue item keeps its existing states (`Queued → Starting → Waiting →
Downloading → Done/Error/Cancelled`) for the import as a whole.

- **Pause/stop** cancels every in-flight archive transfer and keeps the verified ranges.
- **Start** resumes the remaining archives.
- **Retrying one failed archive** resets that archive's entry to `pending` and restarts the import
  task if it had stopped.
- **When it ends**: the import ends `Done` when every archive is `done` or `reused` and the
  tankoubons are rebuilt. It ends `Error` when some archives failed; the completed ones stay
  ingested.

**Inside the import task**:

- A semaphore of `concurrency` permits bounds how many archives transfer at once.
- The import never uses the per-plugin `DownloadManager` job pool, so it cannot hold slots that
  other downloads wait on (FR-019a).

**After each archive is ingested**: the manifest's title, tags and summary are applied, and the
archive's entry becomes `done` with `local_archive_id`.

**Once every archive is settled**: the import creates each manifest tankoubon:

- a **new** `Grouping` with the shared name, tags and summary, and members in manifest order;
- it never merges into an existing same-name tankoubon (spec edge case).

### Helper opt-in on the receiving side

`help_deliver` on the ShareImport. While true and the import is `done`, the instance:

- registers with the sharer as a helper and heartbeats every 5 min;
- stores each heartbeat's `HelperStatus` into the `help_*` fields;
- serves ranges for the manifest's blobs to ticket holders (research §6) while `help_state` is
  `active`, skipping `help_removed_archives`.

**Help state transitions**:

```text
inactive ──opt in + import done──> active
active ──HelperStatus paused(exhausted)──> paused ──HelperStatus active──> active
active|paused ──HelperStatus ended(revoked|expired|helping_disabled)──> ended   (terminal; serving stops)
active|paused ──no contact > 15 min──> sharer_offline (derived) ──contact──> previous state
any ──admin turns help_deliver off──> inactive
```

**Caps (FR-035b, FR-041)**: helping traffic goes through the helping half of the instance-wide
`ShareUploadLimiter` (research §14), shared by all helped shares.

### Rendezvous storage (rendezvous role only) — `LANRURUGI_SHARE_RDV:<lookup_pubkey>` (string, TTL 2 h)

A verified pkarr signed packet, stored as received. The node cannot read it.

### Outside-service status (in memory)

Per configured rendezvous and relay: the last success time and the last error, for the settings
list (FR-049).

## Wire types (`lanrurugi-share-proto`)

### ShareManifest

```text
{
  share_id, title, permissions,
  items: [
    { kind: "archive", archive: ArchiveEntry }
    { kind: "tankoubon", name, tags, summary, members: [ArchiveEntry] }   // ordered
    { kind: "unavailable", title? }
  ],
  sharer_endpoint_id
}
```

### ArchiveEntry

```text
{ archive_id, title, tags, summary, pagecount, filename, size, blake3?, cover_hash }
```

`blake3` is present only when the manifest was requested under a claim. Browsing manifests omit
it, so hashing is not triggered by browsing.

### Ticket

```text
{ share_id, claim_id, importer: EndpointId, roots: [blake3], expires_at }
```

It is signed by the sharer's iroh key.

## Relationships

```text
Share 1─* ShareItem ─→ Archive | Tankoubon(Grouping) ─* Archive
Share 1─* Claim ─(import)→ remote ShareImport 1─* DownloadQueueItem
Share 1─* Helper(EndpointId) ←─ remote ShareImport(help_deliver)
Archive 1─1 ShareBlob (fingerprint + outboard)
```
