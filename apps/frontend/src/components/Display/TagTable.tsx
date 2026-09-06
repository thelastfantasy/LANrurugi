import { useSettings } from "@/api/hooks"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import {
  BARE_TAG_NAMESPACE,
  displayNamespace,
  formatTagValue,
  getTagSearchURL,
  splitTagsByNamespace,
  tagValueForSearch,
} from "@/lib/tagFormat"
import { parseRating } from "@/lib/utils/rating"

/** Per-namespace tag table for a hover tooltip: one row per namespace, each value its own chip.
 * Plain flex layout (not legacy's flexbox-hostile `<table class="itg">`); chip classes still match. */
export function TagTable({
  tags,
  onSearchTag,
}: {
  tags: string
  onSearchTag?: (namespace: string, value: string) => void
}) {
  const settings = useSettings()
  const timezone = settings.data?.timezone ?? ""
  const byNamespace = splitTagsByNamespace(tags)
  const namespaces = Object.keys(byNamespace).sort()
  if (namespaces.length === 0) return null

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {namespaces.map((namespace) => {
        const displayKey = namespace === BARE_TAG_NAMESPACE ? "other" : namespace
        return (
        <div key={namespace} style={{ display: "flex", gap: 6, alignItems: "flex-start", minWidth: 0 }}>
          <div
            className={`caption-namespace ${displayKey.toLowerCase()}-tag`}
            style={{ fontWeight: "bold", flex: "0 0 auto", whiteSpace: "nowrap", padding: 0 }}
          >
            {displayNamespace(displayKey)}:
          </div>
          {/* `minWidth: 0` overrides the flex item's default intrinsic-width sizing, which let a
              long unbroken URL overflow the tooltip's max-width instead of wrapping. */}
          <div style={{ display: "flex", flexWrap: "wrap", minWidth: 0 }}>
            {displayKey.toLowerCase() === "rating" ? (
              <div className="gt">
                <a
                  href={getTagSearchURL(displayKey, byNamespace[namespace][0] ?? "", timezone)}
                  onClick={(e) => {
                    if (!onSearchTag) return
                    e.preventDefault()
                    e.stopPropagation()
                    onSearchTag(displayKey, byNamespace[namespace][0] ?? "")
                  }}
                  style={{ textDecoration: "none", cursor: onSearchTag ? "pointer" : undefined }}
                >
                  <StarRatingDisplay rating={parseRating(byNamespace[namespace][0]) ?? 0} size={14} />
                </a>
              </div>
            ) : (
              byNamespace[namespace].map((value, i) => (
                <div
                  key={i}
                  className="gt"
                  style={{ maxWidth: "100%", whiteSpace: "normal", overflow: "visible", textOverflow: "clip" }}
                >
                  {displayKey === "source" ? (
                    <a
                      href={/^https?:\/\//i.test(value) ? value : `https://${value}`}
                      target="_blank"
                      rel="noreferrer"
                      onClick={(e) => e.stopPropagation()}
                      style={{ wordBreak: "break-all" }}
                    >
                      {value}
                    </a>
                  ) : (
                    <a
                      href={getTagSearchURL(displayKey, value, timezone)}
                      onClick={(e) => {
                        if (!onSearchTag) return
                        e.preventDefault()
                        e.stopPropagation()
                        onSearchTag(displayKey, tagValueForSearch(displayKey, value, timezone))
                      }}
                      style={{ wordBreak: "break-all", cursor: onSearchTag ? "pointer" : undefined }}
                    >
                      {formatTagValue(displayKey, value, timezone)}
                    </a>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
        )
      })}
    </div>
  )
}
