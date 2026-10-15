import { useTranslation } from "react-i18next"

import { useRegenThumbnails } from "@/api/hooks"
import { NumberInput, RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { TimezoneSelect } from "@/components/Form"
import { getImageFormatSupport } from "@/lib/utils/imageFormatSupport"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, CheckboxRow, ImageSupportBadge, Row } from "./shared"

export function TagsThumbnailsSection({
  thumbnailFormat,
  setThumbnailFormat,
  webpquality,
  setWebpquality,
  excludednamespaces,
  setExcludednamespaces,
  tagruleson,
  setTagruleson,
  tagrules,
  setTagrules,
  usedateadded,
  setUsedateadded,
  usedatemodified,
  setUsedatemodified,
  timezone,
  setTimezone,
  onStatus,
}: {
  thumbnailFormat: "jxl" | "webp" | "jpeg"
  setThumbnailFormat: (v: "jxl" | "webp" | "jpeg") => void
  webpquality: number
  setWebpquality: (v: number) => void
  excludednamespaces: string
  setExcludednamespaces: (v: string) => void
  tagruleson: boolean
  setTagruleson: (v: boolean) => void
  tagrules: string
  setTagrules: (v: string) => void
  usedateadded: boolean
  setUsedateadded: (v: boolean) => void
  usedatemodified: boolean
  setUsedatemodified: (v: boolean) => void
  timezone: string
  setTimezone: (v: string) => void
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const regenThumbnails = useRegenThumbnails()
  const browserSupport = getImageFormatSupport()
  const effectiveThumbnailFormat =
    thumbnailFormat === "jpeg"
      ? "JPEG"
      : thumbnailFormat === "webp"
        ? browserSupport === "webp"
          ? "WebP"
          : "JPEG"
        : browserSupport === "jxl"
          ? "JXL"
          : browserSupport === "webp"
            ? "WebP"
            : "JPEG"

  return (
    <CollapsibleSection id="tags-thumbnails" icon="fa-tags" title={t("settings.tagsAndThumbnails")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          <Row label={t("settings.thumbnailFormat")}>
            <RadioGroup
              value={thumbnailFormat}
              onValueChange={setThumbnailFormat}
              style={{ display: "flex", flexDirection: "column", gap: 6 }}
            >
              <RadioItem value="jxl">{t("settings.thumbnailFormatJxl")}</RadioItem>
              <RadioItem value="webp">{t("settings.thumbnailFormatWebp")}</RadioItem>
              <RadioItem value="jpeg">{t("settings.thumbnailFormatJpeg")}</RadioItem>
            </RadioGroup>
            <br />
            {t("settings.thumbnailFormatHint")}
            <br />
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span>{t("settings.currentBrowserSupport")}</span>
              <ImageSupportBadge support={browserSupport} />
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span>{t("settings.effectiveThumbnailFormat")}</span>
              <strong
                style={{
                  color:
                    effectiveThumbnailFormat === "JXL"
                      ? "#1a73e8"
                      : effectiveThumbnailFormat === "WebP"
                        ? "#b45309"
                        : "#dc2626",
                }}
              >
                {effectiveThumbnailFormat}
              </strong>
            </div>
            <br />
            <i className="fas fa-exclamation-triangle" style={{ color: "red" }}></i>{" "}
            {t("settings.changingThisRegeneratesEveryThumbnail")}
          </Row>
          {thumbnailFormat !== "jpeg" && (
            <Row label={t("settings.thumbnailQuality")}>
              <NumberInput
                style={{ width: "100%" }}
                min={0}
                max={100}
                value={webpquality}
                onValueChange={setWebpquality}
              />
              <br />
              {t("settings.thumbnailQualityDescription")}
            </Row>
          )}
          <ActionRow
            id="genthumb-button"
            label={t("settings.generateMissingThumbnails")}
            onClick={async () => {
              await regenThumbnails.mutateAsync(false)
              onStatus(t("settings.thumbnailGenerationQueued") ?? "")
            }}
          >
            {t("settings.generateThumbnailsForAllArchives")}
          </ActionRow>
          <ActionRow
            id="forcethumb-button"
            label={t("settings.regenerateAllThumbnails")}
            onClick={async () => {
              await regenThumbnails.mutateAsync(true)
              onStatus(t("settings.thumbnailRegenerationQueued") ?? "")
            }}
          >
            {t("settings.regenerateAllThumbnailsThisMight")}
          </ActionRow>
          <CheckboxRow
            id="usedateadded"
            checked={usedateadded}
            onChange={setUsedateadded}
            label={t("settings.addTimestampTag")}
          >
            {t("settings.ifEnabledLanrurugiWillAdd")}
          </CheckboxRow>
          {usedateadded && (
            <CheckboxRow
              id="usedatemodified"
              checked={usedatemodified}
              onChange={setUsedatemodified}
              label={t("settings.useLastModifiedTime")}
            >
              {t("settings.enablingThisWillUseFile")}
            </CheckboxRow>
          )}
          <Row label={t("settings.timezone")}>
            {/* `date_added` display + day-range search resolve in this IANA timezone so every
                viewer agrees on which day an archive belongs to, regardless of browser timezone. */}
            <TimezoneSelect value={timezone} onChange={setTimezone} ariaLabel={t("settings.timezone")} />
            <br />
            {t("settings.ianaTimezoneIdentifierEG")}
          </Row>
          <Row label={t("settings.excludedNamespaces")}>
            <input
              className="stdinput"
              style={{ width: "100%" }}
              maxLength={255}
              value={excludednamespaces}
              onChange={(e) => setExcludednamespaces(e.target.value)}
              type="text"
            />
            <br />
            {t("settings.commaseparatedListOfTagNamespaces")}
            <br />
            {t("settings.clientsWillUseThisList")}
          </Row>
          <Row label={t("settings.tagRules")}>
            <input id="tagruleson" className="fa" type="checkbox" checked={tagruleson} onChange={(e) => setTagruleson(e.target.checked)} />
            <br />
            <textarea
              className="stdinput"
              style={{ width: "100%", height: 196 }}
              value={tagrules}
              onChange={(e) => setTagrules(e.target.value)}
            />
            <br />
            {t("settings.whenTaggingArchivesUsingPlugins")}
            <br />
            {t("settings.splitRulesWithLinebreaks")}
            <br />
            <span dangerouslySetInnerHTML={{ __html: t("settings.btagTagB") }} />
            <br />
            <span dangerouslySetInnerHTML={{ __html: t("settings.bnamespaceBRemoves") }} />
            <br />
            {t("settings.namespaceStripsTheNamespaceFrom")}
            <br />
            <span dangerouslySetInnerHTML={{ __html: t("settings.tagRuleReplace") }} />
            <br />
            <span
              dangerouslySetInnerHTML={{
                __html: t(
                  "settings.tagRuleHashReplace",
                ),
              }}
            />
            <br />
            <span dangerouslySetInnerHTML={{ __html: t("settings.bNamespaceNewnamespace") }} />
          </Row>
      </div>
    </CollapsibleSection>
  )
}
