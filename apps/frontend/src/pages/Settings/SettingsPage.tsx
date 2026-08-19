import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { ApiError } from "@/api/client"
import { useChangePassword, useLogout, useServerInfo, useSettings, useUpdateSettings } from "@/api/hooks"
import type { Settings as SettingsType } from "@/api/types"
import { CollapsibleSection } from "@/components/Display"
import { LanguageSelector } from "@/components/Form"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { useSectionDeepLink } from "@/hooks/useSectionDeepLink"
import { routes } from "@/lib/routes"
import { DEFAULT_THEME_ID, FONT_SIZE_SM, THEMES, useApplyTheme, useLegacyConfigCss } from "@/theme"
import { toast } from "@/toast"

import { ApiTokensSection } from "./ApiTokensSection"
import { ArchiveFilesSection } from "./ArchiveFilesSection"
import { GlobalSection } from "./GlobalSection"
import { SecuritySection } from "./SecuritySection"
import { TagsThumbnailsSection } from "./TagsThumbnailsSection"
import { WorkersSection } from "./WorkersSection"

export function Settings() {
  const { t } = useTranslation()
  const settings = useSettings()
  useApplyTheme()
  useLegacyConfigCss()

  if (settings.isLoading) {
    return <div className="ido">{t("common.loadingLibrary")}</div>
  }

  if (settings.isError || !settings.data) {
    // See `LibraryPage.tsx`'s own identical guard: a 401 here just means `RequireAuth`
    // (`RouteGuards.tsx`) is already about to navigate to `/login` in reaction to the same
    // invalidated `login-status` query — rendering nothing for that one render avoids flashing a
    // generic "failed to load" message for what's actually a routine session expiry.
    if (settings.error instanceof ApiError && settings.error.status === 401) return null

    return (
      <div className="ido">{t("common.failedToLoadArchivesError", { error: String(settings.error) })}</div>
    )
  }

  return <SettingsForm settings={settings.data} />
}

function SettingsForm({ settings }: { settings: SettingsType }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useDocumentTitle(t("settings.adminSettings") ?? undefined)
  useSectionDeepLink()
  const logout = useLogout()
  const info = useServerInfo()
  const updateSettings = useUpdateSettings()
  const changePassword = useChangePassword()

  const currentTheme = settings.theme ?? DEFAULT_THEME_ID

  const [htmltitle, setHtmltitle] = useState(settings.htmltitle)
  const [motd, setMotd] = useState(settings.motd)
  const [language, setLanguage] = useState(settings.language)
  const [pagesize, setPagesize] = useState(settings.pagesize)
  const [enableresize, setEnableresize] = useState(settings.enableresize)
  const [sizethreshold, setSizethreshold] = useState(settings.sizethreshold)
  const [readerquality, setReaderquality] = useState(settings.readerquality)
  const [localprogress, setLocalprogress] = useState(settings.localprogress)
  const [authprogress, setAuthprogress] = useState(settings.authprogress)
  const [devmode, setDevmode] = useState(settings.devmode)
  const [newbadgemode, setNewbadgemode] = useState(settings.newbadgemode)
  const [recommendprecision, setRecommendprecision] = useState(settings.recommendprecision)
  // Incremented after each save to remount GlobalSection (resets editingKey).
  const [saveTick, setSaveTick] = useState(0)
  // The real key is never sent to the frontend — `llm_api_key_set` is a boolean only.
  // `keyInput` holds whatever the user types when they actively choose to set/change it.
  const [keyInput, setKeyInput] = useState("")

  const [enablepass, setEnablepass] = useState(settings.enablepass)
  const [newPassword, setNewPassword] = useState("")
  const [newPassword2, setNewPassword2] = useState("")
  const [nofunmode, setNofunmode] = useState(settings.nofunmode)
  const [accessTokenLifetimeSecs, setAccessTokenLifetimeSecs] = useState(settings.access_token_lifetime_secs)
  const [refreshTokenLifetimeSecs, setRefreshTokenLifetimeSecs] = useState(settings.refresh_token_lifetime_secs)
  const [enablecors, setEnablecors] = useState(settings.enablecors)

  const [tempmaxsize, setTempmaxsize] = useState(settings.tempmaxsize)
  const [replacedupe, setReplacedupe] = useState(settings.replacedupe)

  const [hqthumbpages, setHqthumbpages] = useState(settings.hqthumbpages)
  const [enablewebp, setEnablewebp] = useState(settings.enablewebp)
  const [webpquality, setWebpquality] = useState(settings.webpquality)
  const [excludednamespaces, setExcludednamespaces] = useState(settings.excludednamespaces)
  const [tagruleson, setTagruleson] = useState(settings.tagruleson)
  const [tagrules, setTagrules] = useState(settings.tagrules)
  const [usedateadded, setUsedateadded] = useState(settings.usedateadded)
  const [usedatemodified, setUsedatemodified] = useState(settings.usedatemodified)
  const [timezone, setTimezone] = useState(settings.timezone)

  const [status, setStatus] = useState("")

  // Dirty tracking for the bottom save bar: any field the Save button submits differing from
  // the server-loaded snapshot means there's something to save. `theme` is deliberately NOT in
  // this list — the theme switcher saves itself immediately on click (see the Theme section).
  const isDirty = useMemo(
    () =>
      htmltitle !== settings.htmltitle ||
      motd !== settings.motd ||
      language !== settings.language ||
      pagesize !== settings.pagesize ||
      enableresize !== settings.enableresize ||
      sizethreshold !== settings.sizethreshold ||
      readerquality !== settings.readerquality ||
      localprogress !== settings.localprogress ||
      authprogress !== settings.authprogress ||
      devmode !== settings.devmode ||
      enablepass !== settings.enablepass ||
      nofunmode !== settings.nofunmode ||
      accessTokenLifetimeSecs !== settings.access_token_lifetime_secs ||
      refreshTokenLifetimeSecs !== settings.refresh_token_lifetime_secs ||
      enablecors !== settings.enablecors ||
      tempmaxsize !== settings.tempmaxsize ||
      replacedupe !== settings.replacedupe ||
      hqthumbpages !== settings.hqthumbpages ||
      enablewebp !== settings.enablewebp ||
      webpquality !== settings.webpquality ||
      excludednamespaces !== settings.excludednamespaces ||
      tagruleson !== settings.tagruleson ||
      tagrules !== settings.tagrules ||
      usedateadded !== settings.usedateadded ||
      usedatemodified !== settings.usedatemodified ||
      timezone !== settings.timezone ||
      newbadgemode !== settings.newbadgemode ||
      recommendprecision !== settings.recommendprecision,
    [
      htmltitle, motd, language, pagesize, enableresize, sizethreshold, readerquality,
      localprogress, authprogress, devmode, enablepass, nofunmode, accessTokenLifetimeSecs,
      refreshTokenLifetimeSecs, enablecors, tempmaxsize,
      replacedupe, hqthumbpages, enablewebp, webpquality, excludednamespaces, tagruleson,
      tagrules, usedateadded, usedatemodified, timezone, newbadgemode, recommendprecision,
      settings,
    ],
  )

  async function handleSave() {
    if (enablepass && newPassword) {
      if (newPassword !== newPassword2) {
        setStatus(t("settings.passwordsDonTMatch") ?? "")
        return
      }
      await changePassword.mutateAsync(newPassword)
      setNewPassword("")
      setNewPassword2("")
    }
    await updateSettings.mutateAsync({
      htmltitle,
      motd,
      language,
      pagesize,
      enableresize,
      sizethreshold,
      readerquality,
      localprogress,
      authprogress,
      devmode,
      enablepass,
      nofunmode,
      access_token_lifetime_secs: accessTokenLifetimeSecs,
      refresh_token_lifetime_secs: refreshTokenLifetimeSecs,
      enablecors,
      tempmaxsize,
      replacedupe,
      hqthumbpages,
      enablewebp,
      webpquality,
      excludednamespaces,
      tagruleson,
      tagrules,
      usedateadded,
      usedatemodified,
      timezone,
      newbadgemode,
      recommendprecision,
      ...(keyInput.trim() && { llm_api_key: keyInput.trim() }),
    })
    setKeyInput("")
    setSaveTick((n) => n + 1)
    toast({ heading: t("settings.settingsSaved") ?? undefined, icon: "success" })
  }

  async function handleLogout() {
    await logout.mutateAsync()
    navigate(routes.login())
  }

  return (
    <div className="ido">
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("settings.adminSettings")}
      </h2>
      <br />

      <div className="left-column">
        <img className="logo-container" src="/legacy/img/logo.png" alt="LANrurugi" />
        <br />
        <h1 style={{ marginBottom: 2 }}>LANrurugi</h1>
        {t("settings.versionVersionVername", {
          version: info.data?.version ?? "",
          vername: info.data?.version_name ?? "",
        })}
        <br />
        <h2>{t("settings.selectACategoryToShow")}</h2>
        <br />
        <input
          id="plugin-config"
          className="stdbtn"
          type="button"
          value={t("common.pluginConfiguration") ?? undefined}
          onClick={() => navigate(routes.pluginSettings())}
        />{" "}
        <input
          id="backup"
          className="stdbtn"
          type="button"
          value={t("settings.databaseBackupRestore") ?? undefined}
          onClick={() => navigate(routes.backup())}
        />{" "}
        <input id="batch" className="stdbtn" type="button" value={t("batch.batchOperations") ?? undefined} onClick={() => navigate(routes.batch())} />
        <br />
        <br />
        <input id="return" className="stdbtn" type="button" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />

        {status && <p style={{ fontSize: FONT_SIZE_SM }}>{status}</p>}

        {/* Not part of legacy's own left-column (legacy has no visible logout affordance — its
            session just expires — and only one site-wide language, set below in Global
            Settings). Kept minimal and visually separate since this SPA needs both. */}
        <hr style={{ margin: "12px 0" }} />
        <LanguageSelector />{" "}
        <input id="logout" className="stdbtn" type="button" value={t("settings.logout") ?? undefined} onClick={() => void handleLogout()} />
      </div>

      <form
        className="right-column"
        onSubmit={(e) => e.preventDefault()}
        // Bottom clearance so the fixed save bar never covers the last section's content.
        style={{ paddingBottom: 72 }}
      >
        <ul className="collapsible extensible with-right-caret">
          <GlobalSection
            key={saveTick}
            htmltitle={htmltitle}
            setHtmltitle={setHtmltitle}
            motd={motd}
            setMotd={setMotd}
            language={language}
            setLanguage={setLanguage}
            pagesize={pagesize}
            setPagesize={setPagesize}
            enableresize={enableresize}
            setEnableresize={setEnableresize}
            sizethreshold={sizethreshold}
            setSizethreshold={setSizethreshold}
            readerquality={readerquality}
            setReaderquality={setReaderquality}
            localprogress={localprogress}
            setLocalprogress={setLocalprogress}
            authprogress={authprogress}
            setAuthprogress={setAuthprogress}
            devmode={devmode}
            setDevmode={setDevmode}
            newbadgemode={newbadgemode}
            setNewbadgemode={setNewbadgemode}
            llmApiKeySet={settings.llm_api_key_set}
            keyInput={keyInput}
            setKeyInput={setKeyInput}
            recommendprecision={recommendprecision}
            setRecommendprecision={setRecommendprecision}
            onStatus={setStatus}
          />

          <CollapsibleSection id="theme" icon="fa-paint-brush" title={t("settings.theme")}>
              <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
                <tbody>
                  <tr>
                    <td></td>
                    <td className="config-td">
                      <br />
                      {t("settings.theSelectedThemeWillApply")}
                      <br />
                      {t("settings.themeColorMetaHint")}
                      <br />
                      <br />
                      {t("settings.clickOnAThemeTo")}
                    </td>
                  </tr>
                  <tr>
                    <td colSpan={2}>
                      {THEMES.map((theme) => (
                        <div key={theme.id} style={{ display: "inline-block" }}>
                          <input
                            type="radio"
                            id={theme.id}
                            className="theme-switch"
                            name="theme"
                            title={theme.name}
                            value={theme.id}
                            checked={currentTheme === theme.id}
                            onChange={() => {
                              document.documentElement.dataset.theme = theme.id
                              updateSettings.mutate({ theme: theme.id })
                            }}
                          />
                          <label htmlFor={theme.id}>
                            <div
                              id={`${theme.id}-div`}
                              className="theme-switch"
                              title={theme.name}
                              style={{ cursor: "pointer" }}
                            >
                              <img title={theme.name} src={`/legacy/img/theme_preview/${theme.id.replace(".css", "")}.png`} className="theme-preview" />
                              <h3>{theme.name}</h3>
                            </div>
                          </label>
                        </div>
                      ))}
                    </td>
                  </tr>
                </tbody>
              </table>
          </CollapsibleSection>

          <SecuritySection
            enablepass={enablepass}
            setEnablepass={setEnablepass}
            newPassword={newPassword}
            setNewPassword={setNewPassword}
            newPassword2={newPassword2}
            setNewPassword2={setNewPassword2}
            nofunmode={nofunmode}
            setNofunmode={setNofunmode}
            accessTokenLifetimeSecs={accessTokenLifetimeSecs}
            setAccessTokenLifetimeSecs={setAccessTokenLifetimeSecs}
            refreshTokenLifetimeSecs={refreshTokenLifetimeSecs}
            setRefreshTokenLifetimeSecs={setRefreshTokenLifetimeSecs}
            enablecors={enablecors}
            setEnablecors={setEnablecors}
          />

          <ApiTokensSection />

          <ArchiveFilesSection
            tempmaxsize={tempmaxsize}
            setTempmaxsize={setTempmaxsize}
            replacedupe={replacedupe}
            setReplacedupe={setReplacedupe}
            onStatus={setStatus}
          />

          <TagsThumbnailsSection
            hqthumbpages={hqthumbpages}
            setHqthumbpages={setHqthumbpages}
            enablewebp={enablewebp}
            setEnablewebp={setEnablewebp}
            webpquality={webpquality}
            setWebpquality={setWebpquality}
            excludednamespaces={excludednamespaces}
            setExcludednamespaces={setExcludednamespaces}
            tagruleson={tagruleson}
            setTagruleson={setTagruleson}
            tagrules={tagrules}
            setTagrules={setTagrules}
            usedateadded={usedateadded}
            setUsedateadded={setUsedateadded}
            usedatemodified={usedatemodified}
            setUsedatemodified={setUsedatemodified}
            timezone={timezone}
            setTimezone={setTimezone}
            onStatus={setStatus}
          />

          <WorkersSection onStatus={setStatus} />
        </ul>
      </form>

      {/* Fixed bottom save bar, shown only while there are unsaved changes — the Save button
          used to sit at the top-left column, far from the sections being edited. The bar's own
          background/border color is theme-specific (`.settings-save-bar` in each theme file,
          reusing that theme's own accent hue per CLAUDE.md's custom-color rule); this container
          is always mounted so the transition isn't a mount/unmount flash, the bar itself is
          hidden when clean. `pointer-events: none` when hidden so an invisible bar can't block
          clicks on content beneath it. */}
      <div className="settings-save-bar" style={isDirty ? undefined : { display: "none" }}>
        <input id="save" className="stdbtn" type="button" value={t("pluginOptions.saveSettings") ?? undefined} onClick={() => void handleSave()} />
      </div>
    </div>
  )
}
