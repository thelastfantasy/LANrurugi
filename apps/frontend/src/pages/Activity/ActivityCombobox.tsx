import { Combobox } from "@base-ui/react/combobox"
import type { ReactNode } from "react"

import { Tooltip } from "@/components/Display"
import { useIsNarrowViewport } from "@/hooks"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FONT_SIZE_SM, Z_OVERLAY_CONTENT } from "@/theme"

import { ActivityChip, CHIP_REMOVE_BUTTON_STYLE } from "./ActivityChip"
import type { ChipColor } from "./activityColors"

/** A real, always-visible "this is floating above everything else" drop shadow for the Activity
 * page's Combobox popups — deliberately NOT `palette.shadow` (`useMenuPalette`'s own per-theme
 * value, `PopupMenu.tsx`'s source of truth for right-click/gear menus), since two of the five real
 * themes (`g.css`/`ex.css`) set that value to the literal string `"none"` — a popup using it would
 * render with zero elevation cue at all on those two themes, reading as flatly stuck to the page
 * rather than floating over it. A fixed, theme-independent shadow (plus the scale/opacity
 * transition on `data-starting-style`/`data-ending-style` below, Base UI's own documented CSS
 * hook for this) is what actually gives "floats above the page" its visual weight, regardless of
 * which theme happens to be active. */
const FLOATING_POPUP_SHADOW = "0 8px 24px rgba(0,0,0,0.35), 0 2px 6px rgba(0,0,0,0.25)"

/** Scale/opacity enter+exit transition driven by Base UI's own `data-starting-style`/
 * `data-ending-style` attributes (its documented CSS-transition hook, present on the popup only
 * while it's actively animating in/out) — paired with an inline `transformOrigin: "var(--transform-
 * origin)"` (not a Tailwind arbitrary-value class for the same property, which would depend on
 * Tailwind's own CSS-custom-property interpolation support rather than a plain, guaranteed-to-work
 * inline style) so the scale animates from the edge closest to the trigger, matching where the
 * popup is actually anchored, rather than always from dead center. */
const FLOATING_POPUP_TRANSITION_CLASSES =
  "transition-[transform,opacity] duration-150 ease-out data-[starting-style]:scale-95 data-[starting-style]:opacity-0 data-[ending-style]:scale-95 data-[ending-style]:opacity-0"

/** Shared minimum row height for every control in the Activity filter bar (this multi-select
 * shell, the single-select time-range trigger, and — via `ActivityFilterBar.tsx`'s own use of this
 * same constant — the delete button) so the whole row reads as one consistent height regardless of
 * how many chips the multi-select currently holds. Sized to fit one line of 24px-tall
 * `ActivityChip`s plus this shell's own 2px top/bottom padding; without an explicit floor the empty
 * state (just placeholder text, no chips) rendered visibly shorter than the chip-filled state,
 * confirmed live as a jarring height jump when the first chip was added/the last one removed. */
export const ACTIVITY_FILTER_ROW_HEIGHT = 30

/** What a selected value renders as inside its own removable chip — `content` is the chip's own
 * label (i18n-translated where applicable), `color` its fixed categorical color (see
 * `activityColors.ts`), and `tooltip` an optional hover label for when the visible text alone
 * doesn't fully identify the value (e.g. a token's display name doesn't show its id). */
export interface ChipRenderResult {
  content: ReactNode
  color: ChipColor
  tooltip?: ReactNode
}

/** Shared visual shell for the Activity page's two multi-select filter comboboxes (action type /
 * actor — `TimeRangeCombobox` is single-select and keeps its own simpler trigger-only shell).
 * Base UI's own components (`Combobox.Root`/`InputGroup`/`Chips`/`Input`/`Trigger`/`Positioner`/
 * `Popup`/`List`/`Item`/`Group`/`GroupLabel`) are unstyled by design (`className`, which can be a
 * `(state) => string` function, plus `data-*` state attributes are the documented customization
 * surface); `Combobox.Chip`/`ChipRemove` specifically are used only for their *behavior*
 * (removable-chip semantics/keyboard handling) via the `render` prop, composed with this page's
 * own {@link ActivityChip} for the actual colored-pill visual — Base UI's own default chip
 * rendering is an unstyled `<div>`, and this page's chips need a real per-value color, not the
 * single flat theme-palette tint an unconditional `className` could give every chip alike.
 *
 * `renderChip` renders each currently-selected value as a chip inside `Combobox.Value`'s own
 * render-prop, per Base UI's documented multiple-selection pattern (`selectedValue` is `string[]`
 * when the root's `multiple` prop is set) — the caller supplies the chip's own label/color/tooltip
 * (e.g. an i18n-translated action-type name) since only it knows how to look those up from the raw
 * string value. */
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
   * `ActivityFilterCombobox`, whose chip row can carry both action-type and actor selections at
   * once and needs more room than either standalone combobox did on its own. */
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
        // Below the narrow-viewport breakpoint this fills its own container's width instead of
        // the fixed `min`/`maxWidth` caps below — those caps assume the desktop filter bar's own
        // horizontal layout, where several controls share one row and this one shouldn't hog it;
        // on a real phone viewport (`ActivityFilterBar.tsx`'s own narrow layout stacks controls
        // full-width instead) the same fixed `minWidth: 320` forced this control to overflow its
        // container regardless of how narrow the actual viewport was, confirmed live.
        width: narrow ? "100%" : "auto",
        minWidth: narrow ? undefined : wide ? 320 : 160,
        maxWidth: narrow ? undefined : wide ? 560 : 320,
        minHeight: ACTIVITY_FILTER_ROW_HEIGHT,
        padding: "2px 6px",
        boxSizing: "border-box",
        // `.stdinput`'s asymmetric `margin: 4px 1px 0` throws off vertical centering against
        // `.stdbtn`'s neighbors; zero only the vertical axis, keep the horizontal 1px.
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
      {/* No `className` (an unstyled `<button>` otherwise carries the browser's own default
          border/background/padding chrome) — matches the flat, inline caret
          `ActivitySingleSelectShell`'s `.stdbtn` trigger shows next to "全部时间", instead of
          rendering as its own separate boxed button next to the input. */}
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
              // `--anchor-width` (Base UI's own `ComboboxPositionerCssVars`) is the trigger's real
              // rendered width — matching the popup to it keeps the dropdown visually aligned with
              // the input it belongs to instead of shrinking to its own content's intrinsic width,
              // which reads as a mismatched, narrower-than-the-input popup (confirmed live once the
              // input itself became wide, e.g. this shell's `wide`/full-width narrow-mode variants).
              width: "var(--anchor-width)",
              minWidth: wide ? 320 : 220,
              maxHeight: 400,
              overflowY: "auto",
              fontSize: FONT_SIZE_SM,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            {/* Base UI's own docs: `Combobox.Empty`'s root element must stay mounted at all times
                (never conditionally rendered/hidden) for consistent screen-reader announcements —
                only its `children` become `null` once the list isn't empty. Styling belongs on a
                wrapper *inside* those children, not on `Combobox.Empty` itself: padding applied
                directly to it stayed put even with no children, confirmed live as a blank padded
                strip sitting above the results whenever the list wasn't actually empty. */}
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

/** Trigger-only shell for the Activity page's single-select filter combobox (`TimeRangeCombobox`
 * — a time range is one value, never a set to union together, so it has no business rendering
 * chips). Kept separate from {@link ActivityComboboxShell} above rather than making that one
 * handle both shapes, since a single-select trigger button and a multi-select chip-input-group
 * are different enough visual objects that trying to share one component would need more
 * conditional branching than just having two small ones. */
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
        // `.stdbtn` defaults to `content-box` (no explicit `box-sizing`), so force `border-box`
        // to match `.stdinput`'s neighbor and zero only the vertical margin, keeping horizontal.
        boxSizing: "border-box",
        // Matches `.stdbtn`'s own `0 4px 1px` vertical asymmetry (not a flat `0 10px`) — the
        // delete button next to this trigger has no padding override and keeps that asymmetry, so
        // overriding it flat here shifted this trigger's content 1px up relative to it.
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

/** One selectable row — its own label renders as a real miniature `ActivityChip` (the same colored
 * pill its selected-value chip becomes once picked), not plain text next to a color swatch dot —
 * so the dropdown itself directly previews what color/shape each candidate's chip will be, rather
 * than requiring the user to select it first to see. `data-highlighted` (keyboard/pointer focus
 * within the popup, distinct from CSS `:hover` since `PopupMenu`'s own precedent keeps those
 * independently controllable) still drives a background swap on the row itself via the palette,
 * so the highlighted row is visually distinguishable from its neighbors independent of the chip's
 * own fixed color. `color` is optional (a plain row with no categorical color — none exist today,
 * but this keeps the type honest for one that might) — omitted, `label` renders as plain text
 * instead of a chip. `count`/`tooltip` are appended after the chip, outside its own colored
 * boundary, matching how a facet's usage count and a token's hover detail read better as
 * plain neighboring content than crammed inside the pill itself. */
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
  // Compact padding (`px-1 py-[.15em]`) only applies to a real colored chip row — those wrap
  // several to a line (`ActivityComboboxGroup`'s own flex-wrap container), so a tight hit target
  // is what lets many fit per row. A plain-text row (no `color` — `TimeRangeCombobox`'s own preset
  // list, one per line, no wrapping) needs its own full-row padding instead; reusing the chip's
  // compact padding here made those rows look visibly cramped compared to before, confirmed live.
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
      {/* Items wrap onto shared rows instead of each claiming its own full-width line (each
          `Combobox.Item` renders as a plain block `<div>` by default) — with a real facet list
          this could otherwise run to a dozen+ one-per-line rows, confirmed live as needing a lot
          of scrolling for what's mostly short chip labels that fit several to a line. */}
      <div style={{ display: "flex", flexWrap: "wrap", gap: 4, padding: "0 8px 4px" }}>{children}</div>
    </Combobox.Group>
  )
}
