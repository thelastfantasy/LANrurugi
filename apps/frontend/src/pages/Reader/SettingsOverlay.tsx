import { useEffect } from "react"
import { useTranslation } from "react-i18next"

import type { FitMode, ReaderSettings } from "@/hooks/useReaderSettings"
import { ensureLink, FONT_SIZE_MD, FONT_SIZE_XS, removeLink } from "@/theme"

const CONFIG_CSS_ID = "reader-config-css"

/** One labeled group of controls (a button row, or an input + Apply button) — a from-scratch
 * flex layout replacing legacy's own `.config-panel` (`float: left; width: 90%`), which put every
 * section's title/description on their own forced line via floats colliding with the next
 * section's float, a layout mechanism this component doesn't use at all rather than merely
 * neutralizing (an earlier pass tried wrapping the old floated elements in a flex parent, which
 * only works by the CSS spec quietly computing `float` as `none` on a flex child — still
 * *depending* on float semantics under the hood, not actually removing them). */
function SettingSection({
  title,
  description,
  children,
}: {
  title: string
  description?: string
  children: React.ReactNode
}) {
  return (
    <div style={{ textAlign: "left", marginBottom: 20 }}>
      <h2 style={{ margin: "0 0 4px", fontSize: FONT_SIZE_MD, fontWeight: "bold" }}>{title}</h2>
      {description && <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.6, marginBottom: 6 }}>{description}</div>}
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8 }}>{children}</div>
    </div>
  )
}

/** Shared sizing for every plain `<input>`/Apply-button pair below — `.favtag-btn`'s own real
 * theme CSS gives it `height: 25px` with no `!important` (unlike legacy's `.config-btn`, which
 * hard-locks `height: 28px !important` and can't be matched by anything shorter without a fight),
 * so 25px is the number every control here targets instead. */
const CONTROL_HEIGHT = 25

// Mirrors legacy's `#settingsOverlay` (`[% BLOCK config %]` in
// `~/LANraragi/templates/reader.html.tt2`) — every toggle group from the real template, in the
// same order, but laid out with `SettingSection` above instead of legacy's own float-based
// `.config-panel`/`.config-btn`. Both rules have been deleted outright from `/legacy/config.css`
// (confirmed no other component referenced them) rather than just left unused — this page still
// scopes that file in (same pattern as `Settings.tsx`), but only for its real, unrelated
// `input[type=checkbox]` custom-checkbox rules (`Toggle Stamps` below).
export function SettingsOverlay({
  settings,
  update,
  onClose,
}: {
  settings: ReaderSettings
  update: (partial: Partial<ReaderSettings>) => void
  onClose: () => void
}) {
  const { t } = useTranslation()

  useEffect(() => {
    ensureLink(CONFIG_CSS_ID, "/legacy/config.css")
    return () => removeLink(CONFIG_CSS_ID)
  }, [])

  // Legacy marks the active choice in each toggle group by adding a `.toggled` class (reader.js's
  // `initializeSettings`/`toggleFitMode`/etc, e.g. `$("#fit-width").addClass("toggled")`) — a real
  // class already styled (background/border/color) by each theme's own CSS
  // (`~/LANraragi/public/themes/*.css`'s `.toggled` rule), not an ad-hoc inline style. No longer
  // stacked with `config-btn` (see `CONTROL_HEIGHT`'s own docs above).
  function btnClass(active: boolean) {
    return `favtag-btn${active ? " toggled" : ""}`
  }

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — must be explicitly shown, same as
          `ArchiveOverviewOverlay`'s copy of this element. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: "block", opacity: 0.6 }} onClick={onClose} />
      {/* No longer `small-overlay` — that class's real theme CSS (`width: 35% !important`, `left:
          32.5%` to match) isn't actually responsive: `width` on an `inline-block` (`.id1`'s own
          `display`) still yields to its content's real `min-content` size when 35% of a narrow
          viewport can't fit that content without wrapping, and the *fixed* `left: 32.5%` doesn't
          move to match wherever the box actually ended up — verified live at a 545px viewport:
          the box rendered 329px wide (not 190px, 35% of 545), visibly off-center with a large gap
          on one side (exactly what was reported). `width: 'min(90vw, 480px)'` + `left: '50%'` +
          `transform: 'translateX(-50%)'` replace it with a real responsive width that's always
          correctly centered regardless of how wide it ends up, at any viewport size. `maxHeight`/
          `overflowY` (`small-overlay`'s other two properties) are reproduced explicitly below,
          same values. `display: flex; flexDirection: 'column'; overflow: 'hidden'` moves the
          actual scrolling into the plain inner `<div>` (a flex child, `overflowY: 'auto'`) so
          `.id1`'s own 9px `border-radius` isn't squared off by a scrollbar track sharing its box
          (the same fix `.small-overlay`'s own `overflow-y: scroll` needed). */}
      <div
        id="settingsOverlay"
        className="id1 base-overlay"
        style={{
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          left: "50%",
          transform: "translateX(-50%)",
          width: "min(90vw, 480px)",
          maxHeight: "90vh",
        }}
      >
        {/* `width: '100%'` — this flex child wasn't actually being stretched to the parent's full
            cross-size by `align-items: stretch` (the flex container's default) in practice,
            verified live via `getBoundingClientRect`: it sat 87px short of the rounded outer
            box's own right edge, so the scrollbar (which tracks *this* element's own box, not the
            outer one) rendered nowhere near the actual rounded corner it needs to stay clear of. */}
        <div style={{ width: "100%", boxSizing: "border-box", overflowY: "auto", padding: "0 16px" }}>
          <h2 style={{ textAlign: "center" }}>{t("reader.readerOptions")}</h2>
          <h1 style={{ textAlign: "center", fontSize: FONT_SIZE_MD, marginBottom: 16 }}>
            {t("reader.thoseOptionsSaveAutomatically")}
          </h1>

          <SettingSection title={t("reader.fitDisplayTo") ?? ""}>
            {(
              [
                ["container", t("reader.container")],
                ["fit-width", t("reader.width")],
                ["fit-height", t("reader.height")],
              ] as [FitMode, string][]
            ).map(([mode, label]) => (
              <input
                key={mode}
                className={btnClass(settings.fitMode === mode)}
                type="button"
                style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                value={label}
                onClick={() => update({ fitMode: mode })}
              />
            ))}
          </SettingSection>

          {/* Both this section's own gate (`fitMode === 'container'`) and the four below
              (`!infiniteScroll`) mirror upstream's post-rewrite `reader_options.js` exactly
              (`isFitContainerMode`/`notInfiniteScroll` `computed()` signals, verified via
              `git show` against `Difegue/LANraragi@a373e339`, the current tip for that file) —
              this component previously rendered all of them unconditionally. */}
          {settings.fitMode === "container" && (
            <SettingSection
              title={t("reader.containerWidthInPixelsOr") ?? ""}
              description={t("reader.theDefaultValueIs1200px") ?? undefined}
            >
              {/* A short `placeholder` (not the full sentence removed from here — that's the
                  `description` above now, since it didn't fit this field's narrower width) so an
                  empty field (no custom width saved yet) still shows *some* hint of what's
                  actually in effect, instead of reading as blank/broken. */}
              <input
                id="container-width-input"
                className="stdinput"
                style={{ width: "8em", height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                placeholder="1200px"
                defaultValue={settings.containerWidth}
                onKeyDown={(e) => {
                  if (e.key === "Enter") update({ containerWidth: e.currentTarget.value })
                }}
              />
              <input
                className="favtag-btn"
                type="button"
                style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                value={t("reader.apply") ?? undefined}
                onClick={(e) => {
                  const input = e.currentTarget.previousElementSibling as HTMLInputElement
                  update({ containerWidth: input.value })
                }}
              />
            </SettingSection>
          )}

          {!settings.infiniteScroll && (
            <>
              <SettingSection title={t("reader.pageRendering") ?? ""}>
                <input
                  className={btnClass(!settings.doublePageMode)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.single") ?? undefined}
                  onClick={() => update({ doublePageMode: false })}
                />
                <input
                  className={btnClass(settings.doublePageMode)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.double") ?? undefined}
                  onClick={() => update({ doublePageMode: true })}
                />
              </SettingSection>

              <SettingSection title={t("reader.readingDirection") ?? ""}>
                <input
                  className={btnClass(!settings.mangaMode)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.leftToRight") ?? undefined}
                  onClick={() => update({ mangaMode: false })}
                />
                <input
                  className={btnClass(settings.mangaMode)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.rightToLeft") ?? undefined}
                  onClick={() => update({ mangaMode: true })}
                />
              </SettingSection>

              <SettingSection
                title={t("reader.howManyImagesToPreload") ?? ""}
                description={t("reader.theDefaultIsTwoImages") ?? undefined}
              >
                <input
                  id="preload-input"
                  className="stdinput"
                  style={{ width: "4em", height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  type="number"
                  defaultValue={settings.preloadCount}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") update({ preloadCount: Number(e.currentTarget.value) || 2 })
                  }}
                />
                <input
                  className="favtag-btn"
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.apply") ?? undefined}
                  onClick={(e) => {
                    const input = e.currentTarget.previousElementSibling as HTMLInputElement
                    update({ preloadCount: Number(input.value) || 2 })
                  }}
                />
              </SettingSection>

              <SettingSection title={t("reader.header") ?? ""}>
                <input
                  className={btnClass(!settings.hideHeader)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.visible") ?? undefined}
                  onClick={() => update({ hideHeader: false })}
                />
                <input
                  className={btnClass(settings.hideHeader)}
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.hidden") ?? undefined}
                  onClick={() => update({ hideHeader: true })}
                />
              </SettingSection>
            </>
          )}

          <SettingSection
            title={t("reader.showArchiveOverlayByDefault") ?? ""}
            description={t("reader.thisWillShowTheOverlay") ?? undefined}
          >
            <input
              className={btnClass(settings.showOverlayByDefault)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.enabled") ?? undefined}
              onClick={() => update({ showOverlayByDefault: true })}
            />
            <input
              className={btnClass(!settings.showOverlayByDefault)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.disabled") ?? undefined}
              onClick={() => update({ showOverlayByDefault: false })}
            />
          </SettingSection>

          <SettingSection
            title={t("reader.progressionTracking") ?? ""}
            description={t("reader.disablingTrackingWillRestartReading") ?? undefined}
          >
            <input
              className={btnClass(!settings.ignoreProgress)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.enabled") ?? undefined}
              onClick={() => update({ ignoreProgress: false })}
            />
            <input
              className={btnClass(settings.ignoreProgress)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.disabled") ?? undefined}
              onClick={() => update({ ignoreProgress: true })}
            />
          </SettingSection>

          <SettingSection
            title={t("reader.infiniteScrolling") ?? ""}
            description={t("reader.displayAllImagesInA") ?? undefined}
          >
            <input
              className={btnClass(settings.infiniteScroll)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.enabled") ?? undefined}
              onClick={() => update({ infiniteScroll: true })}
            />
            <input
              className={btnClass(!settings.infiniteScroll)}
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.disabled") ?? undefined}
              onClick={() => update({ infiniteScroll: false })}
            />
          </SettingSection>

          <SettingSection
            title={t("reader.autoNextPageIntervalIn") ?? ""}
            description={t("reader.theDefaultIs10Seconds") ?? undefined}
          >
            <input
              id="auto-next-page-input"
              className="stdinput"
              style={{ width: "8em", height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              defaultValue={settings.autoNextPageInterval}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  update({ autoNextPageInterval: Number(e.currentTarget.value) || 10 })
                }
              }}
            />
            <input
              className="favtag-btn"
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.apply") ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget.previousElementSibling as HTMLInputElement
                update({ autoNextPageInterval: Number(input.value) || 10 })
              }}
            />
          </SettingSection>

          {!settings.infiniteScroll && (
            <SettingSection title={t("reader.toggleStamps") ?? ""}>
              <input
                className="fa"
                type="checkbox"
                checked={settings.markersVisible}
                onChange={(e) => update({ markersVisible: e.target.checked })}
              />
            </SettingSection>
          )}
        </div>
      </div>
    </>
  )
}
