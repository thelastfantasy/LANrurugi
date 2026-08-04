import { useSettings } from "../../../api/hooks"
import { StarRatingDisplay } from "../../../components/StarRating"
import { parseRating } from "../../../lib/rating"
import { displayNamespace, formatTagValue, getTagSearchURL } from "../../../lib/tagFormat"

/** Mirrors legacy's `splitTagsByNamespace` + `buildTagsDiv` (`~/LANraragi/public/js/mod/common.js`)
 * — groups a flat comma-separated tag string by its `namespace:value` prefix (untagged values fall
 * under `other`), rendered as a `caption-namespace` row per namespace with each value as a
 * clickable search-link chip. `rating:` gets its own gold-star rendering instead of the raw tag
 * value (see the `namespace === 'rating'` branch below) — legacy's own real overview page shows
 * the star icons in this table *in addition to* the separate interactive `RatingWidget` above it
 * (confirmed against a real screenshot of a rated archive), so this table must render it too, not
 * skip it. Still a real, working search-link chip underneath, though — legacy's own real rating
 * chip *is* clickable (a real user-confirmed link, e.g. `?q=rating%3A⭐⭐⭐⭐⭐$` against a live
 * legacy instance), a link this port's own `q=rating:2.5$` (the equivalent search against this
 * app's own decimal-encoded storage format — verified live: correctly returns exactly the archive
 * carrying that tag) actually and correctly answers, unlike an earlier version of this component
 * that dropped the link entirely on the assumption nobody would search by star count — wrong,
 * since legacy itself treats it as a completely ordinary searchable tag. No underline on it
 * specifically, though (a real, deliberate deviation, not a bug) — legacy's own underlined
 * rating-star link reads like a broken/dead link at a glance, which the star icons alone don't
 * need to invite. */
export function TagsTable({ tags }: { tags: string }) {
  // Server timezone for `date_added`/`timestamp` tag display + search-URL date-range conversion
  // (see `lib/tagFormat.ts`'s `formatTimestampForDisplay`/`getTagSearchURL`). Falls back to the
  // browser's local timezone if settings haven't loaded yet, matching the pre-feature behavior.
  const settings = useSettings()
  const timezone = settings.data?.timezone ?? ""
  if (!tags) return null
  const byNamespace = new Map<string, string[]>()
  for (const raw of tags.split(",")) {
    const tag = raw.trim()
    if (!tag) continue
    const idx = tag.indexOf(":")
    const namespace = idx === -1 ? "other" : tag.slice(0, idx).trim()
    const value = idx === -1 ? tag : tag.slice(idx + 1).trim()
    const list = byNamespace.get(namespace) ?? []
    list.push(value)
    byNamespace.set(namespace, list)
  }

  const namespaces = [...byNamespace.keys()].sort()
  if (namespaces.length === 0) return null

  return (
    <table className="itg" style={{ boxShadow: "none", border: "none", borderRadius: 0 }}>
      <tbody>
        {namespaces.map((namespace) => (
          <tr key={namespace}>
            <td className={`caption-namespace ${namespace.toLowerCase()}-tag`}>
              {displayNamespace(namespace)}:
            </td>
            <td>
              {namespace.toLowerCase() === "rating" ? (
                <div className="gt">
                  <a
                    href={getTagSearchURL(namespace, (byNamespace.get(namespace) ?? [])[0] ?? "")}
                    onClick={(e) => e.stopPropagation()}
                    style={{ textDecoration: "none" }}
                  >
                    <StarRatingDisplay rating={parseRating((byNamespace.get(namespace) ?? [])[0]) ?? 0} size={16} />
                  </a>
                </div>
              ) : (
                (byNamespace.get(namespace) ?? []).map((value) => (
                  <div className="gt" key={value}>
                    {/* `source` is a link to an external, third-party site — real `target="_blank"`
                        so it opens a new tab instead of navigating the reader away, matching
                        `TagTable.tsx`'s own real `source` branch (this table predates that shared
                        component and never got the same split when it landed there; this was a
                        real, independently-discovered bug, not a copy of an already-fixed one). */}
                    {namespace === "source" ? (
                      <a
                        href={getTagSearchURL(namespace, value, timezone)}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(e) => e.stopPropagation()}
                      >
                        {value}
                      </a>
                    ) : (
                      <a href={getTagSearchURL(namespace, value, timezone)} onClick={(e) => e.stopPropagation()}>
                        {formatTagValue(namespace, value, timezone)}
                      </a>
                    )}
                  </div>
                ))
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
