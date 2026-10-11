import { useEffect } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateCheck } from "@/api/hooks"
import { toast } from "@/toast"

/** Fires an update-available toast once a newer release is detected. Renders nothing itself;
 * requires the app-wide `<ToastContainer />` mounted in `App.tsx`. */
export function UpdateBanner() {
  const { t } = useTranslation()
  const check = useUpdateCheck()

  useEffect(() => {
    if (!check.data) return
    // For upstream/trunk builds only advertise an update once the commit actually produced a
    // pullable Docker image (the backend's `latest.commit.pull` field).
    if (check.data.isUpstream && !check.data.upstreamPull) return
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
