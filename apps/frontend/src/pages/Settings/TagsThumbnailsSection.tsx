import { useTranslation } from "react-i18next"

import { useRegenThumbnails } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, CheckboxRow, Row } from "./shared"

// Full IANA timezone list for the Settings-page `<select>`, grouped by continent — phpBB-style
// (every real zone the runtime knows about, not a hand-curated subset). Built at module load from
// `Intl.supportedValuesOf('timeZone')`, which every modern browser ships with the same bundled
// IANA tzdata Node/Rust `chrono-tz` also use, so the offered ids always line up with what the
// backend's `parse_date_range` actually accepts — no list to keep in sync by hand. `UTC` is
// hoisted into its own top group as the documented default. Anything `Intl.supportedValuesOf`
// doesn't return (older browsers, or a genuinely unusual id a user typed before this UI existed)
// still round-trips through the "Custom…" free-text fallback below.
const TIMEZONE_GROUPS: { label: string; zones: string[] }[] = (() => {
  const supported: string[] = (Intl.supportedValuesOf?.("timeZone") as string[] | undefined) ?? []
  const groups = new Map<string, string[]>()
  for (const tz of supported) {
    const area = tz.includes("/") ? tz.slice(0, tz.indexOf("/")) : "Other"
    const bucket = groups.get(area)
    if (bucket) {
      bucket.push(tz)
    } else {
      groups.set(area, [tz])
    }
  }
  const order = ["Africa", "America", "Antarctica", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"]
  const sortedAreas = [...groups.keys()].sort((a, b) => {
    const ai = order.indexOf(a)
    const bi = order.indexOf(b)
    return (ai === -1 ? order.length : ai) - (bi === -1 ? order.length : bi) || a.localeCompare(b)
  })
  return sortedAreas.map((area) => ({ label: area, zones: (groups.get(area) ?? []).sort() }))
})()

function isKnownTimezone(tz: string): boolean {
  return tz === "UTC" || TIMEZONE_GROUPS.some((g) => g.zones.includes(tz))
}

export function TagsThumbnailsSection({
  hqthumbpages,
  setHqthumbpages,
  enablewebp,
  setEnablewebp,
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
  hqthumbpages: boolean
  setHqthumbpages: (v: boolean) => void
  enablewebp: boolean
  setEnablewebp: (v: boolean) => void
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

  return (
    <CollapsibleSection icon="fa-tags" title={t("settings.tagsAndThumbnails")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <tbody>
          <CheckboxRow id="hqthumbpages" checked={hqthumbpages} onChange={setHqthumbpages} label={t("settings.useHighqualityThumbnailsForPages")}>
            {t("settings.lanraragiGeneratesLowerqualityThumbnailsFor")}
            <br />
            {t("settings.ifThisOptionIsChecked")}
          </CheckboxRow>
          <CheckboxRow id="enablewebp" checked={enablewebp} onChange={setEnablewebp} label={t("settings.useWebpForThumbnails")}>
            {t("settings.ifCheckedThumbnailsAreGenerated")}
            <br />
            <i className="fas fa-exclamation-triangle" style={{ color: "red" }}></i>{" "}
            {t("settings.changingThisRegeneratesEveryThumbnail")}
          </CheckboxRow>
          {enablewebp && (
            <Row label={t("settings.webpQuality")}>
              <input
                className="stdinput"
                type="number"
                min={0}
                max={100}
                style={{ width: "100%" }}
                maxLength={255}
                value={webpquality}
                onChange={(e) => setWebpquality(Number(e.target.value))}
              />
              <br />
              {t("settings.qualityOfGeneratedWebpThumbnails")}
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
            {/* `date_added` display + day-range search resolve in this IANA timezone so
                every viewer agrees on which day an archive belongs to, regardless of
                their own browser timezone (see `lanrurugi_search::engine`'s
                `parse_date_range`). The dropdown lists every IANA zone the runtime knows
                (built from `Intl.supportedValuesOf('timeZone')` at module load — see
                `TIMEZONE_GROUPS`), grouped by continent phpBB-style; a value the runtime
                doesn't recognize (older browser, or an unusual id set before this UI
                existed) falls through to a "Custom…" free-text input. */}
            <select
              className="stdbtn"
              value={isKnownTimezone(timezone) ? timezone : "__custom__"}
              onChange={(e) => {
                if (e.target.value === "__custom__") return
                setTimezone(e.target.value)
              }}
            >
              <option value="UTC">UTC</option>
              {TIMEZONE_GROUPS.map((group) => (
                <optgroup key={group.label} label={group.label}>
                  {group.zones.map((tz) => (
                    <option key={tz} value={tz}>{tz}</option>
                  ))}
                </optgroup>
              ))}
              {!isKnownTimezone(timezone) && (
                <option value="__custom__">{t("settings.custom")} ({timezone})</option>
              )}
            </select>
            {!isKnownTimezone(timezone) && (
              <input
                className="stdinput"
                style={{ width: "100%", marginTop: 4 }}
                value={timezone}
                onChange={(e) => setTimezone(e.target.value)}
                type="text"
                placeholder="e.g. Asia/Tokyo"
              />
            )}
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
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
