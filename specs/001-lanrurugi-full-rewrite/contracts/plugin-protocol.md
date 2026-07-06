# Contract: Plugin Host ↔ Deno Subprocess Protocol (US4, constitution Principle IV)

This is the interface plugin authors and the `lanrurugi-plugin` crate both depend on — a genuine
contract in the same sense as the REST API, since third-party plugin authors will write against
it.

## Transport

Newline-delimited JSON over the subprocess's stdin/stdout. One JSON object per line, both
directions. The Deno subprocess runs a small, LANrurugi-provided dispatcher script; individual
plugins are `import()`-ed by the dispatcher on demand, not run as their own subprocess per call
(research.md §7 — persistent worker pool, not per-invocation spawn).

## Request (host → subprocess)

```json
{
  "request_id": "uuid",
  "plugin": "namespace-of-plugin",
  "method": "plugin_info" | "exec_metadata" | "exec_login" | "exec_download",
  "args": { "...": "method-specific" }
}
```

## Response (subprocess → host)

```json
{
  "request_id": "uuid",
  "ok": true,
  "result": { "...": "method-specific" }
}
```

or, on failure:

```json
{
  "request_id": "uuid",
  "ok": false,
  "error": { "message": "string", "kind": "timeout" | "plugin_error" | "permission_denied" }
}
```

A failed/timed-out response MUST NOT crash the dispatcher process or affect any other
in-flight `request_id` (FR-013 — failure isolation).

## `plugin_info` method

Returns the plugin's declared metadata and required permissions, analogous to legacy Perl
plugins' `plugin_info()`:

```json
{
  "namespace": "string",
  "type": "metadata" | "login" | "download",
  "parameters": [ { "name": "string", "description": "string", "required": true } ],
  "declared_permissions": { "net": ["api.example.com"], "read": false, "write": false }
}
```

`declared_permissions` is read by the host **before** the plugin's subprocess/pool is started and
is used to construct the Deno CLI permission flags (`--allow-net=api.example.com`, and no other
`--allow-*` flags unless declared) — the host MUST NOT grant broader permissions than declared
(constitution Principle IV, FR-014).

## `exec_metadata` method

Input: `{ "archive_id": "string", "first_page_path": "string", "parameters": {...} }` (the host
extracts and hands over only what a metadata plugin needs — the archive's own files are not
directly exposed to the plugin's filesystem permissions unless a permission explicitly requires
it).

Output: `{ "new_tags": "string?", "title": "string?", "summary": "string?" }` (mirrors legacy
`exec_metadata_plugin`'s result shape — `new_tags`/`title`/`summary`, all optional).

## Timeout

The host enforces a per-request timeout (mirrors the ingestion-side per-file timeout pattern in
research.md §6); a timeout produces an `"error": {"kind": "timeout"}` response and does not retry
automatically.
