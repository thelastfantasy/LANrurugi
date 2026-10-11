// Server calls for on-page manga translation (`contracts/translation-api.md`).
//
// Note what is deliberately absent: there is no endpoint here for configuring a locally-hosted
// backend, and none that carries a provider API key back to the browser. Local configuration lives
// in `localStorage` (`settings.ts`), and a cloud credential never leaves the server (FR-006).

import { fetchJson, sendJson } from "../api/client"
import type { TextRegionsResponse, TranslationScope, TranslationSettings, UsageSnapshot } from "./types"

export async function fetchTranslationSettings(): Promise<TranslationSettings> {
  return fetchJson<TranslationSettings>("/translation/settings")
}

/** Fields omitted are left unchanged. `apiKey` is write-only and never returned. */
export interface UpdateTranslationSettings {
  provider?: string | null
  endpoint?: string | null
  model?: string | null
  targetLanguage?: string | null
  lookaheadPages?: number
  batchPages?: number
  apiKey?: string
}

export async function updateTranslationSettings(
  patch: UpdateTranslationSettings,
): Promise<TranslationSettings> {
  return sendJson<TranslationSettings>("PUT", "/translation/settings", patch)
}

/** Whether translation is on for `archiveId` — per-archive/per-Tankoubon, not account-wide. */
export async function fetchTranslationScope(archiveId: string): Promise<TranslationScope> {
  return fetchJson<TranslationScope>(`/archives/${encodeURIComponent(archiveId)}/translation/scope`)
}

export async function updateTranslationScope(
  archiveId: string,
  enabled: boolean,
): Promise<TranslationScope> {
  return sendJson<TranslationScope>(
    "PUT",
    `/archives/${encodeURIComponent(archiveId)}/translation/scope`,
    { enabled },
  )
}

export async function fetchUsage(archiveId?: string, page?: number): Promise<UsageSnapshot> {
  const params = new URLSearchParams()
  if (archiveId) params.set("archiveId", archiveId)
  if (page !== undefined) params.set("page", String(page))
  const query = params.toString()
  return fetchJson<UsageSnapshot>(`/translation/usage${query ? `?${query}` : ""}`)
}

export async function updateBudget(limit: number | null): Promise<void> {
  await sendJson("PUT", "/translation/budget", { limit })
}

/**
 * Detection output plus the context the local-backend path needs.
 *
 * Returns `null` when the server reports the page isn't processed yet (HTTP 202) — the caller shows
 * the non-blocking loading state and retries, rather than treating this as a failure (FR-012).
 */
export async function fetchTextRegions(
  archiveId: string,
  page: number,
): Promise<TextRegionsResponse | null> {
  const response = await fetch(`/api/archives/${encodeURIComponent(archiveId)}/page/${page}/text-regions`)
  if (response.status === 202) return null
  if (!response.ok) throw new Error(`text-regions failed: ${response.status}`)
  return (await response.json()) as TextRegionsResponse
}

/** Outcome of asking the server for a composited page (cloud-backend path). */
export type PageTranslationResult =
  | { status: "ready"; blob: Blob }
  | { status: "pending" }
  | { status: "unavailable"; kind: string; message: string }

export async function fetchPageTranslation(
  archiveId: string,
  page: number,
  targetLanguage: string,
): Promise<PageTranslationResult> {
  const response = await fetch(
    `/api/archives/${encodeURIComponent(archiveId)}/page/${page}/translation?lang=${encodeURIComponent(targetLanguage)}`,
  )

  if (response.status === 202) return { status: "pending" }

  if (!response.ok) {
    // A failure here must never block reading — the caller shows the original page with a
    // per-page indicator (FR-019).
    const body = (await response.json().catch(() => null)) as
      | { kind?: string; error?: string }
      | null
    return {
      status: "unavailable",
      kind: body?.kind ?? "unreachable",
      message: body?.error ?? "translation unavailable",
    }
  }

  return { status: "ready", blob: await response.blob() }
}

/**
 * Reports a locally-obtained translation so discovered terms reach the shared glossary (FR-007a).
 *
 * The translation call itself never goes through the server (Principle V); this is a lightweight
 * write only.
 */
export async function recordLocalTranslation(
  archiveId: string,
  page: number,
  payload: {
    translations: { sourceText: string; translatedText: string }[]
    targetLanguage: string
    provider: string
  },
): Promise<{ capturedTerms: string[] }> {
  return sendJson<{ capturedTerms: string[] }>(
    "POST",
    `/archives/${encodeURIComponent(archiveId)}/page/${page}/translation/record`,
    payload,
  )
}

/** One (archive, chapter) source's translation for a glossary term (issue #105) — a term usually
 * has exactly one of these, but a same-named term seen in unrelated archives/chapters of the same
 * volume can legitimately need different translations, so the backend returns every candidate
 * rather than collapsing them into one. */
export interface GlossaryEntry {
  translation: string
  archive_id: string
  chapter_name: string | null
}

export interface GlossaryResponse {
  volumeId: string
  entries: Record<string, GlossaryEntry[]>
}

export async function fetchGlossary(volumeId: string): Promise<GlossaryResponse> {
  return fetchJson<GlossaryResponse>(
    `/volumes/${encodeURIComponent(volumeId)}/terminology-glossary`,
  )
}

export async function updateGlossaryEntry(
  volumeId: string,
  term: string,
  translation: string,
): Promise<void> {
  await sendJson(
    "PUT",
    `/volumes/${encodeURIComponent(volumeId)}/terminology-glossary/${encodeURIComponent(term)}`,
    { translation },
  )
}

export async function deleteGlossaryEntry(volumeId: string, term: string): Promise<void> {
  await sendJson(
    "DELETE",
    `/volumes/${encodeURIComponent(volumeId)}/terminology-glossary/${encodeURIComponent(term)}`,
  )
}

export interface FontPatternResponse {
  volumeId: string
  isLocked: boolean
  goldenSet: string[]
  voteTotal?: number
}

export async function fetchFontPattern(volumeId: string): Promise<FontPatternResponse> {
  return fetchJson<FontPatternResponse>(`/volumes/${encodeURIComponent(volumeId)}/font-pattern`)
}

/** Clears a volume's font pattern so it re-votes from scratch (FR-010). */
export async function resetFontPattern(volumeId: string): Promise<FontPatternResponse> {
  return sendJson<FontPatternResponse>(
    "POST",
    `/volumes/${encodeURIComponent(volumeId)}/font-pattern/reset`,
  )
}
