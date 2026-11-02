# Contract: Sidecar Fragment Format

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

## Purpose

The sidecar is where a maintainer writes request/response detail for endpoints the upstream document
does not cover, and where they record a deliberate divergence from an endpoint the upstream document
does cover. It is the only hand-written input to the generator besides the vendored upstream file.

## Location and naming

```text
docs/api/
├── legacy-openapi.yaml        # vendored upstream — never hand-edited
└── sidecar/
    ├── jobs.yaml              # one file per API group
    ├── download-queue.yaml
    └── …
```

Every `*.yaml` / `*.yml` directly inside `sidecar/` is read. Subdirectories are read too, so a large
group can be split, but a fragment may not be split across two files for the same `(path, method)`.

## Format

A fragment is a plain OpenAPI **path-item map** — the same shape as the `paths:` value of a full
document, with no wrapper key. Paths are absolute (they include the `/api` prefix) because that is
what OpenAPI requires and what the reader sees in the generated output; there is no prefix inference
for hand-written input, since silently prefixing a maintainer's literal string is how a typo becomes
a plausible-looking endpoint.

```yaml
# docs/api/sidecar/jobs.yaml
"/api/jobs":
  get:
    summary: List tracked background jobs
    description: |
      Every job this instance is tracking, most recently created first.
      Finished jobs are retained until cleared.
    tags: [jobs]
    parameters:
      - name: status
        in: query
        required: false
        schema:
          type: string
          enum: [queued, active, finished, failed]
    responses:
      "200":
        description: The current job list.
        content:
          application/json:
            schema:
              type: array
              items: { $ref: "#/components/schemas/JobStatus" }

"/api/jobs/{id}":
  delete:
    summary: Clear a finished or failed job
    tags: [jobs]
    responses:
      "204": { description: Cleared. }
      "404": { description: No such job, or it is still running. }
```

## Merge precedence

For a given `(path, method)`, exactly one layer supplies the operation, chosen by:

1. **Sidecar**, if a fragment declares it.
2. **Upstream document**, if it declares it and no fragment does.
3. **Auto-derived**, if neither does.

Replacement is **whole-operation**, never field-by-field. A fragment that restates an upstream
endpoint replaces it entirely; to keep an upstream field, copy it into the fragment. This is
deliberate: cross-document field merging produces a result no maintainer can predict by reading
either file, and it hides divergence from review.

A fragment may *add* an operation to an upstream path (upstream declares `get`, the fragment declares
`post`) without touching the upstream `get`.

## Validation rules (all hard failures)

| Rule | Rationale |
|---|---|
| Every path key must match at least one route the server registers. | A fragment describing an endpoint that does not exist is the precise rot this feature exists to prevent. It fails loudly instead of being silently published or silently dropped. |
| Every method key must be one of `get`/`post`/`put`/`delete`/`patch`/`head`/`options`. | A typo like `gets:` would otherwise become an ignored key. |
| Two fragments may not declare the same `(path, method)`. | Ambiguity resolved by file-load order is a bug waiting to happen. |
| Path keys must begin with `/`. | OpenAPI requires it; catching it here gives a better message than the renderer would. |

## What belongs in a fragment

Write a fragment when you want to:

- document a LANrurugi-only endpoint's parameters, body, or responses;
- record a deliberate divergence from an upstream endpoint (changed parameter, extra field);
- correct an upstream description that no longer matches this server.

Do **not** write a fragment merely to restate what upstream already says — the upstream layer
already supplies it, and a copy will drift.

## What does not belong in a fragment

- The route itself. Routes come from the code; a fragment can only describe a route that already
  exists. Adding an endpoint to the server is what adds it to the reference.
- Response schemas the maintainer has not actually read out of the handler. A fabricated schema is
  worse than a marked-as-undocumented entry, because a reader cannot tell the difference.
- Anything machine-specific (absolute paths, hostnames, tokens).

## Component schemas

A fragment may define reusable schemas under a top-level `components:` key. These are merged into the
generated document's `components.schemas`, with a duplicate name from two fragments failing rather
than one silently winning. Upstream's own `components.schemas` (if any) are available under their
upstream names; a fragment that needs to override one must use a distinct name and reference it
explicitly, so the override is visible in review.
