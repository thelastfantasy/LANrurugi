import { useState } from "react"
import { useTranslation } from "react-i18next"
import { FaPen, FaTrashCan } from "react-icons/fa6"

import { useAddBookmark, useRemoveBookmark, useSetBookmarkName } from "@/api/hooks"
import { BubbleActionItem, BubbleActionMenu, Popover } from "@/components/common-ui/Display"
import { IconButton, Input } from "@/components/common-ui/Form"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FONT_SIZE_MD } from "@/theme"

const MAX_BOOKMARK_NAME_LENGTH = 200

/** Bookmarked state shows a menu (rename/delete) instead of click-to-remove; the `B` shortcut
 * still adds/removes directly via `Reader.tsx`'s `toggleBookmark()`, unaffected. */
export function BookmarkButton({
  archiveId,
  page,
  bookmarked,
  name,
  loggedIn,
  onRequireLogin,
}: {
  archiveId: string
  page: number
  bookmarked: boolean
  /** Pre-fills the rename popover's input. */
  name?: string | null
  loggedIn: boolean
  onRequireLogin: () => void
}) {
  const { t } = useTranslation()
  const addBookmark = useAddBookmark()
  const removeBookmark = useRemoveBookmark()
  const [renaming, setRenaming] = useState(false)
  const [menuOpen, setMenuOpen] = useState(false)

  function handleAdd() {
    if (!loggedIn) {
      onRequireLogin()
      return
    }
    addBookmark.mutate({ archiveId, page })
  }

  if (!bookmarked) {
    return (
      <IconButton
        variant="ghost-btn"
        icon={<i className="far fa-bookmark fa-2x" />}
        aria-label={t("reader.toggleBookmark") ?? ""}
        title={t("reader.toggleBookmark") ?? undefined}
        className="toggle-bookmark"
        size={32}
        style={{ marginRight: 13, borderRadius: "50%" }}
        onClick={handleAdd}
      />
    )
  }

  return (
    <span style={{ position: "relative", display: "inline-block", marginRight: 13 }}>
      <BubbleActionMenu
        open={menuOpen}
        onOpenChange={setMenuOpen}
        trigger={
          <IconButton
            variant="ghost-btn"
            icon={<i className="fas fa-bookmark fa-2x" />}
            aria-label={t("reader.toggleBookmark") ?? ""}
            title={t("reader.toggleBookmark") ?? undefined}
            className="toggle-bookmark"
            size={32}
            style={{ borderRadius: "50%" }}
          />
        }
      >
        <BubbleActionItem
          icon={<FaPen />}
          label={t("bookmarks.editName")}
          onClick={() => {
            setMenuOpen(false)
            setRenaming(true)
          }}
        />
        <BubbleActionItem
          icon={<FaTrashCan />}
          label={t("common.delete")}
          onClick={() => {
            setMenuOpen(false)
            removeBookmark.mutate({ archiveId, page })
          }}
        />
      </BubbleActionMenu>
      <RenamePopover
        archiveId={archiveId}
        page={page}
        name={name ?? null}
        open={renaming}
        onOpenChange={setRenaming}
      />
    </span>
  )
}

/** Enter saves, Escape cancels; an empty/whitespace-only save clears the name server-side. */
function RenamePopover({
  archiveId,
  page,
  name,
  open,
  onOpenChange,
}: {
  archiveId: string
  page: number
  name: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const setName = useSetBookmarkName()
  const [draft, setDraft] = useState(name ?? "")

  function save() {
    const trimmed = draft.trim()
    setName.mutate(
      { archiveId, page, name: trimmed.length > 0 ? trimmed : null },
      { onSuccess: () => onOpenChange(false) },
    )
  }

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next)
        if (!next) setDraft(name ?? "")
      }}
      triggerNativeButton={false}
      // Overlays the visible bookmark icon exactly (its wrapping span is sized to match), so this
      // popover anchors/centers on the same point as `BubbleActionMenu`'s own trigger.
      trigger={<span style={{ position: "absolute", inset: 0, pointerEvents: "none" }} />}
    >
      <Input
        rows={1}
        value={draft}
        onValueChange={(v) => setDraft(v)}
        placeholder={t("bookmarks.namePlaceholder", { max: MAX_BOOKMARK_NAME_LENGTH }) ?? undefined}
        maxLength={MAX_BOOKMARK_NAME_LENGTH}
        autoFocus
        style={{
          display: "block",
          fontSize: FONT_SIZE_MD,
          background: "transparent",
          color: palette.text,
          border: `1px solid ${palette.border}`,
          borderRadius: 4,
          padding: "4px 8px",
          minWidth: 200,
          margin: 8,
        }}
        onKeyDown={(e) => {
          e.stopPropagation()
          if (e.key === "Enter") {
            e.preventDefault()
            save()
          } else if (e.key === "Escape") {
            e.preventDefault()
            onOpenChange(false)
          }
        }}
      />
    </Popover>
  )
}
