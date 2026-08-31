import { useEffect } from "react"
import { useTranslation } from "react-i18next"

import { useServerInfo, useUpdateCheck } from "@/api/hooks"
import { toast } from "@/toast"

/** Fires an update-available toast once a newer release is detected. Renders nothing itself;
 * requires the app-wide `<ToastContainer />` mounted in `App.tsx`. */
export function UpdateBanner() {
  const { t } = useTranslation()
  const info = useServerInfo()
  const check = useUpdateCheck(info.data?.version, info.data?.debug_mode ?? true)

  useEffect(() => {
    if (!check.data) return
    toast({
      heading: t("components.layout.aNewVersionOfLanrurugi", {
        version: check.data.latestVersion,
      }),
      text: `<a href="${encodeURI(check.data.releaseUrl)}" target="_blank" rel="noreferrer">${t("components.layout.clickHereToCheckIt")}</a>`,
      html: true,
      icon: "info",
      closeOnClick: false,
      draggable: false,
      hideAfter: 7000,
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [check.data])

  return null
}
