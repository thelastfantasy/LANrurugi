# Contract: Plugin Wizard API

Five new, additive endpoints under `/api/plugin-wizard/*` (no legacy contract equivalent — matches
002/003/005's own precedent of adding new, LANrurugi-only endpoints rather than touching the
existing `/api` surface, per constitution Principle II). Session-authenticated only (spec FR-023 —
stricter than the general API-token-allowed surface; see research.md's Principle V discussion for
why this matches how this project already gates other Session-only actions like API-token
management). All five are stateless — see data-model.md's introduction; nothing here reads or
writes a server-held "wizard session" object.

## `POST /api/plugin-wizard/lookup`

Domain → per-type installed-plugin coverage (FR-002/FR-003).

**Request**:

```json
{ "domain": "example.com" }
```

**Response `200`**:

```json
{
  "login": { "covered": false },
  "metadata": {
    "covered": true,
    "namespace": "metadata/example",
    "source_code": "export function pluginInfo() { ... }"
  },
  "download": { "covered": false }
}
```

`source_code` is the covered plugin's full file content, verbatim — this is FR-009's reference
sample, returned once here rather than re-read from disk on every later `generate` call for a
different type on the same domain (the frontend threads it through, per data-model.md).

Coverage is determined the same way `plugins.rs::find_matching_plugin` already determines a
download-plugin match for a real download URL — a `url_pattern` regex match against the domain,
scanned across `discover_namespaces()`'s existing per-type listing (`login/`, `metadata/`,
`download/`, `custom/**`) — reused, not reimplemented, from that existing code path.

## `POST /api/plugin-wizard/analyze-login`

Login type only — a design change made after initial implementation (per real user feedback that
a hardcoded account+secret pair is wrong for sites that actually authenticate via a token/API key
or a raw cookie value). Runs the same FR-010 agentic loop as `generate` (`fetch_page` available,
system executes — AI judges), but the goal is different: inspect a real login page or API
documentation URL and decide what credential field(s) that specific site actually needs, returning
a `parameters` array in the same shape `PluginInfoResult.parameters` already uses — never assumed
up front to be a password pair. When a site plausibly supports more than one authentication method
(e.g. both a password-login form and a token/API-key header), the system prompt instructs AI to
prefer token/API-key over cookie over account+password, in that order (more stable, less likely to
trip anti-automation defenses) — only account+password when that's genuinely the only option, only
a bare cookie field when the site is cookie-only with no programmatic login endpoint at all.

The returned `parameters` MUST be echoed verbatim into the login plugin's own `pluginInfo()` when
`generate` is called afterward (`GenerateRequest.login_parameters`, see below) — AI must not invent
its own field set at generation time, since the wizard's trial-run/save steps are keyed off this
exact list (`trial_run.rs`'s `customargs_for` maps `credentials.fields` onto `customargs`
positionally by matching each declared parameter's `name`).

**Request**:

```json
{ "reference_url": "https://example.com/login" }
```

**Response `200`**:

```json
{
  "parameters": [
    { "name": "api_key", "description": "API Key", "required": true }
  ]
}
```

A password-pair site instead returns something like
`[{"name": "account", ...}, {"name": "secret", ...}]`; a cookie-only site returns a single
`{"name": "cookie", ...}` entry. Field count is meant to stay minimal — only the actual credential
value(s) a user must supply, never extra optional configuration.

**Response `422`** (AI's final output wasn't parseable as a non-empty `parameters` array):

```json
{ "error": "ai_output_not_parameters", "raw_output": "..." }
```

**Response `503`** (LLM unavailable — same as `generate`'s own FR-024 handling):

```json
{ "error": "llm_unavailable", "detail": "..." }
```

## `POST /api/plugin-wizard/generate`

Runs the FR-010 tool-calling loop and returns one finished code draft, or an error.

**Request**:

```json
{
  "plugin_type": "metadata",
  "test_links": ["https://example.com/work/123", "https://example.com/work/456", "https://example.com/work/789"],
  "auxiliary_reference_urls": ["https://example.com/api/work/123.json"],
  "reference_sample_code": "export function pluginInfo() { ... }",
  "login_association": { "namespace": "login/example", "source": "existing" },
  "login_parameters": null,

  "previous_code": null,
  "previous_error": null
}
```

For `plugin_type: "login"`, `login_parameters` is instead populated (from `analyze-login`'s own
response, see above) and `test_links`/`reference_sample_code`/`login_association` are irrelevant:

```json
{
  "plugin_type": "login",
  "auxiliary_reference_urls": ["https://example.com/login"],
  "login_parameters": [{ "name": "api_key", "description": "API Key", "required": true }],
  "previous_code": null,
  "previous_error": null
}
```

AI must declare `login_parameters` verbatim as `pluginInfo().parameters` and read them
positionally from `execLogin`'s `hostArgs.customargs` — see `analyze-login`'s own contract section
above for why this list isn't AI's to invent at generation time.

There is deliberately no free-text "page feature description" field — the wizard collects only
links (metadata/download) or credentials (login), and the system prompt instructs AI to call
`fetch_page` against the real supplied link(s) itself and infer selectors/field extraction from
the actual returned HTML, rather than from a user-written description of the page. This was a
scope simplification made after the initial implementation (the field existed and was later
removed) — real page content is strictly more reliable than a user's manual description of it, and
removing the field also removes an input the UI otherwise needed one whole step/textarea for.

- `test_links[0]` (or `auxiliary_reference_urls[0]` if `test_links` is empty, which it never is for
  metadata/download per FR-005 — always present for those types) is what `fetch_page` fetches by
  default if AI's very first tool call doesn't specify a different URL; AI can call `fetch_page`
  again with any of the other supplied links/aux URLs, or none at all, per FR-010's "AI decides if
  it needs more information."
- `reference_sample_code`/`login_association` are omitted (`null`)/absent when not applicable (no
  existing same-domain sample per FR-009; type isn't login-dependent per FR-007).
- `previous_code`/`previous_error` populated only for an AI-auto-fix call (FR-017) — `null` for a
  fresh generation. This is the *entire* difference between "generate" and "AI auto-fix" as far as
  this endpoint is concerned: same endpoint, same loop, just a different starting prompt.
- The login type's request shape omits `test_links`/`reference_sample_code`'s relevance in the same
  way — it never carries login credentials (FR-012 — credentials only ever appear in a `trial-run`
  request body, never here).

**Response `200`**:

```json
{ "code": "export function pluginInfo() { ... }" }
```

**Response `422`** (AI's final output wasn't parseable as code — spec Edge Cases' "AI returns prose
instead of code" case):

```json
{ "error": "ai_output_not_code", "raw_output": "I couldn't find a clear title selector because..." }
```

**Response `503`** (LLM unavailable — FR-024):

```json
{ "error": "llm_unavailable", "detail": "DeepSeek API key not configured" }
```

### The `fetch_page` tool (internal to this endpoint's own loop, not separately exposed)

The one tool declared in the `tools` array sent to DeepSeek for this call:

```json
{
  "type": "function",
  "function": {
    "name": "fetch_page",
    "description": "Fetch a URL and return its final page content plus the full redirect trail followed to get there.",
    "parameters": {
      "type": "object",
      "properties": { "url": { "type": "string" } },
      "required": ["url"]
    }
  }
}
```

Tool result handed back to AI as a `role: "tool"` message (FR-011):

```json
{
  "requested_url": "https://example.com/work/123",
  "redirect_trail": ["https://example.com/work/123", "https://example.com/w/123"],
  "final_url": "https://example.com/w/123",
  "status": "ok",
  "http_status": 200,
  "content": "<html>...</html>"
}
```

or, on a redirect-cap-exceeded outcome (FR-011):

```json
{
  "requested_url": "https://example.com/work/123",
  "redirect_trail": ["https://example.com/a", "https://example.com/b", "... (cap reached)"],
  "status": "redirect_cap_exceeded"
}
```

## `POST /api/plugin-wizard/trial-run`

Stages the given code, executes it for real via `PluginPool` (research.md §2), always discards the
staging file afterward.

**Request** (metadata/download):

```json
{
  "plugin_type": "metadata",
  "code": "export function pluginInfo() { ... }",
  "test_links": ["https://example.com/work/123", "https://example.com/work/456", "https://example.com/work/789"]
}
```

**Request** (login — note `credentials` never appears in any other endpoint's request body, per
FR-012). `fields` is keyed by whatever field names `analyze-login` determined for this site
(`LoginParameter.name`) — no longer assumed to always be `account`/`secret`:

```json
{
  "plugin_type": "login",
  "code": "export function pluginInfo() { ... }",
  "credentials": { "fields": { "api_key": "..." } }
}
```

`trial_run.rs::customargs_for` converts `fields` into the positional `customargs` array
`execLogin` actually reads, ordered to match the staged draft's own probed `pluginInfo().
parameters` — the same convention every real installed plugin's `customargs` already follows, not
insertion/map order (which is unspecified). A metadata/download draft's `login_credentials` (the
FR-025 "this session's own fresh login draft" case) uses the identical `{ "fields": {...} }` shape.

**Response `200`** (metadata/download — per-link results, matching data-model.md's `TrialRunResult`
shape exactly):

```json
{
  "per_link": [
    { "link": "https://example.com/work/123", "outcome": "success", "data": { "title": "...", "tags": ["..."] } },
    { "link": "https://example.com/work/456", "outcome": "failure", "error": "404 Not Found" },
    { "link": "https://example.com/work/789", "outcome": "success", "data": { "title": "...", "tags": ["..."] } }
  ],
  "login_suggestion": {
    "relevant": false,
    "reasoning": "The failure is a 404, not an access-denied/redirect-to-login pattern."
  }
}
```

`login_suggestion` is present whenever `per_link` contains at least one failure (computed via a
second, focused `tool_chat()` call — no tools needed for this one, just a classification prompt over
the failure's own `error`/redirect-trail data — never via any local status-code/keyword heuristic,
per FR-010). Absent when every link succeeded.

**Response `200`** (login):

```json
{ "outcome": "success", "detail": "Login succeeded, cookies obtained." }
```

**Response `400`** (code doesn't parse / isn't a valid plugin — spec Edge Cases' "not legal code"
case, surfaced here rather than at `/generate` when the invalid code came from a manual edit):

```json
{ "error": "invalid_plugin_code", "detail": "pluginInfo() threw: SyntaxError: ..." }
```

## `POST /api/plugin-wizard/save`

Validates and installs a finished draft as a real plugin — the terminal action for one type
(FR-020), reusing `upload_plugin`'s own stage → `plugin_info()`-validate → move-into-category →
rollback-on-failure sequence via the shared `plugins::move_into_category` helper (research.md §2):
the code is still staged under a throwaway UUID filename first (so the `plugin_info()` probe can
run before committing to a final name), but unlike `upload_plugin` there's no from-scratch upload
validation needed beyond that probe, since the code already passed a real trial run.

**Request** (`filename` is the user-chosen file stem — no `.ts`, no path separators — that
determines the plugin's real on-disk namespace, `plugins/custom/<type>/<filename>.ts`; the wizard
UI defaults it to a sanitized form of the domain the user originally looked up, editable before
saving so FR-021's rename-and-retry has something concrete to retry into):

```json
{
  "plugin_type": "metadata",
  "code": "export function pluginInfo() { ... }",
  "filename": "example_com"
}
```

**Response `200`**:

```json
{ "namespace": "metadata/example" }
```

**Response `409`** (namespace conflict — FR-021):

```json
{ "error": "namespace_conflict", "namespace": "metadata/example" }
```

**Response `400`/`500`**: same failure-leaves-nothing-behind guarantee as `upload_plugin` (FR-022) —
any error here is returned before any file is moved into a real category directory, or the partial
write is rolled back before the response is sent.

## `GET /api/plugins/export?namespace=...`

Not a wizard-only endpoint — works for any installed plugin, wizard-created or hand-written
(FR-028). Downloads a `.zip` containing just that plugin's own `.ts` file, named after the
namespace's own last path component (`custom/metadata/foo` → `foo.zip` containing `foo.ts`).
Exists in this feature's contract set because FR-030/FR-031 (a wizard-saved plugin may only live
inside an ephemeral container writable layer if the deployment's plugin directory isn't
host-mounted) make it the practical mitigation the wizard's own post-save reminder points at — but
the endpoint itself lives in `plugins.rs` alongside the rest of plugin management, not under
`/plugin-wizard/*`, and is surfaced from the existing Plugins page (`PluginCard.tsx`), not from the
wizard UI.

**Response `200`**: `application/zip` body, `Content-Disposition: attachment; filename="<name>.zip"`.

**Response `400`**: invalid namespace (path traversal shape rejected the same way
`lanrurugi-plugin::pool::is_safe_namespace` rejects it for the sandbox itself).

**Response `404`**: no such plugin file on disk.
