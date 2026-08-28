import { useEffect } from "react";
import { useTranslation } from "react-i18next";

import type { Settings } from "@/api/types";
import { RadioGroup, RadioItem } from "@/components/common-ui/Form/Radio";
import { Switch } from "@/components/common-ui/Form/Switch";
import {
  FIT_MODE,
  type FitMode,
  J_SCROLL_UNIT,
  type JScrollUnit,
  K_BEHAVIOR,
  type KBehavior,
  type ReaderSettings,
} from "@/hooks/useReaderSettings";
import { ensureLink, FONT_SIZE_MD, FONT_SIZE_XS, removeLink } from "@/theme";

const CONFIG_CSS_ID = "reader-config-css";

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
  cloudSynced,
  indent,
  children,
}: {
  title: string;
  description?: string;
  /** issue #97: marks this section's control(s) as server-backed (`LRR_CONFIG`, not this
   * component's own `localStorage`-only `ReaderSettings`) — every other section in this overlay
   * is purely local, so a small cloud icon next to the title is the one visual cue distinguishing
   * this section's setting from the rest. */
  cloudSynced?: boolean;
  /** issue #97: left-indents the *entire section* (title included, not just its control) so a
   * sub-option section visually reads as "belongs to the section above" — same `24px` amount
   * `Settings` page's own `CheckboxRow` `indent` prop uses, for the same parent/child pairing.
   * Without indenting the title itself too, only the control row shifted, leaving the two
   * section titles looking like unrelated siblings rather than parent/child (confirmed live,
   * 2026-08-27, against a real screenshot). */
  indent?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <div
      style={{
        textAlign: "left",
        marginBottom: 20,
        ...(indent ? { paddingLeft: 24 } : {}),
      }}
    >
      <h2
        style={{
          margin: "0 0 4px",
          fontSize: FONT_SIZE_MD,
          fontWeight: "bold",
          display: "flex",
          alignItems: "center",
          gap: 6,
        }}
      >
        {title}
        {cloudSynced && (
          <i
            className="fas fa-cloud"
            title={t("reader.savedToServerNotThisDevice") ?? undefined}
            style={{ opacity: 0.6, fontSize: FONT_SIZE_XS }}
            aria-hidden="true"
          />
        )}
      </h2>
      {description && (
        <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.6, marginBottom: 6 }}>
          {description}
        </div>
      )}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          gap: 8,
        }}
      >
        {children}
      </div>
    </div>
  );
}

/** Shared sizing for every plain `<input>`/Apply-button pair below — `.favtag-btn`'s own real
 * theme CSS gives it `height: 25px` with no `!important` (unlike legacy's `.config-btn`, which
 * hard-locks `height: 28px !important` and can't be matched by anything shorter without a fight),
 * so 25px is the number every control here targets instead. */
const CONTROL_HEIGHT = 25;

// Mirrors legacy's `#settingsOverlay` (`[% BLOCK config %]` in
// `~/LANraragi/templates/reader.html.tt2`) — every toggle group from the real template, in the
// same order, but laid out with `SettingSection` above instead of legacy's own float-based
// `.config-panel`/`.config-btn`. Both rules have been deleted outright from `/legacy/config.css`
// (confirmed no other component referenced them) rather than just left unused — this page still
// scopes that file in (same pattern as `Settings.tsx`), though every toggle in this overlay
// (including `Toggle Stamps`) now renders via the shared `common-ui` `Switch` rather than a
// native `<input type=checkbox>`, so nothing here actually reads config.css's own checkbox rules
// anymore as of issue #97.
export function SettingsOverlay({
  settings,
  update,
  onClose,
  stampAutoBookmark,
  stampAutoUnbookmark,
  onUpdateServerSetting,
  loggedIn,
}: {
  settings: ReaderSettings;
  update: (partial: Partial<ReaderSettings>) => void;
  onClose: () => void;
  /** issue #97: the two server-backed (`LRR_CONFIG`) settings this overlay also surfaces —
   * distinct from every other field here, which lives in `ReaderSettings`'s own `localStorage`. */
  stampAutoBookmark: boolean;
  stampAutoUnbookmark: boolean;
  onUpdateServerSetting: (partial: Partial<Settings>) => Promise<unknown>;
  /** 007: the stamp/bookmark sections below are write-side features (stamps are placed by an
   *  admin session; the two sync toggles PUT a server setting) — hidden entirely for a guest. */
  loggedIn: boolean;
}) {
  const { t } = useTranslation();

  useEffect(() => {
    ensureLink(CONFIG_CSS_ID, "/legacy/config.css");
    return () => removeLink(CONFIG_CSS_ID);
  }, []);

  // Legacy marks the active choice in each toggle group by adding a `.toggled` class (reader.js's
  // `initializeSettings`/`toggleFitMode`/etc, e.g. `$("#fit-width").addClass("toggled")`) — a real
  // class already styled (background/border/color) by each theme's own CSS
  // (`~/LANraragi/public/themes/*.css`'s `.toggled` rule), not an ad-hoc inline style. No longer
  // stacked with `config-btn` (see `CONTROL_HEIGHT`'s own docs above).
  function btnClass(active: boolean) {
    return `favtag-btn${active ? " toggled" : ""}`;
  }

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — must be explicitly shown, same as
          `ArchiveOverviewOverlay`'s copy of this element. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div
        id="overlay-shade"
        style={{ display: "block", opacity: 0.6 }}
        onClick={onClose}
      />
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
        <div
          style={{
            width: "100%",
            boxSizing: "border-box",
            overflowY: "auto",
            padding: "0 16px",
          }}
        >
          <h2 style={{ textAlign: "center" }}>{t("reader.readerOptions")}</h2>
          <h1
            style={{
              textAlign: "center",
              fontSize: FONT_SIZE_MD,
              marginBottom: 16,
            }}
          >
            {t("reader.thoseOptionsSaveAutomatically")}
          </h1>

          <SettingSection title={t("reader.fitDisplayTo") ?? ""}>
            {(
              [
                [FIT_MODE.CONTAINER, t("reader.container")],
                [FIT_MODE.FIT_WIDTH, t("reader.width")],
                [FIT_MODE.FIT_HEIGHT, t("reader.height")],
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
          {settings.fitMode === FIT_MODE.CONTAINER && (
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
                style={{
                  width: "8em",
                  height: CONTROL_HEIGHT,
                  boxSizing: "border-box",
                }}
                placeholder="1200px"
                defaultValue={settings.containerWidth}
                onKeyDown={(e) => {
                  if (e.key === "Enter")
                    update({ containerWidth: e.currentTarget.value });
                }}
              />
              <input
                className="favtag-btn"
                type="button"
                style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                value={t("reader.apply") ?? undefined}
                onClick={(e) => {
                  const input = e.currentTarget
                    .previousElementSibling as HTMLInputElement;
                  update({ containerWidth: input.value });
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
                  style={{
                    width: "4em",
                    height: CONTROL_HEIGHT,
                    boxSizing: "border-box",
                  }}
                  type="number"
                  defaultValue={settings.preloadCount}
                  onKeyDown={(e) => {
                    if (e.key === "Enter")
                      update({
                        preloadCount: Number(e.currentTarget.value) || 2,
                      });
                  }}
                />
                <input
                  className="favtag-btn"
                  type="button"
                  style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
                  value={t("reader.apply") ?? undefined}
                  onClick={(e) => {
                    const input = e.currentTarget
                      .previousElementSibling as HTMLInputElement;
                    update({ preloadCount: Number(input.value) || 2 });
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
            description={
              t("reader.disablingTrackingWillRestartReading") ?? undefined
            }
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
              style={{
                width: "8em",
                height: CONTROL_HEIGHT,
                boxSizing: "border-box",
              }}
              defaultValue={settings.autoNextPageInterval}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  update({
                    autoNextPageInterval: Number(e.currentTarget.value) || 10,
                  });
                }
              }}
            />
            <input
              className="favtag-btn"
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.apply") ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget
                  .previousElementSibling as HTMLInputElement;
                update({ autoNextPageInterval: Number(input.value) || 10 });
              }}
            />
          </SettingSection>

          <SettingSection
            title={t("reader.jScrollDistance") ?? ""}
            description={t("reader.howFarJScrollsEachPress") ?? undefined}
          >
            {/* Unit switch applies immediately (a `<select>` is a discrete choice, not something
                that benefits from an Apply round-trip) — the amount field next to it keeps its
                own separate Apply button, same pattern as every other numeric setting on this
                page (`autoNextPageInterval`, `preloadCount`), so a half-typed number doesn't take
                effect on every keystroke. Switching units doesn't convert the existing number
                (e.g. `80` stays `80` whether that now means "80%" or "80px") — the two units mean
                different things and there's no single correct conversion to guess at; the user
                types whatever the new unit calls for and hits Apply. */}
            <select
              className="favtag-btn"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={settings.jScrollUnit}
              onChange={(e) =>
                update({ jScrollUnit: e.target.value as JScrollUnit })
              }
            >
              <option value={J_SCROLL_UNIT.PERCENT}>
                {t("reader.percentOfViewportHeight")}
              </option>
              <option value={J_SCROLL_UNIT.PX}>{t("reader.pixels")}</option>
            </select>
            <input
              id="j-scroll-amount-input"
              className="stdinput"
              type="number"
              style={{
                width: "6em",
                height: CONTROL_HEIGHT,
                boxSizing: "border-box",
              }}
              defaultValue={settings.jScrollAmount}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  update({
                    jScrollAmount:
                      Number(e.currentTarget.value) || settings.jScrollAmount,
                  });
                }
              }}
            />
            <input
              className="favtag-btn"
              type="button"
              style={{ height: CONTROL_HEIGHT, boxSizing: "border-box" }}
              value={t("reader.apply") ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget
                  .previousElementSibling as HTMLInputElement;
                update({
                  jScrollAmount: Number(input.value) || settings.jScrollAmount,
                });
              }}
            />
          </SettingSection>

          {/* Applies to `k` in both infinite-scroll mode (`goToInfiniteScrollPage`'s own
              `scrollIntoView` target) and ordinary paged mode (`goTo`'s own `window.scrollTo` —
              see that function's own docs for the non-scroll-mode "bottom" interpretation:
              scrolls to the newly-turned-to page's own full document height). Not conditional on
              `infiniteScroll` — both modes have a real "where did k land me" answer this setting
              configures, confirmed live 2026-08-28 after this section was found hidden while
              testing it in ordinary mode. */}
          <SettingSection title={t("reader.kKeyBehavior") ?? ""}>
            {/* `fontSize: FONT_SIZE_MD` — this overlay's `<body>`-inherited default (~10.7px, this
                page's real legacy base font size) read visibly smaller/blurrier next to every
                other control's own text here (`.favtag-btn`/section titles both real-render at
                ~13.3px) — confirmed live via `getComputedStyle`, 2026-08-28. `RadioItem` itself
                intentionally has no opinion on font size (matching `Checkbox`/`Switch`'s own
                context-inherits-from-caller design), so this is set at the call site, not baked
                into the shared component. */}
            <RadioGroup<KBehavior>
              value={settings.kBehavior}
              onValueChange={(v) => update({ kBehavior: v })}
              style={{
                display: "flex",
                flexDirection: "column",
                gap: 6,
                fontSize: FONT_SIZE_MD,
              }}
            >
              {/* `dangerouslySetInnerHTML`, not plain `{t(...)}` — these two translation strings
                  embed a real `<b>` around "top"/"bottom" (same pattern as
                  `TagsThumbnailsSection.tsx`'s own `<span dangerouslySetInnerHTML={{ __html:
                  t("settings.btagTagB") }} />`), so the option a reader is actually choosing
                  between reads distinctly at a glance rather than blending into the rest of each
                  option's own explanatory text — requested live, 2026-08-28. Safe here the same
                  way it's safe there: the string is a fixed translation-file literal, never
                  interpolated with anything user-supplied. */}
              <RadioItem value={K_BEHAVIOR.BACK_TOP}>
                <span
                  dangerouslySetInnerHTML={{ __html: t("reader.kBehaviorTop") }}
                />
              </RadioItem>
              <RadioItem value={K_BEHAVIOR.BACK_BOTTOM}>
                <span
                  dangerouslySetInnerHTML={{
                    __html: t("reader.kBehaviorBottom"),
                  }}
                />
              </RadioItem>
              <RadioItem value={K_BEHAVIOR.BACK}>
                {t("reader.kBehaviorDefault")}
              </RadioItem>
            </RadioGroup>
          </SettingSection>

          {loggedIn && !settings.infiniteScroll && (
            <SettingSection title={t("reader.toggleStamps") ?? ""}>
              <Switch
                checked={settings.markersVisible}
                onCheckedChange={(v) => update({ markersVisible: v })}
              />
            </SettingSection>
          )}

          {/* issue #97: the only server-backed settings in this otherwise all-local overlay —
              `cloudSynced` marks that distinction visually. Sub-option stays visible (not
              conditionally rendered) but disabled while the main switch is off, matching the
              Settings page's own identical parent/child treatment (`GlobalSection.tsx`). */}
          {loggedIn && (
            <>
              <SettingSection
                title={t("reader.autoBookmarkOnStamp") ?? ""}
                cloudSynced
              >
                <Switch
                  checked={stampAutoBookmark}
                  onCheckedChange={(v) =>
                    void onUpdateServerSetting({ stampautobookmark: v })
                  }
                />
              </SettingSection>
              <SettingSection
                title={t("settings.autoUnbookmarkOnLastStampRemoved") ?? ""}
                cloudSynced
                indent
              >
                <Switch
                  checked={stampAutoUnbookmark}
                  disabled={!stampAutoBookmark}
                  onCheckedChange={(v) =>
                    void onUpdateServerSetting({ stampautounbookmark: v })
                  }
                />
              </SettingSection>
            </>
          )}
        </div>
      </div>
    </>
  );
}
