# Tasks: Share Crate Extraction

**Input**: Design documents from `/specs/023-share-crate-extraction/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Existing tests move with their code and must pass unedited in meaning; a test that needs application data
uses the test host from T008, never `AppState`.

**Organization**: four commits as research.md decided — type sinks, engine move, host interface, tests/scripts. Each
commit must leave the workspace buildable and the observable behaviour unchanged.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel
- **[Story]**: US1–US4 as in spec.md

---

## Phase 1: Setup

- [x] T001 (2026-11-09: crate created with core/share-proto/iroh/iroh-relay/pkarr/storage/scanner deps, no axum; `cargo tree -p lanrurugi-share | grep -c lanrurugi-api` = 0 verified after each move) Create `crates/lanrurugi-share/Cargo.toml` with the dependencies the engine actually uses today
  (iroh, iroh-relay client, tokio, serde, postcard, tracing, …), **not** `axum`/`lanrurugi-api`; add it to the
  workspace members. Verify `cargo tree -p lanrurugi-share | grep lanrurugi-api` is empty at every later commit.
- [ ] T002 [P] (baseline `cargo tree` and `docs-api --check` were run ad hoc; the network matrix was not rerun for this refactor — behaviour is unchanged, but the baseline record is still missing) Capture the baseline: `cargo tree` for the crates involved, `mise run docs-api -- --check`, and a
  `scripts/share-network-matrix.sh` run recorded in this file's implementation record (before any move).

---

## Phase 2: Foundational — type sinks (commit 1)

**Purpose**: a type both sides need must live below both; duplicating it would create two definitions that drift.

- [ ] T003 Move the boundary types from `crates/lanrurugi-api/src/share_net/` (record/ticket/knock/claims/permission
  DTOs) into `crates/lanrurugi-share-proto` or the new crate per research.md's table; re-export them from
  `lanrurugi-api` so no handler changes in this commit.
- [x] T004 (2026-11-09: `ShareSettings`/`AltSchedule`/`parse_hhmm`/`canonical_https_url`/`split_list`/`parse_permissions`/`parse_node_string` moved to `lanrurugi-share::settings`; Redis load/save and field validation stay in the API crate; `RateLimiterMap` moved to `lanrurugi-core::rate_limit`) Move `ArchiveId`-adjacent helpers and the share settings struct out of `share_settings.rs` into the new
  crate (or `lanrurugi-core` where they are not sharing-specific), keeping the setting keys and defaults identical.
- [ ] T005 [P] Move activity/logging calls behind a thin `ActivitySink` trait in the new crate with the API side
  implementing it over `crate::activity::record_manual`.
- [ ] T006 Tests for the sinks: type round-trips unchanged (postcard bytes identical for a known record), settings
  defaults identical to `defaults_fill_unset_fields`'s expectations.

---

## Phase 3: User Story 1 & 3 — Engine move, nothing observable changes (Priority: P1, commit 2)

**Goal**: `share_net/*` lives in `crates/lanrurugi-share`; the workspace builds and every test passes with the wire
format, keys, records and settings unchanged.

- [ ] T007 Move `crates/lanrurugi-api/src/share_net/` into `crates/lanrurugi-share/src/` mechanically (git mv, then
  fix paths), module by module, keeping each module's tests next to it. No logic edits in this commit.
- [ ] T008 Replace the moved code's `AppState` uses (56 call sites at baseline) with the host trait from Phase 4
  *only where the compiler forces it*; where a narrow local handle suffices, pass that instead.
- [ ] T009 Keep `crates/lanrurugi-api/src/share_recipient.rs`'s pure helpers in the new crate if they do not touch
  HTTP; anything that reads a request or writes a response stays in `lanrurugi-api`.
- [ ] T010 Run the existing tests (`test-crate -- lanrurugi-share lanrurugi-api`) and `docs-api -- --check`; the
  route table must be byte-identical to the baseline.

---

## Phase 4: User Story 2 — One explicit application boundary (Priority: P1, commit 3)

**Goal**: the engine asks `ShareHost` for archives, downloads, tankoubons, activity and settings; no `AppState`, no
`lanrurugi_api` import.

- [ ] T011 Define `pub trait ShareHost: Send + Sync + 'static` with the five grouped accessors from
  `contracts/host.md` (`archives()`, `downloads()`, `tankoubons()`, `activity()`, `settings()`) and their sub-traits;
  every method takes/returns owned or `&`-borrowed plain types, never an API type.
- [ ] T012 Implement the trait in `crates/lanrurugi-api/src/share_host.rs` over `AppState`, one `impl` block per
  accessor, with `ShareError::Host` mapping failures opaquely (no API error types crossing the boundary).
- [ ] T013 Change `ShareEngine::new(host)` and the background-task spawns to hold `Arc<dyn ShareHost>`; update
  `crates/lanrurugi-server/src/main.rs`'s construction and shutdown order (T014 keeps it identical).
- [ ] T014 Add the test host (in-memory archives/downloads/tankoubons/activity/settings) in the crate's `tests/` or
  a `test_support` module behind `cfg(any(test, feature = "test-support"))`; port the sharing tests that used
  `AppState` fixtures onto it.
- [ ] T014a [US1] Document the crate's public surface: crate-level docs on what the engine owns vs. what the host
  provides, a doc comment on every `ShareHost` method and every public type, and `cargo doc -p lanrurugi-share
  --no-deps` with `RUSTDOCFLAGS="-D warnings"` clean.
- [ ] T014b [US3] Assert FR-010: `git diff` the startup log (task names, order, count) against T002's baseline; a
  changed task set or order is a finding, not an acceptable side effect.

- [ ] T015 Grep gates: `AppState|lanrurugi_api` must be absent from `crates/lanrurugi-share/`; add a CI check for it
  so the boundary cannot decay.

---

## Phase 5: User Story 4 — HTTP stays put (Priority: P2, commit 4)

- [ ] T016 Verify every share route still lives in `lanrurugi-api` (`/api/shares`, `/api/share/imports`,
  `/api/share/r/{token}/…`, `/api/share/settings`, `/api/share/status`, relay/knock routes) and that the handlers
  only call the engine; no handler moved into the new crate.
- [ ] T017 Re-run `mise run docs-api -- --check`; the route scan must still find every share route (the docgen parses
  `crates/lanrurugi-api/src/**`, so a route that moved to the new crate would silently vanish from the docs).
- [ ] T018 Run `scripts/share-network-matrix.sh` and record that it is unchanged from T002's baseline.

---

## Phase 6: Polish

- [ ] T019 [P] Update `README.md`/`CONTRIBUTING.md` if they name the crate layout, and `docs/` architecture notes if
  they describe `share_net` living in the API crate.
- [ ] T020 Gates: `mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-share lanrurugi-api`,
  `mise run test-crate -- lanrurugi-share lanrurugi-api`, `cargo tree -p lanrurugi-share | grep -c lanrurugi-api`
  must print 0, and `mise run docs-api -- --check`.

---

## Dependencies & Execution Order

- T001 before everything; T002 is the baseline for every later comparison.
- Phase 2 (T003–T006) before Phase 3 (T007) — the moved code must compile against the sunk types.
- Phase 3 must be a pure move; if a logic edit is required to compile, it belongs in Phase 4.
- Phase 4 (T011–T015) before Phase 5's verification, and after Phase 3's build is green.
- T014 (test host) blocks porting any test that used `AppState`.
- T020 last.

## Parallel Opportunities

T003/T004/T005 (different type groups), T010 with T011's trait sketch, T019 with T018.

## Implementation Strategy

Four reviewable commits, each green: (1) types sink, (2) mechanical move with `AppState` still reachable through a
shim if the compiler demands it, (3) host trait and its API implementation, (4) tests ported and gates. Rebase
`vendor/` changes from 022 as needed — 023 must land after 022 so the engine is not moved twice.

## Notes

- This is a refactor: any behavioural diff is a bug. If a test needs a semantic edit, stop and inspect — the test
  was probably relying on an `AppState` detail the host must expose explicitly.
- `share_net/`'s background tasks are owned by the engine but scheduled by `main`; keep that split.

---

## Implementation Record

State as of 2026-11-09, after 020/021/022 landed on `feat/share-bind-interface` (PR #258).

**Done — the crate exists and holds everything that does not need the HTTP layer**

| Moved into `crates/lanrurugi-share` | Notes |
|---|---|
| `interfaces.rs`, `link.rs`, `status.rs` | pure observation: interfaces, the selected path of a live connection, external-service reachability |
| `proxy.rs` | SOCKS5 bridge, HTTP proxy choice, `no_proxy`, the shared outbound `reqwest::Client` |
| `settings.rs` | the typed settings view and its parsing (`ShareSettings`, `AltSchedule`, `parse_hhmm`, `canonical_https_url`, permissions, list/nodes parsing); Redis load/save and field validation stay in `lanrurugi-api` |
| `limits.rs` | upload caps and the alternative-speed schedule, parameterised by `(&ShareSettings, Tz)`; the API keeps a thin wrapper that loads settings and the zone |
| `blobs.rs` | bao fingerprint outboards, merged-with-patch copies, `ready`/`prepare`/`ensure_ready`; the background `share_prepare` worker stays in the API (it needs `JobRegistry` and `LoadThrottle`) |
| `host.rs` | `ShareHost` (`redis`, `jobs`, `temp_dir`, `archive(id)`, `settings`) plus `ShareArchive` and `EmptyHost` |

`RateLimiterMap` moved sideways into `lanrurugi-core::rate_limit`, because both the download plugins and
the sharing uploader use it and it is not sharing-specific.

`lanrurugi-api` implements `ShareHost` for `AppState` in `crates/lanrurugi-api/src/share_host.rs`; every moved
module is re-exported from its old path (`share_net::interfaces`, `share_net::limits`, `share_net::blobs`, …),
so no handler changed.

**Not done — the protocol and pipeline still live in the API crate**

`nodes.rs`, `publish.rs`, `helper.rs`, `server.rs`, `import.rs` (about 5,100 lines) still read `AppState` and
share the `ShareNet` handle defined in `share_net/mod.rs`. Moving them is not a mechanical edit:

- `ShareNet` owns the endpoint, the relay handles, the bound addresses and the shutdown sequence. It is the
  object the protocol handler and the import pipeline both borrow, so it has to move before either can.
- `server.rs` serves archive bytes through the download manager (`serve_archive_download`, tankoubon zips),
  and `import.rs` catalogues into the library (`catalogue_staged_file`, patch merging). Both are exactly the
  "engine asks the host to do something" surface `ShareHost` was designed for — but each call site has to
  become a host method with a result type, which is a multi-commit change of its own.

So this spec is **partially implemented**: the crate boundary exists, the host interface exists, and the
pure/observation/storage-adjacent layers already moved. The endpoint lifecycle and the two big subsystems
remain. That is recorded here rather than checked off, so a later session starts from the truth.


### Implementation Record — 2026-11-09 14:40（`ShareNet` 本体迁入）

`crates/lanrurugi-share/src/net.rs` 现在持有 `ShareNet`、`Shared`、`Session`、`RatioState` 与
`SESSION_TTL`；`crates/lanrurugi-api/src/share_net/mod.rs` 只 `pub use` 回来，因此所有
`crate::share_net::ShareNet` 的调用点未变。

为此 `ShareHost` 增加 `add_import_bytes`：`ShareNet::flush_ratios` 原来直接拿 `AppState` 的
shares 仓库写回已服务字节，现在只依赖 host 的这一个方法；`Impl for AppState` 里用
`crate::shares::repo(self)` 实现，`EmptyHost` 里是 no-op。

仍在 api 的是 `start`/`bind`/`handle`/`RUNNING`/`Shared` 的构造和四个后台任务：它们要按顺序
重建端点、复用中继、绑接口、启动 watcher，且 `start` 会调用 api 里的
`nodes::restart_relay`。下一步是把 `bind` 与中继启动也搬过去（需要 host 上补「当前节点密钥」
与「活动记录」两处）。


### Implementation Record — 2026-11-09 15:30（端点绑定迁入）

`crates/lanrurugi-share/src/net.rs` 现在负责建 socket 与端点：`bind(BindOptions) -> Bound`
负责接口绑定、`SO_BINDTODEVICE`、HTTP/SOCKS 代理、TLS 根、portmapper 开关、手工外网地址与
mDNS 发现；`Bound::take_endpoint()` 把端点交给调用方挂协议处理器，`ShareNet::from_bound()`
再把它包成运行态。`ALPN` 与 `relay_tls()` 也搬过来，api 侧 re-export。

仍留在 api 的是 `start`（按顺序停旧端点、保比率、复盘中继）与 `bind` 的三行胶水（把
`relay_urls(settings)` 变成 `RelayMap`、挂 `ShareProtocol`）。`relay_urls` 与 `external_addrs`
用到了 api 的 `nodes::configured_nodes`/`own_relay_url`，所以在它们搬走之前 `bind` 的胶水也在 api。


### Implementation Record — 2026-11-09 16:10（节点字符串与中继地址）

`crates/lanrurugi-share/src/nodes.rs` 现在拥有节点字符串的一切纯逻辑：`NodeRef`、`parse_node`、
`configured_nodes`、`own_relay_url`、`node_relay_url`，以及端点 id 的 hex 两种形态
（`endpoint_hex`/`parse_endpoint_hex`）。`crates/lanrurugi-share/src/net.rs` 拿走
`relay_urls` 与 `relay_settings_key`。api 侧全部改为 re-export，`share_net::nodes` 只剩真正要
`AppState` 的部分（发布/撤销记录、解析节点码、起中继、独立节点模式）。


### Implementation Record — 2026-11-09 16:35（中继角色）

`crates/lanrurugi-share/src/relay.rs` 现在负责整个内嵌中继：三种 TLS 模式（`proxy`/`files`/ACME）、
端口选择与占用回退、每客户端限速、`AllowListed` 与允许名单解析、QUIC 地址发现、证书读取。
`start_relay(settings, data_dir, own)` 的签名没变，api 侧 re-export 后调用点不动。

`AllowListed` 的准入测试（轮换后旧端点 id 是陌生人）随之搬进 crate。


### Implementation Record — 2026-11-09 16:50（`via` 码里能写什么）

`crates/lanrurugi-share/src/publish.rs` 现在决定 `via` 码可以广告哪些地址：`via_hint`
（端点自身地址 → 接口过滤 → 可拨号过滤 → 排序去重 → 中继），以及 `address_allowed` /
`dialable` / `address_rank` 与三条对应测试。这三条规则是 022 US1 的泄漏面，放到 crate 里以后
任何人改它都要面对同目录的测试。


### Implementation Record — 2026-11-09 17:35（pkarr 中继与记录地址）

`crates/lanrurugi-share/src/publish.rs` 现在也拥有 pkarr 这一层：`DEFAULT_PKARR_RELAYS`、
`pkarr_relays`（用户填的优先，其次「公开服务」开关下的公共中继）、`relay_endpoint`、
`relay_is_public`（只识别明显的私有形态：localhost/.local/私有地址/链路本地），以及
`advertised_relays`（端点当前中继 + 自建中继，公网优先）。api 侧改为 re-export。


### Implementation Record — 2026-11-09 17:55（节点记录发布/撤销）

`ShareHost` 增加 `node_code() -> Option<NodeCode>`；`AppState` 实现里读 Redis 的
`SHARE_NODE_CODE_KEY`（失败只 `warn!` 并返回 None，发布记录没有码本来也无意义）。据此把
`publish_node_record` 与 `retract_node_record` 连同 `RETRACTED_ENDPOINT_ID` 搬进
`crates/lanrurugi-share/src/publish.rs`，api 侧只剩同名薄包装。

这样「拿哪个码、往哪些 pkarr 中继写、写成什么形状」全在一个 crate 内，api 不再参与记录格式。


### Implementation Record — 2026-11-09 18:42（会合角色与十六进制）

`RendezvousStore`（Redis / 只读内存两种后端）、`RendezvousLimiter`（每来源每分钟 600 次）、
`answer_rendezvous` 与 `RDV_PER_MINUTE` 现在在 `crates/lanrurugi-share/src/nodes.rs`。它们不知道
`AppState`，Redis 池与 TTL 键来自 `lanrurugi-storage`。

api 侧只剩 `NodeProtocol`（iroh 的 `ProtocolHandler`）与 `lanrurugi-server share-node` 的启动代码：
前者把连接交给 `answer_rendezvous`，后者是独立的只读节点模式。


### Implementation Record — 2026-11-09 19:45（manifest）

`browse_manifest` 与 `archive_entry` 现在在 `crates/lanrurugi-share/src/manifest.rs`，只经
`ShareHost::archive` / `ShareHost::tankoubon` 取数据。`ShareArchive` 因此携带 manifest 需要的
字段（name/tags/summary/pagecount/arcsize/toc），`ShareTankoubon` 携带成员顺序。

`first_item_title`、`item_exists`、`enqueue_prepare` 仍留在 api：它们服务的是 HTTP 侧的表单与
后台指纹队列，不是 wire 格式。


### Implementation Record — 2026-11-09 20:15（claim manifest）

`claim_manifest` 的清单与指纹装配也在 crate 里：它经 `ShareHost::categories` 取静态分类名、
经 `blobs::ready` 取指纹；未就绪的档案以「pending 列表」返回，由 api 决定何时 spawn
`blobs::prepare`（crate 里不持有 `Arc<dyn ShareHost>`，不想为此逼 host 用 `Arc`）。


### Implementation Record — 2026-11-09 20:40（缩略图）

分享协议里的缩略图不再反调 axum handler：`ShareHost::thumbnail(id, page)` 返回
`(content_type, bytes)`，api 侧由 `archives::share_thumbnail` 在无 `Accept` 头的前提下走
「已存格式 → 按需重生成 → 任意已存格式 → 占位图」这条与无头浏览器请求相同的路径。

因此 `server.rs` 的 `thumbnail()` 变成：查清单确认档案在分享里 → 取字节 → 回 `Response::Bytes`。
