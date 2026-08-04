import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'

import { useServerInfo, useUpdateCheck } from '../api/hooks'
import { toast } from '../toast'

/** Fires legacy's own update-available toast (`~/LANraragi/public/js/mod/index.js::checkVersion`'s
 * `LRR.toast({ heading: I18N.IndexUpdateNotif(...), ... })`) once a newer release is detected —
 * same trigger (`useUpdateCheck`), same `toast()` call shape, just via the shared `toast()` helper
 * instead of legacy's Preact-wrapped one. Renders nothing itself; requires the app-wide
 * `<ToastContainer />` mounted in `App.tsx` to actually display anything. */
export function UpdateBanner() {
  const { t } = useTranslation()
  const info = useServerInfo()
  const check = useUpdateCheck(info.data?.version, info.data?.debug_mode ?? true)

  useEffect(() => {
    if (!check.data) return
    toast({
      heading: t('A new version of LANrurugi ({{version}}) is available!', {
        version: check.data.latestVersion,
      }),
      text: `<a href="${check.data.releaseUrl}" target="_blank" rel="noreferrer">${t('Click here to check it out.')}</a>`,
      icon: 'info',
      closeOnClick: false,
      draggable: false,
      hideAfter: 7000,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only re-fire if the detected release itself changes
  }, [check.data])

  return null
}
