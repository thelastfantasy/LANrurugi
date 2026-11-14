import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import {
  useBlake3Backfill,
  useCleanDatabase,
  useDropDatabase,
  useStopBlake3Backfill,
  useTidyTags,
} from "@/api/hooks"
import { NumberInput } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { routes } from "@/lib/routes"
import { getImageFormatSupport } from "@/lib/utils/imageFormatSupport"
import { FONT_SIZE_SM } from "@/theme"

import { LanguageOrderEditor } from "./LanguageOrderEditor"
import { ActionRow, CheckboxRow, ImageSupportBadge, Row } from "./shared"

export function GlobalSection({
  htmltitle,
  setHtmltitle,
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
  subfoldersToTankoubons,
  setSubfoldersToTankoubons,
  archiveSplitSuggestionsEnabled,
  setArchiveSplitSuggestionsEnabled,
  archiveSplitDeleteOriginalEnabled,
  setArchiveSplitDeleteOriginalEnabled,
  ingestStrictVerify,
  setIngestStrictVerify,
  guestmode,
  setGuestmode,
  newbadgemode,
  setNewbadgemode,
  llmApiKeySet,
  keyInput,
  setKeyInput,
  recommendprecision,
  setRecommendprecision,
  pluginBindInterface,
  egressBinding,
  pluginBindingScope,
  setPluginBindInterface,
  directDownloadUrl,
  onDirectDownloadUrlChange,
  interfaces,
  defaultInterface,
  onStatus,
}: {
  htmltitle: string
  setHtmltitle: (v: string) => void
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
  subfoldersToTankoubons: boolean
  setSubfoldersToTankoubons: (v: boolean) => void
  archiveSplitSuggestionsEnabled: boolean
  setArchiveSplitSuggestionsEnabled: (v: boolean) => void
  archiveSplitDeleteOriginalEnabled: boolean
  setArchiveSplitDeleteOriginalEnabled: (v: boolean) => void
  ingestStrictVerify: boolean
  setIngestStrictVerify: (v: boolean) => void
  guestmode: boolean
  setGuestmode: (v: boolean) => void
  newbadgemode: string
  setNewbadgemode: (v: string) => void
  llmApiKeySet: boolean
  keyInput: string
  setKeyInput: (v: string) => void
  recommendprecision: string
  setRecommendprecision: (v: string) => void
  pluginBindInterface: string
  egressBinding?: { state: "unpinned" | "bound" | "failed"; interface: string | null }
  /** What this platform can actually pin; the hint text and the picker follow it. */
  pluginBindingScope?: "full" | "rust-only" | "none"
  setPluginBindInterface: (v: string) => void
  directDownloadUrl: string
  onDirectDownloadUrlChange: (v: string) => void
  interfaces: string[]
  /** The interface the kernel routes internet traffic through right now, when it can be told. */
  defaultInterface: string | null
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const browserSupport = getImageFormatSupport()
  const [editingKey, setEditingKey] = useState(false)
  // Default route first, then the rest in name order: the list comes from readdir, which has no
  // meaning to a user looking for "the one I am already on".
  const orderedInterfaces = useMemo(() => {
    const rest = interfaces.filter((name) => name !== defaultInterface).sort()
    return defaultInterface && interfaces.includes(defaultInterface)
      ? [defaultInterface, ...rest]
      : rest
  }, [interfaces, defaultInterface])
  const cleanDatabase = useCleanDatabase()
  const tidyTags = useTidyTags()
  const blake3Backfill = useBlake3Backfill()
  const stopBlake3Backfill = useStopBlake3Backfill()
  const dropDatabase = useDropDatabase()

  return (
    <CollapsibleSection id="global" icon="fa-cubes" title={t("settings.globalSettings")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          <Row label={t("settings.siteTitle")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={htmltitle} onChange={(e) => setHtmltitle(e.target.value)} type="text" />
            <br />
            {t("settings.theSiteTitleAppearsOn")}
          </Row>
          <Row label={t("settings.language")}>
            <LanguageOrderEditor value={language} onChange={setLanguage} />
            <br />
            {t("settings.selectTheLanguageForThe")}
          </Row>
          <Row label={t("settings.archivesPerPage")}>
            <NumberInput style={{ width: "100%" }} value={pagesize} onValueChange={setPagesize} />
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
          <Row label={t("settings.pluginInterface")}>
            {pluginBindingScope === "none" ? (
              t("settings.pluginInterfaceUnsupported")
            ) : (
              <>
            {/* The kernel's own default route leads the list and is labelled both on the
                "unbound" entry (what the user is actually getting right now) and on that
                interface's own entry (what pinning it would reproduce). */}
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <select
                className="stdinput"
                style={{ flex: "1 1 auto", minWidth: 0 }}
                value={pluginBindInterface}
                onChange={(e) => setPluginBindInterface(e.target.value)}
              >
                <option value="">
                  {t("settings.pluginInterfaceDefault")}
                  {defaultInterface
                    ? ` · ${t("settings.pluginInterfaceFollowing", { iface: defaultInterface })}`
                    : ""}
                </option>
                {orderedInterfaces.map((name) => (
                  <option key={name} value={name}>
                    {name}
                    {name === defaultInterface
                      ? ` · ${t("settings.pluginInterfaceSystemRoute")}`
                      : ""}
                  </option>
                ))}
              </select>
              {pluginBindInterface ? (
                <button
                  type="button"
                  className="stdbtn"
                  style={{ minWidth: "auto", whiteSpace: "nowrap" }}
                  onClick={() => setPluginBindInterface("")}
                >
                  {t("settings.pluginInterfaceReset")}
                </button>
              ) : null}
            </div>
            <br />
            {pluginBindingScope === "rust-only"
              ? t("settings.pluginInterfaceHintPartial")
              : t("settings.pluginInterfaceHint")}
            {egressBinding?.state === "failed" ? (
              <>
                <br />
                <span className="diag-field-error">
                  {t("settings.egressBindingFailed", {
                    iface: egressBinding.interface ?? "",
                  })}
                </span>
              </>
            ) : null}
              </>
            )}
          </Row>
          <Row label={t("settings.directDownloadUrl")}>
            <input
              className="stdinput"
              style={{ width: "100%" }}
              value={directDownloadUrl}
              onChange={(e) => onDirectDownloadUrlChange(e.target.value)}
              placeholder="https://dl.example.org"
              spellCheck={false}
            />
            <br />
            {t("settings.directDownloadUrlHint")}
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
                <NumberInput style={{ width: "100%" }} value={sizethreshold} onValueChange={setSizethreshold} />
                <br />
                {t("settings.inKbsMaximumRawSize")}
              </Row>
              <Row label={t("settings.resizeQuality")}>
                <NumberInput
                  style={{ width: "100%" }}
                  min={0}
                  max={100}
                  value={readerquality}
                  onValueChange={setReaderquality}
                />
                <br />
                {t("settings.webpQualityOfTheReencoded")}
              </Row>
            </>
          )}
          <Row label={t("settings.imageFormatSupport")}>
            <ImageSupportBadge support={browserSupport} />
          </Row>
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
          <CheckboxRow
            id="subfolders_to_tankoubons"
            checked={subfoldersToTankoubons}
            onChange={setSubfoldersToTankoubons}
            label={t("settings.subfoldersToTankoubons")}
          >
            {t("settings.subfoldersToTankoubonsDescription")}
            <br />
            {t("settings.subfoldersToTankoubonsLlmHint")}
          </CheckboxRow>
          <CheckboxRow
            id="archive_split_suggestions_enabled"
            checked={archiveSplitSuggestionsEnabled}
            onChange={setArchiveSplitSuggestionsEnabled}
            disabled={!llmApiKeySet}
            label={t("settings.archiveSplitSuggestions")}
          >
            {t("settings.archiveSplitSuggestionsDescription")}
            <br />
            {t("settings.archiveSplitSuggestionsLlmHint")}
          </CheckboxRow>
          <CheckboxRow
            id="archive_split_delete_original_enabled"
            checked={archiveSplitDeleteOriginalEnabled}
            onChange={setArchiveSplitDeleteOriginalEnabled}
            disabled={!archiveSplitSuggestionsEnabled || !llmApiKeySet}
            label={t("settings.archiveSplitDeleteOriginal")}
          >
            {t("settings.archiveSplitDeleteOriginalDescription")}
          </CheckboxRow>
          <CheckboxRow
            id="ingest_strict_verify"
            checked={ingestStrictVerify}
            onChange={setIngestStrictVerify}
            label={t("settings.ingestStrictVerify")}
          >
            {t("settings.ingestStrictVerifyDescription")}
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
            id="tidy-tags"
            label={t("settings.tidyTags")}
            onClick={async () => {
              const confirmed = await confirmDialog(
                <div>
                  <div>{t("settings.tidyTagsConfirmIntro")}</div>
                  <ul
                    style={{
                      margin: "4px auto 0",
                      padding: 0,
                      listStyle: "none",
                      display: "inline-block",
                      textAlign: "left",
                    }}
                  >
                    {[
                      "settings.tidyTagsRuleDateAdded",
                      "settings.tidyTagsRuleDuplicates",
                      "settings.tidyTagsRuleWhitespace",
                      "settings.tidyTagsRuleNamespace",
                      "settings.tidyTagsRuleEmptyValue",
                    ].map((key) => (
                      <li key={key} style={{ padding: "2px 0" }}>
                        • {t(key)}
                      </li>
                    ))}
                  </ul>
                  <div style={{ marginTop: 8 }}>{t("settings.tidyTagsConfirmNote")}</div>
                </div>,
              )
              if (!confirmed) return
              const result = await tidyTags.mutateAsync()
              onStatus(
                t("settings.tidyTagsResult", {
                  scanned: result.scanned,
                  repaired: result.repaired,
                }),
              )
            }}
          >
            {t("settings.tidyTagsDescription")}
          </ActionRow>
          <ActionRow
            id="blake3-backfill"
            label={t("settings.blake3Backfill")}
            onClick={async () => {
              const result = await blake3Backfill.mutateAsync()
              onStatus(
                t(result.already_running ? "settings.blake3BackfillRunning" : "settings.blake3BackfillStarted"),
              )
            }}
          >
            {t("settings.blake3BackfillDescription")}{" "}
            <button
              type="button"
              className="stdbtn"
              onClick={async () => {
                const result = await stopBlake3Backfill.mutateAsync()
                onStatus(t(result.stopped ? "settings.blake3BackfillStopped" : "settings.blake3BackfillIdle"))
              }}
            >
              {t("settings.blake3BackfillStop")}
            </button>
          </ActionRow>
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
      </div>
    </CollapsibleSection>
  )
}
