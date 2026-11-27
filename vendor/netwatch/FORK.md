# netwatch (fork)

Upstream: the crates.io release named in `vendor/patches/netwatch/VERSION`. `vendor/netwatch/` is generated
from it by `scripts/vendor-forks.sh` (release + `*.patch` + this file); never edit it by hand.

## Diff

`src/udp.rs`: every UDP socket `SocketState::bind` creates is pinned to an interface when one is set,
via `SO_BINDTODEVICE` on Linux (`set_bind_device` + `bind_to_device`), just before the
`BindOptions::configure_socket` hook upstream runs at the same point. `src/lib.rs` re-exports the
setter. No other line changes.

Upstream added that hook in 0.20 for exactly this kind of option, but iroh does not expose it yet. Once
it does, this fork can go: pass the hook through iroh's endpoint builder instead.

## Why

022 (review 1.4.2): binding the QUIC endpoint to an interface's *address* does not pin the route on
a weak-host kernel, and leaves the socket on the physical link whenever that address changes. iroh's
UDP sockets are created by this crate, so this is the only place the option can be set before `bind`.

## Setting it

`lanrurugi-api`'s `share_net::bind` calls `netwatch::set_bind_device(pinned_interface)` on every
(re)bind, with `None` when nothing is pinned.
