import { useTranslation } from "react-i18next"

import { useApiTokens } from "@/api/hooks"
import type { ActivityEntry } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { DateTimeStack } from "@/components/Display"

import { actorChipParts } from "./activityActor"
import { ActivityChip } from "./ActivityChip"
import { actionTypeColor, outcomeColor } from "./activityColors"
import { actionTypeLabel, outcomeLabel } from "./activityTarget"
import { OperationDescription } from "./OperationDescription"

/** One row of the Activity list. Wide mode is a CSS grid row (`display: contents`); narrow mode
 * renders each entry as its own self-contained card since a 6-column grid has no room below that. */
export function ActivityRow({
  entry,
  selected,
  selectable,
  narrow,
  onToggleSelect,
  onOpenDetail,
  onDelete,
}: {
  entry: ActivityEntry
  selected: boolean
  selectable: boolean
  /** Switches between wide-mode grid-row rendering and narrow-mode card rendering. */
  narrow: boolean
  onToggleSelect: () => void
  onOpenDetail: () => void
  onDelete: () => void
}) {
  const { t } = useTranslation()
  const apiTokens = useApiTokens()
  const token = entry.actor.kind === "token" && entry.actor.id ? apiTokens.data?.find((tk) => tk.id === entry.actor.id) : undefined
  const { label: actorLabel, color: actorColor, tooltip: actorTooltip } = actorChipParts(t, entry, token)

  const actorChip = (
    <ActivityChip color={actorColor}>
      {actorTooltip ? (
        <Tooltip label={actorTooltip} wrapperStyle={{ alignItems: "center" }}>
          {actorLabel}
        </Tooltip>
      ) : (
        actorLabel
      )}
    </ActivityChip>
  )
  const actionChip = <ActivityChip color={actionTypeColor(entry.action_type)}>{actionTypeLabel(t, entry.action_type)}</ActivityChip>
  const outcomeChip = <ActivityChip color={outcomeColor(entry.outcome.status)}>{outcomeLabel(t, entry.outcome.status)}</ActivityChip>

  if (narrow) {
    return (
      <div
        className={entry.auto_or_manual === "automatic" ? "activity-row-automatic" : undefined}
        style={{ display: "flex", alignItems: "flex-start", gap: 6, padding: "10px 4px", cursor: "pointer", minWidth: 0 }}
        onClick={onOpenDetail}
      >
        {selectable && (
          <div style={{ flexShrink: 0, marginTop: 3 }} onClick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              aria-label={t("activity.selectEntry") ?? undefined}
              checked={selected}
              onChange={onToggleSelect}
            />
          </div>
        )}
        <div style={{ display: "flex", flexDirection: "column", gap: 6, flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: 6 }}>
            <span style={{ whiteSpace: "nowrap" }}>
              <DateTimeStack epochSeconds={entry.timestamp} />
            </span>
            {actorChip}
            {actionChip}
            {outcomeChip}
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <span style={{ textAlign: "left", flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              <OperationDescription entry={entry} />
            </span>
            {selectable && (
              <div style={{ flexShrink: 0 }} onClick={(e) => e.stopPropagation()}>
                <IconButton
                  icon="fas fa-trash"
                  className="stdbtn stdbtn-danger"
                  size={26}
                  style={{ borderRadius: 6 }}
                  title={t("common.delete") ?? undefined}
                  onClick={onDelete}
                />
              </div>
            )}
          </div>
        </div>
      </div>
    )
  }

  // `.activity-row:hover > *` targets children via child combinator since this row has no own box
  // (display: contents); `.activity-row-delete-btn` is hidden by default, shown only on hover.
  return (
    <div
      className={`activity-row${entry.auto_or_manual === "automatic" ? " activity-row-automatic" : ""}`}
      style={{ display: "contents", cursor: "pointer" }}
      onClick={onOpenDetail}
    >
      <div
        style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "8px 6px", margin: "0 -6px 0 0" }}
        onClick={(e) => e.stopPropagation()}
      >
        {selectable && (
          <input type="checkbox" aria-label={t("activity.selectEntry") ?? undefined} checked={selected} onChange={onToggleSelect} />
        )}
      </div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px", whiteSpace: "nowrap" }}>
        <DateTimeStack epochSeconds={entry.timestamp} />
      </div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px" }}>{actorChip}</div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px" }}>{actionChip}</div>
      <div
        className="activity-row-content"
        style={{
          display: "flex",
          alignItems: "center",
          padding: "8px 6px",
          margin: "0 -6px",
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          borderRadius: 4,
        }}
      >
        <OperationDescription entry={entry} />
      </div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px" }}>{outcomeChip}</div>
      {selectable && (
        <div
          style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "8px 6px", margin: "0 0 0 -6px" }}
          onClick={(e) => e.stopPropagation()}
        >
          <IconButton
            icon="fas fa-trash"
            className="stdbtn stdbtn-danger activity-row-delete-btn"
            size={26}
            style={{ borderRadius: 6 }}
            title={t("common.delete") ?? undefined}
            onClick={onDelete}
          />
        </div>
      )}
    </div>
  )
}
