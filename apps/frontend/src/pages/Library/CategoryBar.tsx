import { useTranslation } from 'react-i18next'

import type { CategoryMetadata } from '../../api/types'
import { CATEGORY_BUTTON_CAP, NEW_ONLY, UNTAGGED_ONLY } from './shared'

export function CategoryBar({
  selectedCategory,
  sortedCategories,
  onToggleCategory,
}: {
  selectedCategory: string
  sortedCategories: CategoryMetadata[]
  onToggleCategory: (id: string) => void
}) {
  const { t } = useTranslation()
  const visible = sortedCategories.slice(0, CATEGORY_BUTTON_CAP)
  const overflow = sortedCategories.slice(CATEGORY_BUTTON_CAP)

  return (
    <div id="category-container">
      <button
        type="button"
        className={`favtag-btn${selectedCategory === NEW_ONLY ? ' toggled' : ''}`}
        title={t('Archives added within the last day') ?? undefined}
        onClick={() => onToggleCategory(NEW_ONLY)}
      >
        🆕 {t('New Archives')}
      </button>
      <button
        type="button"
        className={`favtag-btn${selectedCategory === UNTAGGED_ONLY ? ' toggled' : ''}`}
        title={t('Archives with no tags at all') ?? undefined}
        onClick={() => onToggleCategory(UNTAGGED_ONLY)}
      >
        🏷️ {t('Untagged Archives')}
      </button>
      {visible.map((c) => (
        <button
          key={c.id}
          type="button"
          className={`favtag-btn${selectedCategory === c.id ? ' toggled' : ''}`}
          onClick={() => onToggleCategory(c.id)}
        >
          {c.pinned ? '📌 ' : ''}
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
            {t('More categories…')}
          </option>
          {overflow.map((c) => (
            <option key={c.id} value={c.id}>
              {c.pinned ? '📌 ' : ''}
              {c.name}
            </option>
          ))}
        </select>
      )}
    </div>
  )
}
