import { useEffect } from "react"

import { useSettings } from "./api/hooks"

/** Matches legacy's own per-page `<title>[% title %] - [% c.lh("Page Name") %]</title>` pattern
 * (`title` is the site's `htmltitle` setting) — `index.html`'s own `<title>` is just the Vite
 * scaffold default ("frontend") and nothing here ever touched `document.title` per route, unlike
 * every real legacy template. */
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
