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

function SettingSection({
  title,
  description,
  cloudSynced,
  indent,
  children,
}: {
  title: string;
  description?: string;
  /** Marks this section as server-backed (`LRR_CONFIG`) rather than local `ReaderSettings`. */
  cloudSynced?: boolean;
  /** Left-indents the entire section so it visually reads as belonging to the section above. */
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

/** Shared sizing for every plain `<input>`/Apply-button pair below, matching `.favtag-btn`'s
 * real `height: 25px`. */
const CONTROL_HEIGHT = 25;

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
  /** The two server-backed (`LRR_CONFIG`) settings this overlay also surfaces. */
  stampAutoBookmark: boolean;
  stampAutoUnbookmark: boolean;
  onUpdateServerSetting: (partial: Partial<Settings>) => Promise<unknown>;
  /** Stamp/bookmark sections below are write-side features, hidden entirely for a guest. */
  loggedIn: boolean;
}) {
  const { t } = useTranslation();

  useEffect(() => {
    ensureLink(CONFIG_CSS_ID, "/legacy/config.css");
    return () => removeLink(CONFIG_CSS_ID);
  }, []);

  function btnClass(active: boolean) {
    return `favtag-btn${active ? " toggled" : ""}`;
  }

  return (
    <>
      <div
        id="overlay-shade"
        style={{ display: "block", opacity: 0.6 }}
        onClick={onClose}
      />
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

          {settings.fitMode === FIT_MODE.CONTAINER && (
            <SettingSection
              title={t("reader.containerWidthInPixelsOr") ?? ""}
              description={t("reader.theDefaultValueIs1200px") ?? undefined}
            >
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

          <SettingSection title={t("reader.kKeyBehavior") ?? ""}>
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
