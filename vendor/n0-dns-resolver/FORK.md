# n0-dns-resolver (fork)

Upstream: the crates.io release named in `vendor/patches/n0-dns-resolver/VERSION`. `vendor/n0-dns-resolver/` is
generated from it by `scripts/vendor-forks.sh` (release + `*.patch`, `Cargo.append.toml` + this file); never edit it
by hand.

## Diff

- `Cargo.toml`: `socket2 = "0.6"` with `all` (for `bind_device`), appended from `Cargo.append.toml`.
- `src/resolver/transport.rs`: `set_bind_device`, and every socket the resolver opens — UDP, TCP, DNS-over-TLS —
  is pinned with `SO_BINDTODEVICE` when an interface is set; the DNS-over-HTTPS client is built with
  `reqwest`'s `.interface()`. A nameserver on loopback (a local stub) is never pinned.
- `src/resolver.rs`, `src/lib.rs`: re-export `set_bind_device`.

## Why

022: with sharing pinned to a tunnel, iroh resolves relay and node names through this crate, and its queries
followed the default route — the physical link — whenever the tunnel's own nameserver was unreachable, telling
the physical network's DNS server which relay the node was about to use. Found by `scripts/share-pin-check.sh`.

## Setting it

`lanrurugi-share`'s `net::bind` calls `n0_dns_resolver::set_bind_device` next to netwatch's and iroh-relay's.
