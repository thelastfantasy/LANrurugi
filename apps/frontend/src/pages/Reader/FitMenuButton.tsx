import { Popover } from "@base-ui/react/popover"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { IconButton } from "@/components/common-ui/Form"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FIT_MODE, type FitMode } from "@/hooks/useReaderSettings"
import {
  FLOATING_POPUP_SHADOW,
  FLOATING_POPUP_TRANSITION_CLASSES,
  FONT_SIZE_MD,
  Z_OVERLAY_TOOLTIP,
} from "@/theme"

/**
 * The fullscreen control, split into a primary action plus a menu (desktop only).
 *
 * The menu carries the reader's fit modes next to fullscreen, and whichever entry was used last
 * becomes the primary button's own action — so a reader who always reads at "fit width" reaches it
 * in one click from the toolbar instead of opening the options panel every time. The chosen fit
 * mode itself still lives in `useReaderSettings`'s `fitMode` (the options panel and this menu are
 * therefore always in sync); only *which action the button performs* is extra state, and it is
 * app-only rather than a legacy reader setting, so it is stored under its own key.
 */
export type FitAction = "fullscreen" | FitMode

/** Exported so the reader can own the choice (top and bottom toolbars are separate instances; a
 *  per-instance state would let them disagree until a reload). */
export const FIT_ACTIONS: readonly FitAction[] = [
  "fullscreen",
  FIT_MODE.CONTAINER,
  FIT_MODE.FIT_WIDTH,
  FIT_MODE.FIT_HEIGHT,
]

const STORAGE_KEY = "readerFitAction"

/** Icon per action. Fullscreen flips to `compress` while actually fullscreen, as before. Returned
 *  as a bare name so the toolbar button and the menu rows can each pick their own size. */
function actionIconName(action: FitAction, isFullscreen: boolean): string {
  switch (action) {
    case "fullscreen":
      return isFullscreen ? "fa-compress" : "fa-expand"
    case FIT_MODE.FIT_WIDTH:
      return "fa-left-right"
    case FIT_MODE.FIT_HEIGHT:
      return "fa-up-down"
    case FIT_MODE.CONTAINER:
      return "fa-vector-square"
  }
}

export function readStoredFitAction(): FitAction {
  const raw = localStorage.getItem(STORAGE_KEY)
  return FIT_ACTIONS.find((action) => action === raw) ?? "fullscreen"
}

export function storeFitAction(action: FitAction): void {
  localStorage.setItem(STORAGE_KEY, action)
}

export function FitMenuButton({
  isFullscreen,
  onToggleFullscreen,
  onSelectFitMode,
  primary,
  onPrimaryChange,
  compact,
}: {
  isFullscreen: boolean
  onToggleFullscreen: () => void
  onSelectFitMode: (mode: FitMode) => void
  /** The action the primary half performs — owned by the reader so both toolbars agree. */
  primary: FitAction
  onPrimaryChange: (action: FitAction) => void
  /** Below the mobile toolbar breakpoint the plain fullscreen button stays: a split control with a
   *  menu is a desktop affordance and only costs a tap target on a phone. */
  compact: boolean
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const [open, setOpen] = useState(false)

  const run = (action: FitAction) => {
    if (action === "fullscreen") {
      onToggleFullscreen()
    } else {
      onSelectFitMode(action)
    }
  }

  if (compact) {
    return (
      <IconButton
        variant="ghost-btn"
        icon={`fas ${actionIconName("fullscreen", isFullscreen)} fa-2x`}
        title={t("reader.fullscreen") ?? undefined}
        size={32}
        style={{ marginRight: 13, borderRadius: "50%" }}
        onClick={onToggleFullscreen}
      />
    )
  }

  const labelFor = (action: FitAction) =>
    action === "fullscreen"
      ? (t("reader.fullscreen") ?? "")
      : `${t("reader.fitDisplayTo") ?? ""} · ${t(
          action === FIT_MODE.FIT_WIDTH
            ? "reader.width"
            : action === FIT_MODE.FIT_HEIGHT
              ? "reader.height"
              : "reader.container",
        ) ?? ""}`

  return (
    <div className="reader-split-control" style={{ marginRight: 13 }}>
      <IconButton
        variant="ghost-btn"
        icon={`fas ${actionIconName(primary, isFullscreen)} fa-2x`}
        title={labelFor(primary) || undefined}
        size={32}
        // Wider than the 32px default so the icon keeps equal clearance on both sides instead of
        // sitting in the pill's rounded corner; `minWidth` repeated for the reason noted below.
        style={{ borderRadius: "50% 0 0 50%", width: 38, minWidth: 38 }}
        onClick={() => run(primary)}
      />
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger
          render={
            <IconButton
              variant="ghost-btn"
              icon="fas fa-caret-down"
              title={t("reader.fitDisplayTo") ?? undefined}
              aria-label={t("reader.fitDisplayTo") ?? undefined}
              aria-haspopup="menu"
              size={32}
              // `minWidth` has to be repeated here: `IconButton` sets its own from `size` before
              // spreading `style`, so a width-only override leaves the button at its default 32px.
              style={{ borderRadius: "0 50% 50% 0", width: 26, minWidth: 26 }}
            />
          }
        />
        <Popover.Portal>
          <Popover.Positioner sideOffset={8} collisionPadding={8} className="outline-none" style={{ zIndex: Z_OVERLAY_TOOLTIP }}>
            <Popover.Popup
              className={`rounded-[.3em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
              style={{
                background: palette.bg,
                border: `1px solid ${palette.border}`,
                boxShadow: FLOATING_POPUP_SHADOW,
                color: palette.text,
                padding: "6px",
                fontSize: FONT_SIZE_MD,
                textAlign: "left",
                minWidth: 160,
                transformOrigin: "var(--transform-origin)",
              }}
            >
              {FIT_ACTIONS.map((action) => (
                <Popover.Close
                  key={action}
                  render={
                    <button
                      type="button"
                      role="menuitem"
                      aria-current={action === primary}
                      onClick={() => {
                        run(action)
                        onPrimaryChange(action)
                      }}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        width: "100%",
                        padding: "6px 8px",
                        background: action === primary ? palette.hoverBg : "transparent",
                        border: 0,
                        borderRadius: ".2em",
                        color: "inherit",
                        font: "inherit",
                        textAlign: "left",
                        cursor: "pointer",
                      }}
                    />
                  }
                >
                  <i
                    className={`fas ${actionIconName(action, isFullscreen)}`}
                    aria-hidden="true"
                    style={{ width: 14, textAlign: "center" }}
                  />
                  <span>{labelFor(action)}</span>
                  <i
                    className="fa fa-check"
                    aria-hidden="true"
                    style={{ marginLeft: "auto", opacity: action === primary ? 1 : 0 }}
                  />
                </Popover.Close>
              ))}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </div>
  )
}
