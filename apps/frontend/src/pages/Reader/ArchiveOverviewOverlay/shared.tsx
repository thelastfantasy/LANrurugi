import type { MouseEvent } from "react"
import { useEffect, useLayoutEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { PopupMenu, PopupMenuItem } from "../../../components/PopupMenu"
import { displayTocName, isReservedTocIdentifier, TOC_CHAPTER_COUNT, TOC_IDENTIFIER_TABLE_OF_CONTENTS, tocChapterIdentifier } from "../../../lib/tocValidation"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "../../../theme"

// How many `ChapterActionMenu` instances are currently mounted/open — a plain module-level
// counter, not React state, since it exists purely to answer one synchronous yes/no question
// inside another component's own capture-phase `keydown` handler (`PageLightbox`'s own Escape
// listener, below), not to drive any render.
//
// Why this exists: both `PageLightbox` and `ChapterActionMenu` register their own capture-phase
// `Escape` listener on `window` (`PageLightbox`'s to close the lightbox, `ChapterActionMenu`'s to
// close just the menu). Per the DOM spec, multiple capture-phase listeners on the *same* target
// fire in **registration order**, not "whichever opened most recently wins" — since
// `PageLightbox` always mounts (and registers) before a user can ever open a menu from inside it,
// `PageLightbox`'s own listener necessarily runs first on every Escape press, no matter what the
// menu's own listener does after it (a real, live-confirmed bug: opening "Delete Chapter" from
// inside the lightbox and pressing Escape closed the whole lightbox in one keypress instead of
// just backing out of that one menu — `ChapterActionMenu`'s own `stopImmediatePropagation()`
// fired too late to matter, since `PageLightbox`'s handler had already run and closed it first).
// Checking this counter lets `PageLightbox`'s handler recognize "a menu is currently open above
// me" and skip acting, so that same Escape press closes only the menu — exactly the layered-popup
// behavior a user would expect, achieved without needing to fight event-registration order at all.
export let openChapterActionMenuCount = 0

export function incrementOpenChapterActionMenuCount(delta: 1 | -1) {
  openChapterActionMenuCount += delta
}

/** A ToC entry's own real span: which page it starts on, and how many pages belong to it (up to
 * but not including the next entry's start page, or the archive's own last page for the final
 * entry) — `count` includes the start page itself, matching how a real reader would count "第一章
 * (13)" as 13 pages total starting from the chapter-start page, not 13 pages *after* it. */
export function tocChapterSpans(toc: { page: number; name: string }[], totalPages: number): { page: number; name: string; count: number }[] {
  const sorted = [...toc].sort((a, b) => a.page - b.page)
  return sorted.map((entry, i) => ({
    ...entry,
    count: (sorted[i + 1]?.page ?? totalPages + 1) - entry.page,
  }))
}

/** Which chapter (if any) a given page belongs to, per `tocChapterSpans` above, plus whether that
 * page is the chapter's own start page (`isStart`) — the lightbox's info bar and filmstrip labels
 * both need this same "which chapter, and is this the first page of it" distinction: the start
 * page gets bold/accent styling with no count suffix, later pages get plain styling plus a
 * "(N)" page-count suffix showing how many pages the chapter spans in total. */
export function chapterForPage(
  spans: { page: number; name: string; count: number }[],
  page: number,
): { page: number; name: string; count: number; isStart: boolean; ordinal: number } | undefined {
  const span = [...spans].filter((c) => c.page <= page).sort((a, b) => b.page - a.page)[0]
  // `ordinal`: this specific page's own 1-based position within the chapter (the start page
  // itself is 1) — distinct from `count`, which is the chapter's fixed *total* span. The
  // filmstrip needs `ordinal` (a running "第 1 章 (5)" that increments frame to frame, showing
  // how far into the chapter this particular thumbnail is) rather than `count` (which would
  // repeat the same total on every frame and not actually distinguish them).
  return span ? { ...span, isStart: span.page === page, ordinal: page - span.page + 1 } : undefined
}

// Small, fixed palette of hue offsets (not fully random per render — a stable, deterministic hash
// of the chapter name so the same chapter always gets the same swatch color across re-renders/
// filmstrip scroll) rotated through HSL space; lightness/saturation are fixed at values that read
// clearly as a small color chip against either a light or dark themed background, adjusted per
// `chapterSwatchColor`'s own `onDark` param for contrast rather than picked once and hoping.
export function hashString(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
  return h
}

/** Deterministic swatch color for a chapter name — same name always maps to the same hue, and
 * lightness is chosen for contrast against the active theme's own background (`onDark`: whether
 * that background reads as dark, per a simple luminance check on `palette.bg` at the call site)
 * rather than a single fixed lightness that would wash out on one theme or the other. */
export function chapterSwatchColor(name: string, onDark: boolean): string {
  const hue = hashString(name) % 360
  return onDark ? `hsl(${hue}, 55%, 62%)` : `hsl(${hue}, 55%, 40%)`
}

/** Simple relative-luminance check on a `#rrggbb`/`rgb(...)` CSS color string — used only to
 * decide whether `chapterSwatchColor` should lean light or dark for contrast; doesn't need to be
 * colorimetrically precise, just consistent enough to pick the right side for each theme's own
 * (always solid, never gradient/transparent) `MENU_PALETTE` background color. */
export function isDarkColor(css: string): boolean {
  const m = css.match(/\d+/g)
  if (!m || m.length < 3) return false
  const [r, g, b] = m.map(Number)
  return (0.299 * r + 0.587 * g + 0.114 * b) / 255 < 0.5
}

/** Positions a popup's `{top, left}` with its own **top-right corner** anchored to `anchor`'s
 * bottom-right corner (matching how a real button-triggered dropdown reads — "opens below and to
 * the same side as the button", same relationship `Library.tsx`'s own gear-icon `SettingsMenu`
 * uses) rather than at raw click coordinates: a popup anchored to wherever inside the button the
 * pointer happened to land reads as misaligned/floating, most visibly when the button sits near
 * the modal's own edge (confirmed live via a real screenshot — a menu triggered near the trash
 * icon rendered oddly offset instead of hanging directly off that icon's own corner).
 *
 * `width` must be the popup's own *real*, already-rendered width (`getBoundingClientRect().width`
 * — see `useAnchoredMenuPosition`'s own two-pass measure-then-reposition dance, the same "don't
 * know the real size until after layout" problem `Tooltip.tsx` already solves the same way) — an
 * earlier version of this function took a hardcoded width *estimate* instead, which silently
 * placed the menu's right edge wherever that guess said to rather than the button's own real
 * right edge, confirmed live via a real screenshot: a 180px estimate for an actually-97px-wide
 * menu left a visible ~80px gap between the menu and the button that triggered it.
 *
 * Also clamped so the result stays fully inside `#archivePagesOverlay` (this overlay's own modal
 * box, not just the browser viewport — also a real, live-confirmed bug: a menu near the modal's
 * own right edge rendered partly outside it). */
export function anchorPopupToOverviewModal(anchor: DOMRect, width: number, height: number): { top: number; left: number } {
  const margin = 8
  const bounds = document.getElementById("archivePagesOverlay")?.getBoundingClientRect()
  const minLeft = (bounds?.left ?? 0) + margin
  const maxLeft = (bounds?.right ?? window.innerWidth) - width - margin
  const minTop = (bounds?.top ?? 0) + margin
  const maxTop = (bounds?.bottom ?? window.innerHeight) - height - margin
  return {
    left: Math.max(minLeft, Math.min(anchor.right - width, maxLeft)),
    top: Math.max(minTop, Math.min(anchor.bottom, maxTop)),
  }
}

/** Two-pass measure-then-reposition for a popup anchored via `anchorPopupToOverviewModal` — first
 * render parks the menu off-screen (so nothing flashes at a wrong position for a frame), a
 * `useLayoutEffect` then measures its own real rendered `getBoundingClientRect()` and recomputes
 * the real position from that, both applied before the browser actually paints. Shared by
 * `QuickAddTocPopover`/`ChapterActionMenu` rather than each hand-rolling the identical dance. */
export function useAnchoredMenuPosition(anchor: DOMRect) {
  const [menuEl, setMenuEl] = useState<HTMLUListElement | null>(null)
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: -9999, left: -9999 })

  useLayoutEffect(() => {
    if (!menuEl) return
    // This is exactly the "synchronize with an external system" case the underlying rule's own
    // description carves out as legitimate — measuring the menu's real, just-rendered DOM box
    // (`getBoundingClientRect()`, unknowable before this menu actually exists in the tree) to
    // reposition it, mirroring `Tooltip.tsx`'s own identical measure-then-reposition dance for the
    // identical reason (a real popup's true size is never known ahead of its own first render).
    const rect = menuEl.getBoundingClientRect()
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setPos(anchorPopupToOverviewModal(anchor, rect.width, rect.height))
    // `anchor` is a fresh `DOMRect` object every render (from `getBoundingClientRect()` at the
    // trigger's own click-time) — comparing it by reference would recompute every render for no
    // reason; comparing its own numeric fields is what actually reflects "the anchor moved".
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuEl, anchor.top, anchor.left, anchor.right, anchor.bottom])
  return { setMenuEl, pos }
}

/** The actual preset rows (封面/封底/目录/彩页 + 第N章 select) shared between `QuickAddTocPopover`
 * (wrapped in a `PopupMenu`, triggered from the page-grid icon's right-click) and
 * `PageLightbox` (rendered flat/inline instead of in a popup — see that component's own docs on
 * why: a lightbox already has room to show these permanently rather than behind another click). */
export function QuickAddTocOptions({ onPick, asMenuItems }: { onPick: (title: string) => void; asMenuItems: boolean }) {
  const { t } = useTranslation()
  // `value`: what's actually stored (`onPick`'s argument). Table of Contents/Chapter N use the
  // reserved identifier (`toc`/`c1`-`c20`) so the backend can dedup by name (see
  // `lib/tocValidation.ts`'s own docs) — Cover/Back Cover/Color Pages have no such uniqueness
  // requirement (a user might legitimately want two "Color Pages" entries at different points in
  // a volume) and keep storing real display text exactly as before.
  const presets: { icon: string; title: string; value: string }[] = [
    { icon: "fa-file-image", title: t("Cover") ?? "Cover", value: t("Cover") ?? "Cover" },
    { icon: "fa-file-image", title: t("Back Cover") ?? "Back Cover", value: t("Back Cover") ?? "Back Cover" },
    { icon: "fa-list", title: t("Table of Contents") ?? "Table of Contents", value: TOC_IDENTIFIER_TABLE_OF_CONTENTS },
    { icon: "fa-palette", title: t("Color Pages") ?? "Color Pages", value: t("Color Pages") ?? "Color Pages" },
    { icon: "fa-gift", title: t("Omake") ?? "Omake", value: t("Omake") ?? "Omake" },
    { icon: "fa-pen-nib", title: t("Afterword") ?? "Afterword", value: t("Afterword") ?? "Afterword" },
    { icon: "fa-image", title: t("Illustration") ?? "Illustration", value: t("Illustration") ?? "Illustration" },
  ]
  const Row = asMenuItems ? PopupMenuItem : "div"
  const chapterSelect = (
    <select
      className="favtag-btn"
      defaultValue=""
      style={{ marginLeft: 4 }}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => {
        if (!e.target.value) return
        onPick(tocChapterIdentifier(Number(e.target.value)))
        e.target.value = ""
      }}
    >
      <option value="" disabled>
        {t("Chapter…")}
      </option>
      {Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => i + 1).map((n) => (
        <option key={n} value={n}>
          {t("Chapter {{n}}", { n })}
        </option>
      ))}
    </select>
  )
  return (
    <>
      {presets.map(({ icon, title, value }) => (
        <Row
          key={title}
          onClick={(e: MouseEvent) => {
            // Without this, the click bubbles up to whatever's underneath (the page-grid cell's
            // own `onClick={() => onSelectPage(page)}` in the popover case) — the chapter got
            // added correctly, but something else also fired as an unwanted side effect.
            e.stopPropagation()
            onPick(value)
          }}
          {...(asMenuItems ? {} : { style: { cursor: "pointer", padding: "4px 8px", display: "flex", alignItems: "center" } })}
        >
          <i className={`fa ${icon}`} style={{ width: 18 }}></i> {title}
        </Row>
      ))}
      {/* `display: flex; alignItems: center` in flat mode — without it, the `<select>`'s own
          default vertical metrics (a form control, not inline text) sit slightly off from the
          preset rows' plain icon+text baseline alignment above, a real visible mismatch confirmed
          on screenshot even though both rows use the same padding. */}
      <Row {...(asMenuItems ? { style: { cursor: "default" } } : { style: { padding: "4px 8px", display: "flex", alignItems: "center" } })}>
        <i className="fa fa-book-medical" style={{ width: 18 }}></i>
        {chapterSelect}
      </Row>
    </>
  )
}

/** Right-click menu on the "add chapter" icon (`PageGridActionIcon`'s `fa-book-medical`) — a
 * purely additive shortcut, no legacy equivalent, for the handful of chapter titles common enough
 * in real doujin/manga scans to not need typing out via the plain left-click `promptDialog` flow
 * every time. Every option submits immediately on pick (no separate confirm step) — the point is
 * speed for a title that's already fully decided the moment it's clicked/selected, not a form. */
export function QuickAddTocPopover({
  anchor,
  onPick,
  onClose,
}: {
  anchor: DOMRect
  onPick: (title: string) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  return (
    <>
      {/* `stopPropagation` — this backdrop's own click-to-close would otherwise bubble up to
          `#overlay-shade` (the outer Archive Overview modal's own click-to-close backdrop,
          covering the same full viewport) and close *that* too, since neither backdrop is a DOM
          ancestor/descendant of the other that a plain click could be scoped to (confirmed live:
          clicking outside this popover closed the whole overview modal along with it). */}
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: "fixed", top: pos.top, left: pos.left, zIndex: Z_OVERLAY_CONTENT }}
        mainLabel={{ icon: "fa-bolt", text: t("Quick Add Chapter") ?? "Quick Add Chapter" }}
      >
        <QuickAddTocOptions
          asMenuItems
          onPick={(title) => {
            onPick(title)
            onClose()
          }}
        />
      </PopupMenu>
    </>
  )
}

/** Delete-chapter menu — legacy's own `.remove-toc` (`reader.js`) only ever deletes whichever
 * chapter the reader currently happens to be scrolled into (`getCurrentChapter()`), with no way
 * to target a different one at all (see `handleRemoveToc`'s own docs for the real-source
 * confirmation). This lists every chapter in the archive (matching the Upload page's own
 * `ConflictMenu`/`RenamePopover` popup-menu visual pattern), so picking one to delete doesn't
 * require first navigating to it. Clicking an entry still goes through the same themed
 * `confirmDialog` as before (see `handleRemoveToc`) — this menu only changes *which* chapter that
 * confirmation is about, not whether one still happens. */
/** Shared by the edit and delete chapter icons — both need the exact same "list every chapter in
 * the archive, pick one" dance (legacy's own `.edit-toc`/`.remove-toc` only ever operate on
 * `getCurrentChapter()`, whichever chapter the reader/lightbox happens to be scrolled/previewing
 * into right now, with no way to target a different one at all — confirmed against
 * `reader.js:157-158,1702`, a real limitation in legacy itself, not a porting gap). Rather than
 * two near-identical popup-menu components, `mode` switches the icon/header text/`zIndex` tier
 * between them; `onPick`'s meaning is mode-dependent (edit: open the rename prompt for that
 * entry; delete: remove it) but the menu itself doesn't need to know which.
 *
 * `zIndexBase` lets a caller opt into `Z_OVERLAY_ABOVE_LEGACY_MODAL`'s tier instead of the
 * generic `Z_OVERLAY_BACKDROP`/`Z_OVERLAY_CONTENT` one — needed when this menu is triggered from
 * *inside* `PageLightbox` (already floating at that higher tier itself), otherwise the menu would
 * render invisible behind the lightbox's own backdrop, the same stacking bug `PageLightbox`'s own
 * docs describe for `Z_OVERLAY_ABOVE_LEGACY_MODAL`'s original introduction. */
export function ChapterActionMenu({
  mode,
  anchor,
  chapters,
  zIndexBase = Z_OVERLAY_BACKDROP,
  onPick,
  onClose,
}: {
  mode: "edit" | "delete"
  anchor: DOMRect
  chapters: { page: number; name: string }[]
  zIndexBase?: number
  onPick: (entry: { page: number; name: string }) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  const icon = mode === "edit" ? "fa-pencil-alt" : "fa-trash-alt"
  const label = mode === "edit" ? (t("Edit Chapter name") ?? "Edit Chapter name") : (t("Delete Chapter") ?? "Delete Chapter")
  // Tracks this menu's own open/closed lifetime in `openChapterActionMenuCount` (see that
  // constant's own docs for why a plain module-level counter, and the full registration-order
  // bug this exists to sidestep) — incremented on mount, decremented on unmount, so any
  // capture-phase `Escape` listener elsewhere (namely `PageLightbox`'s own) can check "is a menu
  // like this currently open?" before deciding whether it's safe to act.
  useEffect(() => {
    incrementOpenChapterActionMenuCount(1)
    return () => {
      incrementOpenChapterActionMenuCount(-1)
    }
  }, [])
  // Registered on the capture phase specifically, mirroring `PageLightbox`'s own Escape handler
  // (see that component's docs for the full reasoning) — this menu can be triggered from *inside*
  // the lightbox, which has its own capture-phase Escape listener. `stopImmediatePropagation`,
  // not just `stopPropagation`, for the same same-target capture-vs-bubble reason documented
  // there (this alone doesn't fully solve the ordering problem — see `openChapterActionMenuCount`
  // for the other half of the real fix, on `PageLightbox`'s own side).
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return
      e.stopImmediatePropagation()
      onClose()
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [onClose])
  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: zIndexBase }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: "fixed", top: pos.top, left: pos.left, zIndex: zIndexBase + 1, maxHeight: 260, overflowY: "auto" }}
        mainLabel={{ icon, text: label }}
      >
        {chapters.map((entry) => {
          // Reserved preset identifiers (`c1`-`c20`/`toc`) aren't editable through this manual
          // free-text flow — their whole point is that the *stored* value and the *displayed*
          // text are deliberately different (see `lib/tocValidation.ts`'s own docs), so "editing"
          // one here would silently convert it into an unrelated free-text entry rather than
          // actually renaming the preset. Changing one of these is done by deleting it and
          // re-applying a (possibly different) preset instead, not by editing it in place.
          const isPreset = mode === "edit" && isReservedTocIdentifier(entry.name)
          return (
            <PopupMenuItem
              key={entry.page}
              disabled={isPreset}
              onClick={(e) => {
                e.stopPropagation()
                onPick(entry)
                onClose()
              }}
            >
              <i className={`fas ${icon}`} style={{ width: 18 }}></i> {displayTocName(entry.name, t)}
            </PopupMenuItem>
          )
        })}
      </PopupMenu>
    </>
  )
}
