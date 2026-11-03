import { useSettings } from "@/api/hooks"
import { useCustomColumnNamespace } from "@/hooks/useCustomColumnNamespace"
import { buildSearchToken, formatTimestampForDisplay, getTagSearchURL, tagValueForSearch } from "@/lib/tagFormat"

export function CustomColumnCell({
  index,
  tags,
  onSearchTag,
}: {
  index: number
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const [namespace] = useCustomColumnNamespace(index)
  const timezone = useSettings().data?.timezone ?? ""
  const matches = [...tags.matchAll(new RegExp(`${namespace}:([^,]+)`, "g"))].map((m) => m[1].trim())
  const isDate = namespace === "date_added" || namespace === "timestamp"
  return (
    <td className={`customheader${index} itd`} style={{ textAlign: "left" }}>
      {matches.map((raw, i) => {
        const text = isDate ? formatTimestampForDisplay(raw, timezone) : namespace === "source" ? raw : raw.replace(/\b./g, (c) => c.toUpperCase())
        return (
          <span key={i}>
            <a
              href={getTagSearchURL(namespace, raw, timezone)}
              style={{ cursor: "pointer" }}
              onClick={(e) => {
                e.preventDefault()
                onSearchTag(buildSearchToken(namespace, tagValueForSearch(namespace, raw, timezone), !isDate))
              }}
            >
              {text}
            </a>
            {i < matches.length - 1 && ", "}
          </span>
        )
      })}
    </td>
  )
}
