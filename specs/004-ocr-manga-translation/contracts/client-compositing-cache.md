# Contract: Client-Side Compositing & Cache (locally-hosted-backend path)

Governs `apps/frontend/src/translation/` — used only when the active backend for a given device is a
locally-hosted one (research.md §6/§7/§8).

## Inputs (from server, via `contracts/translation-api.md`)

- `Detected Text Region` records (position + source text) for the requested page.
- The volume's current `Volume Font Pattern` golden set (font names/references).

## Local translation call

- The browser calls the configured locally-hosted backend directly (e.g. `http://127.0.0.1:11434`
  in Ollama's OpenAI-compatible shape), per constitution Principle V — this call never goes
  through the LANrurugi server.
- On connection failure (e.g. Private Network Access blocked), surface the FR-018 guided-fallback
  UI (research.md §9) rather than a generic error.

## Compositing

- Draw each translated region's text, in the matched golden-set font, over the original page
  image using Canvas/OffscreenCanvas (research.md §7) — no WASM dependency.
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
