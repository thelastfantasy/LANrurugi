# Contract: Share Settings

## Keys and defaults

| Key | Default | Accepted | On violation |
|---|---|---|---|
| `share_bind_interface` | `""` | `""` or an interface present on the host | `400 interface_not_found` |
| `share_udp_port` | `""` | `""` or 1–65535 | `400 invalid_port` |
| `share_port_rotate` | `"480"` | `"0"` or 360–720 (minutes) | clamped, warning `port_rotate_clamped` |
| `share_node_relay_port` | `"11205"` | 1–65535 | `400 invalid_port` |
| `share_external_addrs` | `""` | comma list of `host:port` / `[v6]:port` | `400 invalid_address` |
| `share_node_relay_allow` | `""` | `lrrg-node:<…>@host:port` or 64-hex endpoint id | `400 invalid_allow_entry` |
| `plugin_bind_interface` | `""` | `""` or an interface present on the host | `400 interface_not_found` (was silently accepted before) |

## Semantics

- `share_udp_port = ""` means "a new random port per start and per rotation"; an explicit port is kept across rotation.
- `share_port_rotate = "0"` disables rotation; any other value is clamped into 360–720 minutes (±25 % jitter applied at
  scheduling time).
- Changing `share_node_relay_port` restarts the relay only. Any other sharing setting does not restart it.
- Changing `share_bind_interface` drains running plugin work, rebinds, and rebuilds the endpoint without rotating the
  node code.
- `share_node_relay_allow` accepts a node code, so the settings page (which shows node codes) can be used to grant
  access.

## Response additions

`GET /api/share/status` adds:

```json
{
  "bound_interface": "tun0",
  "binding": "pinned",
  "relay": { "port": 11205, "mode": "proxy", "restarted_by_endpoint": false },
  "rotation": { "state": "deferring", "deferred_since": "…", "active_claims": 2 }
}
```

`binding` is one of `pinned`, `auto`, `unsupported` (platform), `failed` (with `binding_error`).

## Errors

`POST /api/settings` returns `400` with `{ "error": "<code>", "field": "<key>", "message": "…" }`; the frontend shows
the message next to the field. A setting that would break an existing deployment (e.g. a relay port change while a
reverse proxy points at the old one) is accepted but returns a `warning` in the response body.
