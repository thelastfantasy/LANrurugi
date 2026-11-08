# Implementation Plan: Share Network Hardening

**Branch**: `022-share-network-hardening` | **Date**: 2026-11-08 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/022-share-network-hardening/spec.md`

## Summary

Five workstreams, ordered by the damage each failure causes:

1. **Interface binding is completed at the socket.** Fork `netwatch` (iroh's UDP sockets) and `iroh-relay` (its TCP/TLS
   client) to set `SO_BINDTODEVICE` on the sockets they create, patch them in with `[patch.crates-io]`, and give the
   share engine's DNS and pkarr paths the same treatment. When an interface is pinned and cannot be bound, the
   operation fails; there is no fallback to the default route. Port-mapper and mDNS are disabled while pinned.
2. **Errors become "waiting for network".** A transfer that hits a network error retries with capped exponential
   backoff for as long as the process runs, re-reading the current share handle each attempt; only an explicit
   protocol refusal is terminal. The interface watch is extended from "the bound addresses changed" to "the interface
   is back / its address changed", and a changed public address republishes the node record immediately instead of at
   the next refresh.
3. **Rebuilds stop losing state.** The importer's session becomes replaceable and is updated on reopen; ratio counters
   are flushed before a rebuild; helpers are re-dialled after it; and rotation defers while any import holds an active
   claim instead of watching a partial transfer guard.
4. **Destroy means revoked.** Destroy rotates the endpoint identity (or cuts the relay), publishes a retirement record
   with retries, and removes the peer's relay access. The relay's port is fixed by default and its restart is
   decoupled from endpoint rebuilds; helper departure is sent on import deletion, process exit and endpoint stop, with
   retries.
5. **Plugin egress and downloads stop lying.** The shim loads only when an interface is pinned, interface switches
   drain rather than kill plugin work, client construction fails closed, the shim covers `sendmsg`/`sendmmsg` and libc
   resolution, and downloads gain connect/read/idle timeouts with automatic retry. The repack cache is written to a
   temporary path and atomically published, keyed by content, bounded and reclaimable.

`1.4.2` is the dependency-level part and is the only change that cannot be done in this repository alone; the forks
are pinned to the versions already in `Cargo.lock` and their diff is limited to a device-binding option.

## Technical Context

**Language/Version**: Rust 2021 (workspace toolchain), TypeScript 5.x + React 19 (status and controls only)

**Primary Dependencies**: iroh 1.4.1, `netwatch` 0.19.3, `iroh-relay` 1.4.1 (both patched), `tokio`, `reqwest`,
`hickory-resolver`, `socket2`, `libc`; existing `lanrurugi-egress` shim (C, LD_PRELOAD)

**Storage**: Redis for share settings, claims, node records and import state; library filesystem for bytes and the
repack cache under the configured temp directory

**Testing**: per-crate `mise run check-crate`/`test-crate`; the two-instance integration suite already used by 017;
`scripts/share-network-matrix.sh` (15 topologies) extended with interruption cases; a packet-capture test on the
physical interface; a flaky-link simulation for download retry

**Target Platform**: Linux (full binding), FreeBSD (FIB), macOS (unsupported for binding, documented); containers need
host networking or the VPN namespace to see the interface

**Project Type**: network service inside the server binary + settings/status UI

**Performance Goals**: the watcher runs every 15 s and costs one `getifaddrs`; retry backoff caps at 60 s; binding adds
one `setsockopt` per socket; the repack cache must serve a completed entry with no regeneration

**Constraints**: no new long-running process; no second datastore; wire compatibility with 017 peers that do not have
the new optional fields; the plugin runtime's relay-mode protections are the reference behaviour that egress mode must
not weaken

**Scale/Scope**: one node per instance, a handful of concurrent imports, one relay, two interfaces at most (physical
and VPN)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How this plan satisfies it |
|---|---|---|
| I. Legacy data & user-trust compatibility | PASS | No archive identity, setting meaning or record format changes. `KnockV1` gains only optional fields; an old-record read failure gets a truthful message instead of "code not published". |
| II. API contract fidelity | PASS | `/api/share/*` response shapes are additive (new status fields); the settings keys keep their names and gain documented defaults; repack responses keep 200/206 and add a documented 503+`Retry-After`. |
| III. Single-process, resource-conscious, genuinely concurrent | PASS | No new process. Retries are timer-driven wake-ups, not busy loops; the download retry shares the queue's existing concurrency budget; conversion warm-up stays inside the ~30% cap and the load throttle. |
| IV. Plugin extensibility | PASS | The plugin protocol and permissions are untouched; the egress shim is an existing transport mechanism, and the change is to load it only when configured and to stop weakening the runtime protections. |
| V. Secrets & network trust boundaries | PASS | The point of workstream 1 is that a pinned interface cannot leak; retire/relay access is bound to the current node identity; no credential is added. |
| VI. Phased scope discipline | PASS | Phase 1 sharing reliability; no OCR/translation/LLM concern. |
| VII. Frontend discipline & legacy fidelity | PASS | Only settings/status additions; strings through i18next (en/zh/ja); no new page. |
| Tech stack: Redis-only, newtype IDs, shared helpers | PASS | Claims and records stay in the existing Redis structures; no new store; types stay newtyped. |

**Complexity Tracking (justified)**: forking two upstream crates is a maintenance cost. It is the only way to set
`SO_BINDTODEVICE` on sockets the crates create themselves; the alternative — relying on source-address binding — does
not constrain the route on a weak-host kernel and was the reviewer's finding. The forks are version-pinned and their
diff is one option.

## Project Structure

### Documentation (this feature)

```text
specs/022-share-network-hardening/
├── plan.md
├── research.md          # binding mechanisms, retry policy, rotation semantics, cache design
├── data-model.md        # states: transfer, endpoint, rotation, relay allow-list, cache entry
├── quickstart.md        # capture, interruption, switch, destroy, rotation, egress, download
├── contracts/
│   ├── settings.md      # keys, defaults, validation
│   ├── wire.md          # KnockV1 additive fields and version rule
│   ├── repack.md        # cache and 503/Retry-After behaviour
│   └── egress.md        # when the shim loads, what it must intercept, failure mode
└── checklists/requirements.md
```

### Source Code (repository root)

```text
vendor/netwatch/              # NEW — fork, one bind_device option on the UDP socket constructor
vendor/iroh-relay/            # NEW — fork, one bind_device option on the TCP/TLS dialer
Cargo.toml                    # [patch.crates-io] entries, pinned to the current versions

crates/lanrurugi-api/src/share_net/
├── mod.rs                    # bind policy: fail closed; watcher; endpoint rebuild preserving state
├── interfaces.rs             # address-change detection (already), interface-gone/back events
├── import.rs                 # wait-for-network retry, replaceable session, helper re-dial
├── nodes.rs                  # immediate republish, retire retries, allow-list node codes
├── server.rs                 # TransferGuard/claim-aware rotation
└── relay.rs                  # fixed port, decoupled restart, access removal on destroy

crates/lanrurugi-egress/      # sendmsg/sendmmsg, resolver interception, link-local skip
crates/lanrurugi-plugin/src/pool.rs      # shim only when pinned; drain on switch
crates/lanrurugi-api/src/download_manager/{outbound.rs,stream.rs}  # fail closed, timeouts, retry
crates/lanrurugi-api/src/archives.rs     # repack cache: tmp+rename, content key, TTL/LRU, singleflight
crates/lanrurugi-api/src/share_settings.rs  # defaults, validation, relay port

apps/frontend/src/pages/Settings/SharingSection.tsx  # relay allow-list by node code, status text
```

**Structure Decision**: the network engine stays where it is (023 moves it later, and this feature must not depend on
that move); the forks live under `vendor/` so the diff is reviewable; egress stays a separate crate.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| Two forked upstream crates | `SO_BINDTODEVICE` must be set before `connect`/`bind` on sockets the crates create | Source-address binding does not pin the route on weak-host kernels (review 1.4/1.4.1); a proxy process cannot cover iroh's UDP |

## Phase 0 — Research (see [research.md](./research.md))

Decisions: fork-and-patch for binding; DNS pinned through a bound resolver or documented as a platform requirement;
retry-forever with capped backoff and explicit-refusal-only terminal errors; rotation gated on claims; destroy rotates
endpoint identity; relay port fixed by default; egress shim conditional; repack cache content-keyed and atomic.

## Phase 1 — Design (see [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md))

States: transfer (`active → waiting_for_network → active → done/failed_refused`), endpoint (`bound → rebuilding →
bound`), rotation (`idle → deferred → done/skipped`), relay (`serving → revoked`). Contracts: settings defaults and
validation, additive `KnockV1` fields, repack cache/503, egress loading and interception.

## Post-Design Constitution Re-check

Unchanged and still PASS. The design adds two vendored patches, one retry state machine, one cache rewrite and a set of
settings/status additions; it adds no process, no store and no protocol break.
