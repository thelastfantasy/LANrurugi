# Feature Specification: Share Network Hardening

**Feature Branch**: `022-share-network-hardening`

**Created**: 2026-11-08

**Status**: Draft

**Input**: Review `share-pr-review.md` chapters 1–13 (VPN interface binding, penetration, codes, rotation, relay, recovery, plugin egress, conversion), plus the user's decisions of 2026-11-08: `1.4.2` must be implemented by forking `netwatch`/`iroh-relay`; the 99% penetration figure and relay join/leave belong here.

## Context

Sharing grew from "two instances can import from each other" into a network stack with its own contracts, and the review found that several of those contracts are not met. The failures cluster into five groups:

1. **Interface binding is incomplete.** Pinning a network interface binds the QUIC endpoint's addresses, but iroh's relay/TCP path, the DNS resolver, the pkarr client and the plugin egress path each build their own sockets. `share_net` now clears iroh's default transports and binds only the pinned interface, so the blatant leak (a public IPv6 address appearing in `direct_addrs`) is gone, but traffic that is not iroh's UDP socket can still leave through the physical link. The user's decision is to fork `netwatch` and `iroh-relay` and set `SO_BINDTODEVICE` on those sockets via `[patch.crates-io]` — this is a dependency-level change, not a configuration one.
2. **The endpoint does not follow the network.** A 15-second watch rebinds when the interface's addresses change, and a 600-second window rides out short interruptions, but past that window every error is terminal, a changed public address is only republished on the next scheduled record refresh, and a rebuild loses state the transfer needs (the importer's session, the ratio counters, the helper connections).
3. **Destroy and rotation are weaker than they look.** "Destroy and rebuild" changes the node code but not the endpoint identity, so anyone holding the old endpoint id or relay address can still connect; the relay stays open to them. Rotation of the UDP port can cut an active import because the "active transfer" guard only covers part of the response. The relay port is restarted together with the endpoint even when only the endpoint changed, and its allow-list entry format is not what the UI shows.
4. **Plugin egress is unsafe in both directions.** The egress shim is preloaded whenever its shared object exists — even when no interface is pinned — and its presence disables the plugin runtime's own relay protections (per-host concurrency, request coalescing, ETag revalidation, timeouts, size cap, certificate roots). When the interface is pinned, switching it kills every running plugin worker, and if the bound client cannot be built the download code silently falls back to an unbound one. The shim itself only intercepts `connect`/`sendto`, so `sendmsg` and glibc's internal resolver escape.
5. **Downloads and share downloads treat network errors as final.** A dropped connection becomes a queue error that needs a manual retry; the HTTP clients have no connect/read timeout and no keepalive, so a silently dead connection can hang forever. The repack cache used by tankoubon/patch downloads writes directly to its final path and treats "the file exists" as "the file is complete", which under concurrent range requests serves a half-written archive.

The 99% penetration figure is currently a property of a 15-topology test script rather than a measured outcome, and relay join/leave is only partially implemented (leaving an import, exiting the process and stopping an endpoint do not tell the helpers).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Switching the VPN never leaks and never kills a transfer (Priority: P1)

An administrator pins sharing to a VPN interface and switches VPN servers while an import is running, then switches again. No packet from any sharing-related socket — QUIC, relay TCP, DNS, pkarr — leaves through the physical interface; the import neither fails nor starts over; once the new interface is up, transfers continue.

**Why this priority**: leaking is the one failure with a privacy consequence, and killing an in-flight import is the most visible one.

**Independent Test**: with a pinned tunnel and a capture on the physical interface, run an import, switch the tunnel, and confirm zero sharing traffic on the physical link and a completing import.

**Acceptance Scenarios**:

1. **Given** a pinned interface, **When** the interface disappears, **Then** sharing stops advertising and no socket is created on another interface.
2. **Given** a pinned interface that returns with a new address, **When** the watch notices, **Then** the endpoint is rebuilt on the new address and the import resumes without a manual action.
3. **Given** an import running during a switch, **When** the switch completes within the recovery window, **Then** the import completes with the correct bytes.
4. **Given** an interface that does not exist, **When** the user saves it, **Then** the save is rejected rather than silently binding everything.

---

### User Story 2 - Network interruptions are waited out, not treated as failures (Priority: P1)

A home connection drops for two minutes, the router reconnects with a new public IP, and the import resumes on its own. No error is shown to the user; the activity log records the wait.

**Why this priority**: this is the ordinary case for a self-hosted instance, and today it fails permanently after ten minutes.

**Independent Test**: block outbound traffic on the sharing interface for longer than the recovery window, restore it with a different public address, and confirm the import completes.

**Acceptance Scenarios**:

1. **Given** a transfer interrupted by a network error, **When** the network returns, **Then** the transfer resumes from its last verified position.
2. **Given** a changed public address, **When** it is observed, **Then** the node record is republished immediately rather than at the next periodic refresh.
3. **Given** an endpoint rebuild, **When** the rebuild completes, **Then** the importer's session, the ratio counters and known helpers are still available.

---

### User Story 3 - Destroying a node actually revokes access (Priority: P2)

An administrator destroys and rebuilds a node because its code leaked. Anyone who knew the old endpoint id, relay address or code can no longer connect, and the local relay no longer serves them.

**Why this priority**: "destroy" that does not revoke is worse than no destroy, because the user believes the leak is contained.

**Independent Test**: with a second instance holding the old id/address, destroy and rebuild, then attempt a connection and confirm refusal.

**Acceptance Scenarios**:

1. **Given a destroyed node, When a peer tries the old endpoint id or relay address, Then the connection is refused.
2. **Given a destroyed node, When the retirement record is published and one relay fails, Then it is retried and the failure is surfaced.
3. **Given a relay allow-list entry shown in the UI as a node code, When it is saved, Then the relay accepts that peer.

---

### User Story 4 - A rotating port does not cut an active transfer (Priority: P2)

The UDP port rotates while an import is running. The rotation waits for the import to finish or reach a safe point, then happens; if the import is long, the rotation is skipped with a reason instead of severing it.

**Why this priority**: rotation is on by default now, so the failure would be routine.

**Independent Test**: start a long import, trigger rotation, and confirm the import completes and the port changes afterward.

**Acceptance Scenarios**:

1. **Given an active import, When rotation is due, Then it is deferred until the import has no active claim.
2. **Given a deferred rotation longer than the maximum wait, Then it is skipped, logged, and retried on the next interval.

---

### User Story 5 - Plugin downloads use exactly one exit path (Priority: P2)

An administrator pins plugins to a VPN interface. Plugin downloads go through the pinned interface; plugin HTTP keeps the per-host concurrency, coalescing, revalidation, timeout and size protections it has today; switching the interface does not destroy a running plugin invocation.

**Why this priority**: the two failure modes here are a leak (wrong interface) and a silent regression of protections the plugin runtime was built to provide.

**Independent Test**: pin an interface, run a plugin download while a second download to the same host is in flight, and confirm both share one connection budget and both leave through the pinned interface; switch the interface mid-run and confirm the invocation completes.

**Acceptance Scenarios**:

1. **Given no pinned interface, When plugins run, Then the egress shim is not preloaded and the runtime's own protections are active.
2. **Given a pinned interface whose client cannot be built, When a plugin download starts, Then it fails with a clear error instead of using the default client.
3. **Given a pinned interface, When the user switches it, Then running plugin invocations are drained, not killed, and new ones use the new interface.

---

### User Story 6 - Downloads recover from network errors without the user (Priority: P3)

A library download loses its connection for a few seconds. It reconnects and continues from the last byte; if the server does not support ranges, it restarts once and says so. A connection that dies silently is detected and retried rather than hanging forever.

**Why this priority**: it is the same class of failure as story 2, on the HTTP side, and it is the most common user-visible defect after a router reboot.

**Independent Test**: kill the connection mid-download on the test server, then restore it, and confirm the download finishes without a manual retry.

**Acceptance Scenarios**:

1. **Given a download interrupted by a network error, When the network returns, Then it resumes with `Range`/`If-Range`.
2. **Given a server that ignores range requests, Then the download restarts once and reports it.
3. **Given a stalled connection, When the idle timeout elapses, Then the download is retried rather than hanging.

---

### User Story 7 - Penetration is measured, not asserted (Priority: P3)

The project can state a success rate for "two instances with no public address can import from each other", measured across real network conditions, and knows what happens when each mechanism fails.

**Why this priority**: the 99% target is a user requirement, and today it is supported only by a topology script.

**Independent Test**: run the measurement on at least three real ISP/NAT combinations and produce a table of success rates and fallbacks.

**Acceptance Scenarios**:

1. **Given the measurement run, Then each topology reports success/failure, time to first byte, and which mechanism was used.
2. **Given a topology where direct and relay both fail, Then the user sees a specific diagnosis rather than a generic timeout.

---

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every socket the sharing stack creates MUST be bound to the pinned interface when one is configured, including the QUIC endpoint, relay/TCP connections, the DNS resolver used for sharing, and the pkarr client; a socket that cannot be bound MUST fail, never fall back to the default interface.
- **FR-002**: With a pinned interface, the addresses the node publishes MUST contain only addresses belonging to that interface.
- **FR-003**: A network error during a transfer MUST be treated as retryable with backoff for as long as the process runs, unless the remote explicitly refuses; only an explicit protocol refusal is terminal.
- **FR-004**: A changed public address MUST trigger an immediate node-record republication and an endpoint rebuild where required.
- **FR-005**: Rebuilding an endpoint MUST preserve the importer's session, ratio counters and known helpers.
- **FR-006**: An import whose claim exists MUST NOT be refused because the peer's source address changed.
- **FR-007**: Port rotation MUST defer while any import holds an active claim, and MUST log a skip when it gives up.
- **FR-008**: Destroying and rebuilding a node MUST make the previous endpoint identity unreachable, including through the local relay.
- **FR-009**: Retirement records MUST be retried and their failure surfaced to the user.
- **FR-010**: The local relay MUST listen on a fixed, configured port by default and MUST be restarted only when relay settings change.
- **FR-011**: The relay allow-list MUST accept the node-code form shown in the UI, and a destroyed node MUST remove its relay access.
- **FR-012**: Helper departure MUST be sent when an import is deleted, when the process exits and when an endpoint stops, with retries.
- **FR-013**: The plugin egress shim MUST be loaded only when an interface is pinned.
- **FR-014**: Pinning or switching an interface MUST NOT destroy running plugin invocations; they are allowed to finish and new ones use the new interface.
- **FR-015**: Failure to build an interface-bound HTTP client MUST be an error, never a silent unbound fallback.
- **FR-016**: The egress shim MUST cover datagram sends (`sendmsg`/`sendmmsg`) and name resolution performed through libc, and MUST skip IPv4 link-local destinations exactly as it skips IPv6.
- **FR-017**: Platforms that cannot bind per interface MUST either use the platform mechanism (FreeBSD FIB) or refuse the configuration with an explanation.
- **FR-018**: Downloads MUST have connect, read and idle timeouts, and MUST retry retryable network errors without user action.
- **FR-019**: The repack download cache MUST be written to a temporary path and atomically published, MUST be keyed by the archive's content plus member order and names, MUST live under the configured temp directory and count against its limit, and MUST be reclaimed.
- **FR-020**: A Range request for a repack that is not yet published MUST receive a retryable response (503 with `Retry-After`), never a partial file.
- **FR-021**: Visible-page conversion MUST respect the configured concurrency cap, and warm-up work MUST be cancellable when the viewed page changes and MUST respect the load throttle.
- **FR-022**: Node records MUST remain readable across engine versions that only add optional fields; an unreadable record MUST be reported as an incompatibility, not as "the code was never published".
- **FR-023**: The penetration target MUST have a measurement procedure and a recorded baseline; the project MUST publish the measured rate with its methodology.

### Key Entities

- **Pinned interface**: the network interface sharing traffic must use; its absence means "all interfaces".
- **Endpoint**: the iroh node identity and its bound sockets; rebuildable without changing the node code.
- **Claim**: the record that a peer has an active import of an archive, used to protect sessions and defer rotation.
- **Node record**: the published, signed record of a node's addresses and relay.
- **Repack cache entry**: a generated zip stored temporarily and keyed by content.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With a pinned interface, a packet capture on the physical interface during an import shows zero sharing packets.
- **SC-002**: An import survives a two-minute network outage and completes without user action.
- **SC-003**: An import survives two VPN server switches within the recovery window and completes.
- **SC-004**: After destroy/rebuild, 100% of attempts using the old identity are refused.
- **SC-005**: No import is cut by a scheduled rotation in a 7-day run.
- **SC-006**: Plugin downloads use the pinned interface in 100% of attempts when one is configured, and fail rather than leak when it cannot be bound.
- **SC-007**: A download interrupted mid-transfer completes without manual retry in at least 95% of runs on a flaky-link simulation.
- **SC-008**: The penetration measurement reports a success rate per topology, with the current baseline recorded.

## Assumptions

- The user runs Linux for full interface binding; FreeBSD uses FIB; macOS cannot bind per interface and is documented as unsupported for this feature.
- Forking `netwatch` and `iroh-relay` is acceptable and will be pinned through `[patch.crates-io]`; the forks only add a device-binding option.
- "Active claim" is the existing claim mechanism; no new transfer-tracking store is introduced.
- HTTP (library, share pages, downloads) intentionally keeps using the default route; only P2P sharing and plugin downloads use the pinned interface.
- The plugin runtime's relay-mode protections are the reference behaviour; the egress mode must not weaken them.
- The 99% target is measured on real networks by the operator or CI runner, not on a synthetic loopback.
