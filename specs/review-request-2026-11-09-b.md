# Review request — sharing engine split and the first release-fix rounds (2026-11-09, late)

This is the report for the work done **after** `specs/review-result-2026-11-09.md` was answered by
`10ca5af` + `b72b637`. Everything below is on `feat/share-bind-interface` (PR #258) and CI is green
as of `04a9b24`.

Review these in order; the first section is where a mistake would be quiet and expensive.

## 1. Are the four specs actually implemented? (state, not claims)

| Spec | Code | What is left |
|---|---|---|
| 020 `hash-named-library` | complete | real-machine quickstart: a real plugin download landing as `{id}.{ext}` + `{id}.json`, and a >300-byte title surviving a real download. Unit tests cover both; no real download was ever performed. |
| 021 `direct-download-domain` | complete | quickstart A/B: multi-GB download through a non-proxied host, and a cookie-less share-page download. `nginx -t` was run; a real CDN path was not. |
| 022 `share-network-hardening` | complete except platform/routing items | everything that needs a real `tun0` (packet capture, VPN reconnect, interface disappearance), the 99% baseline (T037), and macOS/FreeBSD routing (T025 — FreeBSD `setfib`, macOS refuses and says so in settings). |
| 023 `share-crate-extraction` | **partial, by design** | see §2. |

Open task boxes in `specs/020…/tasks.md` etc. carry a parenthetical saying why they are open; none
of them is unbuilt code.

## 2. 023 is partial — what moved and what did not

Moved into `crates/lanrurugi-share` (`cargo tree -p lanrurugi-share | grep -c lanrurugi-api` is 0):
`host.rs` (`ShareHost`, `ShareArchive`, `EmptyHost`), `net.rs` (`ShareNet`, `Shared`, `Session`,
`RatioState`, `bind`/`BindOptions`/`Bound`, `relay_urls`, `relay_settings_key`), `nodes.rs` (node
strings, `NodeRef`, hex ids), `relay.rs` (the embedded relay), `publish.rs` (`via_hint`, pkarr
relay list, node record publish/retract), `settings.rs`, `limits.rs`, `blobs.rs`, `proxy.rs`,
`interfaces.rs`, `link.rs`, `status.rs`.

Still in `crates/lanrurugi-api/src/share_net/` and why:

- `server.rs` — the protocol handler serves bytes by calling the download manager and the archive
  tables; splitting it needs host methods that return *results* over HTTP semantics, not data.
- `import.rs` — the importer ends in `lanrurugi-scanner`'s cataloguing pipeline.
- `nodes.rs`'s remaining half — pkarr resolution, rendezvous store, node-only mode.
- `start()` and the four background loops — `bind()` must attach `server::ShareProtocol`, which
  holds `AppState`; the loops take `AppState` too.

So the crate owns the engine's *state and wire format*, not yet its *I/O*. `specs/023…/tasks.md`
records this (T003–T014 remain open with reasons) rather than claiming completion.

## 3. What the last review's fixes look like now

- `share_net::bind` **does** call `netwatch::set_bind_device` and
  `iroh_relay::client::set_bind_device` (the previous review's first blocker was that nobody did).
  Test: `share_net::publish`'s pinned/unpinned pair; a real capture still needs a real tunnel.
- `token_guest /api/tankoubons/:id/download-link` is denied in `policy/route_policy.csv`.
- Relocation is atomic (copy to `.part` → fsync → rename on EXDEV, rollback on record-save failure,
  patch either moves with the archive or the archive moves back).
- Ingest never deletes the only copy of a content id, and a failed catalogue keeps the bytes.
- Sidecars are rewritten by the rebuild audit (`repaired` in the result JSON).
- CF trust is "nearest hop must be a CF edge or an unconfigured peer"; a typed CF address in
  `X-Forwarded-For` no longer unlocks `CF-Connecting-IP`.
- Download retries are typed (`HttpRequestFailed`, `TruncatedDownload`, `429/502/503/504`), the read
  has an idle timeout (`Stalled`), and the outbound client has connect/read timeouts + keepalive.
- Platform: `interface_binding_supported()` gates the setting, so FreeBSD/macOS get `400` instead
  of a silent no-bind.

## 4. Things I would look at hardest

1. `crates/lanrurugi-share/src/publish.rs` — `via_hint` decides what a share code advertises; the
   three filters (`address_allowed`, `dialable`, `address_rank`) are the leak surface of 022 US1.
2. `crates/lanrurugi-api/src/share_net/mod.rs::start()` — the rebuild order (release sockets, keep
   relay, retry the port, attach kept relay) is the only place a settings save can wedge sharing.
3. `crates/lanrurugi-scanner/src/relocate.rs` — `RelocationOutcome` now distinguishes settling /
   occupied / outside; the settle window is the thing that keeps an SMB copy from being moved
   mid-write.
4. `crates/lanrurugi-scanner/src/pipeline.rs` — `catalogue_new_archive` reads a sidecar before
   deriving a title, and preserves `original_filename` when rewriting one.
5. `vendor/netwatch` and `vendor/iroh-relay` — each should be exactly one hunk plus its `FORK.md`.

## 5. Gates

`mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-api lanrurugi-scanner`,
`lanrurugi-api` 650 tests, `lanrurugi-share` 19, `lanrurugi-scanner` 128 (one pre-existing
`animation` test needs an asset that is not mounted here), `npx tsc -b --force`, `eslint src` (0
errors), `mise run docs-api -- --check`, `mise run docs-build` — all green; CI (Rust/Docs/Frontend)
green on `04a9b24`.

CI itself needed two changes this round: `scripts/fetch-pdfium.sh` retries the download, and the
workflow caches `.pdfium/linux-x64` keyed by version, because GitHub's release CDN answered 504 on
every attempt of two consecutive runs.
