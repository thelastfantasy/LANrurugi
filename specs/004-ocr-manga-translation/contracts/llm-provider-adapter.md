# Contract: LLM Provider Adapter (internal)

Three adapters implement this contract (research.md §5, §15); callers in `lanrurugi-translate`
depend only on this shape, never on a provider-specific one directly.

**Batched, not per-block** (research.md §15 — supersedes an earlier draft of this contract that
had `source_text`/`translated_text` as single strings): one request carries the text blocks of a
small fixed group of pages (2–4, mirroring T009's OCR batch size), each tagged with a `block_id`
so responses map back unambiguously. Batching amortizes the fixed per-request cost (HTTP
round-trip, repeated glossary prefix) across several blocks, at the cost of failure now being
batch-granular rather than per-block (FR-019/FR-020 apply at batch granularity).

## Normalized request (adapter input)

```json
{
  "context": {
    "glossary_matches": [["source term", "chosen translation"], "..."],
    "known_names": ["source term", "..."],
    "tone_reference": [["source text", "translated text"], "..."]
  },
  "blocks": [
    { "block_id": "string", "source_text": "string" },
    "..."
  ],
  "target_language": "string (BCP-47)",
  "source_language_hint": "string?"
}
```

`context` (spec.md FR-007b/c/e, research.md §14/§15) is assembled server-side before the adapter
call, never left empty-by-omission when material exists — its three parts, in the fixed order
above:
1. `glossary_matches`: Terminology Glossary entries whose source term is an exact substring of a
   block in this batch (source term → chosen translation pairs) — the deterministic fast-path
   reuse (FR-007b).
2. `known_names`: the volume's other known Terminology Glossary source names (names only, not
   their translations) — lets the backend itself recognize a nickname/initialism variant not
   caught by (1) and reuse that name's established translation (FR-007c).
3. `tone_reference`: the current page's other already-translated text blocks (source text +
   chosen translation) — advisory tone/style reference only; LANrurugi performs no tone
   classification of its own (FR-007e).

Each of these three is independently optional (e.g. the first block translated on a page has
nothing yet for `tone_reference`). **Content ordering is load-bearing, not stylistic**: every
provider here caches by exact-prefix match (research.md §15/§18), so the request is rendered
stable-content-first (the constant system prompt, then `context` in the fixed order above),
dynamic-content-last (this batch's own `blocks`) — reordering would silently destroy the cache hit
even with byte-identical content, since a prefix mismatch anywhere invalidates the whole cache
entry for every provider covered here.

## Normalized response (adapter output)

```json
{
  "blocks": [
    { "block_id": "string", "translated_text": "string" },
    "..."
  ],
  "provider_latency_ms": 0
}
```

or, on failure, a normalized error (`unreachable`, `auth_failed`, `rate_limited`,
`malformed_response`) applying to the whole batch — never a raw provider-specific error shape
leaking to callers, so `lanrurugi-translate`'s failure-handling (FR-019/FR-020) doesn't need
per-provider branching. A response's `blocks` MUST be mapped back to request blocks by `block_id`,
never by array position (a provider is not guaranteed to preserve request order).

## OpenAI-compatible adapter (covers OpenAI-compatible providers and Ollama)

- Maps the normalized request to a Chat Completions-shaped request (`messages` array,
  `Authorization: Bearer <key>`), with a constant per-target-language system prompt instructing
  the model to return a JSON object mapping each `block_id` to its translation.
- Ollama is configured as an instance of this same adapter, pointed at Ollama's own
  OpenAI-compatible `/v1` endpoint — not a separate adapter.
- No explicit cache-control marker — OpenAI's caching is automatic prefix-matching, relying
  entirely on the stable-content-first ordering above (research.md §15).

## Anthropic adapter

- Maps the normalized request to Anthropic's Messages API shape: `system` as a top-level field
  (not a message), `content` as a content-block array, `x-api-key` + `anthropic-version` headers,
  mandatory `max_tokens`, and translates Anthropic's distinct SSE event shape back into this
  contract's normalized response if streaming is used.
- Marks an explicit `cache_control` breakpoint after the stable `context`-derived prefix
  (research.md §15) — Anthropic's caching requires this explicit marker, unlike the other two
  adapters.

## DeepSeek adapter

- OpenAI-Chat-Completions-shaped wire format (like the OpenAI-compatible adapter), but documented
  as its own adapter rather than folded into that one because its caching characteristics differ
  meaningfully (research.md §15/§18): fully automatic, disk-backed context caching with **no
  opt-in field at all** (not even OpenAI's implicit automatic-prefix-matching framing quite
  applies — DeepSeek creates cache units at additional points beyond just the request prefix, per
  its own documentation), and a substantially larger cache-hit discount (~31x cheaper on a hit vs.
  a miss for `deepseek-v4-flash`, vs. the ~10x discount Anthropic/OpenAI offer).

## Secret handling

Whichever adapter is used, the credential is resolved server-side from `credential_ref`
(data-model.md) immediately before the provider call and is never included in any log line,
error message, or API response returned to the browser (constitution Principle V).
