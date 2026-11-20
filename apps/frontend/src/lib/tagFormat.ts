export type TagsByNamespace = Record<string, string[]>

/** Bucket key for bare tags — distinct from a real `"other"` namespace to avoid collision. */
export const BARE_TAG_NAMESPACE = " bare"

export function splitTagsByNamespace(tags: string | null | undefined): TagsByNamespace {
  const byNamespace: TagsByNamespace = {}
  if (tags === null || tags === undefined || tags === "") return byNamespace
  for (const tag of tags.split(/,\s?/)) {
    const match = /^([^:]*):(.*)$/.exec(tag)
    const namespace = match ? match[1].trim() : BARE_TAG_NAMESPACE
    const value = match ? match[2].trim() : tag.trim()
    if (!value) continue
    ;(byNamespace[namespace] ??= []).push(value)
  }
  return byNamespace
}

/** Inverse of `splitTagsByNamespace`. Key `""` (not `"other"`) to request a bare tag. */
export function buildTagList(byNamespace: TagsByNamespace): string[] {
  return Object.entries(byNamespace).flatMap(([namespace, values]) =>
    values.map((v) => buildNamespacedTag(namespace === BARE_TAG_NAMESPACE ? "" : namespace, v)),
  )
}

export function buildNamespacedTag(namespace: string, tag: string): string {
  return namespace !== "" ? `${namespace}:${tag}` : tag
}

/** Builds a `namespace:value` search-query token; multi-word values are quoted since space is an
 * AND-separator in the search grammar. */
export function buildSearchToken(namespace: string, value: string, exact = false): string {
  if (value.includes(" ")) return buildNamespacedTag(namespace, `"${value}"`)
  const namespacedTag = buildNamespacedTag(namespace, value)
  return exact ? `${namespacedTag}$` : namespacedTag
}

/** Strips one pair of surrounding double quotes. */
function unquote(text: string): string {
  return text.length >= 2 && text.startsWith('"') && text.endsWith('"') ? text.slice(1, -1).trim() : text
}

/** Turns a tag copied as a search token — this app's own or e-hentai's, `female:"hair buns$"` — into
 * the stored form `female:hair buns` (#297): the quotes and the exact-match `$` are search syntax,
 * not part of the tag. Only a surrounding pair of quotes and a trailing `$` are removed; anything
 * inside the value is kept. Returns `""` when nothing is left. */
export function normalizeTag(raw: string): string {
  const tag = unquote(raw.trim())
  const colon = tag.indexOf(":")
  const namespace = colon > 0 ? tag.slice(0, colon).trim().toLowerCase() : ""
  let value = colon > 0 ? tag.slice(colon + 1).trim() : tag
  value = unquote(value)
  if (value.endsWith("$")) value = value.slice(0, -1).trim()
  value = unquote(value)
  if (!value) return ""
  return buildNamespacedTag(namespace, value)
}

export const TIMESTAMP_NAMESPACE = /^(date|time)/i

export function displayNamespace(key: string): string {
  if (key === "date_added") return "Date Added"
  return key.charAt(0).toUpperCase() + key.slice(1)
}

export function formatTagValue(namespace: string, value: string, timezone: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  return formatTimestampForDisplay(value, timezone)
}

export function formatTimestampForDisplay(value: string, timezone: string): string {
  const seconds = Number(value)
  if (!Number.isFinite(seconds)) return value
  const options: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: timezone || undefined,
  }
  const parts = new Intl.DateTimeFormat("en-US", options).formatToParts(new Date(seconds * 1000))
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? ""
  return `${get("year")}-${get("month")}-${get("day")}`
}

export function tagValueForSearch(namespace: string, value: string, timezone: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  return formatTimestampForDisplay(value, timezone)
}

export function convertTimestamp(value: string): string {
  const seconds = Number(value)
  if (!Number.isFinite(seconds)) return value
  return new Date(seconds * 1000).toLocaleDateString()
}

export interface ColorCodedTag {
  namespace: string
  text: string
}

export function colorCodeTags(tags: string | null | undefined, timezone?: string): ColorCodedTag[] {
  const byNamespace = splitTagsByNamespace(tags)
  const allKeys = Object.keys(byNamespace)
  const filteredKeys = allKeys.filter((k) => !TIMESTAMP_NAMESPACE.test(k))
  const keysToShow = (filteredKeys.length ? filteredKeys : allKeys).sort()

  const out: ColorCodedTag[] = []
  for (const key of keysToShow) {
    const displayNamespaceKey = key === BARE_TAG_NAMESPACE ? "other" : key
    for (const value of byNamespace[key]) {
      out.push({
        namespace: displayNamespaceKey.toLowerCase(),
        text: TIMESTAMP_NAMESPACE.test(key)
          ? timezone !== undefined
            ? formatTimestampForDisplay(value, timezone)
            : convertTimestamp(value)
          : value,
      })
    }
  }
  return out
}

export function getTagSearchURL(namespace: string, tag: string, timezone?: string): string {
  if (namespace === "source") {
    return /^https?:\/\//.test(tag) ? tag : `https://${tag}`
  }
  const isTimestamp = timezone !== undefined && TIMESTAMP_NAMESPACE.test(namespace)
  const searchValue = isTimestamp ? tagValueForSearch(namespace, tag, timezone) : tag
  const query = buildSearchToken(namespace, searchValue, !isTimestamp)
  return `/?q=${encodeURIComponent(query)}`
}
