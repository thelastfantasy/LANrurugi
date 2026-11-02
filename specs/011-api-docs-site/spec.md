# Feature Specification: Developer Documentation Site with Generated API Reference

**Feature Branch**: `011-api-docs-site`

**Created**: 2026-10-11

**Status**: Draft

**Input**: User description: "接下来准备做 #110 和 #52：为 REST API 与插件 SDK 提供机器可读的文档产物，并搭建独立文档站。已定结论：JSON（具体为 OpenAPI 3.1）是唯一产物，生成器不产出 Markdown/HTML，渲染交给文档站；文档站留在同一 monorepo 内（constitution 的单仓硬约束），不新开 repo；托管用 Cloudflare Pages，绝不用 NAS 当主站；不代理用户实例的请求，也不提供公开 demo 站点；API 测试功能（Test it）由文档站在浏览器端直接请求用户自己的实例，实例地址与 token 由用户在 UI 里填，不需要改文件，并且必须有 copy as cURL 兜底；legacy 的 tools/openapi.yaml 是现成的 OpenAPI 3.1 全量文档，兼容面的字段级文档直接复用、不手写；补齐生成器扫描 router 定义以产出全部路径+方法+handler 名，并在 CI 里做『代码有、文档无』的门禁。此外顺带修一个真 bug：CORS 的 Allow-Headers 只放行了 Authorization，缺 Content-Type，导致带 JSON body 的跨域请求预检必失败。"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Every endpoint the running server serves is documented (Priority: P1)

A developer integrating with LANrurugi — a Tachiyomi/Mihon extension author, a community script
writer, an OPDS reader client author — opens the documentation site and finds the complete set of
endpoints the server actually serves, with each one's real HTTP method and path. The reference is
derived from the same route registration the server itself uses, so it cannot advertise an
endpoint that does not exist and cannot silently omit one that does.

**Why this priority**: This is the entire point of the feature. Documentation written by hand has
already failed in this project: the existing contract document stayed at a short, path-group-level
listing while the route count grew into the hundreds, so it stopped describing the server at all.
A reference that is derived rather than transcribed is the only version of this artifact that
stays true over time, and it is the foundation every other story here builds on.

**Independent Test**: Enumerate the endpoints the server registers and the endpoints the published
reference describes; the two sets must be identical, member for member, with matching methods.

**Acceptance Scenarios**:

1. **Given** a running LANrurugi instance, **When** the documentation reference is generated,
   **Then** every endpoint reachable on that instance appears in the reference with its correct
   method and full path.
2. **Given** a developer looking for a specific endpoint, **When** they browse or search the
   reference, **Then** they find it — there is no endpoint that is reachable on the server but
   absent from the reference.
3. **Given** an endpoint's method or path changes in code, **When** the reference is regenerated,
   **Then** the change appears in the reference without any human editing a documentation file.
4. **Given** an endpoint mounted outside the main API prefix (for example a health probe or a
   catalog descriptor), **When** the reference is generated, **Then** it is listed under its true
   absolute path, not a prefix-relative one.

---

### User Story 2 - Undocumented endpoints fail the build instead of drifting (Priority: P1)

A maintainer adds a new endpoint. If they forget to document it, the quality pipeline fails with a
message naming the specific endpoint, before the change can land. Documentation falling behind the
code stops being something that is noticed months later and becomes something that cannot be
merged.

**Why this priority**: Without this gate, User Story 1's guarantee decays exactly the way the
previous hand-written document did — one hurried change at a time. The gate is what makes the
completeness claim durable rather than a one-time snapshot, so it shares P1 with the reference
itself.

**Independent Test**: Register an endpoint with no corresponding documentation entry, run the
pipeline, and confirm it fails naming that endpoint; then add the entry and confirm it passes.

**Acceptance Scenarios**:

1. **Given** a newly registered endpoint with no documentation, **When** the pipeline runs,
   **Then** it fails and the failure message identifies the endpoint by method and path.
2. **Given** a route whose path or method cannot be determined statically, **When** the pipeline
   runs, **Then** it fails rather than quietly omitting that route from the reference.
3. **Given** a documentation entry for an endpoint the server does not register, **When** the
   pipeline runs, **Then** the mismatch is reported rather than published as if it were supported.
4. **Given** every registered endpoint is documented, **When** the pipeline runs, **Then** it
   passes and reports the total number of endpoints covered.

---

### User Story 3 - Legacy-compatible endpoints carry real field-level documentation (Priority: P2)

A developer who already integrates with LANraragi, or who is migrating an integration, opens the
reference for an endpoint that exists in both systems and finds the same detail they had before:
parameters, their types and constraints, and response shapes — not just a path and a method.

**Why this priority**: A path list alone does not let anyone write a client. This is what makes the
reference usable rather than merely complete. It is P2 rather than P1 because the completeness
guarantee (User Story 1) and the anti-drift gate (User Story 2) are what the published site must
have to be trustworthy at all, while this story is what makes it genuinely useful for the
compatibility surface.

**Independent Test**: For every endpoint present in both the legacy API description and this
server, confirm the published reference carries that endpoint's parameter and response
documentation.

**Acceptance Scenarios**:

1. **Given** an endpoint present in both the legacy API description and this server, **When** the
   reference is generated, **Then** the legacy parameter and response documentation is present for
   that endpoint without anyone re-writing it.
2. **Given** an endpoint that exists only in this server, **When** the reference is generated,
   **Then** it is still listed (per User Story 1) and is clearly distinguishable from one carrying
   full field detail.
3. **Given** an endpoint the legacy API description documents but this server deliberately does
   not serve, **When** the reference is generated, **Then** it is reported to the maintainer and is
   not published as a working endpoint.
4. **Given** a maintainer adding field detail for a new endpoint, **When** they write it, **Then**
   they do so in a separate, reviewable source rather than by editing generated output.

---

### User Story 4 - A developer tests an endpoint against their own instance from the docs page (Priority: P2)

A developer reading the reference wants to confirm a call actually works against their own
LANrurugi instance. On the endpoint's page they type their instance's address and their API token
into the page itself — no file to edit, no configuration to find — and issue the request. The page
remembers both for the rest of their visit. When the browser will not allow the request (their
instance is plain HTTP on a local network while the docs page is HTTPS, or cross-origin access is
switched off on their instance), the page still hands them a ready-to-paste command line for the
same request.

**Why this priority**: This is the difference between documentation a developer reads and
documentation a developer uses. It is deliberately independent of the reference's own completeness
so it can be built and demonstrated on its own, and it is P2 behind the completeness/depth stories
because a reference with nothing to test is not useful no matter how good the test panel is.

**Independent Test**: On a documentation page, enter a reachable instance address and a valid
token, issue a request, and observe the real response; then repeat with an unreachable address and
confirm a clear explanation plus a usable copy-as-cURL command.

**Acceptance Scenarios**:

1. **Given** a reader on any endpoint's page, **When** they supply their instance address in the
   page's own input, **Then** requests are sent to that address and no file is edited.
2. **Given** a reader who supplied an address and token earlier in the same visit, **When** they
   navigate to another endpoint, **Then** both are still in effect.
3. **Given** a reader who issues a request, **When** the request is sent, **Then** it travels
   directly from their browser to the address they supplied.
4. **Given** a reader whose browser blocks the request, **When** the request fails, **Then** the
   page explains the likely cause and offers a copy-as-cURL command for the same request that does
   not depend on the browser.
5. **Given** a reader who supplies a token, **When** they use the page, **Then** the token is never
   sent anywhere other than the instance address they supplied.
6. **Given** cross-origin access is enabled on a reader's instance, **When** their browser issues a
   request that carries a JSON body, **Then** the request is allowed rather than rejected by the
   browser's preflight check.

---

### User Story 5 - Plugin authors get a generated SDK reference (Priority: P3)

Someone writing a metadata, login, or download plugin opens the docs site and reads a reference for
the plugin authoring API — the types and functions their plugin can use — generated from the same
SDK source the plugin runtime actually loads.

**Why this priority**: Plugin authoring is the project's main extension surface, and it currently
has no browsable reference at all. It is P3 because it is additive depth on the same site: the
site, the pipeline, and the API reference all deliver value without it.

**Independent Test**: Generate the SDK reference and confirm every exported type and function in
the SDK source appears, with no machine-specific filesystem paths in the output.

**Acceptance Scenarios**:

1. **Given** the plugin SDK source, **When** the SDK reference is generated, **Then** each exported
   type and function appears with its signature and documentation comments.
2. **Given** the generated output, **When** a reader opens it, **Then** no absolute path from the
   machine that generated it is visible.
3. **Given** the SDK source changes, **When** the reference is regenerated, **Then** the change
   appears without anyone editing documentation by hand.

---

### User Story 6 - A self-hoster reads the same docs offline from their own instance (Priority: P3)

An administrator running LANrurugi on a NAS or a home server with no reliable internet access opens
the documentation from their own instance and reads the same material — including the API
reference — without reaching out to any external site.

**Why this priority**: The project's users self-host, often on networks that are not reliably
connected to the public internet, and the instance is where they are already looking. It is P3
because the published site is the primary delivery path and this is the same content made
available through a second, offline channel.

**Independent Test**: With the docs build present in the deployment, load the documentation path on
the running instance with external network access blocked, and confirm the full site including the
API reference renders.

**Acceptance Scenarios**:

1. **Given** an instance deployed with documentation included, **When** a reader opens the
   documentation path, **Then** the site renders on the instance itself.
2. **Given** such an instance with no external network access, **When** the reader browses the
   documentation, **Then** no page depends on fetching anything from an external host.
3. **Given** the documentation is served from a sub-path rather than a domain root, **When** the
   reader navigates and opens the API reference, **Then** internal links and the API description
   still resolve.

---

### Edge Cases

- A route registered with a path that is computed at runtime rather than written literally: the
  generator must refuse and report it, never publish a reference quietly missing that route.
- A route registered through a nested mount point: the published path must be the full absolute
  path a client actually calls, not the path fragment declared locally.
- An endpoint present in the legacy API description but not registered by this server (deliberately
  dropped, or a real gap): must be surfaced to a maintainer, and must not be published as if the
  server served it.
- An endpoint registered by this server with no field-level documentation anywhere: still published
  (it exists), clearly marked as lacking detail, and counted against the coverage report.
- A reader's instance is reachable only over plain HTTP on a local network address while the docs
  page is served over HTTPS: the browser will refuse the request; the page must explain this and
  offer copy-as-cURL, not appear broken.
- Cross-origin access is turned off on the reader's instance: the request fails; the page must name
  that setting as the likely cause rather than reporting a generic network error.
- A reader supplies a token with insufficient privileges for the endpoint: the instance's own error
  response is shown as-is, so the reader sees the real reason.
- The API description document is unavailable or malformed at render time: the docs page must fail
  visibly rather than render an empty reference that looks authoritative.
- The generated API description is consumed both by the published site and by the offline copy:
  both must come from one build of one source, never two independently maintained documents.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The project MUST produce a single machine-readable description of the API surface
  that is the sole source for every rendered form of the API reference (published site and offline
  copy alike).
- **FR-002**: The description MUST be derived from the server's own route registration rather than
  transcribed by hand, so that it cannot list an endpoint the server does not serve.
- **FR-003**: The description MUST cover every endpoint the server registers, with each endpoint's
  real HTTP method and its full absolute path as a client would call it.
- **FR-004**: Generation MUST fail, with a message naming the offending route, when a registered
  route cannot be resolved to a method and path — never omit it silently.
- **FR-005**: Generation MUST fail when a registered endpoint is absent from the description;
  this check MUST run in the project's automated quality pipeline.
- **FR-006**: Generation MUST report endpoints this server registers outside the main API path
  prefix under their true absolute paths.
- **FR-007**: For endpoints that exist both in the legacy LANraragi API description and in this
  server, the generated description MUST carry that legacy documentation (parameters, constraints,
  response shapes) without a human rewriting it.
- **FR-008**: Endpoints documented in the legacy API description but not registered by this server
  MUST be reported to a maintainer and MUST NOT be published as working endpoints.
- **FR-009**: Field-level detail for endpoints that have no legacy documentation MUST be addable in
  a separate, reviewable source, without editing generated output and without editing the vendored
  legacy document.
- **FR-010**: The documentation site MUST render the generated API description as a browsable,
  searchable API reference listing every endpoint.
- **FR-011**: The API reference MUST let a reader set the target instance address in the page's own
  interface, and MUST remember it for the rest of the visit; no file or deployment configuration
  edit may be required.
- **FR-012**: The API reference MUST let a reader supply an API token in the page's own interface
  and issue requests authenticated with it.
- **FR-013**: Requests issued from the API reference MUST go directly from the reader's browser to
  the instance address the reader supplied. The project MUST NOT operate a proxy for those
  requests and MUST NOT receive the reader's token.
- **FR-014**: The API reference MUST offer, for every endpoint, a copy-ready equivalent command
  line that does not depend on the browser being able to make the request.
- **FR-015**: When a request from the reference fails, the page MUST explain the plausible causes —
  in particular that the instance must be reachable from the reader's browser and that cross-origin
  access may need to be enabled on the instance — rather than presenting an unexplained failure.
- **FR-016**: Enabling cross-origin access on an instance MUST actually permit a browser on another
  origin to issue requests that carry a JSON request body. (Today it does not: the setting allows
  the authorization header but not the content-type header, so the browser's preflight check
  rejects those requests.)
- **FR-017**: The documentation site MUST include a reference for the plugin authoring SDK,
  generated from the same SDK source the plugin runtime loads.
- **FR-018**: No generated documentation artifact, and no page of the documentation site, may
  contain an absolute filesystem path from the machine that produced it.
- **FR-019**: The documentation site and the API description source MUST live in the same
  repository as the code they describe, and the site MUST be built from the same revision of that
  repository as the code it documents.
- **FR-020**: The documentation site MUST build reproducibly in the project's automated pipeline,
  and a failing docs build MUST block the change that caused it.
- **FR-021**: The documentation build MUST publish the site as static files, with no server-side
  rendering runtime required to serve it.
- **FR-022**: The deployed server MUST be able to serve the built documentation site from a
  sub-path of its own address, so a self-hoster can read it without external network access.
- **FR-023**: Serving the documentation from a sub-path MUST NOT depend on any asset or data being
  fetched from an external host at page-view time.
- **FR-024**: The project MUST NOT publish or operate a public demo instance, and MUST NOT present
  a shared public endpoint as the default target for the API reference.
- **FR-025**: The default target instance address presented to a reader MUST be the documented
  local default, which a reader running the instance on their own machine or through a local port
  forward can use without any further setup.
- **FR-026**: The documentation content that describes product usage (installation, configuration,
  plugin authoring guidance) MUST be maintainable as ordinary prose pages alongside the generated
  reference, in the same site.
- **FR-027**: The documentation site's infrastructure MUST allow additional display languages to be
  added for hand-written prose pages, consistent with the project's existing multi-language README
  set, without restructuring the site and without requiring the generated API reference to be
  translated or duplicated per language.
- **FR-028**: The specification and plan artifacts of this project's own development process MUST
  NOT be published as part of the documentation site.

### Key Entities

- **Registered endpoint**: An HTTP method plus a full absolute path that the running server serves,
  along with where in the source it is declared. The unit of completeness for the reference.
- **API description document**: The single generated machine-readable description of the API
  surface. Every rendered form of the API reference is a view of this one artifact.
- **Legacy compatibility document**: The vendored upstream description of the LANraragi API,
  authoritative for the field-level documentation of endpoints that exist in both systems.
- **Feature documentation source**: Hand-written, reviewable field-level detail for endpoints that
  the legacy compatibility document does not cover. Never generated output.
- **Coverage report**: The per-build accounting of registered endpoints, described endpoints,
  endpoints lacking field detail, and legacy-documented endpoints this server does not register.
- **Documentation page**: A hand-written prose page of the site (installation, usage, plugin
  authoring guidance, this reference's own introduction).
- **SDK reference**: The generated reference for the plugin authoring API, derived from the SDK
  source the plugin runtime loads.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On every build, the number of endpoints described equals the number of endpoints
  registered by the server — a difference of zero, in both directions.
- **SC-002**: Registering an endpoint without documenting it causes the automated pipeline to fail
  in the same run, naming that endpoint.
- **SC-003**: 100% of endpoints that exist in both the legacy API description and this server carry
  that legacy parameter and response documentation in the published reference.
- **SC-004**: 100% of the endpoints present in the legacy API description but absent from this
  server are reported to a maintainer by the build rather than published as supported.
- **SC-005**: A developer goes from opening the documentation to a successful request against their
  own instance in under two minutes, without editing any file.
- **SC-006**: For every endpoint, a reader whose browser cannot make the request can still obtain a
  runnable equivalent command line.
- **SC-007**: Enabling cross-origin access on an instance makes a browser on another origin able to
  issue a request carrying a JSON body, with zero rejections from the browser's own preflight check.
- **SC-008**: The documentation site publishes automatically on every merge to the release branch,
  with no manual step; a failing documentation build blocks that merge.
- **SC-009**: A self-hoster with no external network access can read the entire documentation site,
  including the API reference, from their own instance.
- **SC-010**: No build output of the documentation site contains a machine-specific absolute path.

## Assumptions

- **No public demo instance.** Per an explicit product decision, the project will not operate a
  publicly reachable LANrurugi instance for readers to try requests against. The API reference
  therefore targets the reader's own instance by default, and its usefulness does not depend on a
  demo existing.
- **No server-side proxy.** The project will never relay readers' API requests through its own
  infrastructure. This is a deliberate consequence of the project's self-hosted, private-data
  positioning: proxying would expose readers' tokens and internal addresses. Where the browser
  cannot make a request directly, a command line is offered instead.
- **Legacy API description is reusable.** The upstream LANraragi repository publishes a complete,
  machine-readable description of its own API under a license compatible with this project's MIT
  license, and it is authoritative for the endpoints the two systems share. It is vendored into
  this repository and refreshed deliberately rather than fetched at build time, so that builds are
  reproducible offline and a upstream change cannot silently alter published documentation.
- **Legacy path spelling differs.** The legacy description expresses paths relative to a server
  prefix, while this project's routes are mounted under an explicit API prefix. Reconciling that is
  this feature's responsibility; it does not imply any change to the paths clients actually call
  (Principle II).
- **The docs site is an additive build target, not a second product frontend.** It is a static site
  built from the same repository, introduces no runtime dependency for the product, and ships no
  part of the product's own user interface. This is what keeps it compatible with the constitution's
  single-repository, single-frontend-app constraint rather than requiring an amendment.
- **Static hosting only.** The site is published as static files to a third-party static host with
  its own deployment credentials; the project runs no documentation server. The deployment step
  itself requires credentials the maintainer supplies, so the deliverable is the automated,
  credential-driven pipeline rather than a live public URL.
- **Documentation language.** Hand-written prose pages are authored in Chinese first, matching the
  project's primary README, with the site structured so other languages can be added. The generated
  API reference is not translated.
- **Spec-kit artifacts stay internal.** The `specs/` tree is the project's own design record, not
  end-user documentation, and is deliberately excluded from the published site.
- **`enablecors` remains the reader's choice.** The cross-origin fix (FR-016) corrects what the
  existing setting permits; it does not change the setting's default, which stays off, and no
  endpoint's behaviour changes when the setting is off.
- **Existing API shapes are untouched.** This feature adds documentation and one header to an
  existing opt-in setting. It changes no endpoint's path, method, request shape, or response shape,
  and therefore raises no Principle II compatibility concern.
- **No new data store.** Nothing in this feature introduces persisted state beyond what the site
  build produces; runtime settings continue to live in Redis as they do today.
