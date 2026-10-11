// Device-local backend selection and its precedence over the server default
// (FR-003, research.md §8).
//
// Why this is client-side at all: a locally-hosted backend selection points at *this* device's own
// loopback address. Storing it server-side would make it apply on every other device too, where
// "localhost" is a different machine entirely — actively wrong, not merely unhelpful. Cloud/API-key
// selections have no such locality and stay server-side with the rest of the settings (and, per
// constitution Principle V, so that a provider credential never reaches the browser).

import type { BackendCategory, LocalBackendConfig, TranslationSettings } from "./types"

/** `localStorage` key holding this device's own locally-hosted backend config, if any. */
export const LOCAL_BACKEND_KEY = "translationLocalBackend"

/**
 * Reads this device's local backend override.
 *
 * Returns `null` on absent, malformed, or incomplete data rather than throwing — a corrupted entry
 * should degrade to "no local override configured" (and fall back to the server default), never
 * break the reader.
 */
export function getLocalBackend(): LocalBackendConfig | null {
  let raw: string | null
  try {
    raw = window.localStorage.getItem(LOCAL_BACKEND_KEY)
  } catch {
    // Private-mode/disabled storage: behave as if nothing is configured.
    return null
  }
  if (!raw) return null

  try {
    const parsed = JSON.parse(raw) as Partial<LocalBackendConfig>
    if (
      typeof parsed?.endpoint !== "string" ||
      typeof parsed?.model !== "string" ||
      !parsed.endpoint.trim() ||
      !parsed.model.trim()
    ) {
      return null
    }
    return {
      endpoint: parsed.endpoint.trim(),
      model: parsed.model.trim(),
      identifier:
        typeof parsed.identifier === "string" && parsed.identifier.trim()
          ? parsed.identifier.trim()
          : deriveIdentifier(parsed.endpoint, parsed.model),
    }
  } catch {
    return null
  }
}

export function setLocalBackend(config: LocalBackendConfig | null): void {
  try {
    if (config === null) {
      window.localStorage.removeItem(LOCAL_BACKEND_KEY)
      return
    }
    window.localStorage.setItem(
      LOCAL_BACKEND_KEY,
      JSON.stringify({
        ...config,
        identifier: config.identifier || deriveIdentifier(config.endpoint, config.model),
      }),
    )
  } catch {
    // Storage unavailable — the selection simply doesn't persist for this session.
  }
}

/** Stable per-backend identifier used in the client-side cache key, mirroring the server's own
 * provider component so switching local models can't reuse another model's cached renders. */
function deriveIdentifier(endpoint: string, model: string): string {
  return `local:${endpoint.replace(/\/+$/, "")}:${model}`
}

/** Which backend is active on this device (FR-003's precedence rule). */
export type ActiveBackend =
  | { category: Extract<BackendCategory, "local">; config: LocalBackendConfig }
  | { category: Extract<BackendCategory, "cloud">; provider: string }
  | { category: "none" }

/**
 * Resolves the backend to use on this device.
 *
 * A device-local selection takes precedence over the server-stored default, so configuring a local
 * model on one device doesn't require changing (or disturbing) the account-wide cloud default.
 */
export function resolveActiveBackend(
  serverSettings: Pick<TranslationSettings, "provider"> | null | undefined,
): ActiveBackend {
  const local = getLocalBackend()
  if (local) {
    return { category: "local", config: local }
  }
  if (serverSettings?.provider) {
    return { category: "cloud", provider: serverSettings.provider }
  }
  // Translation enabled with nothing configured: the caller guides the user to configuration
  // rather than silently failing (FR-021).
  return { category: "none" }
}

/**
 * The effective target language.
 *
 * An unset preference falls back to the *browser's* own language, deliberately not the app's
 * interface language — these are separate settings (FR-004).
 */
export function resolveTargetLanguage(explicit: string | null | undefined): string {
  if (explicit && explicit.trim()) return explicit.trim()
  return navigator.language || "en"
}

/**
 * Target-language choices offered in the reader's translation menu (LTR only, research.md §11).
 *
 * Deliberately not `i18n`'s `SUPPORTED_LANGUAGES` — that list's `code`s (`zh`, `zh_Hant`, `nb_NO`,
 * ...) are i18next's own locale-file naming, not standard BCP-47 tags, and the target language is
 * sent to the LLM inline in a natural-language prompt (`llm_prompts.rs`'s "翻译成{target_language}"),
 * not looked up against any locale table server-side — a value the model won't reliably recognise
 * (or that reads as a code rather than a language name) would degrade translation quality with no
 * error to surface. These use the standard tags/names already exercised in production (`zh-CN`).
 */
export const TRANSLATION_TARGET_LANGUAGES: readonly { code: string; nativeName: string }[] = [
  { code: "zh-CN", nativeName: "简体中文" },
  { code: "zh-TW", nativeName: "繁體中文" },
  { code: "en", nativeName: "English" },
  { code: "ko", nativeName: "한국어" },
  { code: "fr", nativeName: "Français" },
  { code: "de", nativeName: "Deutsch" },
  { code: "es", nativeName: "Español" },
  { code: "it", nativeName: "Italiano" },
  { code: "pt", nativeName: "Português" },
  { code: "vi", nativeName: "Tiếng Việt" },
  { code: "id", nativeName: "Bahasa Indonesia" },
]
