# Quickstart: Share Network Hardening

Each scenario produces a piece of evidence; the ones marked **capture** must be run with packet capture on the physical
interface (`tcpdump -ni <phys> host <peer>`), because "the UI said pinned" is not evidence.

## A. Nothing leaks while pinned (capture)

1. Pin `share_bind_interface=tun0`, start sharing, create a share, import it from a second instance.
2. Capture on the physical interface for the duration.
3. **Expected**: no packets to the peer's endpoint id, pkarr or relay from this process; all of it on `tun0`.
4. Break the tunnel (`ip link set tun0 down`), retry the import.
5. **Expected**: no packets on the physical interface at all; the import enters `waiting_for_network`.

## B. VPN switch mid-transfer

1. Start a 1 GB import.
2. Reconnect the VPN so the interface's address changes.
3. **Expected**: transfer pauses, endpoint rebinds to the new address, import resumes without a new user action; the node
   record shows the new address after at most one refresh interval (and immediately if the address-changed hook fires).

## C. Outage and recovery

1. Cut the network for 60 s during an import and during a plugin download.
2. **Expected**: both enter a waiting state, retry with backoff, and complete after the network returns — no manual
   retry, no `failed` entry. A peer refusal still fails immediately.

## D. Destroy is revocation

1. Note the endpoint id and relay address; destroy and rebuild the node.
2. Try to import with the old code, then connect directly to the old endpoint id/relay port.
3. **Expected**: old code no longer resolves; the old endpoint id is unreachable (identity rotated); the peer's relay
   access is gone; the retirement record was published despite one injected failure (retry).

## E. Rotation does not cut a transfer

1. Start an import that holds a claim, force a rotation due.
2. **Expected**: rotation reports `deferring`; the import finishes; rotation then runs. With no claims, rotation runs on
   schedule.

## F. Plugin egress (capture)

1. Pin `plugin_bind_interface`, run a plugin login and a download.
2. **Expected**: traffic on the pinned interface only; concurrency/coalescing/ETag behaviour unchanged (observable in the
   plugin's request log); with the shim's device missing, the invocation fails instead of falling back.

## G. Download recovery

1. Start a plugin download, kill the connection for 30 s.
2. **Expected**: resumes with `Range`/`If-Range` without user action; a server without range support produces one
   restart and a clear message, not a silent hang.

## H. Repack cache

1. Request a tankoubon zip with `Range: bytes=0-1023` before any build exists.
2. **Expected**: `503` with `Retry-After: 5`; after the build completes, `206` with the correct `Content-Range`.
3. `kill -9` mid-build; **expected**: no file at the final path, only `.tmp` leftovers that the temp sweep removes.
4. Two concurrent requests for the same key; **expected**: one build.

## I. 99 % measurement

Run `scripts/share-network-matrix.sh` with the interruption cases enabled and archive its report: topology, attempts,
successes, mechanism used, and fallback. The feature is not "100 % reliable"; the report says which topologies reach
the target and which need a relay.
