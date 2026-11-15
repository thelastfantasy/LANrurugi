# netwatch (fork)

Upstream: `netwatch 0.19.3` from crates.io, copied verbatim except for the change below.

## Diff

`src/udp.rs`: every UDP socket `SocketState::bind` creates is pinned to an interface when one is set,
via `SO_BINDTODEVICE` on Linux (`set_bind_device` + `bind_to_device`). `src/lib.rs` re-exports the
setter. No other line changes.

## Why

022 (review 1.4.2): binding the QUIC endpoint to an interface's *address* does not pin the route on
a weak-host kernel, and leaves the socket on the physical link whenever that address changes. iroh's
UDP sockets are created by this crate, so this is the only place the option can be set before `bind`.

## Setting it

`lanrurugi-api`'s `share_net::bind` calls `netwatch::set_bind_device(pinned_interface)` on every
(re)bind, with `None` when nothing is pinned.
