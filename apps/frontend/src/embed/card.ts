/**
 * The embeddable share card (017 US6, FR-030–FR-032), as plain DOM code: no React, router or
 * library locale files, so a page that frames many cards stays light. Its actions follow the
 * share's permissions; an import-only share leads with its code.
 */
import { formatBytes } from "../lib/formatBytes"
import { formatShareCode } from "../share/code"
import { copyText } from "../share/copyText"
import { renderQr } from "../share/qr"
import type { CardText } from "./text"

interface CardItem {
  title: string | null
  cover: string | null
  volumes?: number
}

export interface CardData {
  status: "active" | "exhausted" | "revoked" | "expired"
  restricted: boolean
  /** Bytes a download of the whole share would transfer (the sum of its archives). */
  total_size?: number
  title?: string
  item_count?: number
  covers?: string[]
  cover_titles?: string[]
  items?: CardItem[]
  permissions: { import: boolean; read: boolean; download: boolean }
  token?: string
}

type Child = Node | string | null | false | undefined

/** Makes a thumbnail enlarge, with `title`, while hovered or focused; returns it. */
type Previewable = (thumb: HTMLElement, id: string, title: string) => HTMLElement

/** A small element builder: attributes, then children (falsy ones skipped). */
function h(tag: string, attrs: Record<string, string | boolean | undefined> = {}, ...children: Child[]): HTMLElement {
  const el = document.createElement(tag)
  for (const [name, value] of Object.entries(attrs)) {
    if (value === undefined || value === false) continue
    el.setAttribute(name, value === true ? "" : value)
  }
  for (const child of children) {
    if (child) el.append(child)
  }
  return el
}

/** Draws the card for `data` (null: not found) into `root`.
 *
 * `coverRows: 1` is the fallback its caller uses when a two-row card would not fit a short frame;
 * the card itself cannot know the frame's height, so the page that owns the iframe decides. */
export function renderCard(
  root: HTMLElement,
  code: string,
  data: CardData | null,
  t: CardText,
  { coverRows = 2, failure = "unavailable" }: { coverRows?: 1 | 2; failure?: "unavailable" | "retry" } = {},
) {
  const pageUrl = `${window.location.origin}/s/${formatShareCode(code)}`
  const textForm = `lrrg:${formatShareCode(code)}`
  const card = h("div", { class: "lrrg-card" })
  root.replaceChildren(card)

  if (!data) {
    // Two different things reach this branch: a share that is gone (wrong code, embedding off,
    // revoked, expired — all 404) and a request that failed for reasons that may pass (network,
    // 5xx, rate limited). Only the second one is worth a retry.
    if (failure === "retry") {
      const state = h("div", { class: "lrrg-card-error" })
      const retry = h("a", { class: "lrrg-card-retry", href: window.location.href })
      retry.append(t.retry)
      state.append(h("p", { class: "lrrg-card-state" }, t.loadFailed), retry)
      card.append(state)
      return
    }
    card.append(h("p", { class: "lrrg-card-state" }, t.unavailable))
    return
  }
  /** A link to the share page (or a file, for a download); `icon` leads its label. */
  const link = (label: string, icon?: string, href = pageUrl) => {
    const a = h("a", { href, target: "_blank", rel: "noopener noreferrer" })
    if (icon) a.innerHTML = icon
    a.append(label)
    return a
  }
  /** The same action, shown but unusable, for a share that has closed. */
  const disabled = (label: string, icon?: string) => {
    const button = h("button", { type: "button", disabled: true })
    if (icon) button.innerHTML = icon
    button.append(label)
    return button
  }
  // A used-up share is not closed: its content is still there to read and to import, only the
  // download is gone (a notice under the title says so).
  const exhausted = data.status === "exhausted"
  // Revoked or expired: why, and the actions the share had, all disabled; nothing of its content
  // (the server sends none).
  if (data.status === "revoked" || data.status === "expired") {
    const closed = t.closed[data.status]
    const { permissions } = data
    const state = h("div", { class: "lrrg-card-closed" })
    state.innerHTML = CLOSED_ICON
    state.append(h("p", { class: "lrrg-card-title" }, closed.title), h("p", { class: "lrrg-card-meta" }, closed.body))
    card.append(
      state,
      h(
        "div",
        { class: "lrrg-card-actions lrrg-card-main-actions" },
        permissions.read && disabled(t.read, ACTION_ICONS.read),
        permissions.download && disabled(t.download, ACTION_ICONS.download),
        permissions.import && disabled(t.copyCode),
      ),
    )
    return
  }
  if (data.restricted) {
    card.append(
      h("p", { class: "lrrg-card-title" }, t.restricted),
      h("p", { class: "lrrg-card-meta" }, t.restrictedBody),
      h("div", { class: "lrrg-card-actions" }, link(t.open)),
    )
    return
  }

  const { permissions } = data
  const importOnly = permissions.import && !permissions.read && !permissions.download
  const itemCount = data.item_count ?? 0
  // Eight items or more fill two rows of four: seven covers and the "+N" tile as the eighth, so
  // the second row is full instead of one tile alone. Seven or fewer keep the single row of four
  // covers with the narrow tile beside them — and so does a tall share in a frame too short for a
  // second row (`coverRows: 1`).
  const twoRows = coverRows === 2 && itemCount >= 8
  const covers = (data.covers ?? []).slice(0, twoRows ? 7 : 4)
  // Items past the covers shown: a "+N" tile closes the row instead of another cover.
  const more = Math.max(itemCount - covers.length, 0)
  const coverUrl = (id: string) =>
    `/api/share/r/archives/${encodeURIComponent(id)}/thumbnail?rt=${encodeURIComponent(data.token ?? "")}`
  const image = (id: string, extra: Record<string, string | boolean | undefined> = {}) =>
    h("img", { src: coverUrl(id), alt: "", loading: "lazy", referrerpolicy: "no-referrer", ...extra })

  // A hovered or focused thumbnail (in the cover row or the item list), enlarged over the card
  // with its item's title.
  let preview: HTMLElement | null = null
  const closePreview = () => {
    preview?.remove()
    preview = null
  }
  const previewable: Previewable = (thumb, id, title) => {
    const open = () => {
      closePreview()
      preview = h("div", { class: "lrrg-card-preview", "aria-hidden": "true" }, image(id, { loading: undefined }), title && h("p", {}, title))
      card.append(preview)
    }
    // A mouse hovers; a finger or pen holds — shown while pressed, gone on release, and a press that
    // turns into a scroll is cancelled by the browser. Focus (keyboard) shows it too, but not the
    // focus a touch itself gives.
    let pressed = false
    thumb.addEventListener("pointerenter", (e) => e.pointerType === "mouse" && open())
    thumb.addEventListener("pointerleave", (e) => e.pointerType === "mouse" && closePreview())
    thumb.addEventListener("pointerdown", (e) => {
      if (e.pointerType === "mouse") return
      pressed = true
      open()
    })
    const release = (e: PointerEvent) => {
      if (e.pointerType === "mouse") return
      pressed = false
      closePreview()
    }
    thumb.addEventListener("pointerup", release)
    thumb.addEventListener("pointercancel", release)
    // A long press would otherwise open the system's image menu over the preview.
    thumb.addEventListener("contextmenu", (e) => e.preventDefault())
    thumb.addEventListener("focus", () => !pressed && open())
    thumb.addEventListener("blur", closePreview)
    return thumb
  }

  // One item: its cover beside the title and actions, like a link card, rather than one cover in a
  // row of four.
  const single = !importOnly && covers.length === 1 && (data.item_count ?? 0) === 1
  // A single archive can be downloaded straight from the card, so its Download is a real file
  // link (middle-click and "save link as" work). A tankoubon or a multi-item share has no one
  // file to point at, and both actions lead to the share page.
  const downloadHref =
    single && covers[0] && data.items?.[0]?.volumes == null
      ? `/api/share/r/archives/${encodeURIComponent(covers[0])}/download?rt=${encodeURIComponent(data.token ?? "")}`
      : pageUrl
  const canDownload = !exhausted
  const size = data.total_size && data.total_size > 0 ? formatBytes(data.total_size) : null
  const mainActions = () =>
    h(
      "div",
      { class: "lrrg-card-actions lrrg-card-main-actions" },
      permissions.read && link(t.read, ACTION_ICONS.read),
      canDownload && permissions.download && link(t.download, ACTION_ICONS.download, downloadHref),
    )

  if (single) {
    const title = data.cover_titles?.[0] ?? ""
    const volumes = data.items?.[0]?.volumes
    card.append(
      h(
        "div",
        { class: "lrrg-card-media" },
        previewable(image(covers[0], { alt: title, tabindex: "0" }), covers[0], title),
        h(
          "div",
          { class: "lrrg-card-media-body" },
          h("p", { class: "lrrg-card-title" }, data.title ?? ""),
          (volumes != null || size) &&
            h(
              "p",
              { class: "lrrg-card-meta" },
              [volumes != null ? t.volumes(volumes) : null, size].filter(Boolean).join(" · "),
            ),
          mainActions(),
        ),
      ),
    )
  } else if (!importOnly && covers.length > 0) {
    // Two or three covers keep a cover's size and sit centred instead of filling four columns.
    const few = !twoRows && more === 0 && covers.length < 4
    const row = h("div", {
      class: twoRows
        ? // Eight tiles in four equal columns: two full rows, the "+N" tile a tile of its own.
          "lrrg-card-covers lrrg-card-covers-two-rows"
        : more > 0
          ? "lrrg-card-covers lrrg-card-covers-more"
          : few
            ? "lrrg-card-covers lrrg-card-covers-few"
            : "lrrg-card-covers",
    })
    if (few) row.style.gridTemplateColumns = `repeat(${covers.length}, minmax(0, 120px))`
    covers.forEach((id, i) => {
      const title = data.cover_titles?.[i] ?? ""
      row.append(previewable(image(id, { alt: title, tabindex: "0" }), id, title))
    })
    if (more > 0) {
      const label = t.more(more)
      const button = h("button", { type: "button", class: "lrrg-card-more", title: label, "aria-label": label }, `+${more}`)
      button.addEventListener("click", () => openList(card, data, t, image, previewable, closePreview))
      row.append(button)
    }
    card.append(row)
  }

  if (!single) {
    const meta = [t.items(data.item_count ?? 0), size].filter(Boolean).join(" · ")
    card.append(h("p", { class: "lrrg-card-title" }, data.title ?? ""), h("p", { class: "lrrg-card-meta" }, meta))
  }
  if (exhausted) {
    card.append(h("p", { class: "lrrg-card-meta share-status-exhausted" }, t.closed.exhausted.body))
  }

  const importBlock = () => {
    const qr = document.createElement("canvas")
    // A two-module quiet zone, so the code's own edges line up with the text beside it.
    // Drawn in the theme's QR colours (`--card-qr*`, preset or custom), read off the card.
    const style = getComputedStyle(card)
    const color = (name: string) => style.getPropertyValue(name).trim() || undefined
    void renderQr(textForm, qr, 2, { fg: color("--card-qrFg"), bg: color("--card-qrBg"), finder: color("--card-qrFinder") })
    const copy = h("button", { type: "button" }, t.copyCode)
    copy.addEventListener("click", () => void copyText(textForm))
    return h(
      "div",
      { class: "lrrg-card-import" },
      qr,
      h(
        "div",
        {},
        h("div", { class: "lrrg-card-code" }, textForm),
        h("p", { class: "lrrg-card-meta" }, h("span", { class: "lrrg-card-hint-long" }, t.importHint), h("span", { class: "lrrg-card-hint-short" }, t.importHintShort)),
        h("div", { class: "lrrg-card-actions" }, copy),
      ),
    )
  }
  if (importOnly) {
    card.append(importBlock())
    return
  }
  if (!single) card.append(mainActions())
  if (permissions.import) {
    const block = importBlock()
    block.style.marginTop = "10px"
    card.append(block)
  }
}

/** 15px stroke icons leading the Read and Download labels. */
const ACTION_ICONS = {
  read: '<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" aria-hidden="true"><path d="M8 3.5C6.5 2.5 4 2.2 1.5 2.5v10c2.5-.3 5 0 6.5 1 1.5-1 4-1.3 6.5-1v-10C12 2.2 9.5 2.5 8 3.5z"/><path d="M8 3.5v10"/></svg>',
  download:
    '<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 2v8.5M4.5 7 8 10.5 11.5 7M2.5 13.5h11"/></svg>',
}

/** A closed share's mark: a circle with a bar through it. */
const CLOSED_ICON =
  '<svg viewBox="0 0 24 24" width="36" height="36" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M5.6 5.6l12.8 12.8"/></svg>'

type ListView = "list" | "icons" | "thumbs"

const VIEW_KEY = "lrrg-card-view"

function rememberedView(): ListView {
  try {
    const saved = localStorage.getItem(VIEW_KEY)
    if (saved === "list" || saved === "icons" || saved === "thumbs") return saved
  } catch {
    // Unavailable storage: the default view.
  }
  return "list"
}

/** 16px stroke icons for the view switches, drawn in the current text colour. */
const ICONS: Record<ListView, string> = {
  list: '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><rect x="1.5" y="2" width="3" height="3" rx=".5"/><rect x="1.5" y="6.5" width="3" height="3" rx=".5"/><rect x="1.5" y="11" width="3" height="3" rx=".5"/><path d="M7 3.5h7.5M7 8h7.5M7 12.5h7.5"/></svg>',
  icons:
    '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><rect x="1.5" y="1.5" width="5" height="5" rx=".8"/><rect x="9.5" y="1.5" width="5" height="5" rx=".8"/><path d="M1.5 9h5M9.5 9h5"/><rect x="1.5" y="11" width="5" height="3.5" rx=".8"/><rect x="9.5" y="11" width="5" height="3.5" rx=".8"/></svg>',
  thumbs:
    '<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.6"><rect x="1.5" y="1.5" width="5.5" height="13" rx=".8"/><rect x="9" y="1.5" width="5.5" height="13" rx=".8"/></svg>',
}

/** Every item, over the card; closes on Escape, a click outside, or focus leaving the frame (a
 * click on the embedding page never reaches it, but takes focus away). */
function openList(
  card: HTMLElement,
  data: CardData,
  t: CardText,
  image: (id: string, extra?: Record<string, string | boolean | undefined>) => HTMLElement,
  previewable: Previewable,
  closePreview: () => void,
) {
  const close = h("button", { type: "button", "aria-label": t.close }, "×")
  const list = h("ul", {})
  // How the items are laid out: rows, small tiles with titles, or covers alone.
  const views: { view: ListView; label: string; icon: string }[] = [
    { view: "list", label: t.viewList, icon: ICONS.list },
    { view: "icons", label: t.viewIcons, icon: ICONS.icons },
    { view: "thumbs", label: t.viewThumbs, icon: ICONS.thumbs },
  ]
  const switches = views.map(({ view, label, icon }) => {
    const button = h("button", { type: "button", class: "lrrg-card-view", title: label, "aria-label": label })
    button.innerHTML = icon
    button.addEventListener("click", () => setView(view))
    return { view, button }
  })
  const setView = (view: ListView) => {
    list.className = `lrrg-card-list-${view}`
    for (const s of switches) s.button.setAttribute("aria-pressed", String(s.view === view))
    try {
      localStorage.setItem(VIEW_KEY, view)
    } catch {
      // Storage may be unavailable in a framed page; the choice just isn't remembered.
    }
  }
  for (const item of data.items ?? []) {
    const name = item.title ?? t.unavailableItem
    // The title previews its item as the cover does; the preview shows the whole title, so a
    // native tooltip is only left where there is no cover to preview.
    const title = item.cover
      ? previewable(h("span", { class: "lrrg-card-list-title lrrg-card-previewable" }, name), item.cover, item.title ?? "")
      : h("span", { class: "lrrg-card-list-title", title: item.title ?? undefined }, name)
    list.append(
      h(
        "li",
        {},
        item.cover ? previewable(image(item.cover), item.cover, item.title ?? "") : h("span", { class: "lrrg-card-list-nocover" }),
        title,
        item.volumes != null && h("span", { class: "lrrg-card-list-badge" }, t.volumes(item.volumes)),
      ),
    )
  }
  const dialog = h(
    "div",
    { class: "lrrg-card-list", role: "dialog", "aria-modal": "true", "aria-label": data.title ?? "" },
    h(
      "div",
      { class: "lrrg-card-list-head" },
      h("span", {}, t.items(data.item_count ?? 0)),
      h("div", { class: "lrrg-card-views", role: "group" }, ...switches.map((s) => s.button)),
      close,
    ),
    list,
  )
  setView(rememberedView())
  const backdrop = h("div", { class: "lrrg-card-list-backdrop" }, dialog)
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && dismiss()
  const dismiss = () => {
    closePreview()
    backdrop.remove()
    window.removeEventListener("keydown", onKey)
    window.removeEventListener("blur", dismiss)
  }
  backdrop.addEventListener("click", (e) => e.target === backdrop && dismiss())
  close.addEventListener("click", dismiss)
  window.addEventListener("keydown", onKey)
  window.addEventListener("blur", dismiss)
  card.append(backdrop)
  close.focus()
}
