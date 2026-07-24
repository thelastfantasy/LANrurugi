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
  if (tags === null || tags === undefined || tags === '') return byNamespace
  for (const tag of tags.split(/,\s?/)) {
    const match = /^([^:]*):(.*)$/.exec(tag)
    const namespace = match ? match[1].trim() : 'other'
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
  return namespace !== '' && namespace !== 'other' ? `${namespace}:${tag}` : tag
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

/** Ports `colorCodeTags` — strips namespace prefixes for display, excludes `date`/`time`-prefixed
 * namespaces from the *visible* line (falling back to showing them only when there is nothing
 * else to show), sorted alphabetically by namespace. Returns structured data instead of an HTML
 * string (this is React, not string-templated markup) — callers render each entry as
 * `<span className="{namespace}-tag">`, matching the CSS class names legacy's own theme
 * stylesheets already define. */
export function colorCodeTags(tags: string | null | undefined): ColorCodedTag[] {
  const byNamespace = splitTagsByNamespace(tags)
  const allKeys = Object.keys(byNamespace)
  const filteredKeys = allKeys.filter((k) => !/^(date|time)/.test(k))
  const keysToShow = (filteredKeys.length ? filteredKeys : allKeys).sort()

  const out: ColorCodedTag[] = []
  for (const key of keysToShow) {
    for (const value of byNamespace[key]) {
      out.push({
        namespace: key.toLowerCase(),
        text: /^(date|time)/.test(key) ? convertTimestamp(value) : value,
      })
    }
  }
  return out
}

/** Ports `getTagSearchURL` — a `source:` tag's value is an external link (opened as-is, restoring
 * a `https://` scheme if missing); every other namespace searches the library for that tag. */
export function getTagSearchURL(namespace: string, tag: string): string {
  const namespacedTag = buildNamespacedTag(namespace, tag)
  if (namespace !== 'source') {
    return `/?q=${encodeURIComponent(namespacedTag)}$`
  }
  return /^https?:\/\//.test(tag) ? tag : `https://${tag}`
}
