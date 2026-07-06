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
