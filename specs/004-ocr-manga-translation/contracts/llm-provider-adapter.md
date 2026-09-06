# Contract: LLM Provider Adapter (internal)

Two adapters implement this contract (research.md §5); callers in `lanrurugi-translate` depend
only on this shape, never on a provider-specific one directly.

## Normalized request (adapter input)

```json
{
  "source_text": "string",
  "source_language_hint": "string?",
  "target_language": "string (BCP-47)",
  "context": "string?"
}
```

`context` (spec.md FR-007b/c/e, research.md §14) is assembled server-side before the adapter call,
never left empty-by-omission when material exists — composed of, in order:
1. Any Terminology Glossary entries whose source term is an exact substring of `source_text`
   (source term → chosen translation pairs) — the deterministic fast-path reuse (FR-007b).
2. The volume's other known Terminology Glossary source names (names only, not their
   translations) — lets the backend itself recognize a nickname/initialism variant not caught by
   (1) and reuse that name's established translation (FR-007c).
3. The current page's other already-translated text blocks (source text + chosen translation) —
   advisory tone/style reference only; LANrurugi performs no tone classification of its own
   (FR-007e).
Each of these three is independently optional (e.g. the first block translated on a page has
nothing yet for (3)); the adapter contract's `context` field itself doesn't change shape from a
caller's perspective — assembly happens once, in `lanrurugi-translate`, before any adapter sees
the request.

## Normalized response (adapter output)

```json
{
  "translated_text": "string",
  "provider_latency_ms": 0
}
```

or, on failure, a normalized error (`unreachable`, `auth_failed`, `rate_limited`,
`malformed_response`) — never a raw provider-specific error shape leaking to callers, so
`lanrurugi-translate`'s failure-handling (FR-019/FR-020) doesn't need per-provider branching.

## OpenAI-compatible adapter (covers OpenAI-compatible providers and Ollama)

- Maps the normalized request to a Chat Completions-shaped request (`messages` array,
  `Authorization: Bearer <key>`).
- Ollama is configured as an instance of this same adapter, pointed at Ollama's own
  OpenAI-compatible `/v1` endpoint — not a separate adapter.

## Anthropic adapter

- Maps the normalized request to Anthropic's Messages API shape: `system` as a top-level field
  (not a message), `content` as a content-block array, `x-api-key` + `anthropic-version` headers,
  mandatory `max_tokens`, and translates Anthropic's distinct SSE event shape back into this
  contract's normalized response if streaming is used.

## Secret handling

Whichever adapter is used, the credential is resolved server-side from `credential_ref`
(data-model.md) immediately before the provider call and is never included in any log line,
error message, or API response returned to the browser (constitution Principle V).
