import { useSettings } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { TagTable } from "@/components/Display"
import { buildSearchToken, colorCodeTags, TIMESTAMP_NAMESPACE } from "@/lib/tagFormat"
import { parseRating } from "@/lib/utils/rating"

/** Tag line + portaled hover tooltip (an unportaled one gets clipped by the grid card's ancestors).
 * `rating` gets star-icon treatment and sorts to the front — `.id4`'s fixed height can't fit a 2nd line. */
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

  const ratingTag = coded.find((tag) => tag.namespace === "rating")
  const otherTags = coded.filter((tag) => tag.namespace !== "rating")

  // 21rem, one past the default 20rem: the common e-hentai source tag is about a character too
  // long for 320px and used to wrap just before its last character. `closeDelay` is slower than the
  // 150ms default because the pointer often dips into the bubble to click a tag; another tooltip
  // opening still closes this one at once (`activeTooltip` in Tooltip.tsx).
  return (
    <Tooltip
      label={<TagTable tags={tags} onSearchTag={(ns, v) => onSearchTag(buildSearchToken(ns, v, !TIMESTAMP_NAMESPACE.test(ns)))} />}
      wrapperStyle={{ display: "block" }}
      maxWidth="21rem"
      closeDelay={350}
    >
      <span className="tags tag-tooltip">
        {ratingTag && (
          <span
            className={`${ratingTag.namespace}-tag`}
            style={{ cursor: "pointer", display: "inline-flex", verticalAlign: "middle" }}
            onClick={(e) => {
              e.preventDefault()
              e.stopPropagation()
              onSearchTag(ratingTag.text)
            }}
          >
            <StarRatingDisplay rating={parseRating(ratingTag.text) ?? 0} size={12} />
          </span>
        )}
        {ratingTag && otherTags.length > 0 && " "}
        {otherTags.map((tag, i) => (
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
            {i < otherTags.length - 1 && ", "}
          </span>
        ))}
      </span>
    </Tooltip>
  )
}
