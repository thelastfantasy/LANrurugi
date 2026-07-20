import { useTranslation } from 'react-i18next'
import { NavLink, Outlet } from 'react-router-dom'

import Footer from './components/Footer'
import UpdateBanner from './components/UpdateBanner'
import { useApplyTheme } from './theme'

// Legacy's own top nav (`~/LANraragi/templates/index.html.tt2`'s `<p id="nb">`, reused across
// every page) is a single centered text line of *exactly* six links — Font Awesome caret icons
// separating bold links, styled by `p#nb`/`p#nb a` in the copied theme CSS (`useApplyTheme`).
// Batch Operations/Plugin Configuration/Database Backup-Restore aren't here because legacy
// doesn't put them here either — they're buttons on the Settings page itself
// (`~/LANraragi/templates/config.html.tt2`'s left column: `#plugin-config`/`#backup`/`#batch`),
// which is also where the language switcher and logout now live (see `Settings.tsx`) — legacy's
// nav has no room for either.
export default function Layout() {
  const { t } = useTranslation()
  useApplyTheme()

  const links: Array<{ to: string; label: string; end?: boolean }> = [
    { to: '/upload', label: t('Add Archives') },
    { to: '/duplicates', label: t('Duplicate Detection') },
    { to: '/config', label: t('Settings') },
    { to: '/config/categories', label: t('Modify Categories') },
    { to: '/stats', label: t('Statistics') },
    { to: '/logs', label: t('Logs') },
  ]

  return (
    <div>
      <p id="nb">
        {links.map((link, i) => (
          <span key={link.to}>
            <i className="fa fa-caret-right"></i>
            <NavLink to={link.to} end={link.end}>
              {link.label}
            </NavLink>
            {i < links.length - 1 && <span style={{ marginLeft: 5 }}></span>}
          </span>
        ))}
      </p>
      <UpdateBanner />
      <Outlet />
      <Footer />
    </div>
  )
}
