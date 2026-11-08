# iroh-relay (fork)

Upstream: `iroh-relay 1.4.1` from crates.io, copied verbatim except for the change below.

## Diff

- `Cargo.toml`: adds `socket2 = "0.6"` (already in this workspace's lockfile).
- `src/client.rs`: `set_bind_device` + `bind_device`, the process-global interface pin.
- `src/client/tls.rs`: `connect_bound` replaces `TcpStream::connect` for the relay's TCP dial. With an
  interface pinned it builds the socket itself, sets `SO_BINDTODEVICE`, connects non-blockingly, and
  waits for writability; with none it is the original `TcpStream::connect`.

## Why

022 (review 1.4.2): the relay connection is a long-lived outbound TCP stream; leaving it on the
kernel's routing exposes the physical address even when the QUIC endpoint is pinned.

## Setting it

`lanrurugi-api`'s `share_net::bind` calls `iroh_relay::client::set_bind_device` next to netwatch's.
