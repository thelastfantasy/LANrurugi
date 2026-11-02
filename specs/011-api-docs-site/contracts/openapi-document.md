# Contract: The Generated OpenAPI Document

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

The generated document is the single source every rendered form of the API reference reads. This
contract fixes what it contains, what it deliberately does not contain, and which fields a consumer
may rely on. Only custom (`x-`) fields are specified in full; everything else follows OpenAPI 3.1.

## Document skeleton

```jsonc
{
  "openapi": "3.1.0",
  "info": {
    "title": "LANrurugi API",
    "version": "<workspace crate version>",
    "license": { "name": "MIT", "identifier": "MIT" },
    "description": "<one paragraph: this is generated from the server's own route table>"
  },
  "servers": [
    { "url": "http://localhost:3000", "description": "The default local port. Replace this with your own instance's address." }
  ],
  "security": [{ "bearerAuth": [] }],
  "tags": [ /* one per API module that has routes */ ],
  "paths": { /* every registered route */ },
  "components": {
    "securitySchemes": {
      "bearerAuth": {
        "type": "http",
        "scheme": "bearer",
        "description": "<how to create a token in the Settings page>"
      }
    }
  }
}
```

## Guarantees

1. **Completeness.** For every route the server registers, `paths[<path>][<method>]` exists.
   Equivalently, the set of `(path, method)` pairs in the document equals the set of pairs the route
   scanner extracted. There is no "documented but not served" and no "served but not documented".
2. **Absolute paths.** Every path key is the full path a client calls, including the `/api` prefix
   where the router is mounted under it. Nothing is relative.
3. **Determinism.** Paths and methods are emitted in sorted order; JSON keys are sorted. Two runs
   over unchanged sources are byte-identical.
4. **No machine-specific data.** No absolute filesystem path appears anywhere in the document.
   `x-lanrurugi-source` values are repository-relative (`crates/lanrurugi-api/src/jobs.rs:109`).
5. **No secrets.** Nothing derived from configuration, tokens, or a running instance is present. The
   document is a static description of the API surface.

## Authentication semantics

Derived, not declared. `crates/lanrurugi-api/policy/route_policy.csv` is the file the server itself
enforces, and the document states each operation's requirement by evaluating it with the same rule
Casbin applies (`some(allow) && !some(deny)`, with `keyMatch2` patterns).

- Two schemes: `bearerAuth` (`http`/`bearer`, an API token) and `sessionCookie` (`apiKey`, `in:
  cookie`, `lanrurugi_session` — the first-party web session).
- The document-level `security` is `[{bearerAuth}, {sessionCookie}]`: either credential works. That
  covers 193 of 231 operations, which is why it is the default rather than repeated per operation.
- An operation carries its own `security` only when it differs:
  - `[]` — reachable with no credentials (11 bootstrap routes, plus the 2 served outside `/api`).
  - `[{sessionCookie: []}]` — 25 routes that **refuse API tokens outright** (token, session and
    password management, database drop, import, the plugin wizard).
- Operations outside the `/api` prefix are `security: []` by construction: `app.rs` layers
  `require_api_key` *inside* the `/api` nest, so a router merged outside it never reaches the
  middleware. `scan::mount`'s guard is what keeps that a fact rather than a convention.

Upstream's own per-operation `security` is still dropped: it declares a fixed `api_key` and marks
`/search` as needing none, and both statements are false for this server.

**Gate.** A mutating route reachable by a guest token fails the build. This is the one invariant in
the policy that is *not* default-allow — `token_guest` is granted `GET` by wildcard and every
mutating route denies it explicitly — so it is the one a new route can silently break. (An earlier
gate asked whether the policy mentioned a route at all; the `/api/*` wildcards make that
unanswerable, so it could never fire. A gate that cannot fail is worse than no gate.)

## Per-operation custom fields

Consumers may rely on these; they exist so a reader can tell how much to trust an entry and where to
look when something is unclear.

| Field | Type | Meaning |
|---|---|---|
| `x-lanrurugi-source` | string | `path:line` of the `.route()` registration, repository-relative. |
| `x-lanrurugi-schema-source` | `"legacy"` \| `"sidecar"` \| `"none"` | Which layer supplied parameters/request/response detail. |
| `x-lanrurugi-undocumented` | boolean | Present and `true` exactly when `x-lanrurugi-schema-source` is `"none"`. Signals "this endpoint is listed and described, but its request/response shapes are not documented yet". |
| `x-lanrurugi-guest-visible` | boolean | Present and `true` when an unauthenticated visitor may reach the route **while the operator has guest access enabled**. Deliberately not expressed as `security: []`: guest access is a server setting, so "no credentials needed" would be an unconditional claim about a conditional route. |
| `x-lanrurugi-access-notes` | string[] | Behaviour that differs by the caller's role — which `GET /api/activity` entries you receive, a guest visitor's category scoping on the search endpoints, the 404-not-403 answer for an out-of-scope archive. OpenAPI has no per-role condition on parameters or responses, so it cannot be stated structurally. 15 operations carry it today; see `docs/api/access-notes.yaml`. **The same notes are also appended to `description`** as markdown, behind the HTML comment `<!-- lanrurugi:access-notes -->`: Swagger UI has no placement for a vendor extension except its Extensions table at the bottom of the operation, so the prose copy is what puts the note under the endpoint title. The documentation site strips from that marker onward and renders its own localised callout instead. |
| `x-lanrurugi-token-role` | `"admin"` | Present only when an administrator token works and a **guest token is refused** (117 operations). Absent means either "any token works" or "no token works" — unambiguous in combination with `security`, which says whether `bearerAuth` is accepted at all. |

The two role/visibility fields follow one convention: **only the exception is marked.** An absent
field is the default, which is why the pair has to be read alongside `security` rather than alone.

`x-lanrurugi-access-notes` is the one field that is also written into `description`. That is not a
breach of `merge`'s no-field-level-merging rule: that rule is about merging two *documents* field by
field, where the result cannot be predicted from either input. Composing a description from a known
layer during emit is deterministic and happens in one place. The duplication exists because Swagger
UI can only show a vendor extension in its Extensions table, at the bottom of the operation.

## Operation content rules

- `operationId` is derived from the handler name (snake_case → lowerCamelCase) and is unique across
  the document; on a collision the path is appended. Deterministic either way.
- `summary` is the first sentence of the handler's own doc comment when it has one; otherwise the
  handler name verbatim. No summary is invented.
- `description` is the remainder of the handler's doc comment. For an auto-derived operation it also
  states plainly that request/response detail is not yet documented and names the source location.
- `responses` is always present and non-empty (an operation with no `responses` is invalid OpenAPI).
  For an auto-derived operation it carries a single `default` response describing an error envelope
  only if the handler's doc comment establishes one; otherwise a `200` with no schema plus the
  not-yet-documented note. It never carries a fabricated schema.
- `tags` defaults to the module name. The upstream layer's own tags are kept where it supplied the
  operation, since upstream's grouping is what existing client authors already recognise.

## What the document deliberately omits

- **No response schemas for endpoints whose handlers return anonymous JSON.** Inventing them would
  produce documentation that is wrong in a way that looks authoritative, which the spec explicitly
  rules out.
- **No upstream paths this server does not serve.** They are reported to maintainers instead
  (nine at the revision vendored for this feature).
- **No vendor extensions copied blindly from upstream.** Upstream's `x-mojo-to` keys name Perl
  controller methods that do not exist here; carrying them would send readers to a dead end.
