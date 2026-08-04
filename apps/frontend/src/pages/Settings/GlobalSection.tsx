import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useCleanDatabase, useDropDatabase } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { SUPPORTED_LANGUAGES } from "@/i18n"
import { routes } from "@/lib/routes"
import { FONT_SIZE_10PT } from "@/theme"

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
  newbadgemode,
  setNewbadgemode,
  llmApiKeySet,
  keyInput,
  setKeyInput,
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
  newbadgemode: string
  setNewbadgemode: (v: string) => void
  llmApiKeySet: boolean
  keyInput: string
  setKeyInput: (v: string) => void
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  // Only true while the user is actively typing a new key — the real key is never
  // rendered into the DOM as an input value (prevents $0.value from leaking it).
  const [editingKey, setEditingKey] = useState(false)
  const cleanDatabase = useCleanDatabase()
  const dropDatabase = useDropDatabase()

  return (
    <CollapsibleSection icon="fa-cubes" title={t("Global Settings")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_10PT }}>
        <tbody>
          <Row label={t("Site Title")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={htmltitle} onChange={(e) => setHtmltitle(e.target.value)} type="text" />
            <br />
            {t("The site title appears on most pages as...their title.")}
          </Row>
          <Row label={t("MOTD")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={motd} onChange={(e) => setMotd(e.target.value)} type="text" />
            <br />
            {t("Slang for Message of the Day. Appears on top of the main Library view.")}
          </Row>
          <Row label={t("Language")}>
            <select className="stdinput" style={{ width: "100%" }} value={language} onChange={(e) => setLanguage(e.target.value)}>
              <option value="auto">{t("Automatic (browser default)")}</option>
              {SUPPORTED_LANGUAGES.map(({ code, nativeName }) => (
                <option key={code} value={code}>
                  {nativeName}
                </option>
              ))}
            </select>
            <br />
            {t("Select the language for the user interface. Set to Automatic to use your browser's language preference.")}
            <br />
            {t("Fully effective after restarting LANraragi.")}
          </Row>
          <Row label={t("Archives per page")}>
            <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={pagesize} onChange={(e) => setPagesize(Number(e.target.value))} type="number" />
            <br />
            {t("Number of archives shown on a page in the main list.")}
          </Row>
          <Row label={t('"New" badge duration')}>
            <select className="stdinput" style={{ width: "100%" }} value={newbadgemode} onChange={(e) => setNewbadgemode(e.target.value)}>
              <option value="until_opened">{t("Until the archive is opened")}</option>
              <option value="until_finished">{t("Until the archive is fully read")}</option>
              <option value="3d">{t("3 days after it was added")}</option>
              <option value="7d">{t("7 days after it was added")}</option>
              <option value="10d">{t("10 days after it was added")}</option>
            </select>
            <br />
            {t('How long an archive keeps its "new" badge after being added to the library.')}
          </Row>
          <Row label={t("DeepSeek API Key")}>
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
                  {t("Change")}
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
                    {t("Cancel")}
                  </button>
                )}
              </div>
            )}
            <br />
            {t("Entering a DeepSeek API key significantly improves the reader's recommendation algorithm (LLM-based reranking). Currently only DeepSeek official API keys are supported.")}
          </Row>
          <CheckboxRow
            id="enableresize"
            checked={enableresize}
            onChange={setEnableresize}
            label={t("Resize Images in Reader")}
          >
            {t("If enabled, pages exceeding a certain size will be resized when viewed to save bandwidth.")}
            <br />
            <i className="fas fa-exclamation-triangle" style={{ color: "red" }}></i>{" "}
            {t("This option can potentially consume a lot of RAM if enabled and used on large images! Use with caution.")}
          </CheckboxRow>
          {enableresize && (
            <>
              <Row label={t("Image Size Threshold")}>
                <input
                  className="stdinput"
                  type="number"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={sizethreshold}
                  onChange={(e) => setSizethreshold(Number(e.target.value))}
                />
                <br />
                {t("(in KBs.) Maximum size an image can reach before being resized.")}
              </Row>
              <Row label={t("Resize Quality")}>
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
                {t("Quality of the resized images. Less quality = Smaller image. (0-100)")}
              </Row>
            </>
          )}
          <CheckboxRow
            id="localprogress"
            checked={localprogress}
            onChange={setLocalprogress}
            label={t("Clientside Progress Tracking")}
          >
            {t("Enabling this option will save reading progression on the browser (through localStorage) instead of the server.")}
            <br />
            {t("Consider toggling this option if you're sharing the LANraragi instance with multiple users!")}
          </CheckboxRow>
          <CheckboxRow
            id="authprogress"
            checked={authprogress}
            onChange={setAuthprogress}
            label={t("Authenticated Progress Tracking")}
          >
            {t("If enabled, server-side progress will only be saved if you're logged in with a password, and will override clientside progress. This allows guests to browse without affecting the main user's progress.")}
            <br />
            {t("Combine with clientside progress tracking to allow unauthenticated users to track progress locally.")}
            <br />
            {t("Fully effective after restarting LANraragi.")}
          </CheckboxRow>
          <ActionRow
            id="clean-db"
            label={t("Clean Database")}
            onClick={async () => {
              const result = await cleanDatabase.mutateAsync()
              onStatus(
                t("{{count}} deleted entries.", { count: result.deleted }) +
                  (result.unlinked > 0
                    ? " " + t("{{count}} unlinked entries — their file went missing.", { count: result.unlinked })
                    : ""),
              )
            }}
          >
            {t("Cleaning the database will remove entries that aren't on your filesystem.")}
          </ActionRow>
          <ActionRow
            id="drop-db"
            label={t("Reset Database")}
            onClick={async () => {
              if (!(await confirmDialog(t("Clicking this button will reset the entire database and delete all settings and metadata.") ?? ""))) return
              await dropDatabase.mutateAsync()
              setTimeout(() => navigate(routes.library()), 1500)
            }}
          >
            <span style={{ color: "red" }}>
              <i className="fas fa-exclamation-triangle"></i> {t("Danger zone!")}
            </span>
            <br />
            {t("Clicking this button will reset the entire database and delete all settings and metadata.")}
          </ActionRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
