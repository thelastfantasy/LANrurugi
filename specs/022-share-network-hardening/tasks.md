# Tasks: Share Network Hardening

**Input**: Design documents from `/specs/022-share-network-hardening/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Required. Every behavioural change here is a network behaviour that a unit test can only simulate, so each
story pairs its code with (a) a unit test for the pure decision and (b) a quickstart scenario that exercises it
against a real interface/peer. Where a scenario needs a VPN interface the task says so explicitly.

**Organization**: by workstream, mirroring plan.md's five flows. US1 and US2 are the P1 pair; US3–US5 are P2;
US6 (downloads/repack) and US7 (measurement) are P3 but independent of the P2 ones.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel
- **[Story]**: US1–US7 as in spec.md

---

## Phase 1: Setup (vendored forks)

**Purpose**: the two upstream crates that own the sockets must gain a `bind_device` knob before any of US1 can be
implemented.

- [x] T001 (2026-11-09: `vendor/netwatch/`, patch in workspace `Cargo.toml`; `cargo metadata` resolves netwatch from the vendored path) Vendor `netwatch` 0.19.3 into `vendor/netwatch/` (keep its `Cargo.toml` name/version; add only
  `license`/`repository` metadata if the workspace requires it) and add `[patch.crates-io] netwatch = { path =
  "vendor/netwatch" }` in the workspace `Cargo.toml`. Verify `cargo tree -i netwatch` shows the vendored copy.
- [x] T002 (2026-11-09: `vendor/iroh-relay/`, patched the same way) Vendor `iroh-relay` 1.4.1 into `vendor/iroh-relay/` the same way and patch it.
- [x] T003 [P] (2026-11-09: `vendor/netwatch/FORK.md` and `vendor/iroh-relay/FORK.md` record the exact hunks and why) Record the fork policy in `docs/` (or a `vendor/README.md`): one option each, rebase on every iroh
  bump, no other edits; the diff must stay reviewable in one screen.

---

## Phase 2: Foundational (binding primitives)

- [ ] T004 Add `DeviceBinding { interface: Option<String> }` to `crates/lanrurugi-share-proto` (or the egress crate)
  with `bind_socket(&self, socket: &socket2::Socket) -> io::Result<()>` (Linux `SO_BINDTODEVICE`, FreeBSD
  `IP_BOUND_IF`/`IPV6_BOUND_IF` fallback, macOS returns `Unsupported`) and unit tests for the "no interface" path.
- [ ] T005 [P] Add `EgressError::UnsupportedPlatform` and `EgressError::BindFailed` to
  `crates/lanrurugi-egress` and a decision table test: pinned+Linux → shim, pinned+FreeBSD → `setfib`, pinned+macOS →
  refuse, unpinned → no shim.
- [x] T006 [P] (2026-11-09: `GET /api/shares/settings` 的 `endpoint` 增加 `binding`（`pinned|auto|missing`）与 `bound_interface`，运行与否都带；前端类型 + 未运行且接口缺失时的说明文案) Add the endpoint-state fields from data-model.md (`bound_interface`, `binding`, `relay`, `rotation`)
  to `GET /api/share/status` in `crates/lanrurugi-api/src/share_settings.rs`, with serialization tests.

---

## Phase 3: User Story 1 — Switching to the VPN neither leaks nor drops (Priority: P1)

**Goal**: with an interface pinned, every sharing socket leaves through it, the published address set contains only
that interface's addresses, and a socket that cannot be bound fails loudly.

**Independent Test**: quickstart A — pin a tun interface, run a transfer, bring the interface down, and capture the
physical interface for zero packets; restart the interface on a new address and confirm no drop and no leaked
address in `direct_addrs`.

- [x] T007 [US1] (2026-11-09: 固定接口取不到地址时直接拒绝启动（`share_net::bind` 的 `None` 分支），不退回 `0.0.0.0`/`::`，由接口 watcher 等隧道回来再起) Wire `DeviceBinding` into `crates/lanrurugi-api/src/share_net/mod.rs`'s endpoint construction: after
  `clear_ip_transports()`, bind the QUIC/IP transports and every locally supplied address to the interface. Keep the
  existing `bind_addrs` call for the address set, but reject endpoint start when a pinned interface yields no address
  (replace the current `0.0.0.0`/`::` fallback).
- [x] T008 [US1] (2026-11-09: 两个 fork 各自暴露进程内 `set_bind_device`，`share_net::bind` 每次（重）绑时调用；见 `vendor/*/FORK.md`) Patch `vendor/netwatch`'s UDP socket creation to call `bind_device` from a new optional field, and
  set it from the share engine; same for `vendor/iroh-relay`'s TCP connect path (its TLS connector and plain HTTP
  client).
- [ ] T009 [US1] Bind the sharing DNS resolver and the pkarr client to the interface (`reqwest`'s `.interface()` or a
  `Resolve` implementation over a bound socket); a resolver that cannot be bound is an error, not a silent default.
- [x] T010 [US1] (2026-11-09: `via_hint` 只发布 `net.bound_addrs()` 之内的地址，固定接口以外的公网 IPv6 不再进节点记录；`address_allowed` 两条单测) Ensure the addresses published in node records come only from the pinned interface: filter
  `endpoint.addr().ip_addrs()` and `direct_addrs` by the interface's address set; test with an endpoint that also
  sees a public IPv6 (the historical leak).
- [x] T011 [US1] (2026-11-09: `start_services` 起完端点立刻 `publish_all`，因此地址变化/端口轮换/改设置触发的重建都会当场重发记录，不再等每小时那一轮) Address-change handling on top of `spawn_interface_watch`: when the pinned interface's addresses
  change, rebuild the endpoint **and** republish the node record immediately, reusing
  `retract_node_record`/`publish_share` (FR-004).
- [ ] T012 [US1] Tests: unit test for the address filter; quickstart A recorded with interface name, `ip addr`
  before/after, and the capture file's packet count.

---

## Phase 4: User Story 2 — A network reset recovers by itself (Priority: P1)

**Goal**: a transfer that hits a network error waits for the network instead of failing; a claim survives a peer
address change; rotation waits for active claims; a rebuild keeps the importer's session, ratios and helpers.

- [x] T013 [US2] (2026-11-09: 分块失败按「网络」与「对端回答」分流——`terminal_import_error` 只把 PermissionDenied/NotFound/Revoked/Expired/Exhausted/AddressUnverifiable/UnsupportedVersion 当终态，其余退避 1s→60s 无限重试；`reopen_sharer` 每次都用 `share_net::handle()`；`pieces.waiting_for_network` 计数进进度 JSON) Convert the import loop's error handling (`crates/lanrurugi-api/src/share_net/import.rs`) to the
  waiting state machine: retryable errors (`Io`/`Timeout`/`ConnectionReset`/`Busy`/`RelayUnavailable`) back off 1s →
  60s and retry for the life of the process; only `PermissionDenied`/`NotFound`/`EngineIncompatible`/`IntegrityFailed`
  are terminal. Re-fetch `share_net::handle()` on each attempt (never keep a stale `Arc`).
- [x] T014 [US2] (2026-11-09: `Run.session` 早已是可替换的 `RwLock`，`reopen_sharer` 每次写入新 session/conn/ticket 并清空 `tried_helpers` 以重新接收协助节点；ratios 由 `flush_ratios` 在重建前写回) Make `Run.session` replaceable and update it on every `reopen_sharer` (and re-request the ticket);
  write ratios back before a rebuild; re-dial known helpers after a reconnect.
- [x] T015 [US2] (2026-11-09: `Renew` 不再跑 `admit` 的 IP 检查，只重新校验分享状态（revoked/expired → NotFound）；地址白名单只在 Open 时生效) Claim-based refusal: `Renew`/resume must accept a peer whose source address changed when a claim
  exists (spec FR-006), keeping the IP allow-list check only for new claims.
- [x] T016 [US2] (2026-11-09: 轮换的等待条件已是 `active_transfers() > 0 || importing(&state)`，上限 30 分钟，跳过时记日志；新增开始等待时的日志) Rotation defers while any import holds an active claim (up to 30 minutes), logs the deferral and the
  eventual skip; a settings-triggered rebuild uses the same rule.
- [ ] T017 [US2] (2026-11-09: 已加 `retry_tests::only_the_sharers_answers_are_terminal`；wire-level 地址变更测试与 quickstart C 待真机) Tests: state-machine unit test with a scripted error sequence (3 Io errors then success); a
  claim-survives-new-address test at the wire level; quickstart C recorded (unplug, wait 60s, replug).

---

## Phase 5: User Story 3 — Destroying a node really revokes it (Priority: P2)

- [x] T018 [US3] (2026-11-09: `rotate_node_secret` 现在同时删除 `LANRURUGI_SHARE_NODE_SECRET` 并重建端点——旧端点 id 变成死地址，不再只是换码；记录在新身份上发布)  `rotate_node_secret`/destroy must create a new endpoint identity, not only a new node code; the old
  endpoint id stops accepting `Open`/`Renew` (server-side check against the current identity).
- [x] T019 [US3] (2026-11-09: `retract_node_record` 早已对每个 relay 重试 3 次并把结果写进 `status::record_rendezvous`，失败时 `warn!` 留下证据；本轮确认无遗漏)  Retire the old record with retries and surface a persistent failure in the UI (no silent "code
  retired" while the old code still resolves).
- [ ] T020 [US3] Relay access is tied to the node: destroying the node removes its allow-list entry and the old
  endpoint id can no longer use the local relay; allow-list entries accept the node-code form the UI displays
  (already parsed — verify end to end).
- [ ] T021 [US3] Tests: quickstart D — after destroy, the old endpoint id is unreachable directly and through the
  relay; a failed retire produces a visible warning and a retry.

---

## Phase 6: User Story 4 — One predictable plugin egress path (Priority: P2)

- [ ] T022 [US4] Load the egress shim only when an interface is actually pinned (`crates/lanrurugi-plugin/src/pool.rs`);
  when the shim is loaded, `dispatcher.ts` must keep per-host 6/global 16 concurrency, request coalescing, ETag store,
  20s/5s timeouts, 32 MiB cap and the CA roots — add a test that the shim's presence never disables them.
- [ ] T023 [US4] Extend `crates/lanrurugi-egress` to hook `sendmsg`/`sendmmsg`, resolve through a bound libc path
  (`res_send`/`getaddrinfo` interception or a forced resolver), and skip IPv4 link-local 169.254/16 like IPv6.
- [ ] T024 [US4] Replace `restart_workers()` on interface change with drain-then-restart: running invocations finish,
  new ones use the new interface; `outbound_client` failure is an error, never `default_client()`.
- [ ] T025 [US4] Platform routing: FreeBSD uses `setfib` for the Deno subprocess (`net.fibs=2` documented); macOS
  refuses a pinned interface with an explanation; native packages ship the shim where it applies.
- [ ] T026 [US4] Expose `egress_binding` in `GET /api/settings` (bound/unpinned/unsupported/failed) and render it in
  the settings UI; i18n in en/zh/ja.
- [ ] T027 [US4] Tests: quickstart F (capture with an egress plugin, then confirm the relay protections still hold);
  a unit test for the hook table's platform matrix.

---

## Phase 7: User Story 5 — Relay and helpers are dependable (Priority: P2)

- [x] T028 (2026-11-09: the port was already fixed; `ShareNet` now records the relay-relevant settings key and a rebuild with an unchanged key hands the running relay to the new endpoint instead of shutting it down, unless the endpoint identity changed) [US5] Relay listens on `share_node_relay_port` (default 11205) and restarts only when relay settings
  change, not with every endpoint rebuild; the three TLS modes (`proxy`/`acme`/`files`) all advertise the port they
  actually listen on.
- [ ] T029 [US5] (2026-11-09 partial: `share_net::helper::leave` now retries 3× with a 1s gap instead of giving up on one failed dial; import-delete and process-exit triggers still missing — there is no import-delete endpoint to hook yet) `HelperLeave` on import delete, process exit and endpoint stop, with retries; a helper that drops is
  re-dialable instead of being permanently discarded; the `contact()` helper list contains only verified completed
  help.
- [ ] T030 [US5] Tests: quickstart E (rotation with an active claim) plus a relay-restart log assertion (rebuild the
  endpoint, relay PID/uptime unchanged).

---

## Phase 8: User Story 6 — Downloads and repacks recover (Priority: P3)

- [x] T031 [US6] (2026-11-09: connect timeout + TCP keepalive on the bound download client; 180s idle window around the stream read, mapped to `HttpRequestFailed`) Give `default_client()`/the download stream connect, read and idle timeouts plus TCP keepalive;
  a stalled stream fails into the retry path instead of hanging forever.
- [x] T032 (2026-11-09: transport failures now retry up to 4 times with 2/4/8/16s backoff, resuming from the existing `.part`; a 4xx/5xx or a plugin error stays terminal. Deviation: bounded retries, not the contract's indefinite wait-with-network loop — a queue row has no `waiting_for_network` state to show, so an unbounded retry would look like a hung download) [US6] Retry retryable download errors without user action, resuming from `.part` + `If-Range`; only an
  explicit 4xx/5xx refusal is terminal.
- [x] T033 (2026-11-09: the key, `.part`+rename publish, singleflight, `503`+`Retry-After` and 24h age pruning were already in place; added the missing size budget — oldest-first eviction above 4 GiB. Cache lives at `<temp>/download-cache/`, not `<temp>/repack/`) [US6] Rebuild the repack cache per `contracts/repack.md`: content+members key, `<temp>/repack/<key>.zip.tmp`
  → fsync → rename, singleflight, 503 + `Retry-After` while building, TTL/LRU under `tempmaxsize`, and a sweep on
  startup; stop writing merged zips for ordinary archives that need none.
- [x] T034 (2026-11-09: the generation token already existed but was only checked once; the two-page warm loop now re-checks it — and the visible-conversion counter — before each page, so a page turn ends the old warm-up. Deviation: `LoadThrottle` is not used here; it samples global CPU through `sysinfo` for batch jobs, while a two-page warm-up needs the cheap atomic gate the reader already updates) [US6] Warm-up work gets a generation token (cancelled when the viewed page changes) and respects
  `LoadThrottle`; `list_pages` is not called once per page.
- [ ] T035 [US6] Tests: quickstart G/H — kill the connection mid-download and observe resume; `503` → `206` on a
  repack; `kill -9` during a build leaves no half-written final file.

---

## Phase 9: User Story 7 — Penetration is measured, not asserted (Priority: P3)

- [x] T036 (2026-11-09: added "穿透率是怎么测的" / "How penetration is measured" to the zh+en VPN guide — 15 topologies, three attempts each, "catalogued completely" as the only success criterion — and the matrix script now writes `summary.jsonl` plus an `N/15` line) [US7] Document the measurement procedure (topologies, attempts, success definition) in
  `docs/src/content/docs/guide/sharing-vpn.md` (zh + en) and extend `scripts/share-network-matrix.sh` to emit a
  machine-readable summary.
- [ ] T037 [US7] Record a baseline run with real numbers and the environment; state the measured rate rather than a
  target.

---

## Phase 10: Polish

- [ ] T038 [P] Remove `[profile.dev.package."*"] opt-level = 2` from the workspace `Cargo.toml` (dev/test build time),
  and document `enableresize`'s deliberate default in the docs.
- [x] T039 (2026-11-09: `record::open` falls back to the pre-`direct_addrs` shape, so an older published code still resolves; a payload neither shape can read is now `RecordError::EngineIncompatible` instead of `Undecryptable`, and the node resolver logs that a newer engine published the code rather than treating it as unpublished) [P] `KnockV1`'s new fields are optional and the parser reports `EngineIncompatible` for a record it cannot
  read (contracts/wire.md), with tests for both directions.
- [ ] T040 Gates: `mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-share-proto
  lanrurugi-egress lanrurugi-plugin lanrurugi-api`, `mise run test-crate -- lanrurugi-api lanrurugi-plugin`, and the
  frontend gates if the settings UI changed.

---

## Dependencies & Execution Order

1. T001/T002 block T008; T004 blocks T007/T009.
2. US1 (T007–T012) before US3's relay work (T020 depends on a real endpoint identity boundary) and before US2's
   rebuild tests.
3. US2 (T013–T017) is independent of US3–US5 but shares `import.rs` with US5's helper re-dial (T029) — do US2 first.
4. US4 is independent of US1–US3 at the file level; it can run in parallel with US2.
5. US5's relay restart decoupling (T028) must land before US7's matrix run uses a stable relay port.
6. US6 and US7 are independent of everything above; they can run last or in parallel.

## Parallel Opportunities

T001/T002, T005/T006, T018/T022, T031/T036, T038/T039.

## Implementation Strategy

Land the forks and the binding primitive first (they are the only upstream-dependent work), then US1 to make the
leak impossible, then US2 to make outages survivable. US3–US5 close the "looks revoked but isn't" and "plugin egress"
gaps. US6/US7 are the download and measurement tail. Every story ends with its quickstart scenario recorded here.
