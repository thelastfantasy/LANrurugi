// Direct browser → locally-hosted model calls (T049/T049b,
// `contracts/client-compositing-cache.md`).
//
// This call deliberately does NOT go through the LANrurugi server: the server can't reach a user's
// loopback device, so the browser is the only party that can talk to it (constitution Principle V).
// The flip side is that the shared, server-stored Terminology Glossary is unreachable from here
// too — which is why the server hands us its assembled `context` via `text-regions`, and why we
// report results back afterwards so terms this backend discovered still reach that glossary
// (FR-007a).

import { recordLocalTranslation } from "./api"
import type { DetectedTextRegion, LocalBackendConfig, TranslationContext } from "./types"

/** Distinguishes a blocked private-network request from an ordinary failure, so the UI can show
 * the guided fallback (FR-018) rather than a generic error. */
export class LocalBackendUnreachableError extends Error {
  /** True when the failure looks like Private Network Access / CORS blocking rather than the
   * backend simply being down. */
  readonly likelyBlocked: boolean

  constructor(message: string, likelyBlocked: boolean) {
    super(message)
    this.name = "LocalBackendUnreachableError"
    this.likelyBlocked = likelyBlocked
  }
}

/** Renders the server-assembled context into the same stable-prefix-first shape the cloud path
 * sends, so a local backend gets identical glossary/tone guidance (FR-007b/c/e). */
function renderContextPrefix(context: TranslationContext): string {
  let out = ""

  if (context.glossaryMatches.length > 0) {
    out += "Established translations for terms appearing in this batch. Reuse them exactly:\n"
    for (const [source, translation] of context.glossaryMatches) {
      out += `- ${source} => ${translation}\n`
    }
    out += "\n"
  }

  if (context.knownNames.length > 0) {
    out +=
      "Character/term names already established in this volume. If text refers to one of these by " +
      "a nickname, abbreviation, or initialism, reuse that name’s established translation " +
      "instead of coining a new one:\n"
    out += `${context.knownNames.join(", ")}\n\n`
  }

  if (context.toneReference.length > 0) {
    out += "Other text already translated on this page, for tone and style reference only:\n"
    for (const [source, translation] of context.toneReference) {
      out += `- ${source} => ${translation}\n`
    }
    out += "\n"
  }

  return out
}

/** Extracts the JSON object from a reply that may be fenced or wrapped in prose. */
function extractJsonObject(content: string): string | null {
  const start = content.indexOf("{")
  if (start < 0) return null

  let depth = 0
  let inString = false
  let escaped = false

  for (let i = start; i < content.length; i += 1) {
    const c = content[i]
    if (inString) {
      if (escaped) escaped = false
      else if (c === "\\") escaped = true
      else if (c === '"') inString = false
      continue
    }
    if (c === '"') inString = true
    else if (c === "{") depth += 1
    else if (c === "}") {
      depth -= 1
      if (depth === 0) return content.slice(start, i + 1)
    }
  }
  return null
}

/**
 * Translates a page's regions using the device's own local model.
 *
 * Returns the regions with `translatedText` filled in. Regions already translated are passed
 * through untouched rather than re-sent.
 */
export async function translateWithLocalBackend(
  config: LocalBackendConfig,
  regions: DetectedTextRegion[],
  context: TranslationContext,
  targetLanguage: string,
): Promise<DetectedTextRegion[]> {
  const pending = regions
    .map((region, index) => ({ region, index }))
    .filter(({ region }) => !region.translatedText && region.sourceText.trim())

  if (pending.length === 0) return regions

  const systemPrompt =
    `You are translating text from a manga page into ${targetLanguage}. Translate each numbered ` +
    "block faithfully, preserving tone and register. Return ONLY a JSON object mapping each " +
    "block_id to its translation, with no commentary."

  // Stable context first, this batch's blocks last — the same ordering the server uses, which is
  // what makes a provider's exact-prefix prompt cache actually hit.
  let userMessage = renderContextPrefix(context)
  userMessage += "Translate these blocks:\n"
  for (const { region, index } of pending) {
    userMessage += `[p${region.pageNumber}b${index}] ${region.sourceText}\n`
  }

  let response: Response
  try {
    response = await fetch(`${config.endpoint.replace(/\/+$/, "")}/chat/completions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        model: config.model,
        temperature: 0,
        messages: [
          { role: "system", content: systemPrompt },
          { role: "user", content: userMessage },
        ],
      }),
    })
  } catch (e) {
    // A `TypeError` from `fetch` is what both a genuinely-down backend and a PNA/CORS block look
    // like from script — the browser deliberately doesn't distinguish them. Treat it as "likely
    // blocked" so the user gets the actionable guidance rather than a dead end (FR-018).
    throw new LocalBackendUnreachableError(
      e instanceof Error ? e.message : "failed to reach the local backend",
      e instanceof TypeError,
    )
  }

  if (!response.ok) {
    throw new LocalBackendUnreachableError(
      `local backend returned HTTP ${response.status}`,
      false,
    )
  }

  const payload = (await response.json()) as {
    choices?: { message?: { content?: string } }[]
  }
  const content = payload.choices?.[0]?.message?.content
  if (!content) {
    // Malformed/empty output is a failure, never rendered as blank text (spec.md Edge Cases).
    throw new Error("local backend returned no content")
  }

  const json = extractJsonObject(content)
  if (!json) throw new Error("local backend returned no JSON object")

  const parsed = JSON.parse(json) as Record<string, unknown>
  const result = regions.map((region) => ({ ...region }))

  for (const { region, index } of pending) {
    const value = parsed[`p${region.pageNumber}b${index}`]
    if (typeof value === "string" && value.trim()) {
      result[index].translatedText = value
    }
  }

  return result
}

/**
 * Reports what the local backend translated back to the server (T049b, FR-007a).
 *
 * Best-effort: a failure here costs glossary consistency, not the translation the user is looking
 * at, so it must never surface as a page failure.
 */
export async function reportLocalTranslation(
  archiveId: string,
  page: number,
  regions: DetectedTextRegion[],
  targetLanguage: string,
  providerIdentifier: string,
): Promise<void> {
  const translations = regions
    .filter((r) => r.translatedText?.trim())
    .map((r) => ({ sourceText: r.sourceText, translatedText: r.translatedText as string }))

  if (translations.length === 0) return

  try {
    await recordLocalTranslation(archiveId, page, {
      translations,
      targetLanguage,
      provider: providerIdentifier,
    })
  } catch {
    // Intentionally swallowed — see this function's doc comment.
  }
}
