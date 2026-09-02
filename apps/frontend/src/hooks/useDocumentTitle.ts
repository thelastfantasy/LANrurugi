import { useEffect } from "react"

import { useSettings } from "@/api/hooks"

/** Matches legacy's own per-page `<title>{htmltitle} - {Page Name}</title>` pattern
 * (`htmltitle` is a site setting); `index.html`'s own `<title>` never varies per route. */
export function useDocumentTitle(pageName?: string) {
  const settings = useSettings()
  const htmltitle = settings.data?.htmltitle

  useEffect(() => {
    if (!htmltitle) return
    document.title = pageName ? `${htmltitle} - ${pageName}` : htmltitle
    return () => {
      document.title = htmltitle
    }
  }, [htmltitle, pageName])
}
