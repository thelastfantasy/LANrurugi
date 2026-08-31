import { useTranslation } from "react-i18next"

import type { ActivityFacets } from "@/api/types"
import { useIsNarrowViewport } from "@/hooks"

import { ACTIVITY_FILTER_ROW_HEIGHT } from "./ActivityCombobox"
import { ActivityFilterCombobox } from "./ActivityFilterCombobox"
import { TimeRangeCombobox, type TimeRangeValue } from "./TimeRangeCombobox"

export interface ActivityFilterState extends TimeRangeValue {
  actors: string[]
  actionTypes: string[]
  outcomes: string[]
}

export function ActivityFilterBar({
  filter,
  onFilterChange,
  facets,
  canDelete,
  selectedCount,
  onBulkDelete,
  bulkDeleting,
}: {
  filter: ActivityFilterState
  onFilterChange: (filter: ActivityFilterState) => void
  facets: ActivityFacets | undefined
  canDelete: boolean
  selectedCount: number
  onBulkDelete: () => void
  bulkDeleting: boolean
}) {
  const { t } = useTranslation()
  const narrow = useIsNarrowViewport()

  const deleteButton = canDelete && (
    <button
      type="button"
      className="stdbtn stdbtn-danger"
      style={{
        height: ACTIVITY_FILTER_ROW_HEIGHT,
        boxSizing: "border-box",
        marginTop: 0,
        marginBottom: 0,
        marginLeft: 1,
        marginRight: 1,
        ...(narrow ? { width: "100%" } : undefined),
      }}
      disabled={selectedCount === 0 || bulkDeleting}
      onClick={onBulkDelete}
    >
      {bulkDeleting ? t("activity.deleting") : t("activity.deleteSelectedN", { n: selectedCount })}
    </button>
  )

  // Narrow mode: delete button stacks full-width on its own row below the combobox/time-range row.
  if (narrow) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <ActivityFilterCombobox
              actionTypes={filter.actionTypes}
              actors={filter.actors}
              outcomes={filter.outcomes}
              onFilterDimensionsChange={(actionTypes, actors, outcomes) => onFilterChange({ ...filter, actionTypes, actors, outcomes })}
              facets={facets}
            />
          </div>
          <TimeRangeCombobox onValueChange={(range) => onFilterChange({ ...filter, ...range })} />
        </div>
        {deleteButton}
      </div>
    )
  }

  return (
    <div className="control-btn-group" style={{ flexWrap: "wrap", gap: 8, alignItems: "center" }}>
      <ActivityFilterCombobox
        actionTypes={filter.actionTypes}
        actors={filter.actors}
        outcomes={filter.outcomes}
        onFilterDimensionsChange={(actionTypes, actors, outcomes) => onFilterChange({ ...filter, actionTypes, actors, outcomes })}
        facets={facets}
      />
      <TimeRangeCombobox onValueChange={(range) => onFilterChange({ ...filter, ...range })} />
      {deleteButton}
    </div>
  )
}
