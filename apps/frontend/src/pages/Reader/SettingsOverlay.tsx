import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'

import { ensureLink, removeLink } from '../../theme'
import type { FitMode, ReaderSettings } from './useReaderSettings'

const CONFIG_CSS_ID = 'reader-config-css'

// Mirrors legacy's `#settingsOverlay` (`[% BLOCK config %]` in
// `~/LANraragi/templates/reader.html.tt2`) — every toggle group from the real template, in the
// same order. `.config-panel`/`.config-btn` come from `/legacy/config.css`, not loaded by
// `useApplyTheme`, so this page-scopes it itself (same pattern as `Settings.tsx`).
export default function SettingsOverlay({
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
    ensureLink(CONFIG_CSS_ID, '/legacy/config.css')
    return () => removeLink(CONFIG_CSS_ID)
  }, [])

  // Legacy marks the active choice in each toggle group by adding a `.toggled` class (reader.js's
  // `initializeSettings`/`toggleFitMode`/etc, e.g. `$("#fit-width").addClass("toggled")`) — a real
  // class already styled (background/border/color) by each theme's own CSS
  // (`~/LANraragi/public/themes/*.css`'s `.toggled` rule), not an ad-hoc inline style.
  function btnClass(active: boolean) {
    return `favtag-btn config-btn${active ? ' toggled' : ''}`
  }

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — must be explicitly shown, same as
          `ArchiveOverviewOverlay`'s copy of this element. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: 'block', opacity: 0.6 }} onClick={onClose} />
      <div id="settingsOverlay" className="id1 base-overlay small-overlay">
        <div>
          <h2 className="ih" style={{ textAlign: 'center' }}>
            {t('Reader Options')}
          </h2>
          <h1 className="ih config-panel">
            {t('Those options save automatically -- Click around and find out!')}
          </h1>

          <div id="fit-mode">
            <h2 className="config-panel">{t('Fit display to')}</h2>
            {(
              [
                ['container', t('Container')],
                ['fit-width', t('Width')],
                ['fit-height', t('Height')],
              ] as [FitMode, string][]
            ).map(([mode, label]) => (
              <input
                key={mode}
                className={btnClass(settings.fitMode === mode)}
                type="button"
                value={label}
                onClick={() => update({ fitMode: mode })}
              />
            ))}
          </div>

          <div id="container-width">
            <h2 className="config-panel">{t('Container Width (in pixels or percentage)')}</h2>
            <input
              id="container-width-input"
              className="stdinput"
              style={{ display: 'inline', width: '70%' }}
              placeholder={t('The default value is 1200px, or 90% in Double Page Mode.') ?? undefined}
              defaultValue={settings.containerWidth}
              onKeyDown={(e) => {
                if (e.key === 'Enter') update({ containerWidth: e.currentTarget.value })
              }}
            />
            <input
              className="favtag-btn config-btn"
              type="button"
              style={{ display: 'inline' }}
              value={t('Apply') ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget.previousElementSibling as HTMLInputElement
                update({ containerWidth: input.value })
              }}
            />
          </div>

          <div id="toggle-double-mode">
            <h2 className="config-panel">{t('Page Rendering')}</h2>
            <input
              className={btnClass(!settings.doublePageMode)}
              type="button"
              value={t('Single') ?? undefined}
              onClick={() => update({ doublePageMode: false })}
            />
            <input
              className={btnClass(settings.doublePageMode)}
              type="button"
              value={t('Double') ?? undefined}
              onClick={() => update({ doublePageMode: true })}
            />
          </div>

          <div id="toggle-manga-mode">
            <h2 className="config-panel">{t('Reading Direction')}</h2>
            <input
              className={btnClass(!settings.mangaMode)}
              type="button"
              value={t('Left to Right') ?? undefined}
              onClick={() => update({ mangaMode: false })}
            />
            <input
              className={btnClass(settings.mangaMode)}
              type="button"
              value={t('Right to Left') ?? undefined}
              onClick={() => update({ mangaMode: true })}
            />
          </div>

          <div id="preload-images">
            <h2 className="config-panel">{t('How many images to preload')}</h2>
            <input
              id="preload-input"
              className="stdinput"
              // Legacy's own markup (`reader.html.tt2`) sets no width at all on this input, so
              // it's whatever the browser's default `<input type="number">` width happens to be
              // — wide enough (for a field that only ever needs 1-2 digits) to force the Apply
              // button below it onto its own line in this settings panel's narrower column,
              // rather than sitting inline beside it the way `display: 'inline'` alone intends.
              style={{ display: 'inline', width: '4em' }}
              type="number"
              placeholder={t('The default is two images.') ?? undefined}
              defaultValue={settings.preloadCount}
              onKeyDown={(e) => {
                if (e.key === 'Enter') update({ preloadCount: Number(e.currentTarget.value) || 2 })
              }}
            />
            <input
              className="favtag-btn config-btn"
              type="button"
              style={{ display: 'inline' }}
              value={t('Apply') ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget.previousElementSibling as HTMLInputElement
                update({ preloadCount: Number(input.value) || 2 })
              }}
            />
          </div>

          <div id="toggle-header">
            <h2 className="config-panel">{t('Header')}</h2>
            <input
              className={btnClass(!settings.hideHeader)}
              type="button"
              value={t('Visible') ?? undefined}
              onClick={() => update({ hideHeader: false })}
            />
            <input
              className={btnClass(settings.hideHeader)}
              type="button"
              value={t('Hidden') ?? undefined}
              onClick={() => update({ hideHeader: true })}
            />
          </div>

          <div id="toggle-overlay">
            <h2 className="config-panel">{t('Show Archive Overlay by default')}</h2>
            <span className="config-panel">
              {t('This will show the overlay with thumbnails every time you open a new Reader page.')}
            </span>
            <input
              className={btnClass(settings.showOverlayByDefault)}
              type="button"
              value={t('Enabled') ?? undefined}
              onClick={() => update({ showOverlayByDefault: true })}
            />
            <input
              className={btnClass(!settings.showOverlayByDefault)}
              type="button"
              value={t('Disabled') ?? undefined}
              onClick={() => update({ showOverlayByDefault: false })}
            />
          </div>

          <div id="toggle-progress">
            <h2 className="config-panel">{t('Progression Tracking')}</h2>
            <span className="config-panel">
              {t('Disabling tracking will restart reading from page one every time you reopen the reader.')}
            </span>
            <input
              className={btnClass(!settings.ignoreProgress)}
              type="button"
              value={t('Enabled') ?? undefined}
              onClick={() => update({ ignoreProgress: false })}
            />
            <input
              className={btnClass(settings.ignoreProgress)}
              type="button"
              value={t('Disabled') ?? undefined}
              onClick={() => update({ ignoreProgress: true })}
            />
          </div>

          <div id="toggle-infinite-scroll">
            <h2 className="config-panel">{t('Infinite Scrolling')}</h2>
            <span className="config-panel">
              {t('Display all images in a vertical view in the same page.')}
            </span>
            <input
              className={btnClass(settings.infiniteScroll)}
              type="button"
              value={t('Enabled') ?? undefined}
              onClick={() => update({ infiniteScroll: true })}
            />
            <input
              className={btnClass(!settings.infiniteScroll)}
              type="button"
              value={t('Disabled') ?? undefined}
              onClick={() => update({ infiniteScroll: false })}
            />
          </div>

          <div id="auto-next-page">
            <h2 className="config-panel">{t('Auto next page interval in seconds')}</h2>
            <input
              id="auto-next-page-input"
              className="stdinput"
              style={{ display: 'inline' }}
              placeholder={t('The default is 10 seconds.') ?? undefined}
              defaultValue={settings.autoNextPageInterval}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  update({ autoNextPageInterval: Number(e.currentTarget.value) || 10 })
                }
              }}
            />
            <input
              className="favtag-btn config-btn"
              type="button"
              style={{ display: 'inline' }}
              value={t('Apply') ?? undefined}
              onClick={(e) => {
                const input = e.currentTarget.previousElementSibling as HTMLInputElement
                update({ autoNextPageInterval: Number(input.value) || 10 })
              }}
            />
          </div>

          <div id="toggle-stamps-visibility">
            <h2 className="config-panel">{t('Toggle Stamps')}</h2>
            <input
              className="fa"
              type="checkbox"
              checked={settings.markersVisible}
              onChange={(e) => update({ markersVisible: e.target.checked })}
            />
          </div>
        </div>
      </div>
    </>
  )
}
