# Contract: Client-Side Compositing & Cache (locally-hosted-backend path)

Governs `apps/frontend/src/translation/` — used only when the active backend for a given device is a
locally-hosted one (research.md §6/§7/§8).

## Inputs (from server, via `contracts/translation-api.md`)

- `Detected Text Region` records (position + source text) for the requested page.
- The volume's current `Volume Font Pattern` golden set (font names/references).
- The same `context` composition the server assembles for the cloud-backend path (FR-007b/c/e,
  `contracts/llm-provider-adapter.md`) — the `text-regions` endpoint includes it so the
  locally-hosted-backend path gets the same Terminology Glossary/tone-reference benefit despite
  never routing its translation call through the server. Without this, a locally-hosted-backend
  user would get none of FR-007a–e's consistency guarantees, since the Terminology Glossary itself
  is server-stored (data-model.md) and the browser has no other way to reach it.

## Local translation call

- The browser calls the configured locally-hosted backend directly (e.g. `http://127.0.0.1:11434`
  in Ollama's OpenAI-compatible shape), per constitution Principle V — this call never goes
  through the LANrurugi server. The `context` received above MUST be included in this direct call
  the same way the server includes it for the cloud path (`contracts/llm-provider-adapter.md`).
- On connection failure (e.g. Private Network Access blocked), surface the FR-018 guided-fallback
  UI (research.md §9) rather than a generic error.
- The translation the browser receives back MUST still be reported to the server (a lightweight
  "record this translation" call, not the translation call itself) so a new name/term the local
  backend translates is captured into the shared, server-stored Terminology Glossary (FR-007a) —
  otherwise glossary entries discovered via the local path would never benefit later cloud-path (or
  other-device local-path) requests for the same volume.

## Compositing

- Draw each translated region's text, in the matched golden-set font, over the original page
  image using Canvas/OffscreenCanvas (research.md §7) — no WASM dependency.
- Apply each region's own estimated `fg_color`/`bg_color`/`is_bold` (FR-008a, research.md §13)
  when present; each falls back independently to a safe default when the server didn't estimate it
  with confidence — a missing `is_bold` MUST NOT block using an estimated `fg_color` on the same
  region, or vice versa.
- Respect the LTR-only scope (research.md §11) for text shaping/line-wrapping.

## Cache

- Store the composited result (as a Blob/PNG) in IndexedDB or the Cache API, keyed by
  (`archive_id`, `page_number`, `target_language`, `local_provider_identifier`) — mirroring the
  server-side key shape in `data-model.md`'s Translation Cache Entry, but entirely local to this
  browser/device.
- No OS filesystem write, no permission prompt required (research.md §7) — this is standard
  origin-scoped browser storage.
- A change to target language or the local backend identifier MUST NOT reuse a cache entry keyed
  under a different combination (mirrors FR-016).
