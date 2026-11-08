# Feature Specification: Direct Download Domain

**Feature Branch**: `021-direct-download-domain`

**Created**: 2026-11-08

**Status**: Draft

**Input**: User direction — "我要的是整个 app 能正常挂载橙云 CNAME 下的同时，能够让浏览器下载档案（含共享的下载功能）绕开橙云"（session-ef016726, 41265），reviewed in `share-pr-review.md` chapters 14–16.

## Context

The site is meant to sit behind Cloudflare's orange cloud: the page host is proxied, cached and protected, and that is fine for HTML, JSON, images and thumbnails. Downloads are the exception. A CDN is not meant to carry bulk non-HTML content, the edge sees the bytes, and — the failure that actually happened — a repack of a tankoubon or a patched archive that is generated before its first byte is written can exceed the edge's 100-second origin timeout and return 524. (The 100 MB figure often quoted here is Cloudflare's *request body* limit, not a response limit; it is irrelevant to downloads.)

The application already implements most of the dual-domain design:

- `direct_download_url` (general settings) names a second host, and the app treats that Host as downloads-only: everything except `/api/share/r/` and `/api/dl/` is a 404 (`crates/lanrurugi-api/src/download_host.rs`).
- Downloads on the main site get a signed, expiring link (`GET /api/archives/{id}/download-link`, HMAC-SHA256, 15-minute TTL, `crates/lanrurugi-api/src/direct_download.rs`) so no cookie or cross-subdomain session is needed; the frontend calls it through `apps/frontend/src/api/download.ts`.
- Share pages expose the same download base and the share range endpoints carry CORS plus the `x-lrr-*` response headers.
- `CF-Connecting-IP` is trusted only when the request genuinely arrives from a Cloudflare edge address (`crates/lanrurugi-api/src/shares.rs` `CLOUDFLARE_RANGES` / `is_cloudflare_edge`); a client can no longer forge the header to escape the IP allow-list or rate limits.
- Repacks stream: the tankoubon/patch zip is followed while it is still being written, so the first byte leaves immediately instead of after the whole archive is built.

What is missing is the part that makes this a supported deployment rather than a private arrangement: the address is accepted even when it is `http://` while the page is HTTPS (a mixed-content download that a browser will block or warn about), there is no machine-readable reverse-proxy reference (only prose), and the shipped documentation still contains an outdated claim about the CDN timing out.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A large archive downloads through the CDN-fronted site (Priority: P1)

An administrator runs the site behind Cloudflare's orange cloud and downloads a 5 GB archive from the library. The request goes to the download host, starts returning bytes immediately, and completes; it never hits the edge's 100-second origin timeout and does not count as CDN traffic.

**Why this priority**: this is the deployment the feature exists for. Every other story is about making it configurable or supportable.

**Independent Test**: with `direct_download_url` pointed at a host that bypasses the CDN, request a multi-GB archive and a freshly created tankoubon zip; both must return the first byte quickly and complete with a correct byte count.

**Acceptance Scenarios**:

1. **Given** a configured download host and a 5 GB archive, **When** the download starts, **Then** the first byte arrives within seconds and the transfer completes with the exact size.
2. **Given** a tankoubon whose zip has not been generated yet, **When** it is requested with a Range header, **Then** the response is either a stream that starts immediately or a 503 with `Retry-After`, never a silent timeout.
3. **Given** a completed download, **When** the client retries with `Range`/`If-Range`, **Then** it resumes from the offset rather than restarting.

---

### User Story 2 - A share recipient downloads without a session (Priority: P1)

A recipient opens a share link on the page host, views pages, and clicks download. The file comes from the download host through a signed link that needs no cookie and no login, and the page host is never asked for the bytes.

**Why this priority**: sharing is the other half of the same traffic, and it is the case that most often has no logged-in cookie to rely on.

**Independent Test**: open a share page in a fresh browser profile (no cookies) on the page host, download the archive, and confirm the bytes come from the download host and that no admin route is reachable there.

**Acceptance Scenarios**:

1. **Given** a share with download permission, **When** the recipient downloads, **Then** the link works without cookies and expires after its TTL.
2. **Given** the download host, **When** any path outside the download prefixes is requested, **Then** the response is 404 regardless of method or headers.

---

### User Story 3 - The address is validated against the deployment's own scheme (Priority: P2)

An administrator who serves the site over HTTPS types an `http://` download address. Saving is refused with a message that explains the mismatch and what to do instead; an `http://` address is still accepted when the site itself is served over HTTP (a LAN deployment).

**Why this priority**: a silent acceptance produces a download button that browsers block, which looks like a broken feature rather than a misconfiguration.

**Independent Test**: save an `http://` value with an HTTPS page origin and observe the rejection; save the same value with an HTTP page origin and observe it being accepted.

**Acceptance Scenarios**:

1. **Given** an HTTPS page origin, **When** the download address starts with `http://`, **Then** the save fails with an actionable message and the old value remains.
2. **Given** an HTTP page origin, **When** the download address starts with `http://`, **Then** it is accepted.
3. **Given** a saved HTTPS download address, **When** the page origin later changes to HTTPS, **Then** the stored value is re-checked and flagged if it is now invalid.

---

### User Story 4 - An operator can reproduce the dual-domain setup from the repository (Priority: P2)

An operator reads the deployment guide and copies a complete reverse-proxy reference that configures both hosts: the page host accepts only Cloudflare's edge addresses, passes the real client address, and never caches the download paths; the download host serves only the download paths, strips any client-supplied `CF-Connecting-IP`, disables buffering, allows long transfers, and is rate-limited.

**Why this priority**: without a reference configuration the feature works only for the person who already built it, and the security properties above (edge-only page host, stripped header) are exactly the ones a hand-written config gets wrong.

**Independent Test**: apply the reference to a scratch nginx in front of a dev instance and confirm the page host rejects non-edge clients, the download host serves the archive, and every other path on the download host is 404.

**Acceptance Scenarios**:

1. **Given** the reference configuration, **When** the download host is probed for `/`, `/api/info`, `/api/archives`, **Then** all return 404 and downloads still work.
2. **Given** the reference configuration, **When** a client sends a forged `CF-Connecting-IP` to the download host, **Then** the application sees no trusted client address.
3. **Given** the reference configuration, **When** a 10 GB transfer runs, **Then** the proxy does not buffer the response to disk and does not time out.

---

### User Story 5 - The documentation states the constraints honestly (Priority: P3)

A reader learns from the docs, in both languages, that the download host needs A/AAAA records rather than a CNAME to the proxied record, that IPv6 must be configured in parallel, that its certificate cannot come from the HTTP-01 challenge while grey-clouded, that CGNAT makes the whole arrangement impossible, and that 524 — not a size limit — is what a CDN does to a late first byte.

**Why this priority**: each of these is a documented-in-the-past failure mode; leaving them implicit turns a support question into a bug report.

**Independent Test**: read the guide as someone who has never configured a reverse proxy and enumerate the steps without consulting the code.

**Acceptance Scenarios**:

1. **Given** the guide, **When** the reader follows it, **Then** every step needed for a working deployment is present and no step states a false limit.
2. **Given** the offline documentation copy shipped in the image, **When** it is rebuilt, **Then** it contains the corrected text and the reference configuration.

---

### Edge Cases

- Page host behind the CDN but the download host is the only public entry: the app must not require the page host to be reachable for a signed download link to work.
- An expired or tampered signature: 403, indistinguishable from a nonexistent archive.
- A download host that is also a share host: `/api/share/r/` must keep working with CORS while everything else stays 404.
- IPv6-only client and an A-only download host: documented as a configuration error, not silently broken.
- Range requests from a multi-connection downloader on a repack that is still being generated: 503 + `Retry-After` until it exists, then a correct range.
- A proxy that rewrites `Host`: the download path boundary must still be enforced by the Host the app sees, and the guide must say proxies must preserve it.
- A direct-download URL with a path component or trailing slash: normalized or rejected, never producing double-slash links.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The application MUST reject an `http://` download address when the deployment's page origin is HTTPS, with a message naming the mismatch and the fix.
- **FR-002**: The application MUST accept an `http://` download address when the page origin is HTTP.
- **FR-003**: The download address MUST be an absolute URL with an `https`/`http` scheme and a host, and MUST NOT contain credentials, a fragment or a query.
- **FR-004**: Changing the page origin MUST re-validate the stored download address and surface a warning when the combination is no longer acceptable.
- **FR-005**: On the download host, only the download path prefixes MUST be served; every other path MUST return 404 for every method.
- **FR-006**: Signed download links MUST remain the only credential on the download host for library and tankoubon downloads, MUST expire, and MUST NOT depend on cookies or a logged-in session.
- **FR-007**: Share range responses on the download host MUST carry the CORS and exposed-header set the share page needs.
- **FR-008**: The page host MUST trust `CF-Connecting-IP` only when the request arrives from a Cloudflare edge address; the download host MUST NOT trust it at all.
- **FR-009**: Repack downloads MUST begin streaming before the archive is complete, and a Range request that arrives before completion MUST receive a retryable response rather than a timeout.
- **FR-010**: Library downloads MUST continue to be offered under the archive's title, never the stored id.
- **FR-011**: The repository MUST contain a copy-pasteable reverse-proxy reference for both hosts, covering the edge allow-list, real client address handling, header stripping on the download host, buffering, timeouts and connection limits.
- **FR-012**: The deployment guide (both languages) MUST document the DNS, certificate, IPv6, CGNAT and 524 constraints, and link the reference configuration.
- **FR-013**: The offline documentation copy MUST be rebuilt from the corrected sources and shipped in the image.
- **FR-014**: Page images and thumbnails MUST stay on the page host; only download links use the download host.
- **FR-015**: Setting or clearing the download address MUST take effect without a restart.

### Key Entities

- **Download address**: the operator-configured absolute URL of the download host, validated against the page origin.
- **Signed download link**: a time-limited, path-scoped credential for one archive or tankoubon.
- **Host policy**: the rule that a request whose Host is the download host may only reach the download prefixes.
- **Reverse-proxy reference**: the shipped configuration that makes the two-host deployment reproducible.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A 5 GB archive download through the download host starts delivering bytes within 5 seconds and completes without a CDN timeout.
- **SC-002**: On the download host, 100% of probes outside the download prefixes return 404, across at least the ten most sensitive admin routes.
- **SC-003**: An `http://` address on an HTTPS deployment is rejected at save time 100% of the time, with a message that names the mismatch.
- **SC-004**: A fresh browser profile can complete a share download with no cookie and no login.
- **SC-005**: The guide's steps, followed verbatim on a clean host, produce a working deployment on the first attempt.
- **SC-006**: The shipped offline documentation contains the corrected constraint text and the reference configuration.

## Assumptions

- The page host stays behind the orange cloud; the download host is a separate grey-cloud name with A/AAAA records and its own certificate (DNS-01), as decided in the design review.
- One download host is enough; multiple download hosts are out of scope.
- The operator has a public IPv4 or IPv6 address; CGNAT is documented as unsupported, and relaying downloads through a VPS (review §15.5) is explicitly out of scope.
- HTTP traffic (library, share pages, downloads) intentionally does not go through the VPN interface; only P2P sharing does.
- The existing signed-link TTL of 15 minutes stays; making it configurable is out of scope.
