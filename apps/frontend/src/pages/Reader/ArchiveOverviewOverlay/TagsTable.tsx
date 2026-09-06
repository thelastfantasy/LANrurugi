import { useSettings } from "@/api/hooks"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { displayNamespace, formatTagValue, getTagSearchURL } from "@/lib/tagFormat"
import { parseRating } from "@/lib/utils/rating"

/** Mirrors legacy's `splitTagsByNamespace` + `buildTagsDiv` — groups a flat comma-separated tag
 * string by its `namespace:value` prefix, rendered as a `caption-namespace` row per namespace. */
export function TagsTable({ tags }: { tags: string }) {
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
