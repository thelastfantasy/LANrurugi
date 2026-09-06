import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateArchiveMetadata } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { formatRating, parseRating } from "@/lib/utils/rating"

import { StarSprite } from "./StarRating"

// Rating is a tag under the `rating:` namespace (own decimal encoding, e.g. `rating:4.5`), not a
// dedicated field — mirrors legacy's Raty widget but not its whole-number star-repeat storage.

const MAX_STARS = 5
const DEFAULT_STAR_SIZE = 24
/** Pointer-driven clicks stop at half-star granularity (E-Hentai's own convention); a directly-
 * edited tag can carry finer precision, which `StarSprite` still renders correctly. */
const CLICK_STEP = 0.5

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

  return (
    <Tooltip label={t("components.form.clickAStarToRate")}>
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
  )
}
