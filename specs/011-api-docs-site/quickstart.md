> **Superseded in part (2026-10-11) — see [`contracts/deliverables.md`](./contracts/deliverables.md).**
> The rendering decisions recorded in this file were revisited after comparing the result against
> the reference documentation site: the whole-page Scalar viewer was replaced by generated
> per-group pages, and the API document now also feeds a standalone Swagger UI. Where this file
> still says `@scalar/starlight` or `docs/public/sdk/`, the deliverable contract is authoritative.

# Quickstart Validation: Developer Documentation Site with Generated API Reference

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

Runnable scenarios that prove the feature works end to end. Each one states what to run, what to
observe, and which spec requirement it discharges. Nothing here needs a Cloudflare account, a public
network, or a second machine — the only external dependency is the repository itself.

## Prerequisites

- Repository checked out on the branch carrying this feature.
- `mise install` has been run (Rust, Node 24.18.0, pnpm 11.10.0, Deno 2.9.1).
- `pnpm install` has been run from the repository root after `docs/` was added to
  `pnpm-workspace.yaml`.

Nothing needs a running LANrurugi instance, Redis, or any model asset. The generator is a static
text/AST consumer by design.

---

## Scenario 1 — The document covers every route the server registers

**Discharges**: FR-001, FR-002, FR-003, SC-001.

```sh
mise run docs-api
```

**Observe**: the command exits 0 and prints a coverage report. The two figures that matter are

```text
routes_registered: N
routes_described:  N
```

They must be equal. The absolute value of `N` is not asserted here on purpose — it grows as the
project grows, and pinning it in a document would guarantee this page goes stale. Equality is the
property that must hold.

**Cross-check (independent of the generator)**: count the routes straight out of the source and
compare.

```sh
grep -rho '\.route(' crates/lanrurugi-api/src/ | wc -l
jq '[.paths[] | keys | map(select(. != "parameters")) | length] | add' docs/public/openapi.json
```

The second figure counts operations rather than `.route()` calls, so it will exceed the first (one
`.route()` call can register several methods, and the first figure includes `.route(` text inside
comments). What must hold is that the *second* figure is not smaller than the number of real
registrations — if the document were dropping routes, it would be.

---

## Scenario 2 — Every path is absolute, and nothing leaks a machine path

**Discharges**: FR-006, FR-018, SC-010.

```sh
jq -r '.paths | keys[]' docs/public/openapi.json | grep -v '^/api/' | grep -v '^/health$' | grep -v '^/opensearch.xml$'
```

**Observe**: no output. Every route is either under the `/api` mount or one of the two deliberately
bare routes — which is exactly the mount-table behaviour `research.md` §2 describes, and the thing a
blanket-prefix implementation would get wrong.

```sh
grep -c "$(pwd)" docs/public/openapi.json
```

**Observe**: `0`. Also check the SDK artifacts (Scenario 6).

---

## Scenario 3 — A new undocumented route does not need a hand-written entry, and a hand-written entry for a route that does not exist breaks the build

**Discharges**: FR-005, FR-009, SC-002, SC-004.

```sh
# 1. Confirm the current state builds.
mise run docs-api

# 2. Register a throwaway route in the router source, then regenerate.
#    (Add `.route("/__quickstart_probe", get(__quickstart_probe))` to any module router.)
mise run docs-api
jq '.paths["/api/__quickstart_probe"]' docs/public/openapi.json

# 3. Remove the route, then add a sidecar entry naming it and regenerate.
printf '"/api/__quickstart_probe":\n  get:\n    summary: probe\n    responses:\n      "200": { description: ok }\n' \
  > docs/api/sidecar/__probe.yaml
mise run docs-api; echo "exit=$?"
```

**Observe**:
- Step 2: the new route appears in the document with a summary derived from the handler's own doc
  comment and `x-lanrurugi-source` pointing at the registration. **No hand-written entry was
  needed** — this is the property that makes the reference unable to fall behind (FR-005).
- Step 3: the command exits non-zero and names `docs/api/sidecar/__probe.yaml` as a stale entry.
  This is the reverse-direction gate: a hand-written description of an endpoint that does not exist
  cannot be published (SC-004).

Clean up both the probe route and `docs/api/sidecar/__probe.yaml` afterwards.

---

## Scenario 4 — Upstream-only paths are reported, not published

**Discharges**: FR-008, SC-004.

```sh
mise run docs-api 2>&1 | sed -n '/upstream-only/,/^$/p'
jq -r '.paths | keys[]' docs/public/openapi.json | grep -E '/registries|/plugins/install|bookmark_link'
```

**Observe**: the report lists the upstream paths this server does not register (nine at the revision
vendored for this feature, including the `/registries` group), and the `jq` lookup returns nothing —
they are reported to a maintainer and deliberately absent from the published document.

---

## Scenario 5 — Upstream detail survives the merge

**Discharges**: FR-007, SC-003.

```sh
jq '.paths["/api/search"].get.parameters | length' docs/public/openapi.json
jq -r '[.paths[][] | select(."x-lanrurugi-schema-source" == "legacy")] | length' docs/public/openapi.json
jq -r '.paths["/api/search"].get.security' docs/public/openapi.json
```

**Observe**:
- The first figure is non-zero: `/api/search` carries the upstream parameter list, unrewritten.
- The second figure is in the fifties: every upstream endpoint this server shares carries its
  upstream field-level detail.
- The third returns `null` (absent): per-operation `security` is stripped, because upstream's
  `api_key` / `security: []` statements are false for this server. The document-level `bearerAuth`
  applies instead — check with `jq '.security' docs/public/openapi.json`.

---

## Scenario 6 — The SDK artifacts are complete and path-clean

**Discharges**: FR-017, FR-018, SC-010.

```sh
mise run docs-sdk
ls docs/dist-sdk/ | head
grep -rl "$(pwd)" docs/dist-sdk/ ; echo "grep exit=$? (1 means clean)"
```

**Observe**: the HTML tree is present with an `index.html`, `all_symbols.html` and a search index;
`plugin-sdk.json` is present and normalised; the recursive grep finds no file containing the
workspace's absolute path. `grep` exiting `1` is the pass condition — it means nothing matched.

---

## Scenario 7 — The site builds at both bases

**Discharges**: FR-019, FR-020, FR-021, FR-022, FR-023, FR-027, SC-008, SC-009.

```sh
DOCS_BASE=/      pnpm --filter lanrurugi-docs run build && ls docs/dist/index.html
DOCS_BASE=/docs/ pnpm --filter lanrurugi-docs run build && ls docs/dist/index.html
DOCS_BASE=/docs/ pnpm --filter lanrurugi-docs run preview
```

**Observe**: both builds succeed. In the preview of the sub-path build, navigate to the docs root and
into the API reference: every link and every asset must resolve under `/docs/`. A root-based build
served from a sub-path `404`s on the first click, so clicking through is the actual test, not the
build's exit code.

---

## Scenario 8 — The API reference lists every endpoint and offers the try-it affordances

**Discharges**: FR-010, FR-011, FR-012, FR-014, SC-005, SC-006.

With `pnpm --filter lanrurugi-docs run dev` running:

1. Open the API reference page.
2. **Observe**: the operation list contains every path from Scenario 1's document, grouped by tag,
   with the upstream-documented ones showing parameters and response schemas.
3. Find the server selector in the request panel and use it to **add a server**
   (e.g. `http://localhost:3000`). **Observe**: it is an in-page input. No file was edited.
4. Find the authentication panel and supply a Bearer token. **Observe**: it is an in-page input.
5. **Observe**: each operation offers generated code samples, including a cURL command, and a copy
   control. This is the fallback that works regardless of browser policy.

Steps 3–5 need no running instance: they are UI affordances, and their existence is what is being
verified here. Actually issuing a request is Scenario 10.

---

## Scenario 9 — The cross-origin defect is fixed

**Discharges**: FR-016, SC-007.

```sh
grep -n 'ALLOW_HEADERS' crates/lanrurugi-api/src/cors.rs
```

**Observe**: the constant includes `Content-Type` alongside `Authorization`.

**Behavioural confirmation** (requires a running instance with `enablecors` on):

```sh
curl -i -X OPTIONS http://localhost:3000/api/archives \
  -H 'Origin: http://localhost:4321' \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: content-type,authorization' \
  | grep -i 'access-control-allow-headers'
```

**Observe**: the response names `Content-Type`. Before this change it named only `Authorization`, so
the browser would have refused to send the actual JSON-bodied request.

---

## Scenario 10 — A real request against a real instance, from the docs page

**Discharges**: FR-013, FR-015, SC-005.

Requires a running LANrurugi instance with `enablecors` enabled and at least one API token.

1. Open the API reference, add `http://localhost:3000` as a server, paste a token.
2. Issue a `GET /api/archives`. **Observe**: the real response body appears.
3. Point the server address at an unreachable host and issue the same request. **Observe**: the page
   explains the likely causes (instance unreachable, CORS not enabled) rather than showing an
   unexplained failure, and the cURL sample still copies cleanly.
4. **Observe** in the browser's network panel that the request went to the address typed in step 1 —
   not to any site operated by this project.

---

## Scenario 11 — Offline reading from a running instance

**Discharges**: FR-022, FR-023, SC-009.

```sh
# With a deployment built from this branch, or a local build copied into the image's docs dir:
curl -s http://localhost:3000/docs/ | head -20
curl -s http://localhost:3000/docs/api/ | grep -c '搜索 API'
curl -s http://localhost:3000/docs/swagger/ | head -5
curl -s http://localhost:3000/docs/sdk/ | head -5
```

**Observe**: the guides and per-group API reference are served at `/docs/`, the Swagger UI at
`/docs/swagger/`, and the SDK reference at `/docs/sdk/` — all from the instance itself. Then open
each in a browser with the network panel open and confirm **no request leaves for an external
host**: Swagger UI's bundle, the SDK tree's assets and the document are all served from the site's
own files.

---

## Scenario 12 — CI wiring is correct

**Discharges**: FR-020, SC-008, SC-002.

```sh
mise run check
```

**Observe**: `rust-check` passes (the new crate is a workspace member, so it is covered by
`cargo check`/`fmt`/`clippy` automatically) and `frontend-lint` passes. Then, on a branch, verify by
inspection that `.github/workflows/ci.yml` contains a `docs-generate` job that runs the generator in
`--check` mode and a `docs-build` job that `needs:` it, downloads its artifact, and publishes only
when the deployment secrets exist. A build that cannot reach Cloudflare must still be green.

---

## What this quickstart deliberately does not cover

- **Publishing to the real Cloudflare Pages project.** That needs the maintainer's credentials; the
  deliverable is the credential-driven pipeline, verified by exercising the build locally and by the
  conditional deploy step's structure.
- **The full sidecar backlog.** The coverage report is the tool for tracking it; the quickstart
  verifies the mechanism and the precedence rules, not that every LANrurugi-only endpoint has been
  hand-described.
- **Rendering the SDK reference inside Starlight's own layout.** It is served as a static sub-path by
  design (see `contracts/docs-site.md`), so there is nothing to verify beyond it being reachable and
  path-clean.
