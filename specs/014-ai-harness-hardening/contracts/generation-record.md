# Contract: Generation Record

**Feature**: `014-ai-harness-hardening` | **Story**: US6 | **Requirements**: FR-022, FR-024…FR-027, FR-031

## Purpose

Make one question answerable after the fact: *what exactly was the model shown, round by round, when
it produced this plugin?* Today the generation request body lives in a single-use in-memory slot that
is destroyed on first read, and the per-round tool results live only in the loop's local vector — so
a bug report of the form "the wizard wrote a plugin that does not work" is unreproducible by
construction.

This is deliberately **not** a general session log. It covers one generation action of one feature,
because that is the only feature in this codebase with rounds.

## Stored shape

Key `LANRURUGI_GENERATION_RECORD:<generation_id>` (built from a `GenerationId` newtype), a list, appended in round order, TTL 30 days.

Elements, in append order (as implemented in `crates/lanrurugi-api/src/plugin_wizard/record.rs`):

| `kind` | Written | Holds |
| --- | --- | --- |
| `header` | once, first | the generation request with `credential_values` / `login_credentials` / `plugin_parameter_values` reduced to `{value: "{{name}}", supplied}` placeholders, the `model`, the request `params`, and the full `system_prompt` |
| `request` | before each round is sent | `round` and the `messages` added since the previous element — the header's prompt plus every `request`'s and `tool_round`'s messages, in order, is the full conversation as sent |
| `tool_round` | after a round that requested tools | the `tool_calls` (`{id, name, arguments}`, arguments as the model wrote them — placeholders only) and `tool_results` exactly as the model was shown them, bounded and framed |
| `final` | after the round that answered | `outcome` (`ok` / `ai_output_not_code`) and the model's `content` |
| `failed_attempt` | for a retried, failed, cancelled or budget-stopped round | `round` and `outcome` (`retried`, a model-call class, `cancelled`, `context_budget_exceeded`) — never content the model was shown |
| `trial_run` | when a trial run names this generation | `trial: {plugin_type, http_status, result}`, the result including its attribution |

Every element carries `at` (epoch ms); the read surface adds `index`. Every element passes a final
redaction that replaces any known credential value with its placeholder before it is stored.

A trial run carries the `generation_id` of the generation that produced its draft; without one it
appends nothing.

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
7. **May hold logged-in page content.** "Exactly what the model saw" includes bounded pages fetched
   with the user's credential, so the record can contain content from behind the user's login for its
   30-day retention. That is why invariant 6 matters and why the record has no UI or export.

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
