# Phase 0 — Research: Share Network Hardening

## R1. Binding is done on the socket, via forked crates

**Decision**: fork `netwatch` (iroh's UDP sockets) and `iroh-relay` (the TCP/TLS client) and add a device-binding option
that sets `SO_BINDTODEVICE` before `bind`/`connect`; pin both with `[patch.crates-io]` at the versions already in
`Cargo.lock`.

**Rationale**: the reviewer's finding is that binding a source address does not constrain the route on a weak-host
kernel, and the endpoint's own transports are created inside the crates, so there is no injection point otherwise.

**Alternatives considered**: source-address binding only (already done, insufficient); policy routing (needs root and
host configuration the container may not have); running the whole process in the VPN namespace (works only for some
deployments and takes HTTP with it, which the user does not want).

**Cost**: two vendored crates. Mitigation: the diff is one option, pinned versions, and an upgrade note in the plan
for the next iroh bump.

## R2. DNS for sharing goes through the bound path

**Decision**: the share engine's resolver uses a UDP socket created by the same binding helper; the egress shim also
binds on first `sendto`/`sendmsg` for a socket that is not yet bound. If the pinned resolver cannot accept a custom
socket, the platform requirement (per-link DNS on the tunnel) is documented and the resolver is reported as not pinned
in the status view.

**Rationale**: glibc's internal resolver bypasses `connect` interception; binding at first send is the narrowest hook
that covers it without redirecting every DNS query in the process.

## R3. Retry policy: infinite with capped backoff, terminal only on refusal

**Decision**: network errors put a transfer into `waiting_for_network` and retry with exponential backoff capped at
60 s, reset on success, for the lifetime of the job; only an explicit protocol refusal (permission denied, not found,
incompatible engine, integrity failure) is terminal. The user can cancel.

**Rationale**: the old fixed 3-attempt/1–2-second policy turned a two-minute outage into a permanent failure. The
distinction between "the network is gone" and "the peer said no" is the only one the user can act on.

## R4. Rotation is gated on claims, not on the transfer guard

**Decision**: a rotation is deferred while any import holds an active claim, checked every 10 s; after the maximum
wait (30 minutes) it is skipped with a logged reason and retried on the next interval.

**Rationale**: `TransferGuard` covers one response body, so it was almost never held; the claim is the durable record
that a peer is mid-import.

## R5. Destroy rotates the endpoint identity

**Decision**: "destroy and rebuild" rotates the endpoint secret (new endpoint id) as well as the node code, publishes
a retirement record for the old code with retries, and removes the old peer's access to the local relay.

**Rationale**: rotating only the node code leaves the old endpoint id and relay address connectable; a destroyed node
that still accepts connections is a false sense of revocation.

## R6. Relay: fixed port, decoupled restart, identity-bound access

**Decision**: the relay TCP port defaults to `11205` (fixed) and is restarted only when a relay setting changes;
endpoint rebuilds no longer restart it; the allow-list accepts node codes (already implemented) and a destroyed node's
access is removed.

**Rationale**: a random relay port breaks every reverse-proxy deployment (the proxy points at a fixed port) and makes
each endpoint rebuild a relay outage.

## R7. Helper departure is sent with retries

**Decision**: send `HelperLeave` on import deletion, process exit and endpoint stop, through a small retry queue; do
not block shutdown for more than a bounded time.

**Rationale**: peers keep a departed helper for up to 15 minutes; sending on the common paths is cheap and the retry
makes a transient relay failure harmless.

## R8. The egress shim loads only when an interface is pinned, and never disables relay protections

**Decision**: preload the shim only when `plugin_bind_interface`/`share_bind_interface` is set; the dispatcher always
keeps per-host concurrency, coalescing, ETag revalidation, timeouts and the size cap; the shim only adds binding.

**Rationale**: today the shim's mere presence switches the runtime into a mode that loses those protections — an
unrelated regression caused by a file existing.

## R9. Interface switch drains, it does not kill

**Decision**: on a switch, the pool stops accepting new invocations, lets in-flight ones finish (bounded by the
plugin's own timeout), then restarts workers on the new interface.

**Rationale**: killing workers turns every settings change into a failed login/import for whatever was running.

## R10. Client construction fails closed

**Decision**: if the interface-bound client cannot be built, the download fails with an error naming the interface; it
never falls back to the default client.

**Rationale**: the egress shim already fails closed; the download path doing the opposite is the inconsistency that
makes a leak possible.

## R11. Shim coverage: `sendmsg`/`sendmmsg`, and link-local parity

**Decision**: intercept `sendmsg`/`sendmmsg` like `sendto`; bind UDP sockets on first send; skip IPv4 link-local
(`169.254.0.0/16`) exactly as IPv6 link-local is skipped.

## R12. FreeBSD uses FIB; macOS refuses the configuration

**Decision**: on FreeBSD, plugin subprocesses are launched through `setfib` with a configured FIB; on macOS, pinning
is refused with an explanation (no per-interface binding without entitlements).

## R13. Downloads: timeouts, idle detection, automatic retry

**Decision**: HTTP clients get connect and read timeouts and TCP keepalive; the body stream is wrapped in an idle
timeout; a network error retries with `Range`/`If-Range`, and a server without range support gets one restart and a
message.

## R14. Repack cache: content key, atomic publish, bounded

**Decision**: key = archive id + sorted member `(path, size, mtime)` + entry names + compression mode + cache format
version. Write `<temp>/repack/<key>.zip.tmp`, fsync, rename to `<key>.zip`. Singleflight per key. A Range request for
an unpublished key gets 503 + `Retry-After: 5`. Entries are 24 h TTL with a total-size LRU bound, counted against
`tempmaxsize`, and reclaimed by the existing temp sweep.

**Rationale**: today the cache writes to the final path and treats existence as completeness, which serves half files
to multi-connection downloaders and leaves permanent garbage after a crash.

## R15. Conversion warm-up carries a generation token

**Decision**: warm-up work is tagged with the viewed archive/page generation; a generation change cancels queued
warm-ups; `list_pages` is cached per archive version; warm-ups respect `LoadThrottle`.

## R16. `enableresize` and dev `opt-level` stay as deliberate deviations — documented

**Decision**: keep `enableresize = true` (documented in the settings help as a deliberate difference from legacy) and
remove the dev-only `[profile.dev.package."*"] opt-level = 2` entry, which buys nothing for tests and slows every cold
build.

## R17. Record compatibility: additive fields, honest errors

**Decision**: `KnockV1`'s new fields are optional with serde defaults; `SHARE_ENGINE_VERSION` stays at 4; a record that
still cannot be parsed is reported as an engine incompatibility, never as "the code was never published".
