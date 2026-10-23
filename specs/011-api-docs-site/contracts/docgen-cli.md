# Contract: `lanrurugi-api-docgen` CLI

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

The generator is a build-time binary. This contract fixes its command line, its inputs, its outputs,
and its exit codes, so CI, `mise`, and the Docker build can all rely on it without reading its
source.

## Invocation

```text
lanrurugi-api-docgen [OPTIONS]
```

Run from the repository root. All paths are resolved relative to the repository root, not relative to
the binary's location, so the tool behaves identically from a `cargo run`, a `mise run`, or a Docker
`RUN`.

## Options

| Option | Default | Meaning |
|---|---|---|
| `--output <PATH>` | `docs/public/openapi.json` | Where the generated document is written. |
| `--api-src <DIR>` | `crates/lanrurugi-api/src` | Root of the route source to scan. Exists so tests can point the scanner at a fixture tree. |
| `--app-src <PATH>` | `crates/lanrurugi-server/src/app.rs` | The file that mounts the routers. Scanned for mount consistency only. |
| `--legacy <PATH>` | `docs/api/legacy-openapi.yaml` | The vendored upstream document. |
| `--sidecar <DIR>` | `docs/api/sidecar` | Directory of hand-written fragments; every `*.yaml`/`*.yml` directly inside is read. |
| `--access-notes <PATH>` | `docs/api/access-notes.yaml` | Per-role behaviour notes, keyed by `METHOD /path`. A **missing file is not an error** (the notes are optional detail); a file that exists and names a route that does not is. |
| `--repository <PATH>` | `.` | Prefix stripped from `x-lanrurugi-source` values, so the emitted document never carries a machine-specific absolute path. |
| `--check` | off | Validate only. Runs every gate and the coverage report, writes no output file. |
| `--quiet` | off | Suppress the coverage report on success. Never suppresses errors. Gates are unaffected. |

## Behaviour

1. Loads the mount table and asserts it against the router factories actually discovered in
   `--api-src` and referenced in `--app-src` (both directions — see `data-model.md` §2).
2. Scans `--api-src` for route registrations and builds the `RouteRecord` set.
3. Reads `--legacy`, applies the `/api` prefix rewrite, and drops per-operation `security`.
4. Reads every sidecar fragment and validates it against the `RouteRecord` set.
5. Merges the three layers and assembles the document.
6. Runs the gates.
7. Prints the coverage report (unless `--quiet`).
8. Writes `--output` (unless `--check`) as pretty-printed JSON with sorted keys, so two runs over
   unchanged sources produce byte-identical output.

## Gates

Each gate either passes or aborts with a non-zero exit and a message that names the offending route,
file, and line. A gate never "skips" a problem.

| Gate | Fails when |
|---|---|
| Resolution | A route registration cannot be resolved to a method and a literal path (a computed path, an unrecognised method-router expression, or a `.nest()`/`.nest_service()` inside a scanned router factory). |
| Mount consistency | A router factory is discovered but not in the mount table, or is in the mount table but mounted nowhere. |
| Completeness | A `RouteRecord` is missing from the assembled document. Asserted rather than assumed; a failure here means a merge bug, so it is checked even though the layering makes it impossible by construction. |
| Sidecar freshness | A sidecar `(path, method)` has no matching route, or two fragments claim the same one, or a fragment uses an unknown method key. |
| Document integrity | A path key does not start with `/`, an operation has no `responses`, or two operations share an `operationId`. |

## Non-gating reports

Printed on every non-quiet run, and never a failure:

- **Upstream-only paths** — upstream paths this server does not register. Reported because some are
  deliberate omissions and some are real compatibility gaps; only a maintainer can tell which, and
  the tool must not pretend to know.
- **Coverage by schema source** — how many operations carry upstream detail, hand-written detail, or
  none. The `none` count is the visible, shrinking backlog.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | All gates passed. The document was written unless `--check` was given. |
| `1` | A gate failed. The message on stderr names the specific route/file/line. |
| `2` | The tool could not run at all — an input file or directory was missing or unreadable. |

Note on `mise`: the task wrapper surfaces any failure as its own non-zero status (observed as `2`),
so a caller that needs to distinguish a gate failure from an unreadable input must inspect the
message, not the shell status. CI only needs "non-zero", and a gate failure's message always names
the offending route.

## Determinism requirement

Given identical inputs, two runs must produce byte-identical output. This is a contract, not a
nicety: CI compares the published artifact against a locally generated one, and a non-deterministic
document would make that comparison meaningless.
