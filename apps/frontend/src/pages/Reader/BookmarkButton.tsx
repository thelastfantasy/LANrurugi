import { useState } from "react"
import { useTranslation } from "react-i18next"
import { FaPen, FaTrashCan } from "react-icons/fa6"

import { useAddBookmark, useRemoveBookmark, useSetBookmarkName } from "@/api/hooks"
import {
  BottomSheet,
  BubbleActionItem,
  BubbleActionMenu,
  Popover,
  SheetGrid,
  SheetTile,
} from "@/components/common-ui/Display"
import { IconButton, Input } from "@/components/common-ui/Form"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FONT_SIZE_MD, Z_DIALOG } from "@/theme"
import { toast } from "@/toast"

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
  onEditName,
}: {
  archiveId: string
  page: number
  bookmarked: boolean
  /** Pre-fills the rename popover's input. */
  name?: string | null
  loggedIn: boolean
  onRequireLogin: () => void
  /** Hands renaming to the caller (a sheet on touch devices) instead of this button's popover. */
  onEditName?: (page: number, name: string | null) => void
}) {
  const { t } = useTranslation()
  const addBookmark = useAddBookmark()
  const removeBookmark = useRemoveBookmark()
  // The page renaming started on: the reader may scroll to another while the field is open.
  const [renaming, setRenaming] = useState<number | null>(null)
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

  const bookmarkedIcon = (
    <IconButton
      variant="ghost-btn"
      icon={<i className="fas fa-bookmark fa-2x" />}
      aria-label={t("reader.toggleBookmark") ?? ""}
      title={t("reader.toggleBookmark") ?? undefined}
      className="toggle-bookmark"
      size={32}
      style={{ borderRadius: "50%" }}
      onClick={onEditName ? () => setMenuOpen(true) : undefined}
    />
  )

  // Touch devices: the same two actions as a sheet, like the toolbar's other menus there.
  if (onEditName) {
    return (
      <span style={{ display: "inline-block", marginRight: 13 }}>
        {bookmarkedIcon}
        <BottomSheet
          open={menuOpen}
          onClose={() => setMenuOpen(false)}
          cancelLabel={t("common.cancel")}
        >
          <div style={{ paddingTop: 10 }}>
            <SheetGrid>
              <SheetTile
                icon="fa-pen"
                onClick={() => {
                  setMenuOpen(false)
                  onEditName(page, name ?? null)
                }}
              >
                {t("bookmarks.editName")}
              </SheetTile>
              <SheetTile
                icon="fa-trash"
                danger
                onClick={() => {
                  setMenuOpen(false)
                  removeBookmark.mutate({ archiveId, page })
                }}
              >
                {t("common.delete")}
              </SheetTile>
            </SheetGrid>
          </div>
        </BottomSheet>
      </span>
    )
  }

  return (
    <span style={{ position: "relative", display: "inline-block", marginRight: 13 }}>
      <BubbleActionMenu
        open={menuOpen}
        onOpenChange={setMenuOpen}
        trigger={bookmarkedIcon}
      >
        <BubbleActionItem
          icon={<FaPen />}
          label={t("bookmarks.editName")}
          onClick={() => {
            setMenuOpen(false)
            setRenaming(page)
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
        page={renaming ?? page}
        name={name ?? null}
        open={renaming !== null}
        onOpenChange={(open) => setRenaming(open ? page : null)}
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

/** Touch devices: the rename field in a sheet above the keyboard, for the reader's toolbar and the
 * bookmarks page alike. A popover anchored to the toolbar sat where the keyboard opens and went with
 * the toolbar when the reader moved under it. */
export function BookmarkNameSheet({
  archiveId,
  edit,
  onClose,
}: {
  archiveId: string
  /** The page being renamed and its current name; `null` while closed. */
  edit: { page: number; name: string | null } | null
  onClose: () => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const setName = useSetBookmarkName()
  const [draft, setDraft] = useState("")
  // Kept through the exit slide, which outlives `edit`.
  const [page, setPage] = useState<number | null>(null)
  const [seenEdit, setSeenEdit] = useState(edit)
  if (edit !== seenEdit) {
    setSeenEdit(edit)
    if (edit) {
      setPage(edit.page)
      setDraft(edit.name ?? "")
    }
  }

  function save() {
    if (page === null) return
    const trimmed = draft.trim()
    setName.mutate(
      { archiveId, page, name: trimmed.length > 0 ? trimmed : null },
      {
        onSuccess: onClose,
        onError: (err) =>
          toast({
            heading: t("bookmarks.errorRenamingBookmark") ?? undefined,
            text: String(err),
            icon: "error",
          }),
      },
    )
  }

  return (
    <BottomSheet
      open={edit !== null}
      onClose={onClose}
      cancelLabel={t("common.cancel")}
      aboveKeyboard
      // Above the bookmarks page's hover grid, which is itself an overlay.
      zIndex={Z_DIALOG}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 10, paddingTop: 10 }}>
        <Input
          rows={1}
          value={draft}
          onValueChange={(v) => setDraft(v)}
          placeholder={t("bookmarks.namePlaceholder", { max: MAX_BOOKMARK_NAME_LENGTH }) ?? undefined}
          maxLength={MAX_BOOKMARK_NAME_LENGTH}
          autoFocus
          style={{
            display: "block",
            width: "100%",
            boxSizing: "border-box",
            fontSize: 16,
            background: "transparent",
            color: palette.text,
            border: `1px solid ${palette.border}`,
            borderRadius: 4,
            padding: "8px 10px",
          }}
          onKeyDown={(e) => {
            e.stopPropagation()
            if (e.key === "Enter") {
              e.preventDefault()
              save()
            }
          }}
        />
        <button
          type="button"
          className="stdbtn"
          disabled={setName.isPending}
          onClick={save}
          // `.stdbtn` is sized for desktop toolbars; a sheet's own tap target is a full row.
          style={{ width: "100%", height: 44, fontSize: 16, margin: 0 }}
        >
          <i className="fa fa-check" style={{ marginRight: 8 }} />
          {t("Save")}
        </button>
      </div>
    </BottomSheet>
  )
}
