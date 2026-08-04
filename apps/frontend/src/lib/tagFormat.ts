// Direct ports of legacy's own tag-formatting helpers (`~/LANraragi/public/js/mod/common.js`) —
// used by the Library grid card's tag line/tooltip and by the rating context-menu item, which
// both need to read/write the exact same `namespace:value, namespace:value` string shape archives
// store their `tags` field as.

export type TagsByNamespace = Record<string, string[]>

/** Splits a `tags` string into a per-namespace dictionary of value arrays — ports
 * `splitTagsByNamespace` exactly, including its `other` fallback bucket for bare (non-namespaced)
 * tags and its `trim()` on both halves of the split. */
export function splitTagsByNamespace(tags: string | null | undefined): TagsByNamespace {
  const byNamespace: TagsByNamespace = {}
  if (tags === null || tags === undefined || tags === "") return byNamespace
  for (const tag of tags.split(/,\s?/)) {
    const match = /^([^:]*):(.*)$/.exec(tag)
    const namespace = match ? match[1].trim() : "other"
    const value = match ? match[2].trim() : tag.trim()
    if (!value) continue
    ;(byNamespace[namespace] ??= []).push(value)
  }
  return byNamespace
}

/** Inverse of `splitTagsByNamespace` — ports `buildTagList`/`buildNamespacedTag`. */
export function buildTagList(byNamespace: TagsByNamespace): string[] {
  return Object.entries(byNamespace).flatMap(([namespace, values]) =>
    values.map((v) => buildNamespacedTag(namespace, v)),
  )
}

export function buildNamespacedTag(namespace: string, tag: string): string {
  return namespace !== "" && namespace !== "other" ? `${namespace}:${tag}` : tag
}

/** Builds a `namespace:value` (or bare `value`) *search-query* token — distinct from
 * `buildNamespacedTag`, which is only for the archive's own stored `tags` field and must never be
 * quoted. Space is a real AND-separator in the search grammar (`grammar.rs::compute_search_filter`,
 * matching e-hentai's own `f_search` syntax — issue #59), so a multi-word `value` gets wrapped in
 * double quotes here to protect its internal spaces from being split into separate, wrong tokens —
 * only the *value* itself, e.g. `female:"anal intercourse"`, not the whole `namespace:value` pair
 * (matching e-hentai's own literal syntax; the grammar accepts both spellings and treats them
 * identically, but this is the one that keeps `namespace` readable outside the quotes). Quoting
 * already implies an exact-tag match, so `exact` (the `$`-suffix behavior) only applies to the
 * unquoted, single-word case. Visible tag text elsewhere in the UI is built from `value` directly,
 * never from this function's output, so the quotes never show up on-screen. */
export function buildSearchToken(namespace: string, value: string, exact = false): string {
  if (value.includes(" ")) return buildNamespacedTag(namespace, `"${value}"`)
  const namespacedTag = buildNamespacedTag(namespace, value)
  return exact ? `${namespacedTag}$` : namespacedTag
}

/** Regex matching the timestamp namespaces legacy treats as date values (`buildTagsDiv`:
 * `/^(date|time)/`). Kept here so callers that need to know "is this namespace a timestamp?" for
 * display/search-URL decisions don't each re-derive it. */
export const TIMESTAMP_NAMESPACE = /^(date|time)/i

/** Capitalizes a namespace key for display, special-casing `date_added` → `Date Added` rather
 * than the generic capitalize-first-letter rule's own `Date_added`. Was independently duplicated
 * (byte-for-byte identical) in both `components/TagTable.tsx` and
 * `pages/Reader/ArchiveOverviewOverlay.tsx`'s own `TagsTable` before both were consolidated to
 * import this shared copy instead. */
export function displayNamespace(key: string): string {
  if (key === "date_added") return "Date Added"
  return key.charAt(0).toUpperCase() + key.slice(1)
}

/** Formats a tag *value* for display — passes non-timestamp namespaces through unchanged, routes
 * timestamp namespaces through `formatTimestampForDisplay`. Distinct from `tagValueForSearch`
 * below (same timestamp branch, but that one's non-timestamp passthrough and this one's are used
 * for different purposes — display text here, search-query value there — even though they
 * currently compute the identical result, keeping them as separate named functions documents that
 * at each call site rather than relying on one function serving two different-sounding purposes by
 * coincidence). Was independently duplicated (byte-for-byte identical) in both
 * `components/TagTable.tsx` and `pages/Reader/ArchiveOverviewOverlay.tsx`'s own `TagsTable` before
 * both were consolidated to import this shared copy instead. */
export function formatTagValue(namespace: string, value: string, timezone: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  return formatTimestampForDisplay(value, timezone)
}

/** Formats a Unix-seconds timestamp as `yyyy-mm-dd` in the given IANA timezone, using the
 * browser's native `Intl.DateTimeFormat` (which understands any IANA id `chrono-tz` on the
 * backend also accepts — `Asia/Tokyo`, `UTC`, etc.). Falls back to the raw value if it isn't a
 * number, and to the browser's local timezone if `timezone` is empty/unset (matches the
 * pre-timezone-setting behavior exactly). */
export function formatTimestampForDisplay(value: string, timezone: string): string {
  const seconds = Number(value)
  if (!Number.isFinite(seconds)) return value
  const options: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: timezone || undefined,
  }
  // `Intl` emits in the locale's own field order (e.g. `7/20/2026` for en-US); force `yyyy-mm-dd`
  // by reading the parts back out and reassembling — `formatToParts` is the stable way to do this
  // regardless of locale.
  const parts = new Intl.DateTimeFormat("en-US", options).formatToParts(new Date(seconds * 1000))
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? ""
  return `${get("year")}-${get("month")}-${get("day")}`
}

/** Inverse of `formatTimestampForDisplay` for search-URL purposes: a timestamp namespace's
 * displayed value is a `yyyy-mm-dd` date, and clicking it should search that whole calendar day
 * (via the backend's `date_added:YYYY-MM-DD` date-range syntax), not the bare second-level
 * timestamp. For non-timestamp namespaces this returns the raw value unchanged. */
export function tagValueForSearch(namespace: string, value: string, timezone: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  return formatTimestampForDisplay(value, timezone)
}

/** Back-compat overload — the previous signature (no timezone) preserved so existing call sites
 * that haven't been threaded through `useSettings` yet keep compiling; renders in the browser's
 * local timezone exactly as before. Prefer `formatTimestampForDisplay` for new code. */
export function convertTimestamp(value: string): string {
  const seconds = Number(value)
  if (!Number.isFinite(seconds)) return value
  return new Date(seconds * 1000).toLocaleDateString()
}

export interface ColorCodedTag {
  namespace: string
  text: string
}

/** Ports `colorCodeTags` — strips namespace prefixes for display, excludes `date`/`time`-prefixed
 * namespaces from the *visible* line (falling back to showing them only when there is nothing
 * else to show), sorted alphabetically by namespace. Returns structured data instead of an HTML
 * string (this is React, not string-templated markup) — callers render each entry as
 * `<span className="{namespace}-tag">`, matching the CSS class names legacy's own theme
 * stylesheets already define.
 *
 * `timezone` (optional) formats timestamp-namespace values as `yyyy-mm-dd` in the server's
 * configured timezone via [`formatTimestampForDisplay`]; omit it to keep the legacy browser-local
 * `toLocaleDateString` behavior. */
export function colorCodeTags(tags: string | null | undefined, timezone?: string): ColorCodedTag[] {
  const byNamespace = splitTagsByNamespace(tags)
  const allKeys = Object.keys(byNamespace)
  const filteredKeys = allKeys.filter((k) => !TIMESTAMP_NAMESPACE.test(k))
  const keysToShow = (filteredKeys.length ? filteredKeys : allKeys).sort()

  const out: ColorCodedTag[] = []
  for (const key of keysToShow) {
    for (const value of byNamespace[key]) {
      out.push({
        namespace: key.toLowerCase(),
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

/** Ports `getTagSearchURL` — a `source:` tag's value is an external link (opened as-is, restoring
 * a `https://` scheme if missing); every other namespace searches the library for that tag.
 *
 * `timezone` (optional) reroutes a timestamp-namespace tag's search URL through the backend's
 * `date_added:YYYY-MM-DD` date-range syntax (a whole calendar day in that timezone) instead of the
 * bare second-level timestamp the tag actually stores — so clicking a displayed `2026-07-20`
 * finds every archive added that day, not just the exact second this one was. */
export function getTagSearchURL(namespace: string, tag: string, timezone?: string): string {
  if (namespace === "source") {
    return /^https?:\/\//.test(tag) ? tag : `https://${tag}`
  }
  // Timestamp namespaces get rerouted through the date-range syntax when a timezone is available —
  // and that syntax (`date_added:2026-07-20`) must NOT carry a trailing `$`, since `$` would make
  // the grammar treat it as an exact tag match (against an `INDEX_date?added:2026-07-20` key that
  // doesn't exist) instead of the date-range branch in `token_matches`. Plain (non-timestamp)
  // namespaces keep the `$` exact-match suffix as before (`buildSearchToken`'s `exact` param) —
  // `yyyy-mm-dd` never contains a space, so it never hits `buildSearchToken`'s own quoting branch
  // either way.
  const isTimestamp = timezone !== undefined && TIMESTAMP_NAMESPACE.test(namespace)
  const searchValue = isTimestamp ? tagValueForSearch(namespace, tag, timezone) : tag
  const query = buildSearchToken(namespace, searchValue, !isTimestamp)
  return `/?q=${encodeURIComponent(query)}`
}
