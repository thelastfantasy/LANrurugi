# Contract: Share Wire

## Record (`KnockV1`) compatibility

New fields are optional and default when absent, so a record published by 4.0/4.1 still parses:

```rust
#[serde(default)] pub direct_addrs: Vec<SocketAddr>,   // empty = relay-only
#[serde(default)] pub engine_minor: u8,
```

`SHARE_ENGINE_VERSION` remains `4`. A record whose payload still fails to decode is reported as
`EngineIncompatible` — surfaced as "this code was published by an incompatible engine", never as "the code was never
published".

## Refusal codes (terminal vs retryable)

| Code | Meaning | Local handling |
|---|---|---|
| `PermissionDenied` | peer refused the share / not allowed | terminal `failed_refused` |
| `NotFound` | archive or share no longer exists | terminal |
| `EngineIncompatible` | wire version or required field mismatch | terminal |
| `IntegrityFailed` | chunk hash mismatch after a full retry | terminal for that chunk, import failed |
| `Busy` | peer is at capacity | retry with backoff |
| `RelayUnavailable` | relay temporarily gone | `waiting_for_network` |
| `Io` / `Timeout` / `ConnectionReset` | transport | `waiting_for_network` |

## Import session

`Open` and `Renew` return a session handle; the importer stores it in a replaceable slot. After any endpoint rebuild,
the importer re-opens and replaces the handle before requesting the next ticket. A `Prepare` failure whose cause is a
stale session is retried once with a fresh open.

## Claim

Claims are created by the sharer on `Open`, carry the archive id and a ticket, and expire on their own. Rotation
checks claims, not response bodies; an import with a live claim keeps it alive by renewing while it transfers.
