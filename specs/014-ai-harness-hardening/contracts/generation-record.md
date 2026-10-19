# Contract: Generation Record

**Feature**: `014-ai-harness-hardening` | **Story**: US6 | **Requirements**: FR-024…FR-027

## Purpose

Make one question answerable after the fact: *what exactly was the model shown, round by round, when
it produced this plugin?* Today the generation request body lives in a single-use in-memory slot that
is destroyed on first read, and the per-round tool results live only in the loop's local vector — so
a bug report of the form "the wizard wrote a plugin that does not work" is unreproducible by
construction.

This is deliberately **not** a general session log. It covers one generation action of one feature,
because that is the only feature in this codebase with rounds.

## Stored shape

Key `LANRURUGI_GENERATION_RECORD:<generation_id>`, a list, appended in round order, TTL 30 days.

Each element is one **attempt**:

| Field | Type | Notes |
| --- | --- | --- |
| `index` | integer | 0-based, append order |
| `kind` | `request` \| `tool_round` \| `failed_attempt` | A failed/retried round is its own kind, never presented as content the model was shown |
| `tool_calls` | list of `{name, arguments}` | Credentialed header arguments stored in **placeholder form** (`{{field}}`), never substituted |
| `tool_result` | optional object | The bounded result exactly as the model saw it, including `truncated`/`refused` status and any `withheld_credentials_from_hop` |
| `outcome` | model-call outcome | Failure classes recorded, not elided |
| `at` | timestamp | Ordering safeguard for concurrent writers |

Plus a header element (index 0, kind `request`) holding the generation request body with
`credential_values` / `login_credentials` / `plugin_parameter_values` reduced to their placeholder
forms and a boolean per field indicating "was supplied".

## Invariants

1. **No cleartext credential.** A value the user typed into a credential field never appears. The
   placeholder form is the only representation stored (FR-026). This is asserted by a scan test, not
   by review.
2. **Written before issued.** The attempt is appended before its request is sent, so a crash mid-round
   leaves the intent rather than nothing.
3. **Attempts are not messages.** A failed attempt is stored with its own `kind`; no consumer may
   render it as something the model was told (FR-025).
4. **Bounded by construction.** Because only the bounded tool result is stored, the record inherits the
   context bound and cannot grow to hold raw pages.
5. **Isolated expiry.** TTL only; expiry touches no other key and changes no user-visible behaviour
   (FR-027).
6. **Same authority as the wizard.** Readable only by whatever the wizard routes themselves require;
   no new permission surface.

## Read surface

```http
GET /api/plugin-wizard/generate/{id}/record
```

Returns `{ "generation_id": "...", "attempts": [ ... ] }` in index order.
`404` when the record is absent (never written, expired, or the id was never issued).
Denied for guests and token-scoped callers by the existing plugin-wizard route policy rows.

The endpoint is the whole read surface for this feature. **No UI is required**: it exists so a
maintainer can answer the question from a bug report, which is the failing scenario in the spec. A
UI affordance may be added later and is explicitly out of this contract.

## Non-goals

- Not a transcript for translation, rename suggestions, split suggestions, backfill or recommend. Those
  are single-call features; their answer already exists in their own persisted artefacts (split
  suggestions and translation regions are stored) or in the usage ledger's aggregates.
- Not an audit log: it is not tamper-evident, not queryable across generations, and not retained for
  compliance. It exists to make one class of report reproducible.
- Not a resume mechanism. Resuming a generation after a restart is out of scope; the record makes the
  *past* inspectable, it does not make an interrupted run continuable.
