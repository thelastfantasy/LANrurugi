import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useCleanDatabase, useDropDatabase } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { SUPPORTED_LANGUAGES } from "@/i18n"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, CheckboxRow, Row } from "./shared"

export function GlobalSection({
  htmltitle,
  setHtmltitle,
  motd,
  setMotd,
  language,
  setLanguage,
  pagesize,
  setPagesize,
  enableresize,
  setEnableresize,
  sizethreshold,
  setSizethreshold,
  readerquality,
  setReaderquality,
  localprogress,
  setLocalprogress,
  authprogress,
  setAuthprogress,
  stampautobookmark,
  setStampautobookmark,
  stampautounbookmark,
  setStampautounbookmark,
  guestmode,
  setGuestmode,
  newbadgemode,
  setNewbadgemode,
  llmApiKeySet,
  keyInput,
  setKeyInput,
  recommendprecision,
  setRecommendprecision,
  onStatus,
}: {
  htmltitle: string
  setHtmltitle: (v: string) => void
  motd: string
  setMotd: (v: string) => void
  language: string
  setLanguage: (v: string) => void
  pagesize: number
  setPagesize: (v: number) => void
  enableresize: boolean
  setEnableresize: (v: boolean) => void
  sizethreshold: number
  setSizethreshold: (v: number) => void
  readerquality: number
  setReaderquality: (v: number) => void
  localprogress: boolean
  setLocalprogress: (v: boolean) => void
  authprogress: boolean
  setAuthprogress: (v: boolean) => void
  stampautobookmark: boolean
  setStampautobookmark: (v: boolean) => void
  stampautounbookmark: boolean
  setStampautounbookmark: (v: boolean) => void
  guestmode: boolean
  setGuestmode: (v: boolean) => void
  newbadgemode: string
  setNewbadgemode: (v: string) => void
  llmApiKeySet: boolean
  keyInput: string
  setKeyInput: (v: string) => void
  recommendprecision: string
  setRecommendprecision: (v: string) => void
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [editingKey, setEditingKey] = useState(false)
  const cleanDatabase = useCleanDatabase()
  const dropDatabase = useDropDatabase()

  return (
    <CollapsibleSection id="global" icon="fa-cubes" title={t("settings.globalSettings")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <tbody>
          <Row label={t("settings.siteTitle")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={htmltitle} onChange={(e) => setHtmltitle(e.target.value)} type="text" />
            <br />
            {t("settings.theSiteTitleAppearsOn")}
          </Row>
          <Row label={t("settings.motd")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={motd} onChange={(e) => setMotd(e.target.value)} type="text" />
            <br />
            {t("settings.slangForMessageOfThe")}
          </Row>
          <Row label={t("settings.language")}>
            <select className="stdinput" style={{ width: "100%" }} value={language} onChange={(e) => setLanguage(e.target.value)}>
              <option value="auto">{t("settings.automaticBrowserDefault")}</option>
              {SUPPORTED_LANGUAGES.map(({ code, nativeName }) => (
                <option key={code} value={code}>
                  {nativeName}
                </option>
              ))}
            </select>
            <br />
            {t("settings.selectTheLanguageForThe")}
          </Row>
          <Row label={t("settings.archivesPerPage")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={pagesize} onChange={(e) => setPagesize(Number(e.target.value))} type="number" />
            <br />
            {t("settings.numberOfArchivesShownOn")}
          </Row>
          <Row label={t("settings.newBadgeDuration")}>
            <select className="stdinput" style={{ width: "100%" }} value={newbadgemode} onChange={(e) => setNewbadgemode(e.target.value)}>
              <option value="until_opened">{t("settings.untilTheArchiveIsOpened")}</option>
              <option value="until_finished">{t("settings.untilTheArchiveIsFully")}</option>
              <option value="3d">{t("settings.3DaysAfterItWas")}</option>
              <option value="7d">{t("settings.7DaysAfterItWas")}</option>
              <option value="10d">{t("settings.10DaysAfterItWas")}</option>
            </select>
            <br />
            {t("settings.howLongAnArchiveKeeps")}
          </Row>
          <Row label={t("settings.deepseekApiKey")}>
            {llmApiKeySet && !editingKey ? (
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span style={{ fontFamily: "monospace", opacity: 0.7 }}>
                  sk-••••••••••••••••
                </span>
                <button
                  type="button"
                  className="stdbtn"
                  onClick={() => {
                    setKeyInput("")
                    setEditingKey(true)
                  }}
                >
                  {t("settings.change")}
                </button>
              </div>
            ) : (
              <div style={{ display: "flex", gap: 4 }}>
                <input
                  className="stdinput"
                  style={{ flex: 1 }}
                  type="password"
                  value={keyInput}
                  onChange={(e) => setKeyInput(e.target.value)}
                  placeholder="sk-..."
                />
                {editingKey && (
                  <button
                    type="button"
                    className="stdbtn"
                    onClick={() => {
                      setKeyInput("")
                      setEditingKey(false)
                    }}
                  >
                    {t("common.cancel")}
                  </button>
                )}
              </div>
            )}
            <br />
            {t("settings.enteringADeepseekApiKey")}
          </Row>
          <Row label={t("settings.recommendationPrecision")}>
            <select
              className="stdinput"
              style={{ width: "100%" }}
              value={recommendprecision}
              onChange={(e) => setRecommendprecision(e.target.value)}
            >
              <option value="low">{t("settings.lowFastestLeastCpuStorage")}</option>
              <option value="medium">{t("settings.mediumRecommended")}</option>
              <option value="high">{t("settings.highMostAccurateMoreCpu")}</option>
            </select>
            <br />
            {t("settings.controlsHowManySimilarArchives")}
            <br />
            {t("settings.changingThisTriggersABackground")}
          </Row>
          <CheckboxRow
            id="enableresize"
            checked={enableresize}
            onChange={setEnableresize}
            label={t("settings.resizeImagesInReader")}
          >
            {t("settings.pagesOverTheSizeThreshold")}
          </CheckboxRow>
          {enableresize && (
            <>
              <Row label={t("settings.imageSizeThreshold")}>
                <input
                  className="stdinput"
                  type="number"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={sizethreshold}
                  onChange={(e) => setSizethreshold(Number(e.target.value))}
                />
                <br />
                {t("settings.inKbsMaximumRawSize")}
              </Row>
              <Row label={t("settings.resizeQuality")}>
                <input
                  className="stdinput"
                  type="number"
                  min={0}
                  max={100}
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={readerquality}
                  onChange={(e) => setReaderquality(Number(e.target.value))}
                />
                <br />
                {t("settings.webpQualityOfTheReencoded")}
              </Row>
            </>
          )}
          <CheckboxRow
            id="localprogress"
            checked={localprogress}
            onChange={setLocalprogress}
            label={t("settings.clientsideProgressTracking")}
          >
            {t("settings.enablingThisOptionWillSave")}
            <br />
            {t("settings.considerTogglingThisOptionIf")}
          </CheckboxRow>
          <CheckboxRow
            id="authprogress"
            checked={authprogress}
            onChange={setAuthprogress}
            label={t("settings.authenticatedProgressTracking")}
          >
            {t("settings.ifEnabledServersideProgressWill")}
            <br />
            {t("settings.combineWithClientsideProgressTracking")}
            <br />
            {t("settings.fullyEffectiveAfterRestartingLanraragi")}
          </CheckboxRow>
          <CheckboxRow
            id="stampautobookmark"
            checked={stampautobookmark}
            onChange={setStampautobookmark}
            label={t("settings.autoBookmarkOnStamp")}
          >
            {t("settings.autoBookmarkOnStampDescription")}
          </CheckboxRow>
          <CheckboxRow
            id="stampautounbookmark"
            checked={stampautounbookmark}
            onChange={setStampautounbookmark}
            disabled={!stampautobookmark}
            indent
            label={t("settings.autoUnbookmarkOnLastStampRemoved")}
          >
            {t("settings.autoUnbookmarkOnLastStampRemovedDescription")}
          </CheckboxRow>
          {/* Site-wide guest-mode switch (FR-003) — grants nothing alone; also needs a category
              marked visible to guests (Categories.tsx) to actually route guests in. */}
          <CheckboxRow
            id="guestmode"
            checked={guestmode}
            onChange={setGuestmode}
            label={t("settings.guestMode")}
          >
            {t("settings.evenWithThePasswordProtection")}
          </CheckboxRow>
          <ActionRow
            id="clean-db"
            label={t("settings.cleanDatabase")}
            onClick={async () => {
              const result = await cleanDatabase.mutateAsync()
              onStatus(
                t("settings.deletedEntries", { count: result.deleted }) +
                  (result.unlinked > 0
                    ? " " + t("settings.unlinkedEntriesTheirFile", { count: result.unlinked })
                    : ""),
              )
            }}
          >
            {t("settings.cleaningTheDatabaseWillRemove")}
          </ActionRow>
          <ActionRow
            id="drop-db"
            label={t("settings.resetDatabase")}
            onClick={async () => {
              if (!(await confirmDialog(t("settings.clickingThisButtonWillReset") ?? "", true))) return
              await dropDatabase.mutateAsync()
              setTimeout(() => navigate(routes.library()), 1500)
            }}
          >
            <span style={{ color: "red" }}>
              <i className="fas fa-exclamation-triangle"></i> {t("settings.dangerZone")}
            </span>
            <br />
            {t("settings.clickingThisButtonWillReset")}
          </ActionRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
