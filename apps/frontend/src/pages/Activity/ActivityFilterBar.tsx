import { useTranslation } from "react-i18next"

import type { ActivityFacets } from "@/api/types"
import { useIsNarrowViewport } from "@/hooks"

import { ACTIVITY_FILTER_ROW_HEIGHT } from "./ActivityCombobox"
import { ActivityFilterCombobox } from "./ActivityFilterCombobox"
import { TimeRangeCombobox, type TimeRangeValue } from "./TimeRangeCombobox"

export interface ActivityFilterState extends TimeRangeValue {
  actors: string[]
  actionTypes: string[]
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

  // Below the narrow-viewport breakpoint, the delete button still stacks full-width on its own
  // row — the merged filter Combobox's own `wide` mode (320–560px) has nowhere near that much room
  // on a real phone viewport, so letting everything `flex-wrap` onto one shared row (the very first
  // version) left it squeezed into whatever was left over on a half-filled trailing row, confirmed
  // live to render exactly as cramped as that sounds. The filter combobox and time-range trigger,
  // though, share their own row: the combobox's `flex: 1` absorbs the available width while the
  // trigger stays sized to its own (short, translated preset name) label — full-width-stacking the
  // trigger by itself (an earlier version of this row) left it stretched edge-to-edge for no
  // reason, confirmed live as visually much wider than its own short text needed. The retention
  // period setting itself lives in `RetentionSettingsMenu`'s own gear-icon popover in the page's
  // top-right corner now, not in this bar at all — see that component's own doc comment.
  if (narrow) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <ActivityFilterCombobox
              actionTypes={filter.actionTypes}
              actors={filter.actors}
              onActionTypesAndActorsChange={(actionTypes, actors) => onFilterChange({ ...filter, actionTypes, actors })}
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
        onActionTypesAndActorsChange={(actionTypes, actors) => onFilterChange({ ...filter, actionTypes, actors })}
        facets={facets}
      />
      <TimeRangeCombobox onValueChange={(range) => onFilterChange({ ...filter, ...range })} />
      {deleteButton}
    </div>
  )
}
