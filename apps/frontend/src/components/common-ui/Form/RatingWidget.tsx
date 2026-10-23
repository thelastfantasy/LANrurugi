import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateArchiveMetadata } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { useSupportsHover } from "@/hooks/useSupportsHover"
import { formatRating, parseRating } from "@/lib/utils/rating"

import { IconButton } from "./Button"
import { StarSprite } from "./StarRating"

// Rating is a tag under the `rating:` namespace (own decimal encoding, e.g. `rating:4.5`), not a
// dedicated field — mirrors legacy's Raty widget but not its whole-number star-repeat storage.

const MAX_STARS = 5
const DEFAULT_STAR_SIZE = 24
/** Pointer-driven clicks stop at half-star granularity (E-Hentai's own convention); a directly-
 * edited tag can carry finer precision, which `StarSprite` still renders correctly. */
const CLICK_STEP = 0.5
/** Touch-only clear affordance: never smaller than this so the trash target stays tappable even
 * when the widget is embedded in a dense row (Library context menu renders 16px stars). */
const MIN_CLEAR_BUTTON_SIZE = 24

export function currentRating(tags: string): number {
  const match = tags.split(",").find((t) => t.trim().toLowerCase().startsWith("rating:"))
  return match ? (parseRating(match.split(":").slice(1).join(":")) ?? 0) : 0
}

export function RatingWidget({
  archiveId,
  tags,
  size = DEFAULT_STAR_SIZE,
  onChange,
}: {
  archiveId: string
  tags: string
  /** Star glyph size in px — Library context-menu usage renders smaller to match that row height. */
  size?: number
  /** Overrides the default `useUpdateArchiveMetadata`-backed persistence (wrong for a tankoubon).
   * Receives the same next-tags string this component would otherwise send itself. */
  onChange?: (nextTags: string) => void
}) {
  const { t } = useTranslation()
  const supportsHover = useSupportsHover()
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const rating = currentRating(tags)
  // Live-previews the hovered score; null means "not hovering" (show `rating`).
  const [previewRating, setPreviewRating] = useState<number | null>(null)
  const displayRating = previewRating ?? rating

  function setRating(score: number | null) {
    const withoutRating = tags
      .split(",")
      .filter((t) => !t.trim().toLowerCase().startsWith("rating:"))
      .map((t) => t.trim())
      .filter(Boolean)
    const next = score ? [...withoutRating, `rating:${formatRating(score)}`] : withoutRating
    const nextTags = next.join(",")
    if (onChange) onChange(nextTags)
    else updateMetadata.mutate({ tags: nextTags })
  }

  /** Half-star pointer target: star index plus left/right-half hit-testing (E-Hentai convention). */
  function scoreFromPointer(e: MouseEvent<HTMLSpanElement>, starIndex: number): number {
    const rect = e.currentTarget.getBoundingClientRect()
    const isRightHalf = e.clientX - rect.left > rect.width / 2
    return starIndex + (isRightHalf ? 1 : CLICK_STEP)
  }

  // Touch devices never fire a right-click, so the `onContextMenu` clear below is unreachable
  // there — expose an explicit trash button to the right of the stars instead. Hover-capable
  // devices keep the compact row and its right-click gesture.
  const showClearButton = rating > 0 && !supportsHover
  const clearButtonSize = Math.max(size, MIN_CLEAR_BUTTON_SIZE)

  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: showClearButton ? 4 : 0,
      }}
    >
      <Tooltip
        label={t(
          supportsHover
            ? "components.form.clickAStarToRate"
            : "components.form.tapAStarToRate",
        )}
      >
        <span
          style={{ display: "inline-flex" }}
          onMouseLeave={() => setPreviewRating(null)}
          onContextMenu={(e) => {
            e.preventDefault()
            if (rating > 0) setRating(null)
          }}
        >
          {Array.from({ length: MAX_STARS }, (_, i) => i).map((i) => (
            <span
              key={i}
              style={{ cursor: "pointer" }}
              onMouseMove={(e) => setPreviewRating(scoreFromPointer(e, i))}
              onClick={(e) => setRating(scoreFromPointer(e, i))}
            >
              <StarSprite fraction={Math.max(0, Math.min(1, displayRating - i))} size={size} />
            </span>
          ))}
        </span>
      </Tooltip>
      {showClearButton && (
        <Tooltip label={t("Clear Rating")}>
          <IconButton
            variant="ghost-btn"
            icon="fas fa-trash-alt"
            size={clearButtonSize}
            style={{ fontSize: Math.round(clearButtonSize * 0.6), padding: 0 }}
            aria-label={t("Clear Rating")}
            onClick={(e) => {
              // Inside popup menus the row itself has no click handler, but stop the bubble so a
              // future row-level handler can't also treat this as a star/menu selection.
              e.stopPropagation()
              setRating(null)
            }}
          />
        </Tooltip>
      )}
    </span>
  )
}
