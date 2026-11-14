import { useTranslation } from "react-i18next"

import type { CategoryMetadata } from "@/api/types"
import { StatusIcon } from "@/components/Display"
import {
  CATEGORY_BUTTON_CAP,
  NEW_ONLY,
  NO_SOURCE_ONLY,
  SOURCE_ONLY,
  TANKOUBON_ONLY,
  UNTAGGED_ONLY,
} from "@/lib/constants"

export function CategoryBar({
  selectedCategory,
  sortedCategories,
  hasTankoubons,
  onToggleCategory,
}: {
  selectedCategory: string
  sortedCategories: CategoryMetadata[]
  hasTankoubons: boolean
  onToggleCategory: (id: string) => void
}) {
  const { t } = useTranslation()
  const visible = sortedCategories.slice(0, CATEGORY_BUTTON_CAP)
  const overflow = sortedCategories.slice(CATEGORY_BUTTON_CAP)

  return (
    <div id="category-container">
      <button
        type="button"
        className={`favtag-btn${selectedCategory === NEW_ONLY ? " toggled" : ""}`}
        aria-pressed={selectedCategory === NEW_ONLY}
        title={t("library.archivesAddedWithinTheLast") ?? undefined}
        onClick={() => onToggleCategory(NEW_ONLY)}
      >
        <StatusIcon kind="new" /> {t("library.newArchives")}
      </button>
      <button
        type="button"
        className={`favtag-btn${selectedCategory === UNTAGGED_ONLY ? " toggled" : ""}`}
        aria-pressed={selectedCategory === UNTAGGED_ONLY}
        title={t("library.archivesWithNoTagsAt") ?? undefined}
        onClick={() => onToggleCategory(UNTAGGED_ONLY)}
      >
        <StatusIcon kind="untagged" /> {t("library.untaggedArchives")}
      </button>
      <button
        type="button"
        className={`favtag-btn${selectedCategory === SOURCE_ONLY ? " toggled" : ""}`}
        aria-pressed={selectedCategory === SOURCE_ONLY}
        title={t("library.archivesWithASourceTag") ?? undefined}
        onClick={() => onToggleCategory(SOURCE_ONLY)}
      >
        <i className="fas fa-link" aria-hidden="true" /> {t("library.withSource")}
      </button>
      <button
        type="button"
        className={`favtag-btn${selectedCategory === NO_SOURCE_ONLY ? " toggled" : ""}`}
        aria-pressed={selectedCategory === NO_SOURCE_ONLY}
        title={t("library.archivesWithoutASourceTag") ?? undefined}
        onClick={() => onToggleCategory(NO_SOURCE_ONLY)}
      >
        <i className="fas fa-link-slash" aria-hidden="true" /> {t("library.withoutSource")}
      </button>
      {hasTankoubons && (
        <button
          type="button"
          className={`favtag-btn${selectedCategory === TANKOUBON_ONLY ? " toggled" : ""}`}
        aria-pressed={selectedCategory === TANKOUBON_ONLY}
          title={t("library.tankoubonVolumes") ?? undefined}
          onClick={() => onToggleCategory(TANKOUBON_ONLY)}
        >
          <StatusIcon kind="tankoubon" /> {t("categories.tankoubons")}
        </button>
      )}
      {visible.map((c) => (
        <button
          key={c.id}
          type="button"
          className={`favtag-btn${selectedCategory === c.id ? " toggled" : ""}`}
        aria-pressed={selectedCategory === c.id}
          onClick={() => onToggleCategory(c.id)}
        >
          {c.pinned ? "📌 " : ""}
          {c.name}
        </button>
      ))}
      {overflow.length > 0 && (
        <select
          id="catdropdown"
          className="favtag-btn"
          value=""
          onChange={(e) => {
            if (e.target.value) onToggleCategory(e.target.value)
          }}
        >
          <option value="" disabled>
            {t("library.moreCategories")}
          </option>
          {overflow.map((c) => (
            <option key={c.id} value={c.id}>
              {c.pinned ? "📌 " : ""}
              {c.name}
            </option>
          ))}
        </select>
      )}
    </div>
  )
}
