import type { MouseEvent } from "react"
import { useTranslation } from "react-i18next"

import { useBookmarkLink, useCategories, useLoginStatus } from "../../api/hooks"
import { routes } from "../../routes"
import { toast } from "../../toast"

export function BookmarkIcon({ archiveId }: { archiveId: string }) {
  const { t } = useTranslation()
  const bookmarkLink = useBookmarkLink()
  const categories = useCategories()
  const loginStatus = useLoginStatus()
  const bookmarkCategoryId = bookmarkLink.data?.category_id || null
  if (!bookmarkCategoryId) return null
  const loggedIn = loginStatus.data?.logged_in ?? true
  const isBookmarked = Boolean(
    categories.data?.find((c) => c.id === bookmarkCategoryId)?.archives.includes(archiveId),
  )

  async function toggle(e: MouseEvent) {
    e.preventDefault()
    e.stopPropagation()
    if (!loggedIn) {
      toast({
        text: `<a href="${routes.login()}">${t("Login")}</a> ${t("to toggle bookmark feature.")}`,
        icon: "warning",
      })
      return
    }
    const method = isBookmarked ? "DELETE" : "PUT"
    await fetch(`/api/categories/${bookmarkCategoryId}/${archiveId}`, { method })
    await categories.refetch()
  }

  return (
    <i
      className={`${isBookmarked ? "fas" : "far"} fa-bookmark thumbnail-bookmark-icon${loggedIn ? "" : " disabled"}`}
      title={t("Toggle Bookmark") ?? undefined}
      style={!loggedIn ? { opacity: 0.5, cursor: "not-allowed" } : { cursor: "pointer" }}
      onClick={(e) => void toggle(e)}
    ></i>
  )
}
