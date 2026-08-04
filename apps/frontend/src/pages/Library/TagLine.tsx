import { useSettings } from "@/api/hooks"
import { TagTable } from "@/components/TagTable"
import { Tooltip } from "@/components/Tooltip"
import { buildSearchToken, colorCodeTags, TIMESTAMP_NAMESPACE } from "@/lib/tagFormat"

/** Tag line + hover tooltip — ports `colorCodeTags` (namespace-colored, date/time-excluded,
 * CSS-ellipsis-truncated via the `span.tags` rule already present in the copied `lrr.css`) for the
 * always-visible line, and `buildTagsDiv` (the full per-namespace tag table, via the shared
 * `TagTable` component) for the hover body — rendered through the shared `Tooltip` component
 * (portaled to `document.body`) rather than a locally absolutely-positioned `<table>`, since the
 * grid card's own ancestors clip an unportaled tooltip (this was silently never visible on the
 * homepage grid before — a real regression fixed here, not a style tweak). Click-to-search on any
 * individual tag (`.gt[search]` in legacy, intercepted by `index_datatables.js` to fire a live
 * search instead of a full navigation — reproduced here as an in-app filter-apply). */
export function TagLine({
  tags,
  onSearchTag,
}: {
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const timezone = useSettings().data?.timezone ?? ""
  const coded = colorCodeTags(tags, timezone)
  if (coded.length === 0) return null

  return (
    <Tooltip
      label={<TagTable tags={tags} onSearchTag={(ns, v) => onSearchTag(buildSearchToken(ns, v, !TIMESTAMP_NAMESPACE.test(ns)))} />}
      wrapperStyle={{ display: "block" }}
    >
      <span className="tags tag-tooltip">
        {coded.map((tag, i) => (
          <span key={i}>
            <span
              className={`${tag.namespace}-tag`}
              style={{ cursor: "pointer" }}
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onSearchTag(tag.text)
              }}
            >
              {tag.text}
            </span>
            {i < coded.length - 1 && ", "}
          </span>
        ))}
      </span>
    </Tooltip>
  )
}
