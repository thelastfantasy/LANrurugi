# HTTP API Contract: Sharing

All endpoints are new (Principle II: existing paths are unchanged). They are registered in
`crates/lanrurugi-api/src/shares.rs` (admin), `share_recipient.rs` (recipient) and
`tankoubons.rs` (tankoubon download), documented through the 011 docgen sidecar, and gated by
`route_policy.csv`. Errors use the existing `{ "operation", "error", "success": 0 }` shape.

## Admin: `/api/shares*` — `session` and `token_admin` only; deny `token_guest`, `guest_visitor`

| Method & path | Body / query | Response |
|---|---|---|
| `GET /api/shares` | `?status=active\|all` | `[ShareSummary]` |
| `POST /api/shares` | `NewShare` | `ShareCreated` (includes the code, shown once and re-derivable — see below) |
| `GET /api/shares/{id}` | | `ShareDetail` (items, limits, claims history, helpers) |
| `PATCH /api/shares/{id}` | `ShareUpdate` | `ShareDetail` |
| `POST /api/shares/{id}/items` | `{ items: [ShareItem], update?: ShareUpdate }` | `ShareDetail` (FR-002) |
| `DELETE /api/shares/{id}/items` | `{ items: [ShareItem] }` | `ShareDetail` |
| `POST /api/shares/{id}/revoke` | | `ShareDetail` (terminal) |
| `GET /api/shares/{id}/code` | | `{ code, text_form, link?, embed?: { iframe, element } }`. `link` and `embed` are present only when `share_public_url` is set (research §7). `text_form` includes `via` when public services are off |
| `GET /api/shares/by-item/{archive_or_tank_id}` | | `[ShareSummary]` (for "add to existing" and the reader) |
| `GET /api/shares/settings` / `PUT` | sharing settings (data-model) | `GET` also returns `upload_mode: normal\|alternative`, `upload_mode_source: schedule\|manual\|none`, `next_switch_at` (FR-042), `node_string` (this instance's `lrrg-node:` string when it offers a role), and `services: [{ kind: rendezvous\|relay, url, source: public\|node\|custom, reachable, last_ok_at, last_error? }]` (FR-049) |
| `GET /api/shares/imports` | `?helping=1` for the helping list | `[ShareImportSummary]` (incl. `help_deliver`, `help_state`, `help_reason`, `help_state_at`, `help_last_contact_at`, `help_bytes_served`, `help_removed_count`) |
| `GET /api/shares/imports/{id}` | | `ShareImportDetail`: the per-archive list for the expanded queue row |
| `POST /api/shares/imports/{id}/archives/{sharer_archive_id}/retry` | | `204`; restarts the import task if it had stopped |
| `PATCH /api/shares/imports/{id}` | `{ help_deliver?, help_hidden? }` | `help_hidden: true` removes the row from the helping list and turns helping off; imported archives are untouched |
| `POST /api/shares/imports/resolve` | `{ code }` | `ImportPreview` (FR-018) |
| `POST /api/shares/imports` | `{ code, category?, help_deliver? }` | `{ import_id, queue_item_id }` (FR-019, FR-021) |

### Re-displaying a code (FR-006)

The sharer stores only `code_hash`. It also stores the code encrypted with a key derived from the
node secret, in the `code_enc` field (data-model note). That is how `GET …/code` can show the code
again.

### Main types

```text
NewShare    { items: [ShareItem], title?, permissions, download_limit?, ip_allow?, ip_first_n?, expires_at?, embed?, allow_helpers?, short_code? }  // short_code requires expires_at ≤ now + 7 d
ShareUpdate { title?, permissions?, download_limit?|null, ip_allow?, ip_first_n?|null, expires_at?|null, embed?, allow_helpers? }
ShareSummary{ id, title, status, permissions, downloads_used, download_limit, item_count, created_at, expires_at }
ShareDetail = ShareSummary + { items: [ResolvedItem], ip_allow, ip_first_n, embed,
                               claims: [{ kind, address, started_at }], helpers: [{ endpoint_id, last_seen }] }
ImportPreview { manifest: ShareManifest(browse form), total_size, already_present: [archive_id], permissions }
```

### Refusal reasons

Used in import and recipient errors, with `error` set to a stable code:

- `not_found`, covering invalid, unknown and IP-refused requests, so nothing about the share is
  revealed (FR-014);
- `revoked`;
- `expired`;
- `exhausted`;
- `permission_denied`;
- `address_unverifiable`;
- `sharer_unreachable` (FR-029);
- `rate_limited`.

## Recipient: `/api/share/r/*` — allowed for `anonymous`, `guest_visitor`, `token_guest`, `session`

These handlers authenticate by share credential only and never read or create a library session
(FR-013a). Every response carries `Referrer-Policy: no-referrer`.

| Method & path | Credential | Response |
|---|---|---|
| `POST /api/share/r/open` | body `{ code }` | `{ token, manifest (browse form), permissions }`; rate-limited (FR-011) |
| `GET /api/share/r/manifest` | `X-Share-Token` or `?rt=` | browse manifest |
| `GET /api/share/r/archives/{id}/thumbnail` | token | same bytes as the library thumbnail |
| `GET /api/share/r/archives/{id}/pages` | token + `read` | page list, same shape as `GET /api/archives/{id}/files` |
| `GET /api/share/r/archives/{id}/page` | token + `read`, `?path=` | page image, same behaviour as the reader's page route |
| `GET /api/share/r/archives/{id}/download` | token + `download` | file, same bytes as `/api/archives/{id}/download`; creates or reuses the token's claim |
| `GET /api/share/r/tankoubons/{id}/download` | token + `download` | streamed zip (US3); same claim |

### Rules for every request

1. **Unknown token, revoked, expired, or IP not allowed**: `404 not_found`.
2. **Item not in this share**: `404`, identical to the response for no token at all (FR-013a).
3. **Missing permission**: `403 permission_denied`. This does not leak contents, because the
   recipient already holds a valid token.
4. **Download limit exhausted, on a new claim**: `409 exhausted`. Browsing still succeeds (US1-6).
5. **Own-share uploads paused by the schedule or manual switch** (FR-042): file and tankoubon
   downloads get `503` with `Retry-After`. Reading and browsing are unaffected (FR-043).

## Tankoubon download (US3): `GET /api/tankoubons/{id}/download`

- **Access**: `session` and `token_admin` only. The route policy denies `guest_visitor` and
  `token_guest`, mirroring the raw archive download deny.
- **Response**: `200`, `Content-Type: application/zip`, and
  `Content-Disposition: attachment; filename="<tankoubon name>.zip"` (RFC 5987 encoded).
  - The body is streamed with no `Content-Length`, using chunked transfer.
  - Entries are `NN - <title>.<ext>`, stored and not compressed, with ZIP64 enabled.
  - An optional `MISSING.txt` comes last.
- **Not found**: `404`.

## Embeds (no `/api` prefix; public)

| Path | Response |
|---|---|
| `GET /s/{code}` | SPA share viewer (frontend route; no server data in HTML) |
| `GET /embed/s/{code}` | card page for iframes. Only this route sends `Content-Security-Policy: frame-ancestors *`. If embedding is off for the share, it returns a 404 card with no content (FR-030) |
| `GET /embed/share.js` | the `<lrrg-share>` custom element bundle, `Access-Control-Allow-Origin: *` |
| `GET /api/share/r/card?code=` | card data, the minimal payload: `{ status, title?, item_count?, covers?, permissions, restricted }`. It is restricted when the share has IP rules (FR-031), and `404` when embedding is off. CORS `*` |
