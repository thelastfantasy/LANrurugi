import { useSettings } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { TagTable } from "@/components/Display"
import { buildSearchToken, colorCodeTags, TIMESTAMP_NAMESPACE } from "@/lib/tagFormat"
import { parseRating } from "@/lib/utils/rating"

/** Tag line + hover tooltip — ports `colorCodeTags` (namespace-colored, date/time-excluded,
 * CSS-ellipsis-truncated via the `span.tags` rule already present in the copied `lrr.css`) for the
 * always-visible line, and `buildTagsDiv` (the full per-namespace tag table, via the shared
 * `TagTable` component) for the hover body — rendered through the shared `Tooltip` component
 * (portaled to `document.body`) rather than a locally absolutely-positioned `<table>`, since the
 * grid card's own ancestors clip an unportaled tooltip (this was silently never visible on the
 * homepage grid before — a real regression fixed here, not a style tweak). Click-to-search on any
 * individual tag (`.gt[search]` in legacy, intercepted by `index_datatables.js` to fire a live
 * search instead of a full navigation — reproduced here as an in-app filter-apply).
 *
 * `rating` gets the same star-icon treatment as `TagTable`/`ArchiveOverviewOverlay`'s `TagsTable`
 * instead of `colorCodeTags`' generic namespace-stripped text — every other namespace loses its
 * prefix harmlessly (`category:cosplay` → `cosplay` still reads fine on its own), but `rating:5`
 * stripped to a bare `5` reads as a meaningless floating number with zero context. Sorted to the
 * FRONT of the line (not interleaved at its alphabetical position, and NOT split onto its own
 * line) — two things this had to route around: (1) mixing text glyphs and a solid star-icon block
 * on the same `text-align: center` line reads visibly off-center even when the measured left/right
 * whitespace is exactly equal (an optical- vs. geometric-centering mismatch, confirmed via
 * `getBoundingClientRect`), and putting the icon block at a fixed edge rather than floating in the
 * middle of the line sidesteps that; (2) `.id4` (legacy's own footer container class this renders
 * inside) has a hardcoded `height: 20px` + `overflow: visible` — a second line of content doesn't
 * get clipped to fit, it visibly overflows past the card's own bottom edge into whatever renders
 * below it in the grid, so the rating can't be split onto its own line no matter how that's done
 * (confirmed live: an earlier version of this fix that put the star row in its own block-level
 * `Tooltip` did exactly this, spilling into the next grid row). */
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

  return (
    <Tooltip
      label={<TagTable tags={tags} onSearchTag={(ns, v) => onSearchTag(buildSearchToken(ns, v, !TIMESTAMP_NAMESPACE.test(ns)))} />}
      wrapperStyle={{ display: "block" }}
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
