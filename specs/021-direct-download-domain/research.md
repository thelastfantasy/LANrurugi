# Phase 0 — Research: Direct Download Domain

## R1. Reject `http://` only when the page is HTTPS

**Decision**: if the deployment's page origin is HTTPS and the download address is `http://`, refuse the save. If the
page origin is HTTP, accept it.

**Rationale**: a browser on an HTTPS page blocks or warns on an insecure subresource; the user sees a download button
that does nothing and reports a bug. An HTTP page (the LAN default) has no such conflict, and refusing it there would
break every self-hosted LAN instance.

**Alternatives considered**: warn but save (the user still gets a broken button and no second chance to notice);
silently rewrite `http` → `https` (guesses a certificate that may not exist); refuse `http` unconditionally (breaks
LAN deployments).

## R2. The page origin comes from `share_public_url`, not the request

**Decision**: the comparison uses `share_public_url` when it is set; when it is empty, the origin is unknown and the
validator only enforces URL shape.

**Rationale**: the settings request may arrive through a proxy that terminates TLS, so `request.uri().scheme()` is
the proxy's view, not the browser's. `share_public_url` is the value the application already uses to build links, so
it is the deployment's own statement of its origin.

**Follow-up (FR-004)**: saving `share_public_url` re-runs the validator against the stored download address and returns
a warning field rather than refusing the page URL (the operator may be about to fix the other value).

## R3. The reference is nginx

**Decision**: ship one nginx configuration; the guide mentions Caddy/Traefik only as "same two-host split".

**Rationale**: `reverse-proxy.md` already documents nginx first and in most detail, and the security properties
(edge-only allow-list, `real_ip`, header stripping, buffering) map one-to-one onto nginx directives. A second complete
reference doubles what must be kept correct without adding a deployment the project already supports.

## R4. The reference ships as a static docs asset

**Decision**: `docs/public/nginx/lanrurugi-dual-domain.conf`, linked from both guides and served at
`/nginx/lanrurugi-dual-domain.conf`.

**Rationale**: a file can be copied, `nginx -t`-ed and versioned; a fenced block in Markdown drifts and cannot be
tested. `docs/public/` is already the pattern used for `openapi.json`.

## R5. The offline docs copy is rebuilt, not committed

**Decision**: `docs/dist/` stays gitignored; the feature's acceptance includes running `mise run docs-build` locally and
confirming the new file and corrected text are inside the generated tree, which is what the image copies.

**Rationale**: the image build already regenerates it; committing generated HTML would create a second source of truth.

## R6. No new setting key

**Decision**: reuse `direct_download_url`; the validator is server-side only.

**Rationale**: the value already exists and is already shown in the UI; a second "download host" key would create a
migration question for no benefit.
