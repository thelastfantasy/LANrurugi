// Shared types for the on-page manga translation frontend
// (`specs/004-ocr-manga-translation`). Mirrors the server-side shapes in
// `crates/lanrurugi-translate` and `crates/lanrurugi-ocr`.

/** Which category of backend a selection refers to (data-model.md). */
export type BackendCategory = "cloud" | "local"

/** Cloud providers the server knows how to proxy. Local backends are not in this list — they are
 * never proxied and never identified to the server by provider name. */
export type CloudProvider = "openai-compatible" | "anthropic" | "deepseek"

/** An RGB colour estimated from a region's own pixels (FR-008a). */
export interface Rgb {
  r: number
  g: number
  b: number
}

export interface BoundingBox {
  x: number
  y: number
  w: number
  h: number
}

/**
 * A merged block of text on a page — the unit of translation and font matching.
 *
 * `fgColor`/`bgColor`/`isBold` are each independently absent when the server's heuristic couldn't
 * estimate them with confidence. A missing one must never suppress the others when compositing
 * (FR-008a).
 */
export interface DetectedTextRegion {
  archiveId: string
  pageNumber: number
  boundingBox: BoundingBox
  sourceText: string
  isCover: boolean
  fgColor?: Rgb
  bgColor?: Rgb
  /** `undefined` means "couldn't tell" — deliberately distinct from `false` ("estimated not bold"). */
  isBold?: boolean
  font?: string
  translatedText?: string
}

/**
 * The stable, cacheable prefix the server assembles for a translation request (FR-007b/c/e).
 *
 * The local-backend path receives this from `text-regions` and must include it in its own direct
 * call, otherwise a locally-hosted backend would get none of the glossary/tone consistency the
 * cloud path gets.
 */
export interface TranslationContext {
  /** Glossary terms appearing verbatim in this page's text, with their established translations. */
  glossaryMatches: [string, string][]
  /** The volume's other known names, names only — for nickname/initialism recognition. */
  knownNames: string[]
  /** Already-translated blocks on this page, as tone reference only. */
  toneReference: [string, string][]
}

/** Server response for the local-backend compositing path. */
export interface TextRegionsResponse {
  regions: DetectedTextRegion[]
  /** The volume's locked golden font set, empty while the pattern is still voting. */
  goldenSet: string[]
  context: TranslationContext
  volumeId: string
}

/** A locally-hosted backend, configured per device (FR-003). */
export interface LocalBackendConfig {
  /** e.g. `http://127.0.0.1:11434/v1` — an Ollama OpenAI-compatible endpoint. */
  endpoint: string
  model: string
  /** Distinguishes this backend in the client-side cache key, mirroring the server key shape. */
  identifier: string
}

/** Server-stored settings (the cloud half only — a local selection never leaves the device).
 * Deliberately no `enabled` field — whether translation is on is scoped per-archive/per-Tankoubon
 * (see {@link TranslationScope}), not a single account-wide switch (turning it on for one book
 * must never turn it on for every unrelated archive in the library). */
export interface TranslationSettings {
  provider: CloudProvider | null
  endpoint: string | null
  model: string | null
  /** `null` means "fall back to the browser's own language" (FR-004). */
  targetLanguage: string | null
  lookaheadPages: number
  batchPages: number
  /** Whether a credential is stored — never the credential itself (FR-006). */
  credentialSet: boolean
  /**
   * True when the *currently saved* `provider`'s key comes from the project-wide DeepSeek setting
   * rather than a translation-specific one. Derived server-side from the provider already
   * persisted — not useful for reacting to a provider the user just picked in the form but hasn't
   * saved yet. Use `globalDeepseekKeySet` (below) for that.
   */
  usesGlobalKey: boolean
  /**
   * Whether the project-wide DeepSeek key exists at all, independent of which provider is
   * currently saved. Combine with `provider === "deepseek"` (including an as-yet-unsaved selection
   * in the form) to decide whether to hide the API-key input — a second DeepSeek key for the same
   * account would just be a competing source of truth.
   */
  globalDeepseekKeySet: boolean
}

/** Whether translation is on for one archive, and whether that's this archive's own switch or
 * inherited from a Tankoubon it belongs to. `scope === "tankoubon"` is a hint the reader can show
 * the user ("this follows the whole book's setting") rather than presenting the toggle as if it
 * only ever affects the one file currently open. */
export interface TranslationScope {
  enabled: boolean
  scope: "archive" | "tankoubon"
}

/** Consumption at the four granularities FR-014 requires. */
export interface UsageSnapshot {
  provider: string
  limit: number | null
  consumptionCurrentPage: number
  consumptionCurrentArchive: number
  consumptionToday: number
  consumptionCurrentWeek: number
}

/** Normalized failure kinds — no raw provider error shape ever reaches the UI. */
export type TranslationErrorKind =
  | "unreachable"
  | "auth_failed"
  | "rate_limited"
  | "malformed_response"
  | "not_configured"
  | "disabled"

/** Per-page translation state driving the reader overlay (FR-012/FR-019). */
export type TranslationPageState =
  | { status: "idle" }
  | { status: "pending" }
  | { status: "ready"; imageUrl: string }
  | { status: "failed"; kind: TranslationErrorKind; message: string }
