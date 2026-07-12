import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import {
  useChangePassword,
  useCleanDatabase,
  useCleanTempfolder,
  useClearNewFlags,
  useDiscardSearchCache,
  useDropDatabase,
  useLogout,
  useRegenThumbnails,
  useServerInfo,
  useSettings,
  useShinobuAction,
  useShinobuStatus,
  useUpdateSettings,
} from '../../api/hooks'
import type { Settings as SettingsType } from '../../api/types'
import CollapsibleSection from '../../components/CollapsibleSection'
import LanguageSelector from '../../components/LanguageSelector'
import { SUPPORTED_LANGUAGES } from '../../i18n'
import { DEFAULT_THEME_ID, ensureLink, removeLink, THEMES, useApplyTheme } from '../../theme'
import { useDocumentTitle } from '../../useDocumentTitle'

const CONFIG_CSS_ID = 'legacy-config-css'

export default function Settings() {
  const { t } = useTranslation()
  const settings = useSettings()
  useApplyTheme()

  // `config.css` (real vendor file, `~/LANraragi/public/css/config.css`) is only ever linked by
  // legacy's own `config.html.tt2` — it globally restyles every `input[type=checkbox]` on the
  // page into the ON/OFF toggle look. Legacy gets this "for free" since navigating away is a full
  // page load; an SPA has to link/unlink it by hand so those rules don't leak onto other routes.
  useEffect(() => {
    ensureLink(CONFIG_CSS_ID, '/legacy/config.css')
    return () => removeLink(CONFIG_CSS_ID)
  }, [])

  if (settings.isLoading) {
    return <div className="ido">{t('Loading library…')}</div>
  }

  if (settings.isError || !settings.data) {
    return (
      <div className="ido">{t('Failed to load archives: {{error}}', { error: String(settings.error) })}</div>
    )
  }

  return <SettingsForm settings={settings.data} />
}

function SettingsForm({ settings }: { settings: SettingsType }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useDocumentTitle(t('Admin Settings') ?? undefined)
  const logout = useLogout()
  const info = useServerInfo()
  const updateSettings = useUpdateSettings()
  const changePassword = useChangePassword()
  const shinobuStatus = useShinobuStatus()
  const shinobuAction = useShinobuAction()
  const clearNewFlags = useClearNewFlags()
  const regenThumbnails = useRegenThumbnails()
  const resetSearchCache = useDiscardSearchCache()
  const cleanTempfolder = useCleanTempfolder()
  const cleanDatabase = useCleanDatabase()
  const dropDatabase = useDropDatabase()

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

  const [enablepass, setEnablepass] = useState(settings.enablepass)
  const [newPassword, setNewPassword] = useState('')
  const [newPassword2, setNewPassword2] = useState('')
  const [nofunmode, setNofunmode] = useState(settings.nofunmode)
  const [apikey, setApikey] = useState(settings.apikey)
  const [enablecors, setEnablecors] = useState(settings.enablecors)

  const [tempmaxsize, setTempmaxsize] = useState(settings.tempmaxsize)
  const [replacedupe, setReplacedupe] = useState(settings.replacedupe)

  const [hqthumbpages, setHqthumbpages] = useState(settings.hqthumbpages)
  const [enablewebp, setEnablewebp] = useState(settings.enablewebp)
  const [webpquality, setWebpquality] = useState(settings.webpquality)
  const [excludednamespaces, setExcludednamespaces] = useState(settings.excludednamespaces)
  const [tagruleson, setTagruleson] = useState(settings.tagruleson)
  const [tagrules, setTagrules] = useState(settings.tagrules)

  const [status, setStatus] = useState('')

  async function handleSave() {
    if (enablepass && newPassword) {
      if (newPassword !== newPassword2) {
        setStatus(t("Passwords don't match!") ?? '')
        return
      }
      await changePassword.mutateAsync(newPassword)
      setNewPassword('')
      setNewPassword2('')
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
      enablepass,
      nofunmode,
      apikey,
      enablecors,
      tempmaxsize,
      replacedupe,
      hqthumbpages,
      enablewebp,
      webpquality,
      excludednamespaces,
      tagruleson,
      tagrules,
    })
    setStatus(t('Settings saved!') ?? '')
  }

  async function handleLogout() {
    await logout.mutateAsync()
    navigate('/login')
  }

  return (
    <div className="ido">
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Admin Settings')}
      </h2>
      <br />

      <div className="left-column">
        <img className="logo-container" src="/legacy/img/logo.png" alt="LANrurugi" />
        <br />
        <h1 style={{ marginBottom: 2 }}>LANrurugi</h1>
        {t('Version {{version}} {{vername}}', {
          version: info.data?.version ?? '',
          vername: info.data?.version_name ?? '',
        })}
        <br />
        <h2>{t('Select a category to show the matching settings.')}</h2>
        <br />
        <input id="save" className="stdbtn" type="button" value={t('Save Settings') ?? undefined} onClick={() => void handleSave()} />
        <br />
        <input
          id="plugin-config"
          className="stdbtn"
          type="button"
          value={t('Plugin Configuration') ?? undefined}
          onClick={() => navigate('/plugins')}
        />{' '}
        <input
          id="backup"
          className="stdbtn"
          type="button"
          value={t('Database Backup/Restore') ?? undefined}
          onClick={() => navigate('/backup')}
        />{' '}
        <input id="batch" className="stdbtn" type="button" value={t('Batch Operations') ?? undefined} onClick={() => navigate('/batch')} />
        <br />
        <br />
        <input id="return" className="stdbtn" type="button" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />

        {status && <p style={{ fontSize: '9pt' }}>{status}</p>}

        {/* Not part of legacy's own left-column (legacy has no visible logout affordance — its
            session just expires — and only one site-wide language, set below in Global
            Settings). Kept minimal and visually separate since this SPA needs both. */}
        <hr style={{ margin: '12px 0' }} />
        <LanguageSelector />{' '}
        <input id="logout" className="stdbtn" type="button" value={t('Logout') ?? undefined} onClick={() => void handleLogout()} />
      </div>

      <form className="right-column" onSubmit={(e) => e.preventDefault()}>
        <ul className="collapsible extensible with-right-caret">
          <CollapsibleSection icon="fa-cubes" title={t('Global Settings')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <Row label={t('Site Title')}>
                    <input className="stdinput" style={{ width: '100%' }} maxLength={255} value={htmltitle} onChange={(e) => setHtmltitle(e.target.value)} type="text" />
                    <br />
                    {t('The site title appears on most pages as...their title.')}
                  </Row>
                  <Row label={t('MOTD')}>
                    <input className="stdinput" style={{ width: '100%' }} maxLength={255} value={motd} onChange={(e) => setMotd(e.target.value)} type="text" />
                    <br />
                    {t('Slang for Message of the Day. Appears on top of the main Library view.')}
                  </Row>
                  <Row label={t('Language')}>
                    <select className="stdinput" style={{ width: '100%' }} value={language} onChange={(e) => setLanguage(e.target.value)}>
                      <option value="auto">{t('Automatic (browser default)')}</option>
                      {SUPPORTED_LANGUAGES.map(({ code, nativeName }) => (
                        <option key={code} value={code}>
                          {nativeName}
                        </option>
                      ))}
                    </select>
                    <br />
                    {t("Select the language for the user interface. Set to Automatic to use your browser's language preference.")}
                    <br />
                    {t('Fully effective after restarting LANraragi.')}
                  </Row>
                  <Row label={t('Archives per page')}>
                    <input className="stdinput" style={{ width: '100%' }} maxLength={255} value={pagesize} onChange={(e) => setPagesize(Number(e.target.value))} type="number" />
                    <br />
                    {t('Number of archives shown on a page in the main list.')}
                  </Row>
                  <CheckboxRow
                    id="enableresize"
                    checked={enableresize}
                    onChange={setEnableresize}
                    label={t('Resize Images in Reader')}
                  >
                    {t('If enabled, pages exceeding a certain size will be resized when viewed to save bandwidth.')}
                    <br />
                    <i className="fas fa-exclamation-triangle" style={{ color: 'red' }}></i>{' '}
                    {t('This option can potentially consume a lot of RAM if enabled and used on large images! Use with caution.')}
                  </CheckboxRow>
                  {enableresize && (
                    <>
                      <Row label={t('Image Size Threshold')}>
                        <input
                          className="stdinput"
                          type="number"
                          style={{ width: '100%' }}
                          maxLength={255}
                          value={sizethreshold}
                          onChange={(e) => setSizethreshold(Number(e.target.value))}
                        />
                        <br />
                        {t('(in KBs.) Maximum size an image can reach before being resized.')}
                      </Row>
                      <Row label={t('Resize Quality')}>
                        <input
                          className="stdinput"
                          type="number"
                          min={0}
                          max={100}
                          style={{ width: '100%' }}
                          maxLength={255}
                          value={readerquality}
                          onChange={(e) => setReaderquality(Number(e.target.value))}
                        />
                        <br />
                        {t('Quality of the resized images. Less quality = Smaller image. (0-100)')}
                      </Row>
                    </>
                  )}
                  <CheckboxRow
                    id="localprogress"
                    checked={localprogress}
                    onChange={setLocalprogress}
                    label={t('Clientside Progress Tracking')}
                  >
                    {t("Enabling this option will save reading progression on the browser (through localStorage) instead of the server.")}
                    <br />
                    {t("Consider toggling this option if you're sharing the LANraragi instance with multiple users!")}
                  </CheckboxRow>
                  <CheckboxRow
                    id="authprogress"
                    checked={authprogress}
                    onChange={setAuthprogress}
                    label={t('Authenticated Progress Tracking')}
                  >
                    {t("If enabled, server-side progress will only be saved if you're logged in with a password, and will override clientside progress. This allows guests to browse without affecting the main user's progress.")}
                    <br />
                    {t('Combine with clientside progress tracking to allow unauthenticated users to track progress locally.')}
                    <br />
                    {t('Fully effective after restarting LANraragi.')}
                  </CheckboxRow>
                  <ActionRow
                    id="clean-db"
                    label={t('Clean Database')}
                    onClick={async () => {
                      const result = await cleanDatabase.mutateAsync()
                      setStatus(
                        t('{{count}} deleted entries.', { count: result.deleted }) +
                          (result.unlinked > 0
                            ? ' ' + t('{{count}} unlinked entries — their file went missing.', { count: result.unlinked })
                            : ''),
                      )
                    }}
                  >
                    {t("Cleaning the database will remove entries that aren't on your filesystem.")}
                  </ActionRow>
                  <ActionRow
                    id="drop-db"
                    label={t('Reset Database')}
                    onClick={async () => {
                      if (!window.confirm(t('Clicking this button will reset the entire database and delete all settings and metadata.') ?? '')) return
                      await dropDatabase.mutateAsync()
                      setTimeout(() => navigate('/'), 1500)
                    }}
                  >
                    <span style={{ color: 'red' }}>
                      <i className="fas fa-exclamation-triangle"></i> {t('Danger zone!')}
                    </span>
                    <br />
                    {t('Clicking this button will reset the entire database and delete all settings and metadata.')}
                  </ActionRow>
                </tbody>
              </table>
          </CollapsibleSection>

          <CollapsibleSection icon="fa-paint-brush" title={t('Theme')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <tr>
                    <td></td>
                    <td className="config-td">
                      <br />
                      {t('The selected theme will apply to the entire application and be shown to all users.')}
                      <br />
                      {t('If you\'re using a browser that supports "theme-color", the theme\'s primary color will also be applied there.')}
                      <br />
                      <br />
                      {t('Click on a theme to preview it before saving!')}
                    </td>
                  </tr>
                  <tr>
                    <td colSpan={2}>
                      {THEMES.map((theme) => (
                        <div key={theme.id} style={{ display: 'inline-block' }}>
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
                              style={{ cursor: 'pointer' }}
                            >
                              <img title={theme.name} src={`/legacy/img/theme_preview/${theme.id.replace('.css', '')}.png`} className="theme-preview" />
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

          <CollapsibleSection icon="fa-shield-alt" title={t('Security')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <CheckboxRow id="enablepass" checked={enablepass} onChange={setEnablepass} label={t('Enable Password')}>
                    {t("If enabled, everything that isn't reading will require a password.")}
                  </CheckboxRow>
                  {enablepass && (
                    <>
                      <Row label={t('New Password')}>
                        <input
                          className="stdinput"
                          style={{ width: '100%' }}
                          maxLength={255}
                          value={newPassword}
                          onChange={(e) => setNewPassword(e.target.value)}
                          type="password"
                        />
                      </Row>
                      <Row label={t('New Password Confirmation')}>
                        <input
                          className="stdinput"
                          style={{ width: '100%' }}
                          maxLength={255}
                          value={newPassword2}
                          onChange={(e) => setNewPassword2(e.target.value)}
                          type="password"
                        />
                        <br />
                        {t('Only edit these fields if you want to change your password.')}
                        <br />
                        {t('The one already stored will be used otherwise.')}
                      </Row>
                      <CheckboxRow id="nofunmode" checked={nofunmode} onChange={setNofunmode} label={t('No-Fun Mode')}>
                        {t('Enabling No-Fun Mode will lock reading archives behind the password as well.')}
                        <br />
                        {t('Fully effective after restarting LANraragi.')}
                      </CheckboxRow>
                      <Row label={t('API Key')}>
                        <input className="stdinput" style={{ width: '100%' }} maxLength={255} value={apikey} onChange={(e) => setApikey(e.target.value)} type="text" />
                        <br />
                        {t("If you wish to use the Client API and have a password, you'll have to set a key here.")}
                        <br />
                        <span dangerouslySetInnerHTML={{ __html: t('Empty keys will <b>not</b> work!') }} />
                        <br />
                        <span dangerouslySetInnerHTML={{ __html: t('This key will need to be provided in every protected API call as the <i>Authorization</i> header.') }} />
                      </Row>
                    </>
                  )}
                  <CheckboxRow id="enablecors" checked={enablecors} onChange={setEnablecors} label={t('Enable CORS for the Client API')}>
                    {t('Have API requests support Cross-Origin Resource Sharing, which allows web browsers to access it off other domains.')}
                    <br />
                    {t('Turn this on if you want to access this service through a web-based wrapper (e.g. a userscript) used/hosted on another domain.')}
                  </CheckboxRow>
                </tbody>
              </table>
          </CollapsibleSection>

          <CollapsibleSection icon="fa-file-archive" title={t('Archive Files')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <ActionRow
                    id="rescan-button"
                    label={t('Rescan Archive Directory')}
                    onClick={async () => {
                      setStatus(t('Rescanning...') ?? '')
                      await shinobuAction.mutateAsync('rescan')
                      setStatus(t('Rescan queued.') ?? '')
                    }}
                  >
                    {t("Click this button to trigger a rescan of the Archive Directory in case you're missing files, or some data such as total page counts.")}
                  </ActionRow>
                  <Row label={t('Maximum Cache Size')}>
                    <input
                      className="stdinput"
                      style={{ width: '100%' }}
                      maxLength={255}
                      value={tempmaxsize}
                      onChange={(e) => setTempmaxsize(Number(e.target.value))}
                      type="text"
                    />
                    <br />
                    {t('In MBs. The cache contains recently viewed pages, for faster subsequent reading.')}
                    <br />
                    {t('It is automatically emptied when it grows past this specified size.')} {t('The maximum value allowed is 4GB.')}
                  </Row>
                  <ActionRow
                    id="clean-temp"
                    label={t('Clear Cache')}
                    onClick={async () => {
                      await cleanTempfolder.mutateAsync()
                      setStatus(t('Cache cleared.') ?? '')
                    }}
                  >
                    <br />
                    {t('Clear the cache manually by clicking this button.')}
                  </ActionRow>
                  <ActionRow
                    id="reset-search-cache"
                    label={t('Reset Search Cache')}
                    onClick={async () => {
                      await resetSearchCache.mutateAsync()
                      setStatus(t('Search cache cleared.') ?? '')
                    }}
                  >
                    {t('The last searches done in the archive index are cached for faster loads.')}
                    <br />
                    {t("If something went wrong with said cache, you can reset it by clicking this button.")}
                  </ActionRow>
                  <ActionRow
                    id="clear-new-tags"
                    label={t('Clear NEW flags')}
                    onClick={async () => {
                      await clearNewFlags.mutateAsync()
                      setStatus(t('New flags cleared.') ?? '')
                    }}
                  >
                    {t('Newly uploaded archives are marked as "new" in the index until you\'ve opened them.')}
                    <br />
                    {t('If you want to clear those flags, click this button.')}
                  </ActionRow>
                  <CheckboxRow id="replacedupe" checked={replacedupe} onChange={setReplacedupe} label={t('Replace duplicated archives')}>
                    {t('If enabled, LANraragi will overwrite old archives when a newer one (with the same name) is uploaded through the Web Uploader or the Download System.')}
                    <br />
                    <i className="fas fa-exclamation-triangle" style={{ color: 'red' }}></i>{' '}
                    {t("This will delete metadata for old files when they're replaced! Use with caution.")}
                  </CheckboxRow>
                </tbody>
              </table>
          </CollapsibleSection>

          <CollapsibleSection icon="fa-tags" title={t('Tags and Thumbnails')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <CheckboxRow id="hqthumbpages" checked={hqthumbpages} onChange={setHqthumbpages} label={t('Use high-quality thumbnails for pages')}>
                    {t('LANraragi generates lower-quality thumbnails for archive pages for performance reasons.')}
                    <br />
                    {t('If this option is checked, it will instead generate page thumbnails at the same quality as cover thumbnails.')}
                  </CheckboxRow>
                  <CheckboxRow id="enablewebp" checked={enablewebp} onChange={setEnablewebp} label={t('Use WebP for thumbnails')}>
                    {t('If checked, thumbnails are generated as WebP, which is smaller than JPEG at the same quality. If unchecked, thumbnails are generated as JPEG instead.')}
                    <br />
                    <i className="fas fa-exclamation-triangle" style={{ color: 'red' }}></i>{' '}
                    {t('Changing this regenerates every thumbnail in the library, so they all stay in the same format.')}
                  </CheckboxRow>
                  {enablewebp && (
                    <Row label={t('WebP Quality')}>
                      <input
                        className="stdinput"
                        type="number"
                        min={0}
                        max={100}
                        style={{ width: '100%' }}
                        maxLength={255}
                        value={webpquality}
                        onChange={(e) => setWebpquality(Number(e.target.value))}
                      />
                      <br />
                      {t('Quality of generated WebP thumbnails. Higher quality = larger files. (0-100)')}
                    </Row>
                  )}
                  <ActionRow
                    id="genthumb-button"
                    label={t('Generate Missing Thumbnails')}
                    onClick={async () => {
                      await regenThumbnails.mutateAsync(false)
                      setStatus(t('Thumbnail generation queued.') ?? '')
                    }}
                  >
                    {t("Generate Thumbnails for all archives that don't have one yet.")}
                  </ActionRow>
                  <ActionRow
                    id="forcethumb-button"
                    label={t('Regenerate all Thumbnails')}
                    onClick={async () => {
                      await regenThumbnails.mutateAsync(true)
                      setStatus(t('Thumbnail regeneration queued.') ?? '')
                    }}
                  >
                    {t('Regenerate all thumbnails. This might take a while!')}
                  </ActionRow>
                  <Row label={t('Excluded Namespaces')}>
                    <input
                      className="stdinput"
                      style={{ width: '100%' }}
                      maxLength={255}
                      value={excludednamespaces}
                      onChange={(e) => setExcludednamespaces(e.target.value)}
                      type="text"
                    />
                    <br />
                    {t('Comma-separated list of tag namespaces to exclude from search suggestions and tag statistics.')}
                    <br />
                    {t('Clients will use this list to filter out noisy tags from autocomplete and tag clouds.')}
                  </Row>
                  <Row label={t('Tag Rules')}>
                    <input id="tagruleson" className="fa" type="checkbox" checked={tagruleson} onChange={(e) => setTagruleson(e.target.checked)} />
                    <br />
                    <textarea
                      className="stdinput"
                      style={{ width: '100%', height: 196 }}
                      value={tagrules}
                      onChange={(e) => setTagrules(e.target.value)}
                    />
                    <br />
                    {t('When tagging archives using Plugins, the rules specified here will be applied to the tags before saving them to the database.')}
                    <br />
                    {t('Split rules with linebreaks.')}
                    <br />
                    <span dangerouslySetInnerHTML={{ __html: t('<b>-tag | tag</b> : removes the tag (like a blacklist)') }} />
                    <br />
                    <span dangerouslySetInnerHTML={{ __html: t('<b>-namespace:*</b> : removes all tags within this namespace') }} />
                    <br />
                    {t('namespace : strips the namespace from the tags')}
                    <br />
                    <span dangerouslySetInnerHTML={{ __html: t('<b>tag -> new-tag</b> : replaces one tag') }} />
                    <br />
                    <span
                      dangerouslySetInnerHTML={{
                        __html: t(
                          '<b>tag => new-tag</b> : replaces one tag, but use a hash table internally for faster performance. These rules will be executed <i>once</i> after all other rules.',
                        ),
                      }}
                    />
                    <br />
                    <span dangerouslySetInnerHTML={{ __html: t('<b>namespace:* -> new-namespace:*</b> : replaces the namespace with the new one') }} />
                  </Row>
                </tbody>
              </table>
          </CollapsibleSection>

          <CollapsibleSection icon="fa-satellite" title={t('Background Workers')}>
              <table style={{ margin: 'auto', fontSize: '9pt' }}>
                <tbody>
                  <tr>
                    <td className="option-td">
                      <h2 className="ih">{t('Shinobu Status')}</h2>
                    </td>
                    <td className="config-td">
                      {shinobuStatus.data?.is_alive ? (
                        <span>
                          {t('The Shinobu File Watcher is currently')}{' '}
                          <h2 className="ih" style={{ display: 'inline', color: 'rgb(26, 165, 26)' }}>
                            👍 {t('OK!')}
                          </h2>
                        </span>
                      ) : (
                        <span>
                          {t('The Shinobu File Watcher is currently')}{' '}
                          <h2 className="ih" style={{ display: 'inline', color: 'rgb(207, 37, 37)' }}>
                            👹 {t('Kaput!')}
                          </h2>
                        </span>
                      )}{' '}
                      ({t('PID:')} <span id="pid">{shinobuStatus.data?.pid}</span>)
                      <br />
                      {t('This File Watcher is responsible for monitoring your content directory and automatically handling new archives as they come.')}
                      <br />
                    </td>
                  </tr>
                  <ActionRow
                    id="restart-button"
                    label={t('Restart File Watcher')}
                    onClick={async () => {
                      await shinobuAction.mutateAsync('restart')
                      setStatus(t('File Watcher restarted.') ?? '')
                    }}
                  >
                    {t('If Shinobu is dead or unresponsive, you can reboot her by clicking this button.')}
                  </ActionRow>
                  <ActionRow
                    id="open-minion"
                    label={t('Open Minion Console')}
                    onClick={() => navigate('/jobs')}
                  >
                    {t('The Minion Worker handles spare tasks that are too long to execute within the request/response lifecycle of web applications.')}
                    <br />
                    {t('The console shows currently running and concluded tasks.')}
                  </ActionRow>
                </tbody>
              </table>
          </CollapsibleSection>
        </ul>
      </form>
    </div>
  )
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <tr>
      <td className="option-td">
        {/* Legacy's `c.lh()` emits raw HTML — a few real labels embed their own `<br>` (e.g.
            "Maximum <br>Cache Size"), so this has to render as HTML, not escaped text. */}
        <h2 className="ih" dangerouslySetInnerHTML={{ __html: ` ${label} ` }} />
      </td>
      <td className="config-td">{children}</td>
    </tr>
  )
}

function CheckboxRow({
  id,
  checked,
  onChange,
  label,
  children,
}: {
  id: string
  checked: boolean
  onChange: (v: boolean) => void
  label: string
  children: React.ReactNode
}) {
  return (
    <tr>
      <td className="option-td">
        <h2 className="ih"> {label} </h2>
      </td>
      <td className="config-td">
        <input id={id} name={id} className="fa" type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
        <label htmlFor={id}>
          <br /> {children}
        </label>
      </td>
    </tr>
  )
}

function ActionRow({
  id,
  label,
  onClick,
  children,
}: {
  id: string
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <tr>
      <td className="option-td">
        <input id={id} className="stdbtn" type="button" value={label} onClick={onClick} />
      </td>
      <td className="config-td">{children}</td>
    </tr>
  )
}
