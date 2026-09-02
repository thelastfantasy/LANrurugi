import { Combobox } from "@base-ui/react/combobox"
import type { ReactNode } from "react"

import { Tooltip } from "@/components/common-ui/Display"
import { useIsNarrowViewport } from "@/hooks"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, FONT_SIZE_SM, Z_OVERLAY_CONTENT } from "@/theme"

import { ActivityChip, CHIP_REMOVE_BUTTON_STYLE } from "./ActivityChip"
import type { ChipColor } from "./activityColors"

/** Shared minimum row height for every control in the Activity filter bar, so the row doesn't
 * jump height between the empty (placeholder-only) and chip-filled states. */
export const ACTIVITY_FILTER_ROW_HEIGHT = 30

/** What a selected value renders as inside its removable chip: label, categorical color, and an
 * optional hover tooltip for when the visible text doesn't fully identify the value. */
export interface ChipRenderResult {
  content: ReactNode
  color: ChipColor
  tooltip?: ReactNode
}

/** Shared visual shell for the Activity page's two multi-select filter comboboxes. `Combobox.Chip`
 * is used only for its behavior; {@link ActivityChip} supplies the actual colored-pill visual. */
export function ActivityComboboxShell({
  placeholder,
  emptyLabel,
  renderChip,
  children,
  wide,
}: {
  placeholder: string
  emptyLabel: string
  renderChip: (value: string) => ChipRenderResult
  children: ReactNode
  /** Widens the control (and its popup) beyond the default 320px cap — used by the merged
   * combobox, whose chip row can carry both action-type and actor selections at once. */
  wide?: boolean
}) {
  const palette = useMenuPalette()
  const narrow = useIsNarrowViewport()
  return (
    <Combobox.InputGroup
      className="stdinput"
      style={{
        display: "inline-flex",
        flexWrap: "wrap",
        alignItems: "center",
        gap: 4,
        width: narrow ? "100%" : "auto",
        minWidth: narrow ? undefined : wide ? 320 : 160,
        maxWidth: narrow ? undefined : wide ? 560 : 320,
        minHeight: ACTIVITY_FILTER_ROW_HEIGHT,
        padding: "2px 6px",
        boxSizing: "border-box",
        marginTop: 0,
        marginBottom: 0,
        marginLeft: 1,
        marginRight: 1,
      }}
    >
      <Combobox.Chips style={{ display: "contents" }}>
        <Combobox.Value>
          {(selectedValue: string[]) =>
            selectedValue.map((value) => {
              const { content, color, tooltip } = renderChip(value)
              const pill = (
                <ActivityChip
                  color={color}
                  removeSlot={
                    <Combobox.ChipRemove
                      aria-label={emptyLabel}
                      render={
                        <button type="button" style={CHIP_REMOVE_BUTTON_STYLE}>
                          <i className="fa fa-times"></i>
                        </button>
                      }
                    />
                  }
                >
                  {tooltip ? (
                    <Tooltip label={tooltip} wrapperStyle={{ alignItems: "center" }}>
                      {content}
                    </Tooltip>
                  ) : (
                    content
                  )}
                </ActivityChip>
              )
              return <Combobox.Chip key={value} render={<span style={{ display: "contents" }}>{pill}</span>} />
            })
          }
        </Combobox.Value>
      </Combobox.Chips>
      <Combobox.Input
        placeholder={placeholder}
        style={{ border: "none", background: "transparent", outline: "none", flex: 1, minWidth: 60, fontSize: FONT_SIZE_SM }}
      />
      <Combobox.Trigger
        style={{
          display: "inline-flex",
          alignItems: "center",
          cursor: "pointer",
          background: "transparent",
          border: "none",
          padding: 0,
          color: "inherit",
        }}
      >
        <i className="fa fa-caret-down" style={{ fontSize: FONT_SIZE_SM }}></i>
      </Combobox.Trigger>
      <Combobox.Portal>
        <Combobox.Positioner sideOffset={4} className="outline-none" style={{ zIndex: Z_OVERLAY_CONTENT }}>
          <Combobox.Popup
            className={`rounded-[.2em] py-[.25em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              width: "var(--anchor-width)",
              minWidth: wide ? 320 : 220,
              maxHeight: 400,
              overflowY: "auto",
              fontSize: FONT_SIZE_SM,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            <Combobox.Empty>
              <div style={{ padding: "8px 12px", opacity: 0.65 }}>{emptyLabel}</div>
            </Combobox.Empty>
            <Combobox.List>{children}</Combobox.List>
          </Combobox.Popup>
        </Combobox.Positioner>
      </Combobox.Portal>
    </Combobox.InputGroup>
  )
}

/** Trigger-only shell for the single-select filter combobox (`TimeRangeCombobox`) — kept separate
 * from {@link ActivityComboboxShell} since the two are visually different enough objects. */
export function ActivitySingleSelectShell({
  placeholder,
  triggerLabel,
  children,
}: {
  placeholder: string
  triggerLabel: string
  children: ReactNode
}) {
  const palette = useMenuPalette()
  return (
    <Combobox.Trigger
      className="stdbtn"
      style={{
        minWidth: 0,
        width: "auto",
        height: ACTIVITY_FILTER_ROW_HEIGHT,
        // border-box to match .stdinput's neighbor; padding matches .stdbtn's own asymmetry.
        boxSizing: "border-box",
        padding: "0 10px 1px",
        marginTop: 0,
        marginBottom: 0,
        marginLeft: 1,
        marginRight: 1,
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
      }}
      aria-label={triggerLabel}
    >
      <Combobox.Value>{() => triggerLabel}</Combobox.Value>
      <i className="fa fa-caret-down" style={{ fontSize: FONT_SIZE_SM }}></i>
      <Combobox.Portal>
        <Combobox.Positioner sideOffset={4} className="outline-none" style={{ zIndex: Z_OVERLAY_CONTENT }}>
          <Combobox.Popup
            className={`rounded-[.2em] py-[.25em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              minWidth: 220,
              maxHeight: 320,
              overflowY: "auto",
              fontSize: FONT_SIZE_SM,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            <div style={{ padding: "4px 10px" }}>
              <Combobox.Input
                placeholder={placeholder}
                className="stdinput"
                style={{ width: "100%", fontSize: FONT_SIZE_SM }}
              />
            </div>
            <Combobox.Empty>
              <div style={{ padding: "8px 12px", opacity: 0.65 }}>{placeholder}</div>
            </Combobox.Empty>
            <Combobox.List>{children}</Combobox.List>
          </Combobox.Popup>
        </Combobox.Positioner>
      </Combobox.Portal>
    </Combobox.Trigger>
  )
}

/** One selectable row — its label renders as a miniature `ActivityChip` so the dropdown previews
 * the picked look up front. `count`/`tooltip` render outside the chip's own colored boundary. */
export function ActivityComboboxItem({
  value,
  color,
  label,
  count,
  tooltip,
}: {
  value: string
  color?: ChipColor
  label: ReactNode
  count?: number
  tooltip?: ReactNode
}) {
  const palette = useMenuPalette()
  const chip = color ? <ActivityChip color={color}>{label}</ActivityChip> : label
  const itemPadding = color ? "px-1 py-[.15em]" : "px-3 py-[.3em]"
  return (
    <Combobox.Item
      value={value}
      className={`activity-combobox-item-highlighted relative box-content flex cursor-pointer items-center gap-1 whitespace-nowrap select-none rounded-[.2em] ${itemPadding} data-[highlighted]:text-[var(--activity-combobox-highlight-text)]`}
      style={
        {
          "--activity-combobox-highlight-text": palette.hoverText,
        } as React.CSSProperties
      }
    >
      {tooltip ? (
        <Tooltip label={tooltip} wrapperStyle={{ alignItems: "center" }}>
          {chip}
        </Tooltip>
      ) : (
        chip
      )}
      {count != null && <span style={{ opacity: 0.5 }}>{count}</span>}
    </Combobox.Item>
  )
}

export function ActivityComboboxGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Combobox.Group>
      <Combobox.GroupLabel
        style={{ padding: "4px 12px", opacity: 0.65, fontSize: FONT_SIZE_SM, fontWeight: 700, textAlign: "left" }}
      >
        {label}
      </Combobox.GroupLabel>
      {/* Items wrap onto shared rows instead of each claiming its own full-width block line. */}
      <div style={{ display: "flex", flexWrap: "wrap", gap: 4, padding: "0 8px 4px" }}>{children}</div>
    </Combobox.Group>
  )
}
